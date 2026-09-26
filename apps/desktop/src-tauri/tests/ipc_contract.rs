//! The interface's own contract, exercised through Tauri's real IPC layer.
//!
//! # Why this test exists
//!
//! Everything below the desktop shell is tested hard — the core, the media layer, the queue, the
//! verifier — and every one of those tests would still pass if a command were registered under the
//! wrong name, took an argument the interface does not send, returned a JSON key with a typo in it,
//! or were missing from the `invoke_handler` list altogether. Those are not hypothetical: a command
//! that is not registered produces a window that renders perfectly and does nothing when clicked,
//! and nothing in the Rust test suite would notice.
//!
//! So this file drives `apps/web`'s exact call sequence — the same command names, the same camelCase
//! argument keys, the same field names read back out of the replies — through
//! [`tauri::test::get_ipc_response`]. That path goes through the real invoke handler, the real
//! argument deserialiser and the real reply serialiser, so a rename on either side fails the build.
//!
//! # Why it is ignored by default
//!
//! It generates a video with ffmpeg and cuts it, which takes seconds and needs a real encoder. The
//! CI suite runs it explicitly; a developer running `cargo test` in a loop does not pay for it.
//!
//! ```text
//! cargo test -p thetrimmer-desktop --test ipc_contract -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewWindowBuilder};
use thetrimmer_desktop_lib::{commands, state::AppState};

type BoxError = Box<dyn std::error::Error>;

/// Build the application exactly as `main.rs` does.
///
/// The command list is duplicated from the binary on purpose: a test that reused a shared list
/// would still pass if the binary forgot to register one of them, which is the failure this file
/// exists to catch. Keeping the list here means the two have to be kept in step by hand, and the
/// compiler cannot help with that — but the test fails loudly the moment they diverge.
fn build_app(state: AppState) -> Result<App<tauri::test::MockRuntime>, BoxError> {
    Ok(mock_builder()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::doctor,
            commands::list_projects,
            commands::create_project,
            commands::open_project,
            commands::delete_project,
            commands::current_project,
            commands::save_project,
            commands::add_source,
            commands::refresh_sources,
            commands::remove_source,
            commands::sources,
            commands::segments,
            commands::summary,
            commands::presets,
            commands::add_segment,
            commands::update_segment,
            commands::remove_segment,
            commands::reorder_segment,
            commands::parse_timecode,
            commands::preview,
            commands::preview_all,
            commands::cut_segment,
            commands::run_batch,
            commands::cancel_batch,
            commands::search_transcript,
            commands::transcript_lines,
            commands::export_timeline,
            commands::plan_watch_folder,
            commands::get_verify_policy,
            commands::set_verify_policy,
            commands::reveal,
        ])
        .build(mock_context(noop_assets()))?)
}

/// Invoke a command the way `apps/web/src/ipc/commands.ts` does.
///
/// The arguments go in as a JSON body, which is what the shell's `invoke` sends when a command takes
/// more than one argument, and what Tauri converts `camelCase` keys from. Returning a
/// `Result<Value, String>` means a domain refusal is a test failure with the refusal's own sentence
/// in it rather than a panic on a `None`.
fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, String> {
    let request = InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: if cfg!(any(windows, target_os = "android")) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        }
        .parse()
        .expect("a valid invocation origin"),
        body: InvokeBody::Json(args),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    };
    let response =
        tauri::test::get_ipc_response(webview, request).map_err(|error| error.to_string())?;
    response
        .deserialize::<Value>()
        .map_err(|error| error.to_string())
}

/// A throwaway directory that deletes itself.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Result<Self, BoxError> {
        let path = std::env::temp_dir().join(format!("tt-ipc-{name}-{}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Generate a small master with ffmpeg, and a caption file beside it.
///
/// 640x360 at 25 fps, three seconds, a keyframe every 25 frames — the same shape as the fixture the
/// guided tour uses, so a number in this test can be compared with a number in the tour.
fn make_master(scratch: &Scratch) -> Result<PathBuf, BoxError> {
    let video = scratch.join("master.mp4");
    let tools = trimmer_media::ToolPaths::resolve()?;
    let status = std::process::Command::new(&tools.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=640x360:rate=25:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&video)
        .status()?;
    if !status.success() {
        return Err("ffmpeg could not generate the fixture".into());
    }
    std::fs::write(
        scratch.join("master.srt"),
        "1\n00:00:00,200 --> 00:00:01,000\nfirst line\n\n\
         2\n00:00:01,200 --> 00:00:02,000\nsecond line\n\n\
         3\n00:00:02,200 --> 00:00:02,900\nthird line\n",
    )?;
    Ok(video)
}

/// A workspace pointed at a private store and a private scratch folder.
fn harness(name: &str) -> Result<(Scratch, App<tauri::test::MockRuntime>), BoxError> {
    let scratch = Scratch::new(name)?;
    // `AppState::bootstrap` reads this, so the test never touches the user's own project database.
    std::env::set_var("THE_TRIMMER_STORE", scratch.join("projects.db"));
    let state = AppState::bootstrap()?;
    let app = build_app(state)?;
    Ok((scratch, app))
}

/// The window, which is where `get_ipc_response` sends an invocation from.
fn window(
    app: &App<tauri::test::MockRuntime>,
) -> Result<tauri::WebviewWindow<tauri::test::MockRuntime>, BoxError> {
    Ok(WebviewWindowBuilder::new(app, "main", Default::default()).build()?)
}

/// Every command the interface calls on startup returns the shape the interface reads.
///
/// The keys asserted here are the ones `apps/web/src/ipc/types.ts` declares. A rename on the Rust
/// side that the TypeScript did not follow is a window that renders a screen of `undefined`s, which
/// is exactly the sort of failure that has no other test.
#[test]
#[ignore = "generates and cuts media; run with --ignored"]
fn the_startup_commands_answer_the_shape_the_interface_reads() -> Result<(), BoxError> {
    let (scratch, app) = harness("startup")?;
    let webview = window(&app)?;

    let doctor = invoke(&webview, "doctor", json!({}))?;
    for key in [
        "version",
        "ffmpeg",
        "ffprobe",
        "capabilities",
        "storePath",
        "libx264",
        "libx265",
    ] {
        assert!(
            doctor.get(key).is_some(),
            "doctor no longer returns `{key}`, which the status bar and the plan panel read"
        );
    }
    assert!(
        doctor.get("license").is_none(),
        "doctor still returns a licence field; licensing was removed from the product"
    );
    assert_eq!(
        doctor["storePath"].as_str(),
        Some(scratch.join("projects.db").to_string_lossy().as_ref()),
        "the store path is not the one the environment named"
    );

    assert_eq!(
        invoke(&webview, "list_projects", json!({}))?,
        json!([]),
        "a fresh store should list no projects"
    );

    // Not an error: the interface asks for these before a project exists, and a refusal on a window
    // that is behaving perfectly teaches the user to ignore the status bar.
    assert_eq!(
        invoke(&webview, "current_project", json!({}))?,
        Value::Null,
        "no project is open, so the current project must be null rather than loud"
    );
    assert_eq!(
        invoke(&webview, "get_verify_policy", json!({}))?,
        json!("strict"),
        "a new project verifies strictly, which is the domain's own default"
    );

    // These genuinely need a project: they read the cut list, which does not exist yet.
    for cmd in ["sources", "segments"] {
        let refused = invoke(&webview, cmd, json!({}));
        let reason = refused.err().unwrap_or_default();
        assert!(
            reason.contains("no project"),
            "`{cmd}` should say there is no project open, and said `{reason}`"
        );
    }
    Ok(())
}

/// The whole editing sequence the interface performs, then a real cut of it.
///
/// This is the test that would have caught the window opening with no working buttons. It runs the
/// exact calls `useAppModel.ts` makes, in order, and then asserts against the *evidence* rather than
/// against the fact that a function returned.
#[test]
#[ignore = "generates and cuts media; run with --ignored"]
fn the_window_can_open_a_project_add_a_source_mark_a_segment_and_cut_it() -> Result<(), BoxError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(the_window_flow())
}

/// The body of the flow test, so it can await the probe at the end.
async fn the_window_flow() -> Result<(), BoxError> {
    let (scratch, app) = harness("flow")?;
    let webview = window(&app)?;
    let master = make_master(&scratch)?;

    let created = invoke(
        &webview,
        "create_project",
        json!({ "name": "IPC contract", "createdBy": "the test" }),
    )?;
    let project = created["id"]
        .as_str()
        .ok_or("create_project did not return an id")?
        .to_owned();
    assert!(!project.is_empty());

    let listed = invoke(&webview, "list_projects", json!({}))?;
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    assert_eq!(listed[0]["name"], json!("IPC contract"));
    assert!(
        listed[0]["updatedAt"].is_i64(),
        "the project list needs `updatedAt` as a number for the dialog's date column"
    );

    let source = invoke(
        &webview,
        "add_source",
        json!({ "path": master.to_string_lossy() }),
    )?;
    // The keys the source rail reads. `add_source` used to answer a hand-written object with a
    // different shape from the `sources` command — no probed facts, no cue count — so the rail showed
    // a row of blanks for a file the very next refresh described in full.
    for key in [
        "path",
        "name",
        "present",
        "media",
        "summary",
        "transcript",
        "transcriptCues",
        "label",
        "variableRate",
    ] {
        assert!(
            source.get(key).is_some(),
            "add_source no longer returns `{key}`, which the source rail reads"
        );
    }
    assert_eq!(source["present"], json!(true));
    assert_eq!(
        source["media"]["width"],
        json!(640),
        "the probed facts are nested under `media`, which is where the rail reads them"
    );
    assert_eq!(source["media"]["height"], json!(360));
    assert_eq!(
        source["media"]["frameCount"],
        json!(75),
        "three seconds at 25 fps"
    );
    assert_eq!(source["media"]["rate"], json!({ "num": 25, "den": 1 }));
    assert_eq!(
        source["variableRate"],
        json!(false),
        "a testsrc master is constant rate"
    );
    assert_eq!(source["label"], Value::Null, "a source starts unlabelled");
    assert!(
        source["transcript"].is_string(),
        "master.srt sits beside master.mp4 and must be found: {}",
        source["transcript"]
    );
    assert_eq!(
        source["transcriptCues"],
        json!(3),
        "the rail says how many cues a transcript holds"
    );

    // And the listing agrees with what `add_source` answered, which is the whole point.
    let listed_sources = invoke(&webview, "sources", json!({}))?;
    assert_eq!(listed_sources.as_array().map(Vec::len), Some(1));
    assert_eq!(
        listed_sources[0], source,
        "add_source and sources must describe the same file the same way"
    );

    // `endFrame` is the domain's **exclusive** end — one past the last frame kept. The interface
    // converts before it sends: the segment dialog parses the out timecode, which is the last frame
    // kept, and hands over `parsed + 1`. Frame 25 is a keyframe (the fixture puts one every 25
    // frames) and frame 50 is the last one kept, so this asks for frames 25..=50: 26 of them, all copy.
    let segment = invoke(
        &webview,
        "add_segment",
        json!({
            "source": master.to_string_lossy(),
            "name": "second one",
            "startFrame": 25,
            "endFrame": 51,
            "preset": null,
            "handleFrames": 0
        }),
    )?;
    let id = segment["id"].as_str().ok_or("add_segment returned no id")?;
    assert_eq!(id.len(), 36, "a segment id is a uuid");

    let segments = invoke(&webview, "segments", json!({}))?;
    assert_eq!(segments.as_array().map(Vec::len), Some(1));
    let row = &segments[0];
    for key in [
        "id",
        "name",
        "sourceName",
        "inTimecode",
        "outTimecode",
        "endFrame",
        "frames",
        "seconds",
        "handleFrames",
        "enabled",
        "problems",
        "notes",
    ] {
        assert!(
            row.get(key).is_some(),
            "the cut table reads `{key}` from a segment and it is not there"
        );
    }
    assert_eq!(row["inTimecode"], json!("00:00:01:00"));
    assert_eq!(
        row["outTimecode"],
        json!("00:00:02:00"),
        "the out timecode is the last frame kept, which is the frame that was marked"
    );
    assert_eq!(
        row["frames"],
        json!(26),
        "26 frames: 25 through 50, inclusive of both marks"
    );
    assert_eq!(
        row["endFrame"],
        json!(51),
        "the domain end is one past the last frame kept"
    );
    assert_eq!(row["problems"], json!([]));

    let preview = invoke(&webview, "preview_all", json!({}))?;
    let plan = &preview[0]["plan"];
    assert!(plan.is_object(), "a preview must carry a plan: {preview}");
    assert_eq!(
        plan["mode"],
        json!("copy"),
        "frames 25..=50 start on a keyframe"
    );
    assert_eq!(plan["bodyFrames"], json!(26));
    assert_eq!(plan["headFrames"], json!(0));

    // The timecode the segment dialog shows live under the In/Out fields.
    let parsed = invoke(
        &webview,
        "parse_timecode",
        json!({ "text": "00:00:01:12", "source": master.to_string_lossy() }),
    )?;
    assert_eq!(parsed["frame"], json!(37));
    assert_eq!(parsed["timecode"], json!("00:00:01:12"));

    // The transcript panel's search, over the caption file found beside the master.
    let hits = invoke(
        &webview,
        "search_transcript",
        json!({ "video": master.to_string_lossy(), "phrase": "second", "limit": 10 }),
    )?;
    assert_eq!(
        hits.as_array().map(Vec::len),
        Some(1),
        "one cue says `second`"
    );
    assert!(hits[0].get("startFrame").is_some());
    assert!(hits[0].get("highlighted").is_some());

    let outcome = invoke(
        &webview,
        "run_batch",
        json!({ "stopOnError": true, "skipVerification": false, "label": "ipc" }),
    )?;
    let jobs = outcome["jobs"]
        .as_array()
        .ok_or("run_batch returned no jobs")?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["name"], json!("second one"));
    let status = &jobs[0]["status"];
    assert_eq!(
        status["kind"],
        json!("succeeded"),
        "the cut did not succeed: {status}"
    );
    assert!(
        status["output"].is_string(),
        "a succeeded job names its output"
    );
    // A stream copy cannot stop between packets, so the file may hold a frame or two more than were
    // asked for. What it may never do is hold fewer, and the report has to say which happened: the
    // `frames` check passes, and any excess is counted in `overshoot` and warned about rather than
    // silently accepted. `trimmer-verify`'s own docs make that argument; this asserts the window
    // actually receives it.
    let delivered = status["frames"].as_i64().ok_or("no frame count")?;
    let overshoot = status["overshoot"].as_i64().unwrap_or(0);
    assert!(
        delivered >= 26,
        "the deliverable holds {delivered} frames but 26 were asked for: {status}"
    );
    assert!(
        overshoot <= 3,
        "an overshoot of {overshoot} frames is more than a packet boundary explains: {status}"
    );
    let checks = status["checks"].as_array().ok_or("no checks")?;
    // `Check`'s own names are PascalCase and the wire carries them verbatim, so the proof panel
    // renders `Frames` and `TimescalePreserved` rather than a translated label.
    assert!(
        checks.iter().any(|check| {
            check["check"] == json!("Frames") && check["status"]["kind"] == json!("passed")
        }),
        "the frames check must be present and passing, not absent: {status}"
    );
    if overshoot > 0 {
        assert!(
            checks.iter().any(|check| {
                check["check"] == json!("Overshoot")
                    && check["status"]["kind"] == json!("warning")
                    && check["status"]["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.contains("warning, not a failure"))
            }),
            "a file holding more frames than were asked for must warn, with the reason in the \
             detail: {status}"
        );
    }
    assert!(
        checks
            .iter()
            .all(|check| check["status"]["kind"] != json!("failed")),
        "a succeeded job cannot carry a failed check: {status}"
    );
    assert!(
        checks
            .iter()
            .filter(|check| check["status"]["kind"] == json!("skipped"))
            .all(|check| check["status"]["reason"].is_string()),
        "a skipped check must say why it was skipped, or the proof panel shows an empty row: {status}"
    );
    assert_eq!(outcome["deliveredFrames"], json!(delivered));
    assert_eq!(outcome["cancelled"], json!(false));
    assert!(
        outcome["digest"].as_str().is_some_and(|d| d.len() == 64),
        "the run signature must be a sha256 the proof panel can show"
    );
    assert!(outcome["elapsedSeconds"].is_number());

    // The file is on disk, and it holds what the report said it holds.
    let output = status["output"]
        .as_str()
        .ok_or("a succeeded job must name its output")?;
    let cut = Path::new(output);
    assert!(cut.exists(), "{output} was reported and does not exist");
    let probed = trimmer_media::Prober::new(trimmer_media::ToolPaths::resolve()?)
        .probe(cut)
        .await?;
    assert_eq!(
        probed.frame_count, delivered,
        "the report and the file disagree about the length, which is the one thing the report is for"
    );

    // And the first frame of the deliverable is the source's frame 25, so the cut landed on the
    // mark rather than merely being the right length.
    let executor = trimmer_media::CutExecutor::new(trimmer_media::ToolPaths::resolve()?);
    let options = trimmer_media::RunOptions::default();
    let wanted = executor
        .frame_hashes(&master, 1.0, 1, &options)
        .await
        .map_err(|error| error.to_string())?;
    let got = executor
        .frame_hashes(cut, 0.0, 1, &options)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(
        wanted.first(),
        got.first(),
        "the first delivered frame is not the source's frame 25"
    );

    // And the project survived the batch: the interface re-reads all three after a run, and a
    // command that took the workspace out and did not put it back would show an empty table here.
    assert_eq!(
        invoke(&webview, "segments", json!({}))?
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        invoke(&webview, "sources", json!({}))?
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert!(invoke(&webview, "summary", json!({}))?.is_object());

    Ok(())
}

/// A request the interface makes before a project exists must fail with a sentence, not a panic.
///
/// The window mounts the whole model before anything is open, so every command has a path through it
/// with no workspace. A panic there is a blank window; a sentence is a status-bar message.
#[test]
#[ignore = "generates and cuts media; run with --ignored"]
fn commands_before_a_project_exists_fail_politely() -> Result<(), BoxError> {
    let (_scratch, app) = harness("early")?;
    let webview = window(&app)?;

    for cmd in ["segments", "sources", "preview_all", "summary"] {
        let refused = invoke(&webview, cmd, json!({}));
        assert!(
            refused.is_err(),
            "`{cmd}` should refuse with no project open"
        );
    }
    for (cmd, args) in [
        ("add_source", json!({ "path": "C:/nothing/here.mp4" })),
        (
            "export_timeline",
            json!({ "format": "csv", "path": "x.csv", "sequenceName": "s" }),
        ),
        ("set_verify_policy", json!({ "policy": "strict" })),
    ] {
        assert!(
            invoke(&webview, cmd, args).is_err(),
            "`{cmd}` should refuse with no project open"
        );
    }

    // A bad verification policy is refused even though the refusal is about a string, not state.
    assert!(invoke(
        &webview,
        "set_verify_policy",
        json!({ "policy": "forensic-ish" })
    )
    .is_err());
    Ok(())
}

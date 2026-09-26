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
use thetrimmer_desktop_lib::{state::AppState, trimmer_commands};

type BoxError = Box<dyn std::error::Error>;

/// Build the application exactly as `main.rs` does.
///
/// The command list is **the same list** the binary registers — `trimmer_commands!()` lives in the
/// library and both call it. This file used to keep a hand-written copy, on the argument that a shared
/// list "would still pass if the binary forgot to register one of them". That is backwards: two lists
/// drift, and the direction that matters is a command present here and absent from the binary, which
/// passes every test in this crate and is a dead button in the shipped window. One list means
/// "registered in the test" and "registered in the window" are the same statement.
fn build_app(state: AppState) -> Result<App<tauri::test::MockRuntime>, BoxError> {
    Ok(mock_builder()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(trimmer_commands!())
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

/// A second, shorter master with no caption file beside it.
///
/// Deliberately different from [`make_master`] in two ways the interface reads: a different resolution,
/// so a source list cannot be showing the same row twice, and no `.srt`, so "this one has a transcript"
/// and "this one does not" are distinguishable.
fn make_second_master(scratch: &Scratch) -> Result<PathBuf, BoxError> {
    let video = scratch.join("second.mp4");
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
            "testsrc=size=320x240:rate=25:duration=2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&video)
        .status()?;
    if !status.success() {
        return Err("ffmpeg could not generate the second fixture".into());
    }
    Ok(video)
}

/// A workspace pointed at a private store and a private scratch folder.
///
/// The store path is passed in rather than put in `THE_TRIMMER_STORE`. It used to be the environment
/// variable, and that is process-global: with three of these tests running on Rust's default parallel
/// threads, each wrote the variable before calling `bootstrap`, so one test's workspace could be opened
/// over another's database. The symptom was not a crash — it was `storePath` naming the wrong scratch
/// directory, which looks like a broken assertion rather than a shared global.
fn harness(name: &str) -> Result<(Scratch, App<tauri::test::MockRuntime>), BoxError> {
    let scratch = Scratch::new(name)?;
    let state = AppState::bootstrap_at(scratch.join("projects.db"))?;
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

/// A project holds several masters, and removing one takes its segments with it.
///
/// The window works on one master at a time; the *project* does not, because `add_source` upserts into a
/// map keyed by path. So a second video is a second source rather than a replacement, and the source list
/// in the Video zone exists to move between them.
///
/// This is asserted here because the browser stub now claims the same two behaviours — an upsert, and a
/// removal that drops the segments marked against the source — and a fixture that describes the real
/// system wrongly is worse than no fixture. `remove_source` had no behavioural test at all before this.
#[test]
#[ignore = "generates and cuts media; run with --ignored"]
fn a_project_holds_several_masters_and_removing_one_takes_its_segments() -> Result<(), BoxError> {
    let (scratch, app) = harness("masters")?;
    let webview = window(&app)?;

    let project = invoke(
        &webview,
        "create_project",
        json!({ "name": "two masters", "createdBy": "the test" }),
    )?;
    let id = project["id"].as_str().ok_or("no project id")?.to_string();
    invoke(&webview, "open_project", json!({ "id": id }))?;

    let first = make_master(&scratch)?;
    let second = make_second_master(&scratch)?;
    for path in [&first, &second] {
        invoke(
            &webview,
            "add_source",
            json!({ "path": path.to_string_lossy() }),
        )?;
    }

    let sources = invoke(&webview, "sources", json!({}))?;
    let listed = sources.as_array().ok_or("sources is not a list")?;
    assert_eq!(
        listed.len(),
        2,
        "a second master should be added, not replace the first"
    );

    // Different resolutions, so a list showing the same row twice is detectable rather than invisible.
    let widths: Vec<u64> = listed
        .iter()
        .filter_map(|source| source["media"]["width"].as_u64())
        .collect();
    assert_eq!(widths.len(), 2, "both sources should have been probed");
    assert_ne!(
        widths[0], widths[1],
        "the two masters should not describe the same media"
    );

    // One of them has a caption file beside it and the other does not, which is what the transcript
    // panel filters on.
    let with_transcript = listed
        .iter()
        .filter(|source| !source["transcript"].is_null())
        .count();
    assert_eq!(
        with_transcript, 1,
        "only the first master has a .srt beside it"
    );

    // A segment against the second master.
    invoke(
        &webview,
        "add_segment",
        json!({
            "source": second.to_string_lossy(),
            "name": "second master",
            "startFrame": 0,
            "endFrame": 25,
            "preset": null,
            "handleFrames": 0,
        }),
    )?;
    assert_eq!(
        invoke(&webview, "segments", json!({}))?
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    // Removing it takes the segment with it: a segment whose source is gone cannot be planned or cut.
    invoke(
        &webview,
        "remove_source",
        json!({ "path": second.to_string_lossy() }),
    )?;
    assert_eq!(
        invoke(&webview, "sources", json!({}))?
            .as_array()
            .map(Vec::len),
        Some(1),
        "the removed master should be gone and the other left alone"
    );
    assert_eq!(
        invoke(&webview, "segments", json!({}))?
            .as_array()
            .map(Vec::len),
        Some(0),
        "a segment pointing at a source that is no longer in the project cannot be planned"
    );
    assert_eq!(
        invoke(&webview, "sources", json!({}))?[0]["path"].as_str(),
        Some(first.to_string_lossy().as_ref()),
        "the master that was left should be the one that was not removed"
    );

    Ok(())
}

/// The progress events, key by key, exactly as the interface reads them.
///
/// ## Why an event shape needs asserting against the real thing
///
/// The browser suite drives the whole progress display — the determinate bar, the percentage, the rate,
/// the estimate, the log's structure — from events emitted by the *stub*, because a browser has no
/// engine to emit them. That is only sound if the stub's payloads have the shape Rust sends, and this is
/// the only place that can be checked: `apps/web/tests/progress.spec.ts` cannot tell a well-shaped
/// payload from a plausible one.
///
/// Every key named below is read in `apps/web/src/state/useCutLog.ts`. A rename on the Rust side that the
/// TypeScript did not follow is a progress bar that silently shows nothing — a failure with no error in
/// it, which is the same shape as the two faults that shipped before it.
///
/// Synchronous and needs no media, so it runs in the ordinary suite rather than behind `--ignored`.
#[test]
fn the_progress_events_carry_the_keys_the_interface_reads() {
    use trimmer_media::{Progress, ProgressTicks};

    // ---- the media vocabulary, one ffmpeg process at a time --------------------------------
    let step = serde_json::to_value(Progress::Step {
        label: "head encode, frames 25..50".to_owned(),
    })
    .expect("step serialises");
    assert_eq!(step["kind"], "step");
    assert_eq!(step["label"], "head encode, frames 25..50");

    let command = serde_json::to_value(Progress::Command {
        text: "ffmpeg -i a.mp4 out.mp4".to_owned(),
        args: vec!["-i".to_owned()],
    })
    .expect("command serialises");
    assert_eq!(command["kind"], "command");
    assert!(command["text"].is_string(), "{command}");

    let ticks = serde_json::to_value(Progress::Ticks {
        ticks: ProgressTicks {
            out_seconds: 2.2,
            frame: Some(57),
            speed: Some(4.27),
            bytes: Some(158_476),
            expected_seconds: Some(4.4),
        },
    })
    .expect("ticks serialise");
    assert_eq!(ticks["kind"], "ticks");
    // The nested object is camelCase, and the interface reads `outSeconds` — not `out_seconds`.
    assert_eq!(ticks["ticks"]["outSeconds"], 2.2, "{ticks}");
    assert_eq!(ticks["ticks"]["expectedSeconds"], 4.4, "{ticks}");
    assert_eq!(ticks["ticks"]["frame"], 57);
    assert_eq!(ticks["ticks"]["speed"], 4.27);
    assert_eq!(ticks["ticks"]["bytes"], 158_476);

    // A step that does not know its own length reports a null expectation, which is how the interface
    // knows to draw an indeterminate bar rather than measure against the previous step's length.
    let unknown = serde_json::to_value(Progress::Ticks {
        ticks: ProgressTicks {
            out_seconds: 1.0,
            frame: None,
            speed: None,
            bytes: None,
            expected_seconds: None,
        },
    })
    .expect("ticks serialise");
    assert!(unknown["ticks"]["expectedSeconds"].is_null(), "{unknown}");

    let finished = serde_json::to_value(Progress::Finished {
        label: "join head and body".to_owned(),
        seconds: 0.4,
        ok: true,
    })
    .expect("finished serialises");
    assert_eq!(finished["kind"], "finished");
    assert!(finished["label"].is_string(), "{finished}");
    assert!(finished["ok"].is_boolean(), "{finished}");
    // Deliberately no `job`: that absence is exactly what tells the interface this is a pass ending and
    // not a segment ending, and both vocabularies use `finished`.
    assert!(finished.get("job").is_none(), "{finished}");

    let message = serde_json::to_value(Progress::Message {
        text: "DTS out of order".to_owned(),
    })
    .expect("message");
    assert_eq!(message["kind"], "message");
    assert!(message["text"].is_string(), "{message}");

    // ---- the batch vocabulary, one segment at a time ---------------------------------------
    let started =
        serde_json::to_value(trimmer_app::QueueEvent::Started { total: 3 }).expect("started");
    assert_eq!(started["kind"], "started");
    assert_eq!(started["total"], 3);

    let state = serde_json::to_value(trimmer_app::QueueEvent::State {
        job: trimmer_app::JobId(1),
        name: "Segment at 00:00:01:00".to_owned(),
        state: trimmer_app::JobState::Cutting,
    })
    .expect("state");
    assert_eq!(state["kind"], "state");
    assert!(state["name"].is_string(), "{state}");
    assert!(
        state.get("job").is_some(),
        "the interface tells the two vocabularies apart by `job`"
    );
    assert!(state.get("status").is_none(), "{state}");

    let job_finished = serde_json::to_value(trimmer_app::QueueEvent::Finished {
        job: trimmer_app::JobId(1),
        name: "Segment at 00:00:01:00".to_owned(),
        status: trimmer_app::JobStatus::Skipped {
            reason: "not enabled".to_owned(),
        },
    })
    .expect("finished");
    assert_eq!(job_finished["kind"], "finished");
    assert_eq!(
        job_finished["job"], 1,
        "a queue `finished` names its job and a media one does not"
    );
    assert!(job_finished["name"].is_string(), "{job_finished}");

    let completed = serde_json::to_value(trimmer_app::QueueEvent::Completed {
        succeeded: 2,
        unverified: 0,
        failed: 1,
        skipped: 0,
    })
    .expect("completed");
    assert_eq!(completed["kind"], "completed");
    for key in ["succeeded", "unverified", "failed", "skipped"] {
        assert!(
            completed.get(key).is_some(),
            "`{key}` is missing: {completed}"
        );
    }
}

/// Every command the interface can name is registered in the handler.
///
/// ## Why this test had to exist and why it is new
///
/// It did not, while the browser stub was in the shipped bundle. The stub answered all thirty-one
/// commands from TypeScript, so a command named by `apps/web/src/ipc/commands.ts` and absent from
/// `generate_handler!` was answered perfectly in the window — the stub was it. The gap was invisible
/// from every direction at once: invisible to the browser suite, because there the stub *is* the bridge;
/// invisible to this file, because it only invoked the commands it named; and invisible to the smoke
/// test, because the fixture it checked was the fixture the stub returned.
///
/// The stub is no longer in a production build (see `apps/web/tools/check-bundle.mjs`), which turns
/// that gap into a button that fails the moment it is pressed. So the names are **read out of the
/// interface's own source** rather than listed here: a command added there and forgotten in the handler
/// fails this test, instead of being forgotten on both sides and never noticed.
///
/// The detector is Tauri's own "not found" for an unregistered command. Every command invoked with no
/// arguments either succeeds or refuses with a sentence about the *request* — never about the name — so
/// anything mentioning the name is a registration fault.
#[test]
fn every_command_the_interface_names_is_registered() -> Result<(), BoxError> {
    let (_scratch, app) = harness("commands")?;
    let webview = window(&app)?;

    // `apps/desktop/src-tauri` → `apps/desktop` → `apps`, then down into `web`.
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/ipc/commands.ts"),
    )?;
    let start = source
        .find("export const COMMAND_NAMES = [")
        .ok_or("commands.ts no longer declares COMMAND_NAMES")?;
    let end = source[start..]
        .find("] as const;")
        .ok_or("COMMAND_NAMES is no longer closed by `] as const;`")?
        + start;

    let names: Vec<String> = source[start..end]
        .lines()
        .filter_map(|line| {
            let quoted = line.trim().strip_prefix('"')?;
            quoted
                .strip_suffix("\",")
                .or_else(|| quoted.strip_suffix('"'))
                .map(str::to_string)
        })
        .collect();

    // A guard on the parser itself: if the shape of `COMMAND_NAMES` changes, this fails loudly rather
    // than passing over an empty list.
    assert!(
        names.len() >= 30,
        "only {} command names were parsed out of commands.ts; the parser is wrong, not the handler",
        names.len()
    );

    for name in &names {
        let message = invoke(&webview, name, json!({}))
            .err()
            .unwrap_or_default()
            .to_lowercase();
        assert!(
            !message.contains("not found") && !message.contains("unknown command"),
            "`{name}` is named by the interface but is not in `generate_handler!` in main.rs: {message}"
        );
    }

    Ok(())
}

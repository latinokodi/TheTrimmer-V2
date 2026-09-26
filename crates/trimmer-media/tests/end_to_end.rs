//! The end-to-end test: generate a real clip, cut a real segment, check the real frames.
//!
//! Everything else in this workspace tests a decision. This test tests the *outcome*, and it is the
//! only one that can. It runs the actual ffmpeg on the actual machine:
//!
//! 1. Generate a 12-second clip whose every frame is identifiable — a distinct frame number burned
//!    into the picture, with a known keyframe grid (`-g 25`, so a keyframe every second at 25 fps).
//! 2. Cut a segment whose in point is deliberately *not* on a keyframe, so the head-patch path is
//!    exercised rather than the easy copy path.
//! 3. Assert the three things a silent bug would break, in the order they matter:
//!    * **the body is byte-identical to the source** — the frames from the keyframe on decode to the
//!      same MD5s, which is what "lossless" actually means and cannot be faked by a re-encode;
//!    * **the head lands on the mark** — the first frame of the output resembles the source's frame
//!      at the in point more than it resembles its neighbours;
//!    * **the length is exact** — every frame that was asked for is present.
//!
//! ## Why this is a test and not a script
//!
//! Because it is the one test that would have caught `-frames:v` dropping a frame in decode order
//! (V1 ADR-004) and a missing `-video_track_timescale` producing slow motion with exit code 0
//! (V1 ADR-001). Those are the failures that cost real time, and they are invisible to a test of
//! argument vectors. If ffmpeg is not installed the test **skips loudly** rather than passing
//! silently, because a media test that quietly does not run is a media test that will rot.

use std::path::{Path, PathBuf};
use std::process::Command;

use trimmer_core::{plan_cut, FrameRate, MediaInfo, PlanInvariant, Segment, Timescale};
use trimmer_media::{CutConfig, CutExecutor, PollPolicy, RunOptions, ToolPaths};

/// A temporary directory that removes itself.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "trimmer-e2e-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self { path }
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

/// True when ffmpeg and ffprobe can both be found.
fn tools() -> Option<ToolPaths> {
    ToolPaths::resolve().ok()
}

/// Run ffmpeg, failing the test with its own error output rather than a bare exit code.
fn ffmpeg(tools: &ToolPaths, args: &[&str]) {
    let output = Command::new(&tools.ffmpeg)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("could not run ffmpeg: {error}"));
    assert!(
        output.status.success(),
        "ffmpeg {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Run ffprobe with a format and collect stdout.
fn ffprobe(tools: &ToolPaths, args: &[&str]) -> String {
    let output = Command::new(&tools.ffprobe)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("could not run ffprobe: {error}"));
    assert!(
        output.status.success(),
        "ffprobe {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Generate the fixture clip.
///
/// 300 frames at 25 fps, a keyframe every 25 frames, and the frame number drawn into the picture so
/// that a frame compared against its neighbours is recognisably different. `-crf 18` keeps the
/// picture clean enough that similarity scores are meaningful.
fn generate_clip(tools: &ToolPaths, target: &Path) {
    ffmpeg(
        tools,
        &[
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=640x360:rate=25:duration=12",
            "-vf",
            "drawtext=text='%{n}':fontsize=48:fontcolor=white:x=20:y=20,format=yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-g",
            "25",
            "-keyint_min",
            "25",
            "-sc_threshold",
            "0",
            "-video_track_timescale",
            "90000",
            target.to_str().expect("a UTF-8 path"),
        ],
    );
}

/// The MD5 of each decoded frame in a window, as ffmpeg reports it.
///
/// `-map 0:v:0 -an` is not optional: without it the muxer hashes audio frames too and one audio
/// frame per 21 ms pads the list.
fn frame_hashes(tools: &ToolPaths, path: &Path, start_seconds: f64, count: usize) -> Vec<String> {
    let output = Command::new(&tools.ffmpeg)
        .args([
            "-hide_banner",
            "-v",
            "error",
            "-ss",
            &format!("{start_seconds:.6}"),
            "-i",
            path.to_str().expect("a UTF-8 path"),
            "-map",
            "0:v:0",
            "-an",
            "-frames:v",
            &count.to_string(),
            "-f",
            "framemd5",
            "-",
        ])
        .output()
        .unwrap_or_else(|error| panic!("could not hash frames: {error}"));
    assert!(
        output.status.success(),
        "framemd5 failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            line.split(',')
                .next_back()
                .map(|field| field.trim().to_owned())
        })
        .collect()
}

/// How many frames a file holds, from the container.
fn frame_count(tools: &ToolPaths, path: &Path) -> i64 {
    ffprobe(
        tools,
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_frames",
            "-of",
            "csv=p=0",
            path.to_str().expect("a UTF-8 path"),
        ],
    )
    .parse()
    .unwrap_or(-1)
}

/// Structural similarity between two stills, as ffmpeg reports it.
fn ssim(tools: &ToolPaths, a: &Path, b: &Path) -> f64 {
    let output = Command::new(&tools.ffmpeg)
        .args([
            "-hide_banner",
            "-nostats",
            "-i",
            a.to_str().expect("a UTF-8 path"),
            "-i",
            b.to_str().expect("a UTF-8 path"),
            "-lavfi",
            "ssim",
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap_or_else(|error| panic!("could not measure ssim: {error}"));
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    text.split("All:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(-1.0)
}

/// Extract one frame as a PNG.
fn extract_frame(tools: &ToolPaths, path: &Path, seconds: f64, target: &Path) {
    ffmpeg(
        tools,
        &[
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-ss",
            &format!("{seconds:.6}"),
            "-i",
            path.to_str().expect("a UTF-8 path"),
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            target.to_str().expect("a UTF-8 path"),
        ],
    );
}

#[tokio::test]
async fn a_head_patch_cut_is_lossless_exact_and_lands_on_the_mark() {
    let Some(tools) = tools() else {
        println!(
            "SKIPPED the end-to-end cut: ffmpeg or ffprobe is not resolvable on this machine. \
             Install it (winget install Gyan.FFmpeg) or point THE_TRIMMER_FFMPEG at one."
        );
        return;
    };
    // The text filter is not in every build; skip with a reason rather than failing on a build
    // choice that is not the code's fault.
    if !ffprobe(&tools, &["-hide_banner", "-filters"]).contains("drawtext") {
        println!("SKIPPED the end-to-end cut: this ffmpeg has no drawtext filter to label frames.");
        return;
    }

    let scratch = Scratch::new("headpatch");
    let source = scratch.join("source.mp4");
    generate_clip(&tools, &source);

    let executor = CutExecutor::new(tools.clone());
    let media: MediaInfo = executor
        .prober()
        .probe(&source)
        .await
        .expect("the fixture probes");

    assert_eq!(media.rate, FrameRate::FPS_25, "the fixture is 25 fps");
    assert_eq!(media.audio, None, "the fixture has no audio");
    assert_eq!(
        media.timebase,
        Timescale::NINETY_KHZ,
        "the timescale was pinned"
    );
    assert!(
        media.frame_count >= 290,
        "the fixture should be about 300 frames, got {}",
        media.frame_count
    );

    // A range that deliberately does not begin on a keyframe: 37 is between the keyframes at 25 and
    // 50, so this takes the head-patch path with a 13-frame head.
    let segment = Segment::new(media.path.clone(), "e2e", 37, 137);
    let keyframes = executor
        .prober()
        .keyframes(&media, 37, 137)
        .await
        .expect("keyframes list");
    let plan = plan_cut(&media, &segment, &keyframes).expect("the plan is legal");

    assert_eq!(
        plan.mode,
        trimmer_core::CutMode::HeadPatch,
        "the fixture's keyframe grid should force a head patch; keyframes were {:?}",
        keyframes.keyframes
    );
    assert_eq!(plan.requested_frames(), 100);
    assert!(
        plan.invariants
            .contains(&PlanInvariant::HeadMatchesSourceTimebase),
        "the plan must claim the timescale invariant"
    );
    assert!(
        plan.violated_invariants().is_empty(),
        "the plan contradicts itself: {:?}",
        plan.violated_invariants()
    );

    // Cut it.
    let output = scratch.join("cut.mp4");
    let preset = trimmer_core::delivery::standard_preset("master").expect("the master preset");
    let options = RunOptions {
        policy: PollPolicy::long(),
        ..RunOptions::default()
    };
    let outcome = executor
        .cut_with_plan(
            &media,
            &plan,
            &preset,
            &CutConfig::default(),
            &output,
            &options,
        )
        .await
        .expect("the cut runs");

    assert!(
        output.is_file() && output.metadata().expect("metadata").len() > 1_000,
        "the cut wrote no file"
    );
    assert!(
        outcome.is_lossless(),
        "a head patch keeps the body's packets"
    );

    // ---- 1. Every frame that was asked for is present. ---------------------------------
    let produced = frame_count(&tools, &output);
    assert!(
        produced >= plan.requested_frames(),
        "the output holds {produced} frames but {} were asked for",
        plan.requested_frames()
    );
    // A stream copy stops on a packet boundary, so a few frames past the out point are expected and
    // are not a defect. Anything more than a handful would be.
    assert!(
        produced <= plan.requested_frames() + 6,
        "the output overshot by {} frames, which is more than a packet boundary explains",
        produced - plan.requested_frames()
    );

    // ---- 2. The body is byte-identical to the source. ----------------------------------
    //
    // The frames from the keyframe on are the original packets, so their MD5s must match exactly.
    // This is the assertion that a re-encode cannot pass, which is what makes it worth making.
    let body_start_frame = plan.keyframe.expect("a head patch has a keyframe");
    let body_source_seconds = media.rate.seconds_of(body_start_frame);
    let body_output_seconds = media.rate.seconds_of(body_start_frame - plan.start_frame);

    let sample = 20usize;
    let source_hashes = frame_hashes(&tools, &source, body_source_seconds, sample);
    let output_hashes = frame_hashes(&tools, &output, body_output_seconds, sample);

    assert!(
        source_hashes.len() >= sample - 2,
        "only {} source frames were hashed",
        source_hashes.len()
    );
    assert!(
        output_hashes.len() >= sample - 2,
        "only {} output frames were hashed",
        output_hashes.len()
    );
    let compared = source_hashes.len().min(output_hashes.len());
    let matching = (0..compared)
        .filter(|index| source_hashes[*index] == output_hashes[*index])
        .count();
    assert_eq!(
        matching,
        compared,
        "the copied body is not byte-identical: {matching} of {compared} frames match.\n\
         source: {:?}\n output: {:?}",
        &source_hashes[..compared.min(4)],
        &output_hashes[..compared.min(4)]
    );

    // ---- 3. The head lands on the mark. -------------------------------------------------
    //
    // A re-encoded frame never hashes equal, so this is a similarity comparison. The first frame of
    // the output must resemble the source's frame at the in point more than its neighbours.
    let source_frame_at_in = scratch.join("source-in.png");
    let source_frame_before = scratch.join("source-before.png");
    let output_first = scratch.join("output-first.png");

    extract_frame(
        &tools,
        &source,
        media.rate.seconds_of(plan.start_frame),
        &source_frame_at_in,
    );
    extract_frame(
        &tools,
        &source,
        media.rate.seconds_of(plan.start_frame - 5),
        &source_frame_before,
    );
    extract_frame(&tools, &output, 0.0, &output_first);

    let at_mark = ssim(&tools, &output_first, &source_frame_at_in);
    let five_before = ssim(&tools, &output_first, &source_frame_before);

    assert!(at_mark > 0.0, "the similarity could not be measured");
    assert!(
        at_mark > 0.90,
        "the first output frame only reaches {at_mark:.4} against the source's frame {}: the head \
         did not land on the mark",
        plan.start_frame
    );
    assert!(
        at_mark > five_before,
        "the first output frame is as similar to the source's frame five frames earlier \
         ({five_before:.4}) as to the mark ({at_mark:.4}), so it is off by frames"
    );

    // ---- 4. The output is not in slow motion. ------------------------------------------
    //
    // The timescale bug produces a file that plays at the wrong rate with exit code 0, so the
    // duration is the detector. 100 frames at 25 fps is 4 s, and a few frames of overshoot is
    // within a tenth of a second.
    let duration: f64 = ffprobe(
        &tools,
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=duration",
            "-of",
            "csv=p=0",
            path_of(&output),
        ],
    )
    .parse()
    .unwrap_or(-1.0);
    assert!(
        (duration - 4.0).abs() < 0.4,
        "the cut runs {duration:.3}s for 100 frames at 25 fps; 4.0s was asked for. A wrong \
         timescale rescales the copied body into slow motion and still exits 0"
    );

    // And the steps are on the record, which is what the proof panel renders. The fixture has no
    // audio, so the body is one copy rather than a picture copy, a sound copy and a mux.
    assert!(
        outcome.steps.len() >= 3,
        "a head patch is at least a head encode, a body copy and a join; got {:?}",
        outcome
            .steps
            .iter()
            .map(|step| step.label.as_str())
            .collect::<Vec<_>>()
    );
    for expected in ["head encode", "body", "join"] {
        assert!(
            outcome
                .steps
                .iter()
                .any(|step| step.label.contains(expected)),
            "no step mentions {expected:?}: {:?}",
            outcome
                .steps
                .iter()
                .map(|step| step.label.as_str())
                .collect::<Vec<_>>()
        );
    }
    let head_step = outcome
        .steps
        .iter()
        .find(|step| step.label.starts_with("head encode"))
        .expect("a head encode step");
    let args = head_step.args.join(" ");
    assert!(
        args.contains("-video_track_timescale 90000"),
        "the head encode did not pin the source timescale: {args}"
    );
    assert!(
        !args.contains("-frames:v"),
        "the cut used -frames:v, which counts packets in decode order and can drop a wanted frame"
    );
}

/// A path as a `&str`, for a probe call.
fn path_of(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

#[tokio::test]
async fn a_cut_that_lands_on_a_keyframe_copies_everything() {
    let Some(tools) = tools() else {
        println!("SKIPPED the keyframe-aligned cut: ffmpeg is not resolvable on this machine.");
        return;
    };
    if !ffprobe(&tools, &["-hide_banner", "-filters"]).contains("drawtext") {
        println!("SKIPPED: this ffmpeg has no drawtext filter.");
        return;
    }

    let scratch = Scratch::new("copy");
    let source = scratch.join("source.mp4");
    generate_clip(&tools, &source);

    let executor = CutExecutor::new(tools.clone());
    let media = executor.prober().probe(&source).await.expect("probed");

    // 50 is a keyframe in the fixture grid.
    let segment = Segment::new(media.path.clone(), "aligned", 50, 150);
    let keyframes = executor
        .prober()
        .keyframes(&media, 50, 150)
        .await
        .expect("keyframes");
    let plan = plan_cut(&media, &segment, &keyframes).expect("a legal plan");
    assert_eq!(
        plan.mode,
        trimmer_core::CutMode::Copy,
        "an in point of 50 should be a keyframe; the grid is {:?}",
        keyframes.keyframes
    );

    let output = scratch.join("aligned.mp4");
    let preset = trimmer_core::delivery::standard_preset("master").expect("the master preset");
    let options = RunOptions {
        policy: PollPolicy::long(),
        ..RunOptions::default()
    };
    let outcome = executor
        .cut_with_plan(
            &media,
            &plan,
            &preset,
            &CutConfig::default(),
            &output,
            &options,
        )
        .await
        .expect("the copy runs");

    assert_eq!(outcome.steps.len(), 1, "a copy is a single ffmpeg call");
    assert_eq!(outcome.reencode_fraction(), 0.0, "nothing was re-encoded");

    let produced = frame_count(&tools, &output);
    assert!(
        produced >= 100,
        "the output holds {produced} frames but 100 were asked for"
    );

    // Every frame is the original packet, so the hashes match from the very first frame.
    let source_hashes = frame_hashes(&tools, &source, media.rate.seconds_of(50), 20);
    let output_hashes = frame_hashes(&tools, &output, 0.0, 20);
    let compared = source_hashes.len().min(output_hashes.len());
    assert!(compared >= 18, "only {compared} frames could be compared");
    assert_eq!(
        source_hashes[..compared],
        output_hashes[..compared],
        "a pure copy must be byte-identical from its first frame"
    );
}

/// The capability report against the *real* ffmpeg on this machine.
///
/// The parser has already been wrong twice in ways that produced a plausible answer rather than an
/// error — once demanding six-character flags, once swallowing the name column — and both times the
/// unit tests passed because their fixtures were what the author imagined ffmpeg printed. This test
/// asks ffmpeg itself, so a fixture that does not match reality cannot hide the bug.
#[tokio::test]
async fn the_capability_report_is_believed_by_the_ffmpeg_that_is_actually_installed() {
    let Some(tools) = tools() else {
        println!("SKIPPED the capability report: ffmpeg is not resolvable on this machine.");
        return;
    };
    let prober = trimmer_media::Prober::new(tools);
    let found = prober
        .capabilities()
        .await
        .expect("the listings are readable");

    // The encoders the product cannot work without.
    assert!(
        found.has_encoder("libx264"),
        "no libx264: {:?}",
        found.encoders.len()
    );
    // The muxer every cut is written with, and the one a delivery preset needs.
    assert!(
        found.has_muxer("mp4"),
        "no mp4 muxer among {} muxers",
        found.muxers.len()
    );
    // The filter the loudness presets need, and the one the head check needs.
    assert!(
        found.has_filter("loudnorm"),
        "no loudnorm among {} filters",
        found.filters.len()
    );
    assert!(
        found.has_filter("ssim"),
        "no ssim among {} filters",
        found.filters.len()
    );

    // And the report must not contradict the checks, because a false NO sends a user hunting.
    let report = found.doctor_report();
    assert!(report.contains("libx264     yes"), "{report}");
    assert!(report.contains("mp4         yes"), "{report}");
    assert!(report.contains("loudnorm    yes"), "{report}");
}

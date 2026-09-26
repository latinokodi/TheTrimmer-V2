//! The differential test: the Rust core against the V1 engine.
//!
//! This is the test that makes the rewrite defensible. `trimmer-core` is a reimplementation of a
//! planner that was validated on real broadcast material, and a reimplementation is exactly the
//! kind of thing that is quietly *almost* right. So the original is kept as ground truth: this test
//! generates cases, runs each one through the Rust core, runs the same case through the V1 engine
//! by way of `tools/oracle/run_oracle.py`, and compares the answers.
//!
//! ## What is compared
//!
//! * **Timecode** — the drop-frame arithmetic in both directions, over every rate the product
//!   supports and around the boundaries where the label skipping happens (the first minute of each
//!   ten, the hour, and the frames either side of a skipped label).
//! * **The cut plan** — mode, keyframe, head and body frame counts, and whether the engine
//!   refused and why. The V1 planner is driven with a supplied keyframe grid rather than an
//!   ffprobe call, so the decision under test is a function of the numbers and nothing else.
//! * **Caption retiming** — the cues that survive, the ones clamped at the marks and the ones
//!   dropped for being too short.
//!
//! ## What is deliberately *not* compared
//!
//! The `describe()` text. It is a presentation format in both implementations and could be
//! reworded on either side without the decision changing; comparing it would make the test fail
//! for cosmetic reasons and teach everyone to ignore it. The fields are compared instead.
//!
//! ## When the test cannot run
//!
//! It needs a Python interpreter and the V1 checkout. When either is missing the test **skips**,
//! loudly, and passes — because a test that cannot distinguish "not applicable here" from "the
//! engine is wrong" gets deleted by the first person it inconveniences. A skipped run is printed
//! with the reason so it is visible in the log rather than invisible in the count.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use trimmer_core::{caption, plan_cut, timecode, FrameRate, KeyframeGrid, MediaInfo, MediaPath,
    Segment};

/// Where the V1 engine lives, overridable for a checkout in an unusual place.
fn v1_root() -> PathBuf {
    std::env::var("THE_TRIMMER_V1_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"H:\THEROLLUPFILES\TheTrimmer"))
}

/// The oracle runner, relative to this crate's manifest directory.
fn oracle_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tools")
        .join("oracle")
        .join("run_oracle.py")
}

/// Why the oracle cannot run here, if it cannot.
fn unavailable() -> Option<String> {
    let script = oracle_script();
    if !script.is_file() {
        return Some(format!("the oracle script is not at {}", script.display()));
    }
    let root = v1_root();
    if !root.join("trimmer").join("timecode.py").is_file() {
        return Some(format!(
            "the V1 engine is not at {} (set THE_TRIMMER_V1_ROOT)",
            root.display()
        ));
    }
    None
}

/// Run cases through the V1 engine and return its answers, one per case.
///
/// Panics when the oracle cannot be run at all, because a test that silently treats "the oracle
/// crashed" as "the oracle agreed" is worse than no test.
fn ask_the_oracle(cases: &[Value]) -> Vec<Value> {
    let mut child = Command::new("python")
        .arg(oracle_script())
        .env("THE_TRIMMER_V1_ROOT", v1_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("could not start python: {error}"));

    {
        let stdin = child.stdin.as_mut().expect("stdin was piped");
        for case in cases {
            writeln!(stdin, "{case}").expect("could not write a case to the oracle");
        }
    }

    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("the oracle did not finish: {error}"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the oracle exited {}:\n{stderr}",
        output.status
    );

    let answers: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("the oracle wrote unreadable output: {error}\n{line}"))
        })
        .collect();

    assert_eq!(
        answers.len(),
        cases.len(),
        "the oracle answered {} of {} cases:\n{stderr}",
        answers.len(),
        cases.len()
    );
    answers
}

/// Every rate the product supports, as the oracle spells them.
const RATES: [&str; 8] = [
    "24000/1001",
    "24",
    "25",
    "30000/1001",
    "30",
    "50",
    "60000/1001",
    "60",
];

/// Frames to compare at. Chosen around the places the drop-frame count skips labels: the first
/// second, the first minute, the tenth minute, the hour, and either side of a skip.
const FRAMES: [i64; 24] = [
    0, 1, 2, 3, 24, 25, 29, 30, 31, 1_798, 1_799, 1_800, 1_801, 1_802, 2_999, 3_000, 17_982,
    17_983, 17_984, 10_000, 107_891, 107_892, 108_000, 1_234_567,
];

fn timecode_format_cases() -> Vec<Value> {
    let mut cases = Vec::new();
    for rate in RATES {
        for frame in FRAMES {
            cases.push(json!({"op": "timecode_format", "frame": frame, "rate": rate}));
            // And the non-drop rendering, which is what a non-drop conform looks like.
            cases.push(
                json!({"op": "timecode_format", "frame": frame, "rate": rate, "drop": false}),
            );
        }
    }
    cases
}

fn timecode_parse_cases() -> Vec<Value> {
    let mut cases = Vec::new();
    for rate in RATES {
        for frame in FRAMES {
            // Round trip through the *V1 engine's* rendering, so the parse is tested against real
            // spellings rather than against spellings this implementation happens to produce.
            let text = ask_the_oracle(&[json!({
                "op": "timecode_format", "frame": frame, "rate": rate
            })])[0]["text"]
                .as_str()
                .expect("a rendered timecode")
                .to_owned();
            cases.push(json!({"op": "timecode_parse", "text": text, "rate": rate}));
        }
    }
    // Spellings a person types, including the refusals.
    for text in [
        "00:00:00:00",
        "01:00:00:00",
        "00:01:00;02",
        "00:01:00,02",
        "00:01:00.02",
        "10",
        "1.5",
        "0",
        "  12.04  ",
        "00:00:02",
        "00:60:00:00",
        "00:00:60:00",
        "00:00:00:25",
        "abc",
        "",
        "1:2:3:4:5",
        "00:00:00;00",
    ] {
        for rate in ["25", "30000/1001"] {
            cases.push(json!({"op": "timecode_parse", "text": text, "rate": rate}));
        }
    }
    cases
}

/// The media the plan cases are made against.
fn oracle_media() -> MediaInfo {
    MediaInfo {
        path: MediaPath::new(r"H:\masters\oracle.mp4"),
        codec: "h264".to_owned(),
        pix_fmt: "yuv420p".to_owned(),
        width: 1920,
        height: 1080,
        rate: FrameRate::FPS_29_97,
        average_rate: Some(FrameRate::FPS_29_97),
        timebase: trimmer_core::Timescale::NINETY_KHZ,
        frame_count: 216_000,
        audio: Some(trimmer_core::AudioFormat {
            codec: "aac".to_owned(),
            sample_rate: 48_000,
            channels: 2,
        }),
        size_bytes: 6_000_000_000,
        start_time: 0.0,
    }
}

/// The plan cases: a spread of ranges over a keyframe grid, plus the refusals.
fn plan_cases() -> Vec<Value> {
    let keyframes = vec![0, 250, 900, 1_150, 1_400, 1_650, 4_000, 100_000];
    let mut cases = Vec::new();
    for (start, end) in [
        (1_000, 1_600),    // between keyframes: a head patch
        (1_150, 1_600),    // on a keyframe: a pure copy
        (1_100, 1_110),    // a keyframe outside the range: a whole re-encode
        (0, 500),          // from the very start
        (215_000, 216_000), // to the very end
        (4_000, 4_001),    // one frame
        (500, 501),        // one frame between keyframes
        (1_150, 1_151),    // one frame on a keyframe
    ] {
        cases.push(json!({
            "op": "plan",
            "in_frame": start,
            "out_frame": end,
            "keyframes": keyframes,
            "rate": "30000/1001",
            "timescale": 90000,
        }));
    }
    // Refusals, which must be refusals on both sides.
    cases.push(json!({
        "op": "plan", "in_frame": 1_600, "out_frame": 1_600,
        "keyframes": keyframes, "rate": "30000/1001"
    }));
    cases.push(json!({
        "op": "plan", "in_frame": 216_000, "out_frame": 216_001,
        "keyframes": keyframes, "rate": "30000/1001"
    }));
    cases.push(json!({
        "op": "plan", "in_frame": 1_000, "out_frame": 1_600,
        "keyframes": keyframes, "rate": "30000/1001", "codec": "prores"
    }));
    cases
}

/// Caption cases: cues that straddle a mark, cues entirely outside, and the drop threshold.
fn caption_cases() -> Vec<Value> {
    let cues = json!([
        {"start": 0.0, "end": 1.1, "text": "almost over"},
        {"start": 1.0, "end": 4.0, "text": "well inside"},
        {"start": 4.1, "end": 5.0, "text": "short"},
        {"start": 9.0, "end": 11.0, "text": "crosses the out point"},
        {"start": 20.0, "end": 21.0, "text": "after"}
    ]);
    let mut cases = Vec::new();
    for (start, end, min_overlap) in [
        (1.0, 10.0, 0.25),
        (2.0, 10.0, 0.25),
        (0.0, 30.0, 0.25),
        (1.0, 10.0, 1.0),
        (25.0, 30.0, 0.25),
    ] {
        cases.push(json!({
            "op": "captions", "start": start, "end": end,
            "min_overlap": min_overlap, "cues": cues
        }));
    }
    cases
}

/// The Rust core's answer to a case, in the same shape the oracle answers in.
fn rust_answer(case: &Value) -> Value {
    let op = case["op"].as_str().expect("an op");
    match op {
        "timecode_format" => {
            let rate = FrameRate::parse(case["rate"].as_str().expect("a rate")).expect("a rate");
            let frame = case["frame"].as_i64().expect("a frame");
            let drop = case["drop"].as_bool();
            json!({"text": timecode::format_timecode(frame, rate, drop)})
        }
        "timecode_parse" => {
            let rate = FrameRate::parse(case["rate"].as_str().expect("a rate")).expect("a rate");
            let text = case["text"].as_str().expect("text");
            match timecode::parse_timecode(text, rate) {
                Ok(frame) => json!({"frame": frame}),
                Err(error) => json!({"error": error.to_string()}),
            }
        }
        "plan" => {
            let media = oracle_media();
            let start = case["in_frame"].as_i64().expect("an in point");
            let end = case["out_frame"].as_i64().expect("an out point");
            let segment = Segment::new(media.path.clone(), "oracle", start, end);
            let keyframes: Vec<i64> = case["keyframes"]
                .as_array()
                .expect("keyframes")
                .iter()
                .map(|value| value.as_i64().expect("a keyframe"))
                .collect();
            let grid = KeyframeGrid::new(keyframes, start, end);
            let mut media = media;
            if let Some(codec) = case["codec"].as_str() {
                media.codec = codec.to_owned();
            }
            match plan_cut(&media, &segment, &grid) {
                Ok(plan) => json!({
                    "mode": match plan.mode {
                        trimmer_core::CutMode::HeadPatch => "headpatch",
                        trimmer_core::CutMode::Copy => "copy",
                        trimmer_core::CutMode::Reencode => "reencode",
                    },
                    "keyframe": plan.keyframe,
                    "head_frames": plan.head_frames,
                    "body_frames": plan.body_frames,
                    "requested": plan.requested_frames(),
                    "note_count": plan.notes.len(),
                    "rate_text": media.rate.as_ffmpeg(),
                    "start_timecode": media.timecode_of(plan.start_frame),
                    "end_timecode": media.timecode_of(plan.end_frame),
                }),
                Err(error) => json!({"error": error.to_string()}),
            }
        }
        "captions" => {
            let cues: Vec<caption::Cue> = case["cues"]
                .as_array()
                .expect("cues")
                .iter()
                .map(|cue| {
                    caption::Cue::new(
                        cue["start"].as_f64().expect("a start"),
                        cue["end"].as_f64().expect("an end"),
                        cue["text"].as_str().unwrap_or(""),
                    )
                })
                .collect();
            let start = case["start"].as_f64().expect("a start");
            let end = case["end"].as_f64().expect("an end");
            let min_overlap = case["min_overlap"].as_f64().unwrap_or(caption::MIN_OVERLAP);
            match caption::retime(&cues, start, end, min_overlap) {
                Ok(result) => json!({
                    "cues": result.cues.iter().map(|cue| json!({
                        "start": round3(cue.start),
                        "end": round3(cue.end),
                    })).collect::<Vec<_>>(),
                    "clamped": result.clamped.len(),
                    "dropped": result.dropped.len(),
                    "outside": result.outside,
                }),
                Err(error) => json!({"error": error.to_string()}),
            }
        }
        other => panic!("unknown op {other}"),
    }
}

/// Round to three decimals, the resolution an SRT file has and the oracle rounds to.
fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Compare the two answers for one case, and report both when they differ.
fn compare(case: &Value, oracle: &Value, rust: &Value) {
    // Both refused: the engine's own error text is not part of the comparison, because the two
    // implementations have different prose. What matters is that *neither* silently accepted.
    if oracle.get("error").is_some() && rust.get("error").is_some() {
        return;
    }
    assert!(
        oracle.get("error").is_none(),
        "the V1 engine refused a case the Rust core accepted.\n  case:   {case}\n  oracle: {oracle}\n  rust:   {rust}"
    );
    assert!(
        rust.get("error").is_none(),
        "the Rust core refused a case the V1 engine accepted.\n  case:   {case}\n  oracle: {oracle}\n  rust:   {rust}"
    );

    if case["op"] == "plan" {
        // `note_count` is advisory prose in both implementations and is not part of the decision;
        // everything that determines the file is compared exactly.
        for field in [
            "mode",
            "keyframe",
            "head_frames",
            "body_frames",
            "requested",
            "rate_text",
            "start_timecode",
            "end_timecode",
        ] {
            let left = normalise_plan_field(field, oracle);
            let right = normalise_plan_field(field, rust);
            assert_eq!(
                left, right,
                "the two engines disagree about `{field}`.\n  case:   {case}\n  oracle: {oracle}\n  rust:   {rust}"
            );
        }
    } else {
        assert_eq!(
            oracle, rust,
            "the two engines disagree.\n  case:   {case}\n  oracle: {oracle}\n  rust:   {rust}"
        );
    }
}

/// Map a representational difference onto a common form before comparing.
///
/// There is exactly one, and it is not a behaviour: V1 has no `Option`, so "this plan has no
/// keyframe" is spelled `keyframe = -1`, while V2 spells it `null`. Both mean the same thing, both
/// appear only in `reencode` mode, and comparing the spellings instead of the meaning would make
/// this test fail for a reason nobody should have to think about.
///
/// The mapping is deliberately narrow — `-1` becomes `null` **only** for the keyframe of a
/// `reencode` plan — so it cannot mask a real disagreement about a frame number anywhere else.
fn normalise_plan_field(field: &str, answer: &Value) -> Value {
    let value = answer.get(field).cloned().unwrap_or(Value::Null);
    let is_reencode = answer.get("mode").and_then(Value::as_str) == Some("reencode");
    if field == "keyframe" && is_reencode && value == Value::Number((-1).into()) {
        return Value::Null;
    }
    value
}

/// Run a batch of cases through both engines.
fn differential(label: &str, cases: Vec<Value>) {
    if let Some(reason) = unavailable() {
        println!("SKIPPED the {label} oracle: {reason}");
        return;
    }
    let answers = ask_the_oracle(&cases);
    let mut checked = 0usize;
    for (case, oracle) in cases.iter().zip(answers.iter()) {
        compare(case, oracle, &rust_answer(case));
        checked += 1;
    }
    assert!(checked > 0, "the {label} oracle checked nothing");
    println!("the {label} oracle agreed on {checked} case(s)");
}

#[test]
fn the_timecode_renderer_agrees_with_the_v1_engine() {
    differential("timecode-format", timecode_format_cases());
}

#[test]
fn the_timecode_parser_agrees_with_the_v1_engine() {
    differential("timecode-parse", timecode_parse_cases());
}

#[test]
fn the_cut_planner_agrees_with_the_v1_engine() {
    differential("plan", plan_cases());
}

#[test]
fn the_caption_retimer_agrees_with_the_v1_engine() {
    differential("captions", caption_cases());
}

#[test]
fn the_oracle_can_be_found_or_the_reason_is_named() {
    // A guard on the guard: if the oracle goes missing, this says so explicitly rather than
    // letting four silent skips look like four passes.
    match unavailable() {
        None => println!("the oracle is available at {}", oracle_script().display()),
        Some(reason) => println!("the oracle is NOT available: {reason}"),
    }
}

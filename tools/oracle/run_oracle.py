#!/usr/bin/env python
"""The V1 engine, answering the same questions the Rust core answers.

This is the other half of the differential test. `crates/trimmer-core/tests/oracle.rs` generates
cases, runs them through the Rust core, and runs the same cases through *this* script, which
executes the V1 engine that was validated on real broadcast material. Any disagreement is a
failure, and the disagreement is printed with both answers.

The V1 engine is imported from its own checkout rather than copied here. That matters: a vendored
copy would be a fork, and a fork drifts. The whole value of this oracle is that it is the original
implementation, unmodified, still passing its own test suite.

Usage
-----
    python tools/oracle/run_oracle.py < cases.jsonl > answers.jsonl
    python tools/oracle/run_oracle.py --v1-root H:\\path\\to\\TheTrimmer < cases.jsonl

Each input line is a JSON object with an `op` field. Each output line is a JSON object with the
answer, or `{"error": "..."}` when the engine refused. Nothing is written to stdout except the
answers, so the output can be piped straight into the Rust test.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from fractions import Fraction
from pathlib import Path

DEFAULT_V1_ROOT = Path(r"H:\THEROLLUPFILES\TheTrimmer")


def load_engine(v1_root: Path):
    """Import the V1 engine from its checkout.

    The package is `trimmer`, which is also the name of this project's Rust crates — so it is
    imported by path rather than by installing it, and the sys.path entry is removed afterwards
    so nothing else in this process can accidentally resolve to it.
    """
    if not (v1_root / "trimmer" / "timecode.py").is_file():
        raise SystemExit(
            f"the V1 engine is not at {v1_root}. Pass --v1-root, or set THE_TRIMMER_V1_ROOT."
        )
    sys.path.insert(0, str(v1_root))
    try:
        from trimmer import ffmpeg as ff  # noqa: F401  (imported for the planner's sake)
        from trimmer import subtitles as subs
        from trimmer import timecode as tc
        from trimmer.trim import TrimSpec, plan_trim
    finally:
        sys.path.pop(0)
    return tc, subs, plan_trim, TrimSpec


def rate_of(text):
    from fractions import Fraction as F

    if "/" in str(text):
        numerator, denominator = str(text).split("/", 1)
        return F(int(numerator), int(denominator))
    return F(str(text)).limit_denominator(1_000_000)


def media_from(case):
    """Build the `MediaInfo` the planner needs, without touching a filesystem.

    `trimmer.trim` is handed a probed media object rather than probing one itself, so the oracle
    can describe a file that does not exist. That is the point: the planner's decisions are about
    numbers, and requiring a real two-hour master to test them would mean testing them rarely.
    """
    tc, _, _, _ = ENGINE
    from fractions import Fraction

    from trimmer.ffmpeg import AudioStream, MediaInfo

    return MediaInfo(
        path=Path(case.get("path", r"H:\masters\oracle.mp4")),
        codec=case.get("codec", "h264"),
        width=case.get("width", 1920),
        height=case.get("height", 1080),
        pix_fmt=case.get("pix_fmt", "yuv420p"),
        rate=rate_of(case.get("rate", "30000/1001")),
        average_rate=rate_of(case["average_rate"]) if case.get("average_rate") else None,
        timebase=Fraction(1, case.get("timescale", 90000)),
        frames=case.get("frames", 216000),
        duration=0.0,
        audio=AudioStream(codec="aac", sample_rate=48000, channels=2)
        if case.get("has_audio", True)
        else None,
        size_bytes=0,
        start_time=0.0,
    )


def answer_timecode_parse(case):
    tc, _, _, _ = ENGINE
    rate = rate_of(case["rate"])
    frame = tc.parse_timecode(case["text"], rate)
    return {"frame": frame}


def answer_timecode_format(case):
    tc, _, _, _ = ENGINE
    rate = rate_of(case["rate"])
    drop = case.get("drop")
    text = tc.format_timecode(case["frame"], rate, drop)
    return {"text": text}


def answer_plan(case):
    """The cut planner's decision, as data.

    The `describe()` output is deliberately not used: it is a presentation format and could be
    reworded without the decision changing. The fields are read directly.
    """
    tc, _, plan_trim, TrimSpec = ENGINE
    from trimmer.ffmpeg import FileFacts

    media = media_from(case)
    rate = media.rate
    spec = TrimSpec(
        source=media.path,
        output=Path(case.get("output", r"H:\masters\out.mp4")),
        in_frame=case["in_frame"],
        out_frame=case["out_frame"],
        crf=case.get("crf", 18),
        preset=case.get("preset", "veryfast"),
        concat_offset=case.get("concat_offset", 0),
        head_audio_copy=case.get("head_audio_copy", False),
        audio_bitrate="192k",
        overwrite=True,
        subtitles=None,
    )

    # The planner asks ffmpeg for keyframes. The oracle supplies them instead, so the decision is
    # a pure function of the case rather than of whatever ffprobe would have said.
    keyframe_times = [frame / float(rate) for frame in case.get("keyframes", [])]

    import trimmer.ffmpeg as ff

    original = ff.keyframes
    ff.keyframes = lambda path, start, end: [
        time for time in keyframe_times if start - 1.0 <= time <= end + 1.0
    ]
    try:
        plan = plan_trim(spec, media)
    finally:
        ff.keyframes = original

    return {
        "mode": plan.mode,
        "keyframe": plan.keyframe,
        "head_frames": plan.head_frames,
        "body_frames": plan.body_frames,
        "requested": plan.requested,
        "note_count": len(plan.notes),
        "rate_text": tc.format_rate(rate),
        "start_timecode": tc.format_timecode(spec.in_frame, rate),
        "end_timecode": tc.format_timecode(spec.out_frame, rate),
    }


def answer_captions(case):
    _, subs, _, _ = ENGINE

    cues = [subs.Cue(cue["start"], cue["end"], cue.get("text", "")) for cue in case["cues"]]
    result = subs.retime(cues, case["start"], case["end"], case.get("min_overlap", subs.MIN_OVERLAP))
    return {
        "cues": [
            {"start": round(cue.start, 3), "end": round(cue.end, 3)} for cue in result.cues
        ],
        "clamped": len(result.clamped),
        "dropped": len(result.dropped),
        "outside": result.outside,
    }


HANDLERS = {
    "timecode_parse": answer_timecode_parse,
    "timecode_format": answer_timecode_format,
    "plan": answer_plan,
    "captions": answer_captions,
}

ENGINE = None


def main() -> int:
    global ENGINE

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--v1-root",
        default=os.environ.get("THE_TRIMMER_V1_ROOT", str(DEFAULT_V1_ROOT)),
        help="the checkout holding the V1 engine",
    )
    arguments = parser.parse_args()

    try:
        ENGINE = load_engine(Path(arguments.v1_root))
    except SystemExit as error:
        print(f"oracle: {error}", file=sys.stderr)
        return 2

    written = 0
    for line_number, line in enumerate(sys.stdin, start=1):
        line = line.strip()
        if not line:
            continue
        try:
            case = json.loads(line)
        except json.JSONDecodeError as error:
            print(json.dumps({"error": f"case {line_number} is not JSON: {error}"}))
            written += 1
            continue
        handler = HANDLERS.get(case.get("op"))
        if handler is None:
            print(json.dumps({"error": f"unknown op {case.get('op')!r}"}))
            written += 1
            continue
        try:
            print(json.dumps(handler(case)))
        except Exception as error:  # noqa: BLE001 - the refusal IS the answer
            print(json.dumps({"error": f"{type(error).__name__}: {error}"}))
        written += 1

    print(f"oracle: answered {written} case(s)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

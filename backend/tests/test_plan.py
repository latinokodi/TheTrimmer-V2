"""What a range will do, decided before ffmpeg is started.

## Why this is the test that matters most

The plan is the product's central claim made checkable: *the body is the original packets*.
It is decided here, from the keyframe list, and every branch of it is reachable without
cutting anything. `ff.keyframes` is the single process launch involved, and it is replaced —
so this file is a few milliseconds and there is no reason not to run it on every change.
"""

from __future__ import annotations

from fractions import Fraction
from pathlib import Path
from types import SimpleNamespace

import pytest

from trimmer import trim as cutter
from trimmer.ffmpeg import AudioStream, MediaInfo

SOURCE = Path("C:/media/reel.mp4")
NTSC_30 = Fraction(30000, 1001)
CINEMA = Fraction(24000, 1001)


def media(frames: int = 500, rate: Fraction = Fraction(25), codec: str = "h264") -> MediaInfo:
    """A source with the fields the planner reads. No file behind it, deliberately: the
    planner's decisions are arithmetic and arithmetic does not need a disk."""
    return MediaInfo(
        path=SOURCE,
        codec=codec,
        width=1920,
        height=1080,
        pix_fmt="yuv420p",
        rate=rate,
        average_rate=rate,
        timebase=Fraction(1, 12800),
        frames=frames,
        duration=frames / float(rate),
        audio=AudioStream(codec="aac", sample_rate=48000, channels=2),
        size_bytes=1_000_000,
    )


def spec(in_frame: int, out_frame: int, **kwargs) -> cutter.TrimSpec:
    return cutter.TrimSpec(
        source=SOURCE,
        output=Path("C:/media/out.mp4"),
        in_frame=in_frame,
        out_frame=out_frame,
        **kwargs,
    )


@pytest.fixture
def keyframes(monkeypatch):
    """Stand in for ffprobe's keyframe list, so no process is launched."""

    def install(*seconds: float) -> None:
        monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: [float(s) for s in seconds])

    return install


# ---------------------------------------------------------------------------------------
# The three pieces: the run to the opening keyframe, the copied packets, the run to the mark
# ---------------------------------------------------------------------------------------

def test_an_in_point_on_a_keyframe_re_encodes_only_the_tail(keyframes):
    # Keyframes at 4.0s and 8.0s (frames 100 and 200 at 25 fps). The in point is the first
    # keyframe, so nothing needs re-encoding at the front; the body is the original packets
    # from 100 to 199; the tail is re-encoded from 200 to the out point.
    keyframes(4.0, 8.0)
    plan = cutter.plan_trim(spec(100, 300), media())
    assert plan.mode == "copy"
    assert (plan.keyframe, plan.head_frames, plan.body_frames, plan.tail_frames) == \
        (100, 0, 100, 100)
    assert plan.requested == 200


def test_both_ends_are_re_encoded_and_the_middle_is_copied(keyframes):
    # Keyframes at frames 100, 200 and 300 at 25 fps; in point at frame 150, out point at 350.
    # The in point sits inside the first GOP, so frames 150..199 are re-encoded; 200..299 are
    # the original packets; and 300..349 are re-encoded again so the segment ends on the frame
    # that was asked for. Getting the first split wrong is the slow-motion bug: the copied
    # body is rescanned against a timescale it did not have.
    keyframes(4.0, 8.0, 12.0)
    plan = cutter.plan_trim(spec(150, 350), media())
    assert plan.mode == "headpatch"
    assert plan.keyframe == 200
    assert plan.head_frames == 50
    assert plan.body_frames == 100
    assert plan.tail_frames == 50
    assert plan.requested == 200


def test_the_tail_re_encodes_from_the_last_keyframe_before_the_out_point(keyframes):
    """Its length is whatever is left after that keyframe, not a whole GOP."""
    keyframes(4.0, 8.0, 12.0)
    plan = cutter.plan_trim(spec(150, 450), media())
    # Out point is frame 450; the last keyframe before it is frame 300, so 150 frames are
    # re-encoded at the tail -- not until some later keyframe, and not a whole GOP.
    assert plan.tail_frames == 150
    assert plan.tail_keyframe_seconds == 12.0
    assert plan.requested == 300


def test_a_tail_that_ends_before_the_next_keyframe_needs_no_body(keyframes):
    """The tail is a run of frames, so it does not wait for the next keyframe: a range that
    ends before one is re-encoded whole rather than lengthened to reach a boundary."""
    keyframes(4.0, 12.0)
    plan = cutter.plan_trim(spec(150, 200), media())
    assert plan.mode == "reencode"
    assert plan.tail_frames == 0
    assert any("no keyframe inside the segment" in note for note in plan.notes)


def test_a_single_keyframe_leaves_nothing_to_copy(keyframes):
    """A copied middle needs two keyframes: one to open it and one to close it. A file that
    only has the first has no packets that can be passed through untouched."""
    keyframes(4.0)
    plan = cutter.plan_trim(spec(100, 200), media())
    assert plan.mode == "reencode"
    assert plan.body_frames == 0
    assert plan.tail_frames == 0
    assert any("no copied body fits" in note for note in plan.notes)


def test_no_keyframe_at_all_is_re_encoded_whole(keyframes):
    keyframes()
    assert cutter.plan_trim(spec(50, 200), media()).mode == "reencode"


# ---------------------------------------------------------------------------------------
# Refusals, before anything is written
# ---------------------------------------------------------------------------------------

def test_an_empty_or_backwards_range_is_refused(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="not after the in point"):
        cutter.plan_trim(spec(200, 200), media())
    with pytest.raises(cutter.TrimError, match="not after the in point"):
        cutter.plan_trim(spec(200, 100), media())


def test_a_negative_in_point_is_refused(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="before the start"):
        cutter.plan_trim(spec(-1, 100), media())


def test_an_out_point_past_the_end_is_refused_and_says_where_the_end_is(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="past the end of the file"):
        cutter.plan_trim(spec(100, 501), media())


def test_a_source_whose_codec_cannot_be_patched_is_refused_with_a_reason(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="long-GOP"):
        cutter.plan_trim(spec(100, 200), media(codec="vp9"))


# ---------------------------------------------------------------------------------------
# The two things that are warnings rather than refusals
# ---------------------------------------------------------------------------------------

def test_a_concat_offset_that_would_empty_the_head_is_re_encoded_whole(keyframes):
    # A tail cannot absorb a concat offset the way a head does -- it is anchored to the out
    # point -- so when the offset would leave no copied middle, the segment is re-encoded
    # whole. It used to clamp the head to one frame instead, which silently left the out
    # point overshooting, so the note is the evidence that it refused rather than guessed.
    keyframes(4.0, 8.0, 12.0)
    plan = cutter.plan_trim(spec(150, 450, concat_offset=50), media())
    assert plan.mode == "reencode"
    assert any("no copied body fits" in note for note in plan.notes)

def test_a_file_whose_timestamps_are_off_its_grid_is_planned_anyway_and_says_so(keyframes):
    # No check can make a mark exact against a frame grid that does not exist, so the honest
    # answer is a warning beside a plan that still works -- and the warning carries the
    # measurement, because "0.99 frame(s) away by the end" is the number that says whether a
    # failed alignment check means the cut moved or the file has no grid to be exact against.
    keyframes(4.0)
    off_grid = media(frames=300, rate=Fraction(30))
    off_grid.average_rate = Fraction(24000, 1001)
    plan = cutter.plan_trim(spec(100, 200), off_grid)
    assert any("away from that grid" in note for note in plan.notes)


def test_a_file_whose_timestamps_accumulate_a_whole_frame_of_drift_is_flagged(keyframes):
    """The case a rate comparison misses.

    The reference master reports 30/1 and averages 29.99943 -- 0.0019% apart, under any
    sensible threshold -- yet over 52210 frames the difference is exactly one frame. A mark
    then lands a frame out in one part of the file and exactly right in another.
    """
    keyframes(4.0)
    drifting = media(frames=52210, rate=Fraction(30))
    drifting.duration = 52210 / 30 + 1 / 30           # a frame of drift by the end
    drifting.average_rate = Fraction(30)
    assert drifting.grid_drift == pytest.approx(1.0, abs=0.01)
    assert drifting.variable is True
    assert any("away from that grid" in note for note in
               cutter.plan_trim(spec(100, 200), drifting).notes)


def test_frames_are_counted_on_the_grid_the_file_is_actually_on():
    """The case that made alignment checks fail on real material.

    The reference master reports ``30/1`` and averages ``156630000/5221099``. Counting its
    frames at 30 puts them a whole frame away from where they are by the end of the file, so a
    mark is a frame out in one part of the file and exact in another -- which is not a grid any
    check can be exact against. The engine converts on the file's own grid instead.
    """
    master = media(frames=52210, rate=Fraction(30))
    master.average_rate = Fraction(156630000, 5221099)
    master.duration = 1740.366333
    master.start_time = 0.021

    assert master.grid_rate_text == "156630000/5221099"
    assert master.grid_drift == pytest.approx(0.99, abs=0.02)

    on_the_grid = master.seconds_of(52209)
    on_the_claimed_rate = master.start_time + 52209 / 30
    assert on_the_grid - on_the_claimed_rate == pytest.approx(1 / 30, abs=0.002)

    # And the frame a time names comes back the same way, so the two are inverses.
    assert master.frame_of(on_the_grid) == 52209


def test_an_ordinary_rate_is_its_own_grid():
    plain = media(frames=300, rate=Fraction(25))
    assert plain.grid_rate == Fraction(25)
    assert plain.grid_rate_text == "25"
    assert plain.seconds_of(100) == pytest.approx(4.0)
    assert plain.frame_of(4.0) == 100


def test_an_ordinary_constant_rate_file_is_not_flagged(keyframes):
    # The check has to be quiet on clean material or it is noise nobody reads.
    keyframes(4.0)
    clean = media(frames=300, rate=Fraction(25))
    assert clean.grid_drift == pytest.approx(0.0, abs=0.01)
    assert clean.variable is False
    assert not any("away from that grid" in note
                   for note in cutter.plan_trim(spec(100, 200), clean).notes)


# ---------------------------------------------------------------------------------------
# Naming
# ---------------------------------------------------------------------------------------

def test_the_default_output_name_says_which_range_it_holds():
    # Named by timecode rather than by frame number, because a timecode is what the operator
    # typed and what they will look for afterwards. Colons are not legal in a Windows file
    # name, so they are dotted.
    named = cutter.default_output(SOURCE, 100, 199, Fraction(25))
    assert named.suffix == ".mp4"
    assert named.parent == SOURCE.parent
    assert named.name == "reel 00.00.04.00-00.00.07.24.mp4"


def test_the_default_output_name_never_contains_a_character_windows_refuses():
    for rate in (Fraction(25), NTSC_30, CINEMA):
        named = cutter.default_output(SOURCE, 0, 1, rate)
        assert not set(':*?"<>|') & set(named.name)


# ---------------------------------------------------------------------------------------
# Naming a segment
# ---------------------------------------------------------------------------------------

def test_a_named_segment_is_called_after_its_name():
    named = cutter.output_for(SOURCE, 100, 199, Fraction(25), name="Interview wide")
    assert named.name == "Interview wide.mp4", "the name is used as typed"
    assert named.parent == SOURCE.parent, "and it lands beside its source"


def test_a_named_segment_keeps_the_sources_own_container():
    # A ProRes source lives in a .mov, and a .mov is what the pieces are muxed into, so the
    # extension follows the source rather than being forced to .mp4.
    source = SOURCE.with_suffix(".mov")
    named = cutter.output_for(source, 100, 199, Fraction(25), name="Interview wide")
    assert named.name == "Interview wide.mov"


def test_no_name_still_names_the_segment_for_its_range():
    # The behaviour that existed before naming, unchanged for anyone who does not type a name.
    plain = cutter.output_for(SOURCE, 100, 199, Fraction(25))
    assert plain == cutter.default_output(SOURCE, 100, 199, Fraction(25))
    for blank in ("", "   "):
        assert cutter.output_for(SOURCE, 100, 199, Fraction(25), name=blank) == plain


def test_a_named_segment_can_arrive_in_a_folder_of_its_own():
    named = cutter.output_for(SOURCE, 100, 199, Fraction(25), name="Interview wide",
                              in_folder=True)
    assert named.name == "Interview wide.mp4"
    assert named.parent.name == "Interview wide", "the folder is named after the segment"
    assert named.parent.parent == SOURCE.parent, "and it sits beside the source"


def test_the_transcript_is_named_after_the_segment_and_lands_with_it():
    """The caption file has to follow the segment, name and folder alike.

    It is written as ``spec.output.with_suffix(".srt")``, so this is the whole of that guarantee:
    if the output is right, the transcript is right, in a plain folder and in a named one. Checked
    here rather than only through a cut, because a transcript that lands somewhere else is only
    noticed after the work is done.
    """
    for in_folder in (False, True):
        named = cutter.output_for(SOURCE, 100, 199, Fraction(25), name="Interview wide",
                                  in_folder=in_folder)
        captions = named.with_suffix(".srt")
        assert captions.stem == named.stem, "the transcript does not share the segment's name"
        assert captions.parent == named.parent, "the transcript is not beside the segment"
        assert captions.name == "Interview wide.srt"


def test_a_name_that_windows_would_refuse_is_refused_here_with_the_reason():
    """A name is not silently repaired. Trimming `Take 1/2` to `Take 12` writes a file that
    exists under a name nobody chose and nobody will look for, which is worse than a refusal."""
    for typed, because in (
        ("", "empty"),
        ("   ", "only spaces"),
        ("Take 1/2", "a folder separator"),
        ("Take 1\\2", "a Windows separator"),
        ("Take: 1", "a colon"),
        ("Take 1?", "a question mark"),
        ("Take 1.", "a trailing dot"),
        ("..", "a parent reference"),
        (".", "the folder itself"),
        ("CON", "a reserved device name"),
        ("con.mp4", "a reserved name with an extension"),
        ("x" * 101, "longer than the limit"),
    ):
        with pytest.raises(cutter.TrimError) as refused:
            cutter.clean_segment_name(typed)
        assert str(refused.value), f"{because} was refused without a reason"


def test_a_blank_name_means_no_name_rather_than_a_refusal():
    """The distinction the field relies on: empty is not an error, it is the default.

    An empty box is what the window starts with and what a person gets back to by clearing it, so
    it has to mean "name it for its range" rather than "you did something wrong".
    """
    plain = cutter.default_output(SOURCE, 100, 199, Fraction(25))
    for blank in ("", "   ", None):
        assert cutter.output_for(SOURCE, 100, 199, Fraction(25), name=blank) == plain


def test_a_refused_name_is_refused_through_the_output_path_too(monkeypatch, tmp_path):
    """And the refusal reaches the caller that builds the path, not only the validator."""
    with pytest.raises(cutter.TrimError) as refused:
        cutter.output_for(SOURCE, 100, 199, Fraction(25), name="Take 1/2", in_folder=True)
    assert "/" in str(refused.value)
    # Nothing may be created for a name that was refused: the folder is a consequence of a name
    # that passed, never of one that did not.
    assert not (SOURCE.parent / "Take 1").exists()


def test_a_refusal_names_the_character_it_objected_to():
    # "That name cannot be used" is not actionable. The character, or the rule, is.
    with pytest.raises(cutter.TrimError) as refused:
        cutter.clean_segment_name("Take 1/2")
    assert "/" in str(refused.value)

    with pytest.raises(cutter.TrimError) as refused:
        cutter.clean_segment_name("CON")
    assert "CON" in str(refused.value)


def test_a_refusal_says_the_thing_that_is_most_usefully_wrong():
    """Two rules can both be true of one name, and only one of them is worth saying.

    `..` ends with a dot and is also a folder reference. Reporting the dot is true and useless: the
    person who typed `..` did not mistype a dot, they typed a folder reference. So the more
    specific rule is checked first, and the order of the checks is part of the behaviour.
    """
    with pytest.raises(cutter.NameRefused) as refused:
        cutter.clean_segment_name("..")
    assert "folder reference" in str(refused.value)

    with pytest.raises(cutter.NameRefused) as refused:
        cutter.clean_segment_name(".")
    assert "folder reference" in str(refused.value)

    # And a name that is only a trailing dot still gets the dot explained.
    with pytest.raises(cutter.NameRefused) as refused:
        cutter.clean_segment_name("Take 1.")
    assert "dot or a space" in str(refused.value)


def test_a_name_that_is_merely_unusual_is_accepted():
    """Refusing what the filesystem accepts would be its own bug: punctuation, accents, spaces,
    dots inside the name and a non-Latin script all have to work."""
    for typed in ("Take 1.2", "Émilie — finale", "第 3 段", "a b  c", "mix_v2-final", "1" * 100):
        assert cutter.clean_segment_name(typed) == typed


def test_space_around_a_name_is_trimmed_rather_than_refused():
    """A trailing space is a slip, not a mistake worth stopping for.

    Windows drops it silently, so refusing it would be pedantry about something the filesystem
    does not care about -- and `Take 1 ` becoming `Take 1` is what the person meant. A trailing
    *dot* is refused instead, because Windows also drops that, so accepting it would write a file
    whose name is not the one that was asked for.
    """
    assert cutter.clean_segment_name("Take 1 ") == "Take 1"
    assert cutter.clean_segment_name("  Take 1  ") == "Take 1"
    assert cutter.output_for(SOURCE, 100, 199, Fraction(25), name=" Take 1 ").name == "Take 1.mp4"


# ---------------------------------------------------------------------------------------
# Locating ffmpeg
# ---------------------------------------------------------------------------------------

def _portable(root: Path, names=("ffmpeg", "ffprobe")) -> Path:
    """A fake application folder with a portable build beside it, as start.bat leaves one."""
    module = root / "backend" / "trimmer" / "ffmpeg.py"
    module.parent.mkdir(parents=True, exist_ok=True)
    module.write_text("", encoding="utf-8")
    binaries = root / ".tools" / "ffmpeg" / "bin"
    binaries.mkdir(parents=True, exist_ok=True)
    for name in names:
        (binaries / f"{name}.exe").write_bytes(b"")
    return binaries


def test_a_portable_ffmpeg_beside_the_application_is_found(monkeypatch, tmp_path):
    """The engine has to work when it is started directly, not only through start.bat.

    start.bat puts a portable build in `.tools` on a machine that had no ffmpeg, and points the
    environment at it. The packaged application has no start.bat, so the engine looks there
    itself; without this, an app that installed everything correctly would report that ffmpeg was
    not found the moment it was launched any other way.
    """
    from trimmer import ffmpeg

    root = tmp_path / "app"
    binaries = _portable(root)
    monkeypatch.setattr(ffmpeg, "__file__", str(root / "backend" / "trimmer" / "ffmpeg.py"))
    # Both names, not just the one being asked about. start.bat exports both before it runs these
    # tests, so a test that clears only `THE_TRIMMER_FFMPEG` passes in a developer's shell and
    # fails under the launcher it is supposed to be checking.
    monkeypatch.delenv("THE_TRIMMER_FFMPEG", raising=False)
    monkeypatch.delenv("THE_TRIMMER_FFPROBE", raising=False)

    assert ffmpeg.tool("ffmpeg") == str(binaries / "ffmpeg.exe")
    assert ffmpeg.tool("ffprobe") == str(binaries / "ffprobe.exe")


def test_the_environment_override_still_wins_over_the_portable_copy(monkeypatch, tmp_path):
    """So that what start.bat verified is what gets used, even with a copy lying beside it."""
    from trimmer import ffmpeg

    root = tmp_path / "app"
    _portable(root)
    chosen = tmp_path / "elsewhere" / "ffmpeg.exe"
    chosen.parent.mkdir(parents=True, exist_ok=True)
    chosen.write_bytes(b"")

    monkeypatch.setattr(ffmpeg, "__file__", str(root / "backend" / "trimmer" / "ffmpeg.py"))
    monkeypatch.setenv("THE_TRIMMER_FFMPEG", str(chosen))
    assert ffmpeg.tool("ffmpeg") == str(chosen)


def test_an_override_pointing_at_nothing_is_an_error_not_a_fallback(monkeypatch, tmp_path):
    """A path that was set and is gone must be said out loud.

    Falling back to whatever is on PATH would run a different build than the one somebody chose,
    which is how a cut comes out wrong for a reason nothing in the log explains.
    """
    from trimmer import ffmpeg

    root = tmp_path / "app"
    _portable(root)
    monkeypatch.setattr(ffmpeg, "__file__", str(root / "backend" / "trimmer" / "ffmpeg.py"))
    monkeypatch.setenv("THE_TRIMMER_FFMPEG", str(tmp_path / "gone" / "ffmpeg.exe"))

    with pytest.raises(ffmpeg.FFmpegError) as refused:
        ffmpeg.tool("ffmpeg")
    assert "gone" in str(refused.value)


def test_nothing_anywhere_says_so_and_says_what_to_do(monkeypatch, tmp_path):
    """The last resort: the message names start.bat, because that is the thing that fixes it."""
    from trimmer import ffmpeg

    root = tmp_path / "app"
    (root / "backend" / "trimmer").mkdir(parents=True, exist_ok=True)
    (root / "backend" / "trimmer" / "ffmpeg.py").write_text("", encoding="utf-8")
    monkeypatch.setattr(ffmpeg, "__file__", str(root / "backend" / "trimmer" / "ffmpeg.py"))
    monkeypatch.delenv("THE_TRIMMER_FFMPEG", raising=False)
    monkeypatch.setattr(ffmpeg.shutil, "which", lambda name: None)

    with pytest.raises(ffmpeg.FFmpegError) as refused:
        ffmpeg.tool("ffmpeg")
    assert "start.bat" in str(refused.value)


# ---------------------------------------------------------------------------------------
# The body copy's contract with the rest of the trim
# ---------------------------------------------------------------------------------------

def test_the_body_copy_is_handed_the_cancellation_token(monkeypatch, tmp_path):
    """The copy is a packet read rather than a subprocess, so nothing else in the engine polls
    the token for the length of it. The first version called a method the token does not have,
    which surfaced as an `AttributeError` in the middle of a real trim on a 34-minute master
    rather than as a failing test -- so the contract is asserted here directly."""
    media_obj = media(frames=500)
    plan = cutter.TrimPlan(media_obj, "headpatch", 100, 100, 300, tail_frames=100)
    seen = []

    def fake(source, destination, first, last, cancel=None):
        seen.append(cancel)
        Path(destination).write_bytes(b"")

    monkeypatch.setattr(cutter, "_copy_packets", fake)
    monkeypatch.setattr(cutter.ff, "inspect", lambda path: SimpleNamespace(frames=300))

    token = cutter.CancelToken()
    cutter._copy_body(media_obj, spec(0, 500), plan, tmp_path / "b.mp4",
                      lambda *_: None, token)
    assert seen == [token], "the token was not handed to the copy"


def test_a_cancelled_job_does_not_start_its_body_copy(monkeypatch, tmp_path):
    media_obj = media(frames=500)
    plan = cutter.TrimPlan(media_obj, "headpatch", 100, 100, 300, tail_frames=100)
    monkeypatch.setattr(cutter, "_copy_packets",
                        lambda *a, **k: pytest.fail("the copy ran after cancellation"))

    stopped = cutter.CancelToken()
    stopped.cancel()
    with pytest.raises(cutter.Cancelled):
        cutter._copy_body(media_obj, spec(0, 500), plan, tmp_path / "b.mp4",
                          lambda *_: None, stopped)


def test_the_packet_reader_stops_when_the_job_is_cancelled():
    """Checked inside the demux loop, not only before it: reading a long file's packets is the
    longest single operation in a trim, and a token looked at once would leave Cancel appearing
    to do nothing for the whole of it."""
    import inspect as _inspect

    source_text = _inspect.getsource(cutter._select_body_packets)
    assert "cancel.cancelled" in source_text, "the demux loop does not poll the token"
    assert "raise Cancelled" in source_text


# ---------------------------------------------------------------------------------------
# Choosing the packets the body is made of
# ---------------------------------------------------------------------------------------

class _Stream:
    """The parts of a PyAV stream the selector reads. No container, no file.

    Time base 1/1000 with a rate of 40 puts one frame every 25 ticks, so a packet's frame number
    is its timestamp over 25 -- which keeps the fixtures below readable.
    """

    def __init__(self, packets, time_base=Fraction(1, 1000), rate=40):
        self.time_base = time_base
        self.average_rate = rate
        self.guessed_rate = rate
        self.container = self
        self._packets = packets

    def demux(self, _stream):
        return iter(self._packets)


class _Packet:
    """A video packet: presentation time, and whether it can be decoded on its own."""

    def __init__(self, pts, key=False, dts=None):
        self.pts = pts
        self.dts = pts if dts is None else dts
        self.is_keyframe = key


def _frames(count: int, gop: int = 10) -> list:
    """One packet per frame, with a keyframe every ``gop`` frames, at 25 ticks a frame."""
    return [_Packet(index * 25, key=(index % gop == 0)) for index in range(count)]


def _frame_of(packet) -> int:
    """The frame number the selector gives a packet from these fixtures: the container's own,
    which starts at one."""
    return packet.pts // 25 + 1


def _index_with_keyframe_at(row: int, frames: int = 300, rate: int = 25):
    """A stand-in for the container index whose first keyframe is at a chosen **row**.

    The defect this pins down is that a keyframe's row and the frame number the planner gives it
    are not the same thing. On the real Joseph Chalom master the planner said keyframe 2750
    while the keyframe is container row 2751 -- and the head was sized from the frame number, so
    it covered one row too few and the delivered file showed row 2750 in neither piece.
    """

    class _Index:
        count = frames

        def time_of(self, at: int) -> float:
            return (at - 1) / rate

        def keyframe_at_or_after(self, frame: int):
            return row

    return _Index()


def _plan_for_rows(monkeypatch, keyframe_row: int, in_frame: int = 20, out_frame: int = 240):
    from trimmer import container as packets

    fake = _index_with_keyframe_at(keyframe_row)
    monkeypatch.setattr(packets, "read", lambda path, cancel=None: fake)
    monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: [2.0, 6.0, 10.0])
    return cutter.plan_trim(spec(in_frame, out_frame), media())


def test_the_head_is_long_enough_to_reach_the_body_row(monkeypatch):
    """The head must span every row between the mark and the row the copy begins on.

    The stutter, as a test. The copy begins on the keyframe's **row**; the head has to cover the
    rows before it. Sizing the head from a frame number instead leaves a row in neither piece,
    and one missing picture at the join is what is seen.
    """
    # The keyframe sits on row 51 while the planner will number that keyframe 50: the two grids
    # are one apart, which is the case measured on the real master.
    plan = _plan_for_rows(monkeypatch, keyframe_row=51)
    body_opens_on_row = plan.keyframe + 1
    assert body_opens_on_row == 51, f"the copy opens on row {body_opens_on_row}, not 51"
    head_first_row = 21                     # the mark's own row
    rows_the_head_must_cover = body_opens_on_row - head_first_row
    assert plan.head_frames == rows_the_head_must_cover, (
        f"head is {plan.head_frames} frames but must cover {rows_the_head_must_cover} rows")
    assert plan.head_frames + plan.body_frames + plan.tail_frames == out_frame - in_frame


out_frame, in_frame = 240, 20


def test_the_reported_spans_join_up_and_cover_the_request(keyframes):
    """What the log says must not read as a gap.

    The report used to give the head in frame numbers and the body in container rows, so a real
    cut printed head 7514..7741 and ody 7743..9492 -- row 7742 in neither, which reads as a
    missing frame. It was read as one, and sent a diagnosis down the wrong path. Spans are now
    given in one grid, and this asserts they join end to end and cover exactly the range asked
    for.
    """
    keyframes(2.0, 6.0, 10.0)
    plan = cutter.plan_trim(spec(20, 240), media())
    spans = plan.rows()

    assert spans["head"][1] + 1 == spans["body"][0], "head and body do not join"
    assert spans["body"][1] + 1 == spans["tail"][0], "body and tail do not join"
    covered = spans["tail"][1] - spans["head"][0] + 1
    assert covered == plan.requested == 220


def test_the_report_names_rows_and_says_so(keyframes):
    """The word is part of the contract: a reader has to know which grid the numbers are on."""
    keyframes(2.0, 6.0, 10.0)
    text = cutter.plan_trim(spec(20, 240), media()).describe(Fraction(25))
    assert "rows" in text
    assert "frames 20.." not in text, "a span is still being given in frame numbers"


def test_a_body_copy_keeps_every_packet_of_the_range_asked_for():
    """The frames wanted are 10..14, with a keyframe at 10 -- so 10, 11, 12, 13, 14 and nothing
    else, whatever the internal indexing says."""
    chosen = cutter._select_body_packets(_Stream(_frames(25)), first=10, last=14)
    assert [_frame_of(p) for p in chosen] == [11, 12, 13, 14]


def test_two_packets_at_the_same_presentation_time_are_both_kept():
    """The fault behind a 500-frame cut arriving six frames short.

    A master can carry two packets at one presentation time -- the reference one has frames 86
    and 87 both at pts 259890, the second of them the keyframe with the SPS/PPS/IDR. Keeping one
    packet per frame number dropped a real picture, and the delivered segment ran ahead of its
    sound by that many frames. Nothing may be collapsed.
    """
    packets = _frames(25)
    # The opening keyframe is frame 0. Frames 4 and 5 share a presentation time, in the middle
    # of the body, which is where the reference master has its pair.
    packets[5] = _Packet(4 * 25, key=False, dts=4 * 25)
    packets[4] = _Packet(4 * 25, key=False, dts=4 * 25)

    chosen = cutter._select_body_packets(_Stream(packets), first=1, last=10)
    frames_wanted = list(range(0, 10))
    assert len(chosen) == len(frames_wanted), (
        f"expected {len(frames_wanted)} packets, got {len(chosen)}")
    assert chosen[0].is_keyframe, "the copy must begin on a keyframe"
    # Both of the packets that share a time survive, so the frame after them is still there.
    assert [_frame_of(p) for p in chosen] == [1, 2, 3, 4, 5, 5, 7, 8, 9, 10]


def test_the_copy_starts_at_the_keyframe_that_opens_the_body():
    """Packets before it belong to the head's re-encode and cannot be copied: the first frame of
    the body has to be one a decoder can start on."""
    # The in point is inside the second GOP, so the opening keyframe is frame 10 and frames 5..9
    # are the head's business.
    chosen = cutter._select_body_packets(_Stream(_frames(25)), first=5, last=14)
    assert [_frame_of(p) for p in chosen] == [11, 12, 13, 14]


def test_the_copy_stops_before_the_keyframe_that_opens_the_tail():
    """That keyframe is re-encoded as the tail's first frame, so copying it too would put the
    same picture into the segment twice."""
    chosen = cutter._select_body_packets(_Stream(_frames(40)), first=10, last=19)
    assert max(_frame_of(p) for p in chosen) == 19
    assert not any(p.is_keyframe and _frame_of(p) == 20 for p in chosen)


def test_a_body_copy_needs_no_frame_rate_at_all():
    """The copy numbers frames by the container's packet order, so a source that reports no
    usable rate is no obstacle -- and a test that used to demand a refusal now asserts the
    opposite, because the refusal is what the derived numbering needed and this does not."""
    chosen = cutter._select_body_packets(_Stream(_frames(12), rate=0), first=1, last=5)
    assert [_frame_of(p) for p in chosen] == [1, 2, 3, 4, 5]


def test_the_keyframe_reader_uses_the_fast_path(monkeypatch, tmp_path):
    """The packet-flag reader is the reason listing keyframes is affordable, and the first
    implementation named `av` without importing it -- the outer handler caught the NameError and
    fell back to ffprobe every single time, silently, at 2.7x the cost. A fallback that is
    reached by an error rather than by an inability is invisible unless something asserts the
    fast path was taken, which is what this does.

    Measured on a 35-minute master: PyAV's packet flags are 2.6 s against ffprobe's 7.0 s for an
    identical list of 252 keyframes.
    """
    from trimmer import ffmpeg

    reached = []
    monkeypatch.setattr(ffmpeg, "_keyframes_by_packet",
                        lambda path, start, end: reached.append("packet") or [1.0, 2.0])
    monkeypatch.setattr(ffmpeg, "_keyframes_by_probe",
                        lambda path, start, end: reached.append("probe") or [9.0])

    marks = ffmpeg.keyframes(tmp_path / "absent.mp4", 0.0, 10.0)
    assert reached == ["packet"], f"the slow path was taken: {reached}"
    assert marks == [1.0, 2.0]


def test_the_keyframe_reader_falls_back_when_the_container_cannot_be_read(monkeypatch, tmp_path):
    """And the fallback still works, for a source PyAV will not open."""
    from trimmer import ffmpeg

    def refuse(path, start, end):
        raise ValueError("cannot open")

    monkeypatch.setattr(ffmpeg, "_keyframes_by_packet", refuse)
    monkeypatch.setattr(ffmpeg, "_keyframes_by_probe", lambda path, start, end: [3.0])
    assert ffmpeg.keyframes(tmp_path / "absent.mp4", 0.0, 10.0) == [3.0]


# ---------------------------------------------------------------------------------------
# Reading one frame's own time
# ---------------------------------------------------------------------------------------

def _fake_probe(monkeypatch, rate: float, first: float = 0.0):
    """Stand in for ffprobe, answering a `-read_intervals` window from a synthetic file whose
    frames sit exactly ``1 / rate`` apart starting at ``first``.

    Returns the list of windows it was asked for, so a test can assert the read was *near* the
    frame rather than from the beginning.
    """
    from trimmer import ffmpeg

    asked: list[tuple[float, float]] = []

    class Done:
        returncode = 0

        def __init__(self, text: str) -> None:
            self.stdout = text
            self.stderr = ""

    def fake(args, **kwargs):
        window = args[args.index("-read_intervals") + 1]
        start_text, span_text = window.split("%+")
        begin = float(start_text)
        span = float(span_text)
        asked.append((begin, span))
        count = max(1, int(span * rate))
        lines = [f"{first + (begin * rate + i) / rate:.6f}," for i in range(count + 1)]
        return Done("\n".join(lines))

    monkeypatch.setattr(ffmpeg, "_run_quiet", fake)
    return asked


def test_a_frame_time_is_read_from_near_the_frame_not_from_the_start(monkeypatch, tmp_path):
    """The fault this fixes: reading frames from the beginning to learn one timestamp took over
    27 s for frame 10048 of a 2.6 GB master, before the first log line -- so a run that was
    working looked hung. The window must open near the frame."""
    from trimmer import ffmpeg

    rate = 30.0
    asked = _fake_probe(monkeypatch, rate)
    at = ffmpeg.frame_pts_near(tmp_path / "absent.mp4", 10048, rate)

    assert len(asked) == 1, "more than one probe for one frame"
    begin, span = asked[0]
    assert begin < 10048 / rate < begin + span, "the window does not contain the frame"
    assert begin > 0, "the read started at the beginning of the file"
    # Near, not a walk: the frame is 334 s into the file and the read opens a few seconds before
    # it. The window has to be wide enough to survive the seek `-read_intervals` performs, which
    # lands on the keyframe *before* the time asked for rather than on the time -- on the
    # reference master that is 219 frames back, so a window of a few frames came back holding
    # frames from before the one wanted and the reader answered with the wrong one.
    assert 10048 / rate - begin <= 20, f"the read opens {10048 / rate - begin:.1f}s early"
    assert span <= 20, f"the window is {span:.2f}s wide, which is a walk not a read"
    assert at is not None


def test_a_frame_time_is_refused_when_the_window_never_reaches_the_frame(monkeypatch, tmp_path):
    """And when the read comes back without the frame that was asked about, the answer is
    *nothing* rather than the nearest thing in the window.

    This is the fault that mattered. `-read_intervals` seeks to the keyframe before the time, so
    a window shorter than the gap between keyframes holds frames from before the one wanted, and
    picking the nearest to the computed guess hands one of those back as though it were the
    answer. Measured on the reference master: a request for frame 162241 was answered with the
    time of a frame 222 earlier, 7.3 s out, and that was used as the seek target for a cut.
    Nothing may be returned unless it is the frame that was asked for.
    """
    from trimmer import ffmpeg

    class Done:
        returncode = 0
        stderr = ""

        def __init__(self, text):
            self.stdout = text

    # Frames 750..780 of a 30 fps file: nowhere near the 1000 that was asked for, so the window
    # came back from an earlier keyframe. The nearest of them to the guess is still 7 s out.
    def fake(args, **kwargs):
        return Done("\n".join(f"{frame / 30.0:.6f}," for frame in range(750, 781)))

    monkeypatch.setattr(ffmpeg, "_run_quiet", fake)
    assert ffmpeg.frame_pts_near(tmp_path / "absent.mp4", 1000, 30.0) is None


def test_a_frame_time_is_the_one_the_container_states(monkeypatch, tmp_path):
    """And the answer is the frame's own time, not the computed grid time -- which is the whole
    reason this reader exists."""
    from trimmer import ffmpeg

    rate = 30.0
    _fake_probe(monkeypatch, rate)
    for frame in (0, 1, 23, 10048, 56178):
        got = ffmpeg.frame_pts_near(tmp_path / "absent.mp4", frame, rate)
        assert got == pytest.approx(frame / rate, abs=1e-5), f"frame {frame}"


def test_a_frame_time_survives_a_seek_that_lands_a_frame_off(monkeypatch, tmp_path):
    """The seek inside `-read_intervals` lands on the frame at the time it is given, which is
    not knowable before reading. The frame wanted is found by its timestamp, so a window that
    starts a frame early or late still answers with the frame that was asked about."""
    from trimmer import ffmpeg

    rate = 25.0
    frames = [i / rate for i in range(200)]        # 25 fps, frames 0..199

    class Done:
        returncode = 0
        stderr = ""

        def __init__(self, text):
            self.stdout = text

    def fake(args, **kwargs):
        window = args[args.index("-read_intervals") + 1]
        begin = float(window.split("%+")[0])
        span = float(window.split("%+")[1])
        # Start one frame *after* the time asked for: a legal place for a seek inside the window
        # to land, and the frame wanted is still inside what comes back.
        begin = begin + 1.0 / rate
        inside = [t for t in frames if begin <= t <= begin + span]
        return Done("\n".join(f"{t:.6f}," for t in inside))

    monkeypatch.setattr(ffmpeg, "_run_quiet", fake)
    for frame in (5, 50, 150):
        assert ffmpeg.frame_pts_near(tmp_path / "absent.mp4", frame, rate) == \
            pytest.approx(frame / rate, abs=1e-5)


def test_a_frame_time_falls_back_when_the_container_reports_none(monkeypatch, tmp_path):
    """No timestamps at all: the computed grid time is used, which is what every build before
    this did."""
    from trimmer import ffmpeg

    class Done:
        returncode = 0
        stdout = ""
        stderr = ""

    monkeypatch.setattr(ffmpeg, "_run_quiet", lambda *a, **k: Done())
    media_times = ffmpeg.FrameTimes(tmp_path / "absent.mp4",
                                    lambda frame: frame / 25.0, 25.0, 0.0)
    assert media_times(100) == pytest.approx(4.0)

    # And with no rate to aim by, the computed time is used directly.
    without_rate = ffmpeg.FrameTimes(tmp_path / "absent.mp4", lambda frame: frame / 25.0)
    assert without_rate(100) == pytest.approx(4.0)

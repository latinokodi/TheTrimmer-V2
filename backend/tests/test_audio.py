"""The audio map's own logic, plus one measurement against a real file.

Most of this is pure functions over captured ``silencedetect`` text, because that is where the
faults live: a log that ends without an end line, a log whose lines were reordered, a duplicate
end. Mocking ffmpeg would let the parser agree with a format this project invented, so the
fixtures below are stderr captured verbatim from ffmpeg 8.0.1 running the exact command
:func:`trimmer.audio.silences` builds. Only the last test starts a process; it exists because the
stderr format is the one thing a pure test cannot prove.
"""

from __future__ import annotations

import time
from pathlib import Path

import pytest

from trimmer import audio
from trimmer import ffmpeg as ff

#: One second of 440 Hz tone then one second of silence, from
#: ``ffmpeg -hide_banner -nostats -i beep.wav -vn -af silencedetect=noise=-35dB:d=0.25 -f null -``.
#: The start is 0.999958 and the end prints as a bare ``2`` -- both are real quirks, not typos.
ONE_GAP = r"""[aist#0:0/pcm_s16le @ 0000018135df76c0] Guessed Channel Layout: mono
Input #0, wav, from 'C:\Users\ferna\AppData\Local\Temp\trimmer_audio_probe\beep.wav':
  Duration: 00:00:02.00, bitrate: 768 kb/s
  Stream #0:0: Audio: pcm_s16le ([1][0][0][0] / 0x0001), 48000 Hz, mono, s16, 768 kb/s
Stream mapping:
  Stream #0:0 -> #0:0 (pcm_s16le (native) -> pcm_s16le (native))
[silencedetect @ 0000018135def280] silence_start: 0.999958
[silencedetect @ 0000018135def280] silence_end: 2 | silence_duration: 1.000042
[out#0/null @ 0000018135dd6280] video:0KiB audio:188KiB subtitle:0KiB other streams:0KiB
size=N/A time=00:00:02.00 bitrate=N/A speed= 619x elapsed=0:00:00.00
"""

#: The same file's log with the final end line removed: what a version of silencedetect that does
#: not flush at EOF leaves behind, and the reason the duration argument exists at all.
TRAILING_START = r"""[aist#0:0/pcm_s16le @ 0000018135df76c0] Guessed Channel Layout: mono
Input #0, wav, from 'C:\Users\ferna\AppData\Local\Temp\trimmer_audio_probe\beep.wav':
  Duration: 00:00:02.00, bitrate: 768 kb/s
  Stream #0:0: Audio: pcm_s16le ([1][0][0][0] / 0x0001), 48000 Hz, mono, s16, 768 kb/s
[silencedetect @ 0000018135def280] silence_start: 0.999958
"""

#: Two half-second pauses in a two-second file (300 Hz tone, silence, 900 Hz tone, silence).
TWO_GAPS = r"""Input #0, wav, from 'C:\Users\ferna\AppData\Local\Temp\trimmer_audio_probe\two_gaps.wav':
  Duration: 00:00:02.00, bitrate: 768 kb/s
  Stream #0:0: Audio: pcm_s16le ([1][0][0][0] / 0x0001), 48000 Hz, mono, s16, 768 kb/s
[silencedetect @ 0000016d8b8b4e40] silence_start: 0.499937
[silencedetect @ 0000016d8b8b4e40] silence_end: 1.000042 | silence_duration: 0.500104
[silencedetect @ 0000016d8b8b4e40] silence_start: 1.499979
[silencedetect @ 0000016d8b8b4e40] silence_end: 2 | silence_duration: 0.500021
"""


def _ffmpeg_present() -> bool:
    try:
        ff.tool("ffmpeg")
    except Exception:  # noqa: BLE001 - any failure to locate it means skip, not fail
        return False
    return True


FFMPEG_PRESENT = _ffmpeg_present()


# ---------------------------------------------------------------------------------------
# Reading the log
# ---------------------------------------------------------------------------------------

def test_a_real_log_yields_the_gap_ffmpeg_reported():
    """The ordinary case: one start, one end, rounded onto the millisecond grid."""
    assert audio.parse_silences(ONE_GAP, duration=2.0) == [audio.Span(1.0, 2.0)]


def test_the_end_line_is_not_needed_when_the_duration_is_known():
    """A pause that runs to EOF has no end line; the duration is what finishes it.

    This is not a hypothetical: silencedetect does not always flush an end at EOF, and a master
    that ends on dead air -- most of them do -- would otherwise lose the silence at its end.
    """
    assert audio.parse_silences(TRAILING_START, duration=2.0) == [audio.Span(1.0, 2.0)]


def test_a_trailing_start_without_a_duration_is_dropped_rather_than_guessed():
    """Without a duration there is no honest end to invent, so the open pause is dropped."""
    assert audio.parse_silences(TRAILING_START) == []


def test_an_empty_log_is_no_silence():
    """ffmpeg can say nothing, and nothing is not an error."""
    assert audio.parse_silences("") == []
    assert audio.parse_silences("\n\n  \n") == []


def test_junk_lines_are_ignored_and_never_raise():
    """A log is full of lines that are not ours, and a malformed one must not stop the read."""
    junk = r"""frame=   25 fps=0.0 q=-1.0 Lsize=N/A time=00:00:02.00
[silencedetect @ 0x1] silence_start: 1.0
unrelated [silencedetect @ 0x2] noise
[silencedetect @ 0x1] silence_end: 2.5 | silence_duration: 1.5
[silencedetect @ 0x1] silence_start: garbage
[silencedetect @ 0x1] silence_end: 9.0 | silence_duration: garbage
"""
    assert audio.parse_silences(junk, duration=10.0) == [audio.Span(1.0, 2.5)]


def test_unsorted_and_duplicated_lines_pair_by_their_own_durations():
    """A captured log can be reordered or repeated; line order must not invent a silence.

    Pairing by position would attach the second end to the first start and shade the speech
    between them. Each end carries the length it covers, so it can be matched to its own start
    wherever that start appears; the repeated end then finds its start claimed and is dropped.
    """
    lines = [line for line in TWO_GAPS.splitlines() if "silencedetect @" in line]
    assert len(lines) == 4
    scrambled = "\n".join([lines[2], lines[3], lines[0], lines[1], lines[1]])
    assert audio.parse_silences(scrambled, duration=2.0) == [
        audio.Span(0.5, 1.0), audio.Span(1.5, 2.0)]


def test_an_open_start_before_a_finished_pause_is_not_stretched_to_the_end():
    """A stale start cannot run to EOF through a pause that already ended, so it is dropped.

    Stretching it would shade the speech between the stale start and the real pause -- the exact
    fault the duration argument exists to avoid, arriving from the other direction.
    """
    stderr = (
        "[silencedetect @ 0x1] silence_start: 1.0\n"
        "[silencedetect @ 0x1] silence_start: 5.0\n"
        "[silencedetect @ 0x1] silence_end: 6.0 | silence_duration: 1.0\n"
    )
    assert audio.parse_silences(stderr, duration=10.0) == [audio.Span(5.0, 6.0)]


def test_values_are_clamped_to_the_file_and_rounded_to_milliseconds():
    """A negative start and an end past the file are made drawable, not reported as measured."""
    stderr = (
        "[silencedetect @ 0x1] silence_start: -0.5\n"
        "[silencedetect @ 0x1] silence_end: 1.23456 | silence_duration: 1.73456\n"
        "[silencedetect @ 0x1] silence_start: 4.8\n"
        "[silencedetect @ 0x1] silence_end: 9.0 | silence_duration: 4.2\n"
    )
    assert audio.parse_silences(stderr, duration=5.0) == [
        audio.Span(0.0, 1.235), audio.Span(4.8, 5.0)]


# ---------------------------------------------------------------------------------------
# The inverse, which is what the strip draws
# ---------------------------------------------------------------------------------------

def test_speech_is_the_inverse_of_silence():
    """Two pauses split the file into three blocks of speech, in order."""
    gaps = [audio.Span(1.0, 2.0), audio.Span(4.0, 5.0)]
    assert audio.speech(gaps, 6.0) == [
        audio.Span(0.0, 1.0), audio.Span(2.0, 4.0), audio.Span(5.0, 6.0)]


def test_a_gap_narrower_than_a_hairline_is_filled_in():
    """A 10 ms silence is under a pixel on a long strip; left as a gap it is a hairline seam."""
    gaps = [audio.Span(1.0, 1.01), audio.Span(3.0, 4.0)]
    assert audio.speech(gaps, 4.0) == [audio.Span(0.0, 3.0)]


def test_a_gap_wider_than_a_hairline_is_still_drawn():
    """The threshold must not swallow a pause the eye can see."""
    assert audio.speech([audio.Span(1.0, 1.03)], 4.0) == [
        audio.Span(0.0, 1.0), audio.Span(1.03, 4.0)]


def test_a_fully_silent_file_has_no_speech():
    """An empty list is the honest answer; a zero-length span would draw a hairline at zero."""
    assert audio.speech([audio.Span(0.0, 4.0)], 4.0) == []


# ---------------------------------------------------------------------------------------
# Snapping a mark into a pause
# ---------------------------------------------------------------------------------------

def test_a_mark_inside_a_pause_snaps_to_the_middle_of_it():
    """A mark just inside the pause moves to its centre, which is furthest from either word."""
    gaps = [audio.Span(2.0, 4.0)]
    snapped = audio.quietest_frame(gaps, 61, 30.0)  # 2.033 s, just inside the gap
    assert snapped == 90
    assert gaps[0].start <= snapped / 30.0 < gaps[0].end


def test_a_pause_just_outside_the_limit_does_not_move_the_mark():
    """A pause 2.1 s away is a different edit, not a snap."""
    gaps = [audio.Span(3.1, 4.0)]
    assert audio.quietest_frame(gaps, 30, 30.0, limit=2.0) == 30  # mark at 1.0 s


def test_a_pause_exactly_at_the_limit_still_counts():
    """The limit is a distance to the nearest edge, inclusive; the target is still the middle."""
    gaps = [audio.Span(3.0, 5.0)]
    assert audio.quietest_frame(gaps, 30, 30.0, limit=2.0) == 120  # middle 4.0 s


def test_with_no_pauses_the_mark_is_returned_unchanged():
    """No measurement is not a reason to move an edit the operator made."""
    assert audio.quietest_frame([], 42, 30.0) == 42


# ---------------------------------------------------------------------------------------
# The one line for the log
# ---------------------------------------------------------------------------------------

def test_summary_reads_as_one_line():
    """The example line in the spec, reproduced from real arithmetic."""
    gaps = [audio.Span(i * 120.0, i * 120.0 + 12.0) for i in range(31)]
    assert audio.summary(gaps, 3840.0) == (
        "silence     31 gaps, 6m 12s silent of 1h 4m (9.7%)")


def test_summary_of_nothing_is_still_a_line():
    """A file with no pause gets the same shape of line, not a special case the log has to parse."""
    assert audio.summary([], 10.0) == "silence     0 gaps, 0s silent of 10s (0.0%)"


# ---------------------------------------------------------------------------------------
# The run: what it invokes, and what is a failure
# ---------------------------------------------------------------------------------------

def test_cancelled_before_the_run_starts_no_process(monkeypatch):
    """A cancel that has already happened must not pay for a decode."""
    def forbidden(*args, **kwargs):
        raise AssertionError("a cancelled measurement must not start ffmpeg")

    monkeypatch.setattr(audio.ff, "run", forbidden)
    token = ff.CancelToken()
    token.cancel()
    assert audio.silences(Path("C:/media/long.mp4"), cancel=token) == []


def test_the_command_is_the_one_the_captured_format_belongs_to(monkeypatch):
    """The filters and the path go where the measured invocation puts them."""
    seen: dict = {}

    def capture(command, **kwargs):
        seen["command"] = command
        seen["kwargs"] = kwargs
        return ONE_GAP

    monkeypatch.setattr(audio.ff, "run", capture)
    spans = audio.silences(Path("C:/media/beep.wav"), noise_db=-40.0, minimum=0.4, timeout=12.0)

    assert spans == [audio.Span(1.0, 2.0)]
    command = seen["command"]
    assert command[0] == ff.tool("ffmpeg")
    assert command[1:3] == ["-hide_banner", "-nostats"]
    assert command[3] == "-i"
    assert Path(command[4]) == Path("C:/media/beep.wav")
    assert command[5] == "-vn"
    assert command[6:8] == ["-af", "silencedetect=noise=-40dB:d=0.4"]
    assert command[8:] == ["-f", "null", "-"]
    assert seen["kwargs"] == {"cancel": None, "timeout": 12.0}


def test_a_source_with_no_audio_track_is_not_an_error(monkeypatch):
    """A master with no sound is a fact; its stderr is the exact text ffmpeg 8 printed for one."""
    def refuses(command, **kwargs):
        raise ff.FFmpegError(
            "command failed (exit 4294967274)",
            command,
            "Output #0, null, to 'pipe:':\n"
            "[out#0/null @ 00000257f3974c00] Output file does not contain any stream\n"
            "Error opening output file -.\n"
            "Error opening output files: Invalid argument\n",
        )

    monkeypatch.setattr(audio.ff, "run", refuses)
    assert audio.silences(Path("C:/media/video_only.mp4")) == []


def test_a_genuine_ffmpeg_failure_is_raised(monkeypatch):
    """A broken file must not read as a file with no pauses, or the strip would lie."""
    def refuses(command, **kwargs):
        raise ff.FFmpegError("command failed (exit 2)", command,
                             "Error opening input file broken.mp4.\nInvalid data found")

    monkeypatch.setattr(audio.ff, "run", refuses)
    with pytest.raises(ff.FFmpegError, match="command failed"):
        audio.silences(Path("C:/media/broken.mp4"))


# ---------------------------------------------------------------------------------------
# One measurement against a real file
# ---------------------------------------------------------------------------------------

@pytest.mark.skipif(not FFMPEG_PRESENT, reason="ffmpeg is not installed")
def test_a_tone_then_a_silence_is_one_gap(tmp_path):
    """The whole path against real sound: build 1 s tone + 1 s silence, measure one gap.

    Captured stderr can only prove the parser agrees with a log that was captured once. This
    proves the command, the filter's own threshold and the trailing-silence handling still fit
    the ffmpeg on the machine. The fixture is PCM, so the boundary is digital silence rather than
    an encoder's idea of it.
    """
    beep = tmp_path / "beep.wav"
    started = time.monotonic()
    ff.run([
        ff.tool("ffmpeg"), "-hide_banner", "-y",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=1",
        "-f", "lavfi", "-i", "anullsrc=r=48000:cl=mono",
        "-filter_complex", "[0:a][1:a]concat=n=2:v=0:a=1[out]",
        "-map", "[out]", "-t", "2", "-c:a", "pcm_s16le", str(beep),
    ])

    gaps = audio.silences(beep)

    assert len(gaps) == 1, gaps
    assert abs(gaps[0].start - 1.0) < 0.05, gaps
    assert abs(gaps[0].end - 2.0) < 0.05, gaps
    assert time.monotonic() - started < 5.0, "the measurement must stay cheap enough to run on load"

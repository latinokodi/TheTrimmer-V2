"""Naming a segment: the caption file has to arrive with it, under the same name.

The promise is two sentences long -- "the .srt for the segment has the same name as the segment"
and "there is a toggle to put it in a folder with that name" -- and it is exactly the kind of
promise that is easy to keep for the picture and break for the captions, because the captions are
written by a different pass, from a different file, at a different moment. A cut that produces
`Interview wide.mp4` beside an `.srt` still called after the source has failed at the thing it
looked like it did.

These tests write real subtitle files into a temporary folder and read them back. No media is
involved and no encoder is launched: the transcript pass takes a source `.srt`, the marks, and a
media description, and its whole job is arithmetic and where the file lands.
"""

from __future__ import annotations

import re
from fractions import Fraction
from pathlib import Path
from types import SimpleNamespace

import pytest

from trimmer import ffmpeg as ff
from trimmer import trim as cutter


def media(source: Path) -> ff.MediaInfo:
    """A source description with nothing behind it but the numbers a transcript pass needs."""
    return ff.MediaInfo(
        path=source,
        codec="h264",
        width=1920,
        height=1080,
        pix_fmt="yuv420p",
        rate=Fraction(25),
        average_rate=Fraction(25),
        timebase=Fraction(1, 12800),
        frames=10000,
        duration=400.0,
        audio=ff.AudioStream(codec="aac", sample_rate=48000, channels=2),
        size_bytes=1_000_000,
    )


CUE = """1
00:00:04,000 --> 00:00:06,000
first line spoken

2
00:00:08,500 --> 00:00:11,250
second line spoken

3
00:00:30,000 --> 00:00:32,000
past the out point
"""


def source_with_captions(folder: Path) -> tuple[Path, Path]:
    """A source and its transcript, as a real one arrives: `clip.mp4` beside `clip.srt`."""
    video = folder / "clip.mp4"
    captions = folder / "clip.srt"
    video.write_bytes(b"")
    captions.write_text(CUE, encoding="utf-8")
    return video, captions


def spec_for(video: Path, captions: Path, output: Path) -> cutter.TrimSpec:
    # Frames 100..300 at 25 fps: 4.0s to 12.0s, so two of the three cues fall inside.
    return cutter.TrimSpec(source=video, output=output, in_frame=100, out_frame=300,
                           subtitles=captions)


def test_a_named_segment_gets_captions_with_the_same_name(tmp_path):
    """The ask, literally: name the segment and the .srt carries the same name."""
    video, captions = source_with_captions(tmp_path)
    output = cutter.output_for(video, 100, 299, Fraction(25), name="Interview wide")
    result = cutter.cut_subtitles(spec_for(video, captions, output), media(video))

    written = tmp_path / "Interview wide.srt"
    assert written.exists(), "the captions were not written under the segment's own name"
    # And nothing was left under the name the segment would have had without one.
    assert not (tmp_path / "clip 00.00.04.00-00.00.11.24.srt").exists()

    # The retimed cues are the ones inside the marks, shifted to the segment's own clock.
    text = written.read_text(encoding="utf-8")
    assert "first line spoken" in text
    assert "second line spoken" in text
    assert "past the out point" not in text, "a cue outside the segment was kept"
    assert result.written == written


def test_a_named_segment_in_a_folder_gets_its_captions_there(tmp_path):
    """And with the toggle on, both files arrive inside the folder, and the folder is made."""
    video, captions = source_with_captions(tmp_path)
    output = cutter.output_for(video, 100, 299, Fraction(25), name="Interview wide",
                               in_folder=True)
    folder = tmp_path / "Interview wide"
    assert not folder.exists(), "the folder existed before the cut, so this proves nothing"

    cutter.cut_subtitles(spec_for(video, captions, output), media(video))

    assert folder.is_dir(), "the folder was not created"
    assert (folder / "Interview wide.srt").exists()
    assert (folder / "Interview wide.srt").parent.name == "Interview wide"
    # The segment goes in the same folder, so the two travel together.
    assert output.parent == folder
    assert output.name == "Interview wide.mp4"


def test_the_caption_file_sits_where_a_sidecar_is_looked_for(tmp_path):
    """A sidecar is only a sidecar if a player finds it: same folder, same stem.

    `clip.mp4` and `clip.srt` is the convention every player implements. Naming the segment is
    only useful if the pair still satisfies it, so the pairing is checked as a pairing rather
    than as two paths that happen to be right.
    """
    video, captions = source_with_captions(tmp_path)
    for in_folder in (False, True):
        output = cutter.output_for(video, 100, 299, Fraction(25), name="Interview wide",
                                   in_folder=in_folder)
        cutter.cut_subtitles(spec_for(video, captions, output), media(video))
        sidecar = output.with_suffix(".srt")
        assert sidecar.exists()
        assert sidecar.parent == output.parent
        assert sidecar.stem == output.stem


def test_captions_are_not_written_for_a_name_that_was_refused(tmp_path):
    """A refused name must not create a folder on its way to being refused.

    The folder is a consequence of a name that passed. Creating it first and failing afterwards
    would leave an empty `Take 1` folder beside the footage every time somebody typed a colon.
    """
    video, _ = source_with_captions(tmp_path)
    before = sorted(entry.name for entry in tmp_path.iterdir())
    with pytest.raises(cutter.TrimError):
        cutter.output_for(video, 100, 299, Fraction(25), name="Take 1:2", in_folder=True)
    after = sorted(entry.name for entry in tmp_path.iterdir())
    assert after == before, "something was created for a refused name"


def test_a_segment_with_no_name_changes_nothing_about_its_captions(tmp_path):
    """The behaviour that existed before any of this, kept for anyone who types no name."""
    video, captions = source_with_captions(tmp_path)
    output = cutter.output_for(video, 100, 299, Fraction(25))
    cutter.cut_subtitles(spec_for(video, captions, output), media(video))
    expected = tmp_path / "clip 00.00.04.00-00.00.11.24.srt"
    assert expected.exists(), f"expected {expected.name} beside the segment"
    assert output.name == "clip 00.00.04.00-00.00.11.24.mp4"


def test_the_transcript_summary_names_the_file_it_wrote(tmp_path):
    """The log line is how the operator confirms where the captions went, so it names the file."""
    video, captions = source_with_captions(tmp_path)
    output = cutter.output_for(video, 100, 299, Fraction(25), name="Interview wide",
                               in_folder=True)
    lines: list[str] = []
    result = cutter.cut_subtitles(spec_for(video, captions, output), media(video),
                                  log=lines.append)
    assert lines, "the transcript pass said nothing"
    assert "Interview wide.srt" in lines[0], f"the line does not name the file: {lines[0]}"
    # And it says how many cues, which is the number a person checks against the segment.
    assert re.search(r"\b\d+ cue", lines[0]), f"the line carries no cue count: {lines[0]}"
    assert len(result.cues) == 2


# ------------------------------------------------------------------------------------------------
#  Replacing a segment that is already there
# ------------------------------------------------------------------------------------------------
#
#  A range is a name, so cutting the same range twice writes to the same path. Before this, that
#  second cut replaced the first file without a word -- which is the one thing an operator cannot
#  undo and cannot have failed to notice until it is too late. Permission is now asked for, and the
#  asking is only as good as the two answers it depends on: which files are in the way, and what the
#  question says about them.

def test_the_files_in_the_way_are_both_the_segment_and_its_captions(tmp_path):
    """The unit of a replacement is the pair, not the picture.

    Replacing `Interview wide.mp4` and keeping the `.srt` beside it delivers a segment whose
    captions belong to a different cut, and nothing on screen would say so.
    """
    video, captions = source_with_captions(tmp_path)
    output = cutter.output_for(video, 100, 299, Fraction(25), name="Interview wide")
    spec = spec_for(video, captions, output)
    assert cutter.existing_pieces(spec) == [], "nothing was written yet, so nothing is in the way"

    output.write_bytes(b"the previous cut")
    assert cutter.existing_pieces(spec) == [output]

    output.with_suffix(".srt").write_text(CUE, encoding="utf-8")
    assert cutter.existing_pieces(spec) == [output, output.with_suffix(".srt")]


def test_a_transcript_this_cut_will_not_write_is_not_reported_as_being_in_the_way(tmp_path):
    """A leftover `.srt` beside an unnamed range's output is not this run's business.

    The pass only writes captions when the source has a transcript. Counting a file the cut will
    never touch would ask the operator to approve replacing something that is not going to be
    replaced, and a question that is not true is worth less than no question.
    """
    video, captions = source_with_captions(tmp_path)
    output = cutter.output_for(video, 100, 299, Fraction(25), name="Interview wide")
    output.with_suffix(".srt").write_text(CUE, encoding="utf-8")

    without_transcript = cutter.TrimSpec(source=video, output=output, in_frame=100, out_frame=300)
    assert cutter.existing_pieces(without_transcript) == []

    with_transcript = spec_for(video, captions, output)
    assert cutter.existing_pieces(with_transcript) == [output.with_suffix(".srt")]


def test_the_question_names_the_file_it_is_about(tmp_path):
    """A dialog that does not say which file is a dialog nobody can answer."""
    only = tmp_path / "Interview wide.mp4"
    assert cutter.replace_refusal([only]) == "Interview wide.mp4: already exists"

    both = [only, tmp_path / "Interview wide.srt"]
    sentence = cutter.replace_refusal(both)
    # The verb agrees with the count. "2 files already exists" is what concatenating produces, and
    # it is the first thing anybody reads.
    assert sentence == "2 files already exist: Interview wide.mp4, Interview wide.srt", sentence
    assert "exist" in sentence and "exists" not in sentence


def test_a_cut_does_not_replace_an_existing_segment_without_being_told_to(tmp_path, monkeypatch):
    """The default is no. This is the whole of the ask, at the level where the file is written.

    Two things are being checked and both matter: that the refusal happens at all, and what the
    output looked like afterwards. A refusal that had already truncated the file would satisfy the
    first on its own -- which is why the bytes are compared rather than the exception.
    """
    video, captions = source_with_captions(tmp_path)
    output = tmp_path / "Interview wide.mp4"
    output.write_bytes(b"the previous cut, four hours of work")
    before = output.read_bytes()

    monkeypatch.setattr(cutter.ff, "probe", lambda *a, **k: media(video))
    monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: [8.0])

    spec = spec_for(video, captions, output)
    spec.in_frame, spec.out_frame = 100, 300
    assert spec.overwrite is False, "the engine's own default must be to refuse, not to replace"

    with pytest.raises(cutter.TrimError) as refused:
        cutter.trim(spec)

    assert cutter.ALREADY_THERE in str(refused.value)
    assert output.name in str(refused.value)
    assert output.read_bytes() == before, "the file was touched on the way to being refused"


def test_a_cut_does_replace_an_existing_segment_once_it_has_been_told_to(tmp_path, monkeypatch):
    """And with permission the same cut goes ahead: the gate is a gate and not a wall.

    The encoder itself is stood in for -- the point here is which branch the guard takes, not what
    ffmpeg does with the pictures -- so a run that gets past the refusal fails later and for a
    different reason, or not at all. What must not happen is the refusal.
    """
    video, captions = source_with_captions(tmp_path)
    output = tmp_path / "Interview wide.mp4"
    output.write_bytes(b"the previous cut")

    monkeypatch.setattr(cutter.ff, "probe", lambda *a, **k: media(video))
    monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: [8.0])
    monkeypatch.setattr(cutter.ff, "run", lambda *a, **k: None)
    monkeypatch.setattr(cutter, "_copy_packets", lambda *a, **k: None)
    monkeypatch.setattr(cutter, "_write_sound",
                        lambda *a, **k: ["-hide_banner", "-y", "-i", str(video), "out.m4a"])
    monkeypatch.setattr(cutter, "_mux",
                        lambda *a, **k: ["-hide_banner", "-y", "-i", str(video), str(output)])
    monkeypatch.setattr(cutter.ff, "inspect",
                        lambda *a, **k: SimpleNamespace(frames=200, video_duration=8.0))

    spec = spec_for(video, captions, output)
    spec.overwrite = True
    try:
        cutter.trim(spec)
    except Exception as later:  # noqa: BLE001 - any later failure is still not the refusal
        assert cutter.ALREADY_THERE not in str(later), (
            "the cut refused a replacement it had been given permission for"
        )

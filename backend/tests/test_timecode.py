"""Timecode, in both directions, at the rates that actually appear in a timeline.

## What is worth testing here

One thing, and it is the reason this module exists rather than a `float` division: on 29.97
material, one hour of wall clock is 107892 frames while non-drop timecode counts 108000
labels. The two readings of `01:00:00:00` are **108 frames apart**, and a trimmer that picks
the wrong one is wrong by four and a half seconds at the top of a reel.

Nothing here launches a process, so all of it runs in milliseconds.
"""

from __future__ import annotations

from fractions import Fraction

import pytest

from trimmer.timecode import (
    TimecodeError,
    format_rate,
    format_seconds,
    format_timecode,
    nominal_rate,
    parse_rate,
    parse_timecode,
)

NTSC_30 = Fraction(30000, 1001)
NTSC_60 = Fraction(60000, 1001)
PAL = Fraction(25)
CINEMA = Fraction(24000, 1001)


# ---------------------------------------------------------------------------------------
# Rates
# ---------------------------------------------------------------------------------------

def test_a_rate_spelled_the_way_ffprobe_spells_it_is_exact():
    # ffprobe writes 29.97 material as the fraction, never as the decimal, so this is the
    # branch a real file takes — and it is exact, which is the point of the whole module.
    assert parse_rate("30000/1001") == NTSC_30
    assert parse_rate("24000/1001") == CINEMA
    assert parse_rate("25") == PAL
    assert parse_rate(25) == PAL


def test_the_decimal_spelling_is_read_as_the_decimal_it_is():
    # `29.97` is 2997/100, not 30000/1001: it is a decimal approximation a person types, and
    # pretending otherwise would be inventing precision the text does not carry. It still
    # counts to 30 and still drops two labels a minute, which is what every caller uses it
    # for — see the two assertions below.
    typed = parse_rate("29.97")
    assert typed == Fraction(2997, 100)
    assert nominal_rate(typed) == 30
    assert parse_timecode("00:01:00;02", typed) == 1800


def test_a_rate_that_is_not_a_whole_number_counts_to_the_next_one():
    # Timecode counts to 30 on 29.97 material: there is no frame labelled 30.
    assert nominal_rate(NTSC_30) == 30
    assert nominal_rate(NTSC_60) == 60
    assert nominal_rate(CINEMA) == 24
    assert nominal_rate(PAL) == 25


def test_a_fractional_rate_is_handed_to_ffmpeg_as_a_fraction():
    assert format_rate(NTSC_30) == "30000/1001"
    assert format_rate(PAL) == "25"


# ---------------------------------------------------------------------------------------
# Drop frame: the 108 frames
# ---------------------------------------------------------------------------------------

def test_one_hour_of_drop_frame_is_one_hour_of_wall_clock():
    # 108000 labels minus two per minute, nine minutes in ten = 107892 real frames.
    assert parse_timecode("01:00:00:00", NTSC_30) == 108000
    assert parse_timecode("01:00:00;00", NTSC_30) == 107892
    assert parse_timecode("01:00:00:00", NTSC_30) - parse_timecode("01:00:00;00", NTSC_30) == 108


def test_drop_frame_skips_the_labels_it_says_it_does():
    # Two labels a minute are skipped, except on every tenth minute.
    assert parse_timecode("00:01:00;02", NTSC_30) == 1800
    assert parse_timecode("00:10:00;00", NTSC_30) == 18000 - 18


def test_a_full_round_trip_holds_at_every_rate():
    # The labels that exist are the labels that come back. A round trip is the only check
    # that catches an off-by-one in either direction at once.
    for rate in (NTSC_30, NTSC_60, PAL, CINEMA, Fraction(30), Fraction(24)):
        for frame in (0, 1, 24, 25, 29, 30, 1799, 1800, 107891, 107892, 200000):
            rendered = format_timecode(frame, rate)
            assert parse_timecode(rendered, rate) == frame, f"{rate} {frame} -> {rendered}"


def test_premiere_gets_the_separator_it_expects():
    assert ";" in format_timecode(1000, NTSC_30)
    assert ";" in format_timecode(1000, NTSC_60)
    assert ":" in format_timecode(1000, PAL)
    # Exactly 30 fps counts thirty labels a second and skips none, so a semicolon on it
    # would be a different reading rather than a stylistic choice.
    assert ":" in format_timecode(1000, Fraction(30))


# ---------------------------------------------------------------------------------------
# Refusals
# ---------------------------------------------------------------------------------------

def test_a_frame_that_this_rate_cannot_name_is_refused():
    with pytest.raises(TimecodeError, match="counts to 24"):
        parse_timecode("00:00:00:25", PAL)
    with pytest.raises(TimecodeError, match="counts to 29"):
        parse_timecode("00:00:00:30", NTSC_30)


def test_drop_frame_on_a_rate_that_has_no_drop_frame_form_is_refused():
    # Rendering this drop-frame would shift every stamp by two labels a minute while
    # looking perfectly plausible.
    with pytest.raises(TimecodeError, match="no drop-frame form"):
        parse_timecode("00:00:00;00", PAL)
    with pytest.raises(TimecodeError, match="no drop-frame form"):
        parse_timecode("00:00:00;00", Fraction(30))


def test_nonsense_is_refused_rather_than_guessed_at():
    for bad in ("", "   ", "hello", "00:70:00:00", "00:00:70:00", "1:2:3:4:5"):
        with pytest.raises(TimecodeError):
            parse_timecode(bad, PAL)


def test_the_short_forms_are_accepted_because_people_paste_them():
    # Three fields are HH:MM:SS with no frames; a bare number is seconds.
    assert parse_timecode("00:00:04", PAL) == 100
    assert parse_timecode("4", PAL) == 100
    assert parse_timecode("00:00:04.00", PAL) == parse_timecode("00:00:04:00", PAL)


# ---------------------------------------------------------------------------------------
# Log formatting
# ---------------------------------------------------------------------------------------

def test_seconds_are_rendered_with_milliseconds_for_a_log():
    assert format_seconds(0) == "00:00:00.000"
    assert format_seconds(3661.5) == "01:01:01.500"

"""Premiere-style timecode, in both directions, drop-frame included.

Premiere shows source timecode as ``HH:MM:SS:FF`` and, on 29.97 and 59.94 material,
as ``HH:MM:SS;FF`` -- the semicolon means drop-frame. The distinction is not cosmetic:
on a 29.97 clip one hour of wall-clock is 107892 frames, but non-drop timecode counts
108000 labels, so the two readings of "01:00:00:00" are 108 frames apart. Parsing the
separator is therefore the only honest way to read a pasted timecode.

Everything here works in **frame numbers**, never in seconds. A timecode names a frame;
the source's frames are numbered from zero at its own rate; converting through seconds
on the way in would only add rounding to a job whose whole point is being frame-exact.
Seconds appear once, at the very end, when a command line needs them.
"""

from __future__ import annotations

import re
from fractions import Fraction

#: Rates whose timecode is normally written drop-frame, and how many labels they skip.
DROP_FRAMES_PER_MINUTE = {30: 2, 60: 4}


class TimecodeError(ValueError):
    """A timecode that cannot be read at this frame rate."""


def nominal_rate(rate: Fraction | float) -> int:
    """The integer rate timecode counts at: 30 for 29.97, 24 for 23.976, 25 for 25."""
    value = float(rate)
    if abs(value - round(value)) < 0.001:
        return round(value)
    return int(value) + 1


def parse_rate(text: str | float | Fraction) -> Fraction:
    """``"30000/1001"``, ``"29.97"`` or ``30`` -> an exact Fraction."""
    if isinstance(text, Fraction):
        return text
    if isinstance(text, (int, float)):
        return Fraction(text).limit_denominator(1001)
    text = text.strip()
    if "/" in text:
        numerator, denominator = text.split("/", 1)
        return Fraction(int(numerator), int(denominator))
    return Fraction(text).limit_denominator(1_000_000)


def drop_frame_rate(rate: Fraction, drop: bool) -> int:
    """Labels skipped per minute, or 0 when this rate has no drop-frame form.

    Drop-frame exists for the 1000/1001 rates only. Exact 30 fps counts thirty labels a
    second and skips none, so a semicolon on such a file is a mistake rather than a
    different reading -- and rendering it drop-frame would shift every stamp by two
    labels a minute, which is what an early version of this module did.
    """
    if not drop:
        return 0
    if abs(float(rate) - nominal_rate(rate)) < 0.001:
        return 0
    return DROP_FRAMES_PER_MINUTE.get(nominal_rate(rate), 0)


def format_rate(rate: Fraction | float) -> str:
    """A rate as ffmpeg wants it: a whole number or an exact fraction."""
    fraction = parse_rate(rate)
    if fraction.denominator == 1:
        return str(fraction.numerator)
    return f"{fraction.numerator}/{fraction.denominator}"


def parse_timecode(text: str, rate: Fraction | float) -> int:
    """A timecode -> the frame number it names, counted from zero.

    Four fields are ``HH:MM:SS:FF``; a semicolon before the frames marks drop-frame,
    and ``.`` or ``,`` are accepted for it too because people paste timecode out of
    tools that render it that way. Three fields are read as ``HH:MM:SS`` (no frames),
    and a bare number is seconds. Anything else is refused rather than guessed at.
    """
    rate = parse_rate(rate)
    text = text.strip().replace(",", ";")
    if not text:
        raise TimecodeError("empty timecode")

    if re.fullmatch(r"\d+(\.\d+)?", text):
        # Plain seconds, for scripting: rounded to the nearest frame.
        return round(float(text) * float(rate))

    drop = ";" in text
    fields = re.split(r"[:;.]", text)
    if len(fields) == 3:
        fields = [*fields, "0"]
    if len(fields) != 4 or not all(re.fullmatch(r"\d+", field) for field in fields):
        raise TimecodeError(f"cannot read {text!r} as HH:MM:SS:FF")
    hours, minutes, seconds, frames = (int(field) for field in fields)

    nominal = nominal_rate(rate)
    if minutes > 59 or seconds > 59:
        raise TimecodeError(f"{text!r} has minutes or seconds above 59")
    if frames >= nominal:
        raise TimecodeError(
            f"{text!r} names frame {frames}, but this clip counts to {nominal - 1} "
            f"({format_rate(rate)} fps); use ; for drop-frame on 29.97/59.94"
        )
    if drop and drop_frame_rate(rate, True) == 0:
        raise TimecodeError(
            f"{text!r} is written drop-frame, but {format_rate(rate)} fps has no "
            "drop-frame form"
        )

    label = (hours * 3600 + minutes * 60 + seconds) * nominal + frames
    if drop:
        skipped = drop_frame_rate(rate, True)
        total_minutes = hours * 60 + minutes
        label -= skipped * (total_minutes - total_minutes // 10)
    return label


def format_timecode(frame: int, rate: Fraction | float, drop: bool | None = None) -> str:
    """A frame number -> HH:MM:SS:FF (or HH:MM:SS;FF when drop-frame is used).

    ``drop=None`` picks the form Premiere would use: drop-frame for 29.97 and 59.94,
    non-drop otherwise.
    """
    rate = parse_rate(rate)
    nominal = nominal_rate(rate)
    if drop is None:
        drop = drop_frame_rate(rate, True) > 0
    skipped = drop_frame_rate(rate, True) if drop else 0

    if skipped:
        frames_per_minute = nominal * 60 - skipped
        frames_per_ten = nominal * 600 - skipped * 9
        tens, rest = divmod(frame, frames_per_ten)
        frame = frame + skipped * 9 * tens
        if rest > skipped:
            frame += skipped * ((rest - skipped) // frames_per_minute)
    hours, rest = divmod(frame, nominal * 3600)
    minutes, rest = divmod(rest, nominal * 60)
    seconds, frames = divmod(rest, nominal)
    separator = ";" if drop else ":"
    return f"{hours:02d}:{minutes:02d}:{seconds:02d}{separator}{frames:02d}"


def format_seconds(seconds: float) -> str:
    """Seconds -> ``HH:MM:SS.mmm``, for logs."""
    total_ms = round(seconds * 1000)
    hours, rest = divmod(total_ms, 3_600_000)
    minutes, rest = divmod(rest, 60_000)
    secs, millis = divmod(rest, 1000)
    return f"{hours:02d}:{minutes:02d}:{secs:02d}.{millis:03d}"

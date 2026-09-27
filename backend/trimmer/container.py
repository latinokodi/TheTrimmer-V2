"""What a container holds, in the order it holds it.

Every frame-numbering bug in this engine came from the same mistake: deriving a frame number by
dividing a timestamp. A timestamp is not a frame number. Two packets can carry the same
presentation time, a file's frames need not sit evenly on its average rate, and a computed time
can drift a fraction of a millisecond past the frame it names -- each of which has produced a
segment that was a frame or six frames wrong.

A container, on the other hand, hands over exactly one video packet per frame, in decode order.
So that is the numbering: **the Nth video packet is frame N**. Nothing is derived, nothing is
rounded, and the answer cannot slide.

This module owns that one fact so the rest of the engine never has to reason about it again.
"""

from __future__ import annotations

import threading
from dataclasses import dataclass
from pathlib import Path

import av

from .ffmpeg import CancelToken, Cancelled


@dataclass(frozen=True)
class Packet:
    """One video packet: which frame it is, and how to find it again."""

    #: The packet's position in decode order, counted from zero. This **is** the frame number.
    frame: int
    #: Presentation and decode times, in the stream's own units. Kept for the callers that need
    #: to explain where a frame sits on the clock; never used to work out a frame number.
    pts: int
    dts: int
    #: True when this packet can be decoded on its own, so a copy may begin here.
    key: bool
    size: int


@dataclass(frozen=True)
class Index:
    """A source's video packets, in decode order, with the rate and time base they live on."""

    path: Path
    packets: tuple[Packet, ...]
    time_base_denominator: int
    rate_numerator: int
    rate_denominator: int
    #: The first presentation time in the file, in the stream's units.
    first_pts: int

    @property
    def count(self) -> int:
        return len(self.packets)

    def rate(self) -> float:
        return self.rate_numerator / self.rate_denominator

    def time_of(self, frame: int) -> float:
        """The presentation time of a frame, in seconds.

        The container's own stated time, converted -- not a frame number times an interval.
        """
        if not 0 <= frame < len(self.packets):
            return 0.0
        return (self.packets[frame].pts - self.first_pts) / self.time_base_denominator

    def keyframes(self) -> list[int]:
        """Every frame that can start a copy, in ascending order."""
        return [packet.frame for packet in self.packets if packet.key]

    def keyframe_at_or_after(self, frame: int) -> int | None:
        for packet in self.packets:
            if packet.frame >= frame and packet.key:
                return packet.frame
        return None

    def keyframe_at_or_before(self, frame: int) -> int | None:
        found = None
        for packet in self.packets:
            if packet.frame > frame:
                break
            if packet.key:
                found = packet.frame
        return found

    def frames(self, first: int, last: int) -> tuple[Packet, ...]:
        """Frames ``first..last`` inclusive, in decode order. Empty when the range is empty."""
        if first > last:
            return ()
        return self.packets[max(0, first):last + 1]


_cache: dict[tuple[str, int, int], Index] = {}
_lock = threading.Lock()


def read(path: Path, cancel: CancelToken | None = None) -> Index:
    """Read a source's packet index, remembering it for a file that has not changed.

    A demux is not a decode, so this is a walk of the container's packet headers rather than of
    its pictures: measured on the 2.5 GB reference master it is a few seconds, and the result is
    what lets every later decision be exact instead of inferred.
    """
    try:
        stat = path.stat()
        key = (str(path), stat.st_size, stat.st_mtime_ns)
    except OSError:
        key = None
    if key is not None:
        with _lock:
            remembered = _cache.get(key)
        if remembered is not None:
            return remembered

    packets: list[Packet] = []
    with av.open(str(path)) as container:
        stream = container.streams.video[0]
        time_base = stream.time_base
        rate = stream.average_rate or stream.guessed_rate
        if rate is None or time_base is None:
            raise ValueError(f"{path.name} reports no usable rate or time base")
        first_pts: int | None = None
        for seen, packet in enumerate(container.demux(stream)):
            if packet.pts is None or packet.dts is None:
                # A flush packet carries the last frames of the stream out of the decoder and
                # holds no picture of its own. It has no presentation time, so it is not a frame.
                continue
            if cancel is not None and seen % 2000 == 0 and cancel.cancelled:
                raise Cancelled("cancelled")
            if first_pts is None:
                first_pts = packet.pts
            packets.append(Packet(
                frame=len(packets),
                pts=packet.pts,
                dts=packet.dts,
                key=bool(packet.is_keyframe),
                size=packet.size or 0,
            ))
    if not packets:
        raise ValueError(f"{path.name} holds no video packets")

    index = Index(
        path=path,
        packets=tuple(packets),
        time_base_denominator=int(time_base.denominator),
        rate_numerator=int(rate.numerator),
        rate_denominator=int(rate.denominator),
        first_pts=first_pts or 0,
    )
    if key is not None:
        with _lock:
            _cache[key] = index
    return index


def forget() -> None:
    """Drop every remembered index. For tests, and for a source that was written to in place."""
    with _lock:
        _cache.clear()

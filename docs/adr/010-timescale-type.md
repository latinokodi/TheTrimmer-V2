# ADR-010: A container timescale is not a frame rate

**Status:** accepted

**Context.** This one is a bug that was written, found, and then removed by changing a type.

MP4 keeps a timescale per track: the number of ticks a track counts per second. These sources
almost always use `1/90000`. ffprobe reports that field as `time_base`, and the first V2 probe
layer modelled it as a `FrameRate` — a rate, `1/90000`, built by `FrameRate::new(1, 90_000)`.
That is the wrong type for the value in two ways, and both of them are silent:

1. A timescale of `1/90000` has a "frames per second" of **0.000011**. A rate validator that
   reasons about frame rates — `nominal()`, `is_fractional()`, `seconds_of()`, the range check
   that refuses anything outside what timecode can be counted on — is being handed a number that
   is not a rate at all, and refuses it, correctly, for a value that is perfectly valid.
2. Worse, when it does not refuse it: `FrameRate::numerator()` off a rate built from `1/90000`
   returns **1**, not 90000. The plan would then carry `video_timescale = 1`, and
   `prepare_head` would write `-video_track_timescale 1`. The muxer joins a head at that timescale
   to a body at `1/90000` and does the only thing it can: it rescales the **copied** packets. The
   deliverable plays in slow motion with frozen stretches and ffmpeg exits `0`.

That second failure is V1 ADR-001, reintroduced by a type choice in the code that was written to
prevent it. The invariant that was supposed to catch it — `PlanInvariant::HeadMatchesSourceTimebase`
— checks `video_timescale > 0`, and `1` is greater than zero. The check was satisfied by exactly
the wrong value.

**Decision.** `Timescale` is its own type in `trimmer-core::domain`, with one field,
`ticks_per_second`, and `Timescale::ticks()` returning the number ffmpeg is given. It is not
derived from, convertible to, or comparable with `FrameRate`; there is no method on it that
answers a frames-per-second question, because that question is meaningless for a tick rate.
`Timescale::from_ffprobe_time_base` parses the `1/den` form by taking the **denominator**
explicitly and refusing anything whose numerator is not 1, which is where the `1` versus `90000`
confusion used to happen.

**Consequences.** The confusion is unrepresentable rather than merely fixed: there is no `.numerator()`
to call on the wrong object because the wrong object is the wrong type, and a caller who has a
`Timescale` cannot accidentally pass it to anything that wants a rate. `MediaInfo::timebase` is a
`Timescale`, `CutPlan::video_timescale` carries `ticks()`, and `prepare_head` and
`prepare_reencode` write that integer straight into `-video_track_timescale`.

The cost is one more type and one more conversion at the ffprobe boundary. In exchange, the
slow-motion failure has a positive check rather than a plausible one:
`PlanInvariant::HeadMatchesSourceTimebase` now guards a value that can only come from the source's
own time base.

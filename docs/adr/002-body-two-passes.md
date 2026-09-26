# ADR-002: Copy the body's picture and sound separately

**Status:** accepted

**Context.** The first implementation copied the body's video and audio in one ffmpeg command.
Measurements against the source then showed the segment's content sitting a constant **three
frames (100 ms) early**. The cause, once instrumented: with a stream copy ffmpeg seeks the
*audio* to its own sync point, up to four AAC frames before the keyframe, and
`-avoid_negative_ts make_zero` rebases the file on that earlier audio packet — so the body's
video began 80 ms into its own file, and the concatenation put that body straight after the
head.

**Decision.** Build the body from two copies — picture from the keyframe, sound cut with an
*output* seek, which drops the packets before the mark instead of hunting for a sync point —
and mux them back together.

**Consequences.** The measured offset goes from −3 frames to 0, and it stays 0 across the
segment rather than drifting. One extra stream copy per trim (fast, no re-encode).

**Change in V2.** The three commands are three value objects, built before anything runs, and
the difference between an *input* seek and an *output* seek is now visible in the argument
vectors rather than in the order of two lines of an imperative function.
`trimmer-media::executor::prepare_body` returns **three** `Prepared` commands when the source
has audio: the picture copy (`-ss` before `-i`, the input seek on the keyframe), the sound copy
(`-i` first and then `-ss`, the output seek that lands on the mark), and the mux that joins
`0:v:0` and `1:a:0` with `-c copy`. A source with no audio returns one.

The rule is enforced by the test
`the_body_picture_is_seeked_as_an_input_and_the_sound_as_an_output` in
`crates/trimmer-media/src/executor.rs`, which asserts on the position of `-ss` relative to `-i`
in each command. A refactor that merges the passes — the natural-looking simplification that
produced the three-frame error — fails that test instead of shipping a hundred-millisecond
slip into every cut.

# ADR-007: Cut the transcript with the segment, clamped at the marks

**Status:** accepted

**Context.** The segment usually arrives with a transcript: `clip.mp4` next to `clip.srt`,
exported from the same session. Cutting the video and leaving the captions behind means retiming
them by hand in an editor, and a caption file that is a second out is worse than useless on a
deadline.

**Decision.** The app finds `<video>.srt` (or `<video>.<lang>.srt`) beside the video unless told
otherwise, retimes it onto the segment and writes it as a sidecar named after the output, so
players and Premiere pick it up by themselves. The rules:

* A cue that crosses a mark is **clamped** to the window when at least 0.25 s of it survives, and
  dropped when less does. Clamping keeps a word that is still audible captioned; dropping avoids
  a caption that flashes for forty milliseconds.
* Cues shift so the segment starts at 00:00:00,000 and are renumbered from 1.
* The window is the one that was *asked for*, never the file that came out: the frames a stream
  copy adds past the out point must not grow captions.
* The source's own shape — BOM, line endings, decimal separator — is preserved, because these
  files get diffed and re-imported.
* When nothing falls inside the window, no file is written and the log says so, with the hint
  that the transcript may already be relative to the segment.

**Consequences.** Captions and picture arrive together, and `--verify` compares the written file
cue by cue with the source shifted, so a drift fails the check instead of shipping. Pointing the
app at a transcript that is already segment-relative is refused rather than producing an empty
file.

**Change in V2.** The rules are the same; they now live in a function that cannot be reached by
anything but a caller who hands it the window. `trimmer-core::caption::retime` takes cues, a
start, an end and a minimum overlap, and returns the retimed cues; it opens no file, starts no
process and reads no clock, so the clamp rules can be tested exhaustively without media. The
window arithmetic is in frames at the source's rate, and `MIN_OVERLAP` is 0.25 s as before. The
file's shape — `Transcript::newline`, `Transcript::bom` — is carried on the parsed value, and
`render` writes it back.

**One bug V1 had, found in V2 and fixed.** V1's renderer used the file's own line ending
*between* cues but not *inside* a multi-line cue, so a CRLF transcript came back with LF inside
every cue that wrapped. The file still parsed and the captions still displayed; what changed was
that every cue showed as modified in `git diff`, which for files that are diffed and re-imported
is a real cost. `caption::render` now normalises `\r\n`, `\r` and `\n` in the cue text to the
file's own ending, so a cue cannot carry a mixture and a round trip is byte-identical. This was
found by running the V1 engine to establish ground truth before changing an expectation, not by
reading the code — see ADR-008.

The verification side is deliberately not a second implementation of the clamp rules:
`trimmer-verify`'s `Captions` check compares the written file against the source cues retimed by
`caption::retime` itself, so the checker cannot drift from the thing it checks. A caption file
that is a second out fails, and one that is a frame out fails too — both sides are
millisecond-resolution SRT and the tolerance is a millisecond.

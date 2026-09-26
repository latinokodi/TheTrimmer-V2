# ADR-006: A window with no preview

**Status:** superseded by ADR-009

**Context.** A preview means decoding the file to draw a frame — the slowest thing the app could
do, for the least information. The timecodes are exact and the plan is printed before anything
is written.

**Decision.** One window: file picker (no preview pane), two timecode fields with the parsed
frame range shown live underneath, options, Trim/Cancel, a progress bar and a log of the ffmpeg
commands. Validation errors appear next to the field that caused them; buttons are disabled
while a trim runs so a second click cannot start a second ffmpeg.

**Consequences.** The window opens instantly on a 6 GB source and needs no media pipeline. Users
who want to *see* the frames use Premiere, which is where the timecodes come from anyway.

**Change in V2.** V2 overturns the decision, and the honest version of why is worth stating
plainly: V1 was right about the thing it was arguing against, and wrong about the product it was
building.

V1 refused a preview because a preview meant *a live view of the playhead* — decode on demand,
during editing, on the machine that is also running the cut. That is still refused. What V2 adds
is a different thing with a different cost: a **timeline strip** and a **filmstrip** built from a
bounded number of thumbnails, extracted once, lazily, and cached on disk. The reason is that the
V2 workspace is multi-segment: a project holds many segments across several sources, and an
operator looking at a list of twenty cuts has no way to tell which is which from timecodes alone.
One thumbnail at the segment's in point is what makes the list readable, and one thumbnail per
segment is a bounded cost that does not scale with the length of the material or with how long
the window is open.

What is still refused, and must stay refused:

* **No scrubbing preview.** Dragging a playhead must not trigger a decode per frame.
* **No real-time decode during a cut.** Thumbnail extraction runs through `trimmer-media` like
  every other process the application starts, on its own schedule, and never concurrently with a
  cut. A cut is disk-bound, not CPU-bound; a decode competing for the same spindle makes the cut
  slower and the progress display meaningless.
* **No pipeline in the UI process.** The page draws a cached image. It does not open a media
  file, and it never will — see ADR-009.

The cost is bounded and stated: a cache directory of small images, an extraction pass the first
time a segment is shown, and a rule that the strip degrades to a placeholder rather than stalling
when the cache is cold.

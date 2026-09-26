# ADR-018: The visual language — colour is state, the faces are bundled, and no string is clipped

**Status:** accepted
**Date:** the day three faults were reported in one sentence and all three turned out to be one rule each

## Context

The panel shipped three times and was rejected three times, and the third rejection named its reasons
precisely: *"Theme is not correct, fonts are ugly and unprofessional, some texts cutoff."*

Those read as taste. They were not. Each was a specific, measurable fault, and each had a cause worth
recording — including two that no test in the suite could have seen and one that was a real defect
being hidden by the stylesheet rather than shown by it.

### Fault one: the theme was not correct

The palette was built on the sentence *"one accent, aviation hazard red."* That is a defensible idea for
a warning lamp and a bad one for a product, because the accent was applied to the primary action:

```css
.btn--primary { background: var(--hazard); color: var(--phosphor-inverse); }
```

So the **Trim** button — the button the entire application exists for — was painted in the same red the
interface used for *a cut that cannot be made*. `input[aria-invalid="true"]`, `.status--danger`,
`--signal-danger` and the primary action were one colour. Red means stop. A tool that shouts STOP on the
button you press to work is a tool that fights its user, and the practical result was that the loudest
object on screen was also the most ambiguous one.

There was a second problem in the same set: the focus ring was hazard red too. A focus ring that is
indistinguishable from an error state is a focus ring nobody can trust.

### Fault two: the fonts were ugly

Two separate causes, and the fix for one would not have fixed the other.

**Everything was monospace.** `body { font-family: var(--font-data) }` and `.app` inherited it, so the
prose went into a code face: the plan line, the empty states, the help under a field, the sentences in
the export dialog. A sentence set in a monospace face reads as a log entry, not as software, and a
window where *every* string is monospace has no typographic hierarchy at all — the eye has nothing to
rank.

**The faces belonged to the machine, not to the product.**

```css
--font-data: "Cascadia Mono", Consolas, "Cascadia Code", ui-monospace, monospace;
--font-ui: "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
```

Cascadia Mono ships with Windows Terminal and with Windows 11. Segoe UI Variable is Windows 11. A
Windows 10 workstation with neither falls back to Consolas and to Segoe UI — different faces with
different advance widths. For a column of timecodes that is not a cosmetic difference: Consolas is nine
units per character and Cascadia Mono is eight, so the column stops lining up the moment it lands on the
wrong machine. A product that is sold cannot change shape depending on what the buyer happens to have
installed.

### Fault three: text was cut off

The one that had been reported before and had never been fixed, because the mechanism was invisible.

* **Strips shorter than their own line box.** A 10 px uppercase title at `line-height: 1.25` is a 12.5 px
  line box. The zone header was 22 px with 0.16 em of tracking, holding a title *and* a note; the footer
  was 24 px holding five items with `white-space: nowrap` and no ellipsis. Things clipped.
* **A hard-coded viewport.** `const VIEWPORT_HEIGHT = 320` in the transcript list, while the zone it sat
  in was 60 px tall. The list rendered 320 px of rows into a 60 px box with `overflow: hidden` above it.
* **A class with no rule.** `ProofPanel` rendered `<div className="panel-body">`, and `.panel-body` was
  **declared in no stylesheet** — the card it belonged to had been deleted in an earlier shell and its
  CSS went with it. So it was a plain block: as a flex item it defaulted to `min-height: auto`, refused
  to shrink below its content, and pushed 692 px of proof into a 524 px zone under an
  `overflow: hidden` ancestor. Measured, not guessed: 822 px of content in a 654 px column.
* **A minimum the content could not meet.** `tauri.conf.json` declared `minHeight: 900` while the form
  column needed 572 px of a 538 px space. Two zones were silently overlapping by 34 px at the exact size
  the window promised to support.

That last pair is the important part. **Nothing in a browser calls either of those an error.** A
squeezed flex item does not clip — its content overflows *visibly* and lands on top of the zone below,
and a clipped element with `overflow: hidden` is just an element that is smaller than its content. Both
had been shipping for several revisions, and both would have kept shipping, because the only test that
existed asked whether the *frame* scrolled and whether the zone *headings* were on screen. Neither was
false.

## Decision

Three rules, one per fault.

### 1. Colour is state, never decoration

There is exactly one filled primary control and it has **no hue**: near-white on dark, dark type on it.
It is the highest-contrast object in the window, which is what "this is the button that does the work"
ought to look like, and because it carries no colour it can never be confused with a verdict.

Hue appears in four places, each with one meaning:

| Token | Meaning | Used for |
|---|---|---|
| `--verified` green | a measurement was made, and it matched | the ffmpeg lamp, `passed`, `verified`, `copy` |
| `--caution` amber | a person should look at this; it is not wrong | `uncertified`, `head 12%`, a note |
| `--hazard` red | it failed, or it cannot be done | `cannot cut`, `failed`, `aria-invalid` |
| `--select` blue | where the keyboard is, and which row is selected | focus ring, selected row, chosen preset |

Blue for focus and selection specifically because *none of the three status colours can be the focus
ring*: a ring that looks like a verdict makes the verdict unreadable.

The substrate is cool graphite in six even steps. Cool rather than warm because a neutral-grey panel
beside a video frame tints the operator's perception of the frame, and a blue-shifted grey does it less
than a warm one. Six even steps because uneven ones read as banding across a large dark surface.

Every text token is measured against the surface it is actually used on and clears **WCAG 2.1 AA**.
`--phosphor-faint`, the weakest, is 4.8:1 on a panel and 5.3:1 on the floor; the comment beside each
token records its measured ratio, so the next person to change one knows what they are spending.

### 2. Bundle the faces, and use each for what it is for

Two families, latin subset, **158 KB total**, both under the SIL Open Font License 1.1 which permits
bundling in a commercial product provided the notice travels with the files — see
`apps/web/src/fonts/LICENCE.md`.

* **Inter** for everything read as language. Drawn for screens, tall x-height, open apertures that hold
  at 11–13 px, and it has a real tabular-figure feature.
* **JetBrains Mono** for everything compared as data. Deliberately tall x-height for a code face, and
  disambiguated `0/O` and `1/l/I` — which matters more here than in most places, because this
  application prints `00:00:01:00` and `00:00:01:0O` is a different frame.

Inter is the **default**. Mono is applied per element: timecodes, frame counts, paths, digests, the log,
the checks table, and `.figures` anywhere a column has to line up with the row above it. A button is a
word, not a datum, so buttons are Inter.

Only the latin subset ships. The interface has no localisation.

### 3. A size has a floor, a strip is derived from its type, and nothing is clipped

* **No type below 10 px**, and `layout.spec.ts` fails on any computed `font-size` under it. Below 10 px,
  letter-spaced capitals stop resolving on a 1× display, which is what made the old headers look cut off
  even where they technically fitted.
* **A strip's height is derived, not chosen.** 10 px capitals at 1.25 are a 12.5 px line box; the strip
  is 26 px, which leaves 6.75 px of air above and below.
* **Every string that can be any length ends in one of two ways**: an explicit `text-overflow: ellipsis`
  with `min-width: 0`, or a wrap. There is no third option, because the third option is text running off
  the edge of the panel.
* **The frame's minimum is derived from the budget.** `tokens.css` states the arithmetic: 518 px of form,
  plus the title bar, the two bands at their own minimums and the footer, is 960. Declaring a minimum
  that the content cannot meet is worse than declaring a larger one.
* **One zone never absorbs all the slack.** Every form zone grows by the same amount (`flex: 1 0 auto`)
  so the leftover height becomes even air rather than a hole inside one compartment — which is what the
  first version did, with 116 px of nothing under the range fields at 1080p. `flex-shrink: 0` because a
  zone squeezed under its content does not clip, it *overlaps*.

## Consequences

**The tests are the point of this ADR, not the colours.**

`apps/web/tests/layout.spec.ts` now measures the rendered window rather than describing it:

| Test | What it would have caught |
|---|---|
| `no text is clipped, at the opening size` | walks every leaf element; fails when an element with `overflow: hidden` hides content **and is not ellipsised**, when a box leaves the window, or when any type is under 10 px |
| `…at the window's own minimum` | the same at 1440x960, where the budget is tight |
| `no zone is squeezed below its own content` | **both** silent defects: the missing `.panel-body` rule and the 900 px minimum the content could not meet |
| `no text is clipped inside the dialogs` | a fixed-width panel in a scrim, where a long note has nowhere to go |
| `the two bundled faces, and nothing else, are what the window asks for` | a woff2 that 404s and falls back silently — the font stack assertion alone would still pass |
| `the light theme is the same panel, and nothing is clipped in it either` | the light value set, which is shipped and therefore measured |

The clipping probe is deliberately narrow. An element with `text-overflow: ellipsis` is **not** reported,
because a deliberate ellipsis is a decision the operator can see and a clip is not; `.sr-only` is exempt
by name, because it is a 1×1 box on purpose; and an element inside a scroll well is exempt from the
"outside the window" rule, because holding more than it shows is what a well is for.

**Good.**

* Three faults that had been reported as taste are now three rules with a test each.
* The panel cannot silently overlap itself again. Both of the invisible defects were found by the new
  probe within one run of writing it.
* The interface renders identically on a machine with neither Cascadia Mono nor Segoe UI Variable.
* Two classes that had **no rule at all** — `.panel-body` and `.field__label`/`.field__help`, which is
  why every label in both dialogs was rendering at body size in body colour — were found by auditing
  every `className` in the source against the stylesheet, and are now either styled or gone.

**Costs, stated plainly.**

* 158 KB of fonts in a bundle whose JavaScript is 200 KB. It is the cheapest 158 KB in the product: it
  is what makes the window look the same everywhere.
* The minimum window grew from 1280x800 to **1440x960**. That is a real reduction in the machines this
  runs on, and it is the honest one: a panel that promises 800 px and needs 960 is a panel that clips.
* The clipping probe is a heuristic. It cannot see text that is *overlapped* by a later sibling in a way
  that keeps both boxes inside the window — which is why the squeeze test exists separately, and why it
  compares each zone body's `scrollHeight` to its `clientHeight` rather than anything visual.

## Alternatives considered

**Rejected — put the accent back but pick a nicer one.** Amber, teal and a desaturated indigo were all
tried as the primary action's colour. Every one of them competes with a status colour: amber with
`--caution`, teal-ish green with `--verified`, indigo with `--select`. A colourless primary is not a
compromise, it is the only choice that leaves all four semantic colours free.

**Rejected — a light theme as the design.** A grading suite is dim. A bright surface beside a video
monitor destroys the operator's adaptation to the picture. A light value set still exists, because
somebody will open this on a machine set to light and a window that ignores that is a window they invert
with a system tool that produces worse results — but it is not the design, and the design is dark.

**Rejected — keep the system faces and accept the fallback.** It would have saved 158 KB and cost the
one property that makes a data column readable. It also fails silently, which is the worst way to fail:
the developer's machine has Windows 11 and Cascadia Mono, so the fallback is invisible to whoever is
building it and visible to every customer who is not.

**Rejected — `clamp()` on the control heights so the panel fills a 1440p screen.** It would have used
the leftover height instead of distributing it, and it would have kept every position identical. It was
rejected because it is responsiveness under another name: the operator's row height would change with
their monitor, so muscle memory learned on one machine would be wrong on the next. The rule is that the
layout does not adapt; the slack becomes air instead.

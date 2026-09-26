# ADR-011: Decimal frame rates resolve from a table, not a continued fraction

**Status:** accepted

**Context.** This is the second bug of the same shape as ADR-010: a number that looks like a rate
but is not the rate it means.

An operator types `29.97`, or a tool writes it into a preset, or a CSV of markers carries it. The
rate that string means is `30000/1001`, because that is the rate 29.97 material actually runs at
and because that is what the V1 engine's `Fraction(2997, 100).limit_denominator(...)` resolves to.
Since ADR-008 makes the V1 engine the oracle, "what does V1 say `29.97` is" is not a matter of
taste — it is a value two implementations have to agree on, field for field.

The first implementation turned the decimal into an exact ratio with a plain continued-fraction
(Stern–Brocot) search under a denominator bound of 1001. That fails, and it fails *silently and
reasonably*:

* `29.97` is exactly `2997/100`, and `100` is under the bound, so the search accepts it and stops.
  `2997/100` is 29.97 exactly — and the wrong rate, by **six parts per million**.
* Six parts per million over a two-hour master is about **a third of a frame**. Not a visual
  difference; an accumulating one, and exactly the class of error a frame-exact tool exists not
  to make.
* The convergent that would fix it, `30000/1001`, has a denominator of 1001 — which does fit the
  bound — but the plain continued-fraction walk stops at `30/1` on the way there, and `30/1` also
  fits, so the answer depends on which fitting candidate the walk happens to return first. The
  semiconvergent between them *is* `30000/1001`, and taking it is what makes the two paths agree.

The deeper point is that no cleverer bound rescues this. A continued-fraction search is trying to
answer "what is the simplest rational near this decimal?", and the question that needs answering
is "which frame rate did the person mean?" Those are different questions, and only the second one
has a right answer.

**Decision.** `CANONICAL_RATES` is a table of the decimal spellings people actually write, mapped
to the rates this program supports. It is matched before any arithmetic, tolerating trailing
zeros so `29.970` and `29.97` are the same claim:

| Spelling | Rate |
|---|---|
| `23.976` | `24000/1001` |
| `23.98` | `24000/1001` |
| `29.97` | `30000/1001` |
| `47.952` | `48000/1001` |
| `59.94` | `60000/1001` |
| `119.88` | `120000/1001` |

`23.976` and `23.98` deliberately resolve to the same rate, because they are the same rate written
two ways and treating them as different would create two sources that cannot be joined.

The continued-fraction path is **kept** as the fallback for a decimal that is not in the table —
a `25.5`, a rate from some future capture format, a `29.97002997` — with the denominator bound at
1001 and the semiconvergent step retained, because that step is what keeps the fallback agreeing
with V1 on values near the 1000/1001 family rather than six parts per million away from them.

**Consequences.** Adding a newly met rate is adding a row, not touching an algorithm, and the row
is a statement that somebody checked what the rate means. Every entry is a rate the program
otherwise supports, so the table cannot introduce a rate the rest of the code cannot handle.

The cost is that the table is data to maintain and can be wrong: a spelling mapped to the wrong
fraction is worse than no entry, because it bypasses the arithmetic that might have caught it.
That risk is the reason the table is short and the reason the entries are the six rates a
post-production shop actually types.

One inconsistency to record while it is visible: the doc comment on `FrameRate::parse` in
`timecode.rs` still says decimal forms map through the continued-fraction expansion to exactly
`2997/100`. That sentence describes the pre-table implementation and is contradicted by
`CANONICAL_RATES` and by `decimal_to_ratio`'s own doc comment a few lines below it. The behaviour
is the table; the comment is stale and should be corrected.

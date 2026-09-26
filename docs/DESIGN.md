# Design decisions

Short records of the choices that were not obvious, and of the failures that produced them.
Each was measured on real material rather than reasoned about in the abstract; the numbers are
in the text.

V2 keeps every ADR from V1. Rewriting a product is exactly when hard-won knowledge gets lost,
so each V1 decision is carried forward and marked with what V2 changed about it. Where a V1
decision no longer applies, it is *superseded* and the reason is stated — never deleted.

- [ADR-001 — Cut with a re-encoded head, copy the body](adr/001-head-patch.md) *(carried forward)*
- [ADR-002 — Copy the body's picture and sound separately](adr/002-body-two-passes.md) *(carried forward)*
- [ADR-003 — Measure alignment with framemd5, on each file's own frame grid](adr/003-framemd5-alignment.md) *(carried forward)*
- [ADR-004 — Let the copy stop on its own packet boundary](adr/004-overshoot.md) *(carried forward)*
- [ADR-005 — Calibrate by measuring, not by trusting the arithmetic](adr/005-calibration.md) *(carried forward)*
- [ADR-006 — A window with no preview](adr/006-no-preview.md) *(superseded)*
- [ADR-007 — Cut the transcript with the segment, clamped at the marks](adr/007-caption-clamp.md) *(carried forward)*
- [ADR-008 — The domain core is pure, and the V1 engine is its oracle](adr/008-pure-core-oracle.md) *(new in V2)*
- [ADR-009 — Tauri v2 and React, not Electron and not a local web server](adr/009-desktop-stack.md) *(new in V2)*
- [ADR-010 — A container timescale is not a frame rate](adr/010-timescale-type.md) *(new in V2)*
- [ADR-011 — Decimal frame rates resolve from a table, not a continued fraction](adr/011-canonical-rates.md) *(new in V2)*
- [ADR-012 — SQLite for projects, and why not a document file](adr/012-sqlite.md) *(new in V2)*
- [ADR-013 — Presets are data, and a preset that reshapes the frame forfeits passthrough](adr/013-presets.md) *(new in V2)*
- [ADR-014 — Overshoot is a warning; a missing frame is a failure](adr/014-overshoot-verdict.md) *(new in V2)*
- [ADR-015 — This build ships without a licensing feature](adr/015-licensing.md) *(new in V2)*
- [ADR-016 — The interface is developed in a browser, not in the window](adr/016-dev-loop.md) *(new in V2)*
- [ADR-017 — The product is four executables, and the window is checked by starting it](adr/017-shipping.md) *(new in V2)*

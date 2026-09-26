//! # trimmer-core
//!
//! The pure heart of TheTrimmer V2. Nothing in this crate opens a file, starts a
//! process, reads the clock or talks to a network: every function here is a total
//! function of its arguments. That is a deliberate architectural choice, not a style
//! preference.
//!
//! ## Why purity is the product
//!
//! The V1 engine earned its quality the expensive way — by discovering, on real
//! material, three failure modes that all exit `0` while producing a wrong file:
//!
//! 1. **Timescale rescale.** libx264 defaults to a `1/15360` timebase; joining that to a
//!    `1/90000` body makes the muxer rescale the *copied* packets, and the clip plays in
//!    slow motion with frozen stretches. Nothing in the exit code says so.
//! 2. **Codec mismatch.** A re-encoded head of a different codec than the body leaves one
//!    track holding two sample descriptions. Players and Premiere refuse it.
//! 3. **Concat drift.** The concat demuxer places the second file at the first file's
//!    *declared* duration, which is not always the head's *content* length. The body then
//!    lands a few frames early, repeating a slice of the head.
//!
//! In V1 those lessons live in prose ([`docs/DESIGN.md`] ADR-001..007) and in the executor
//! that performs the cut. Here they are promoted into the type system and into assertions
//! that [`plan`] enforces before any process is launched. A wrong plan becomes
//! *unrepresentable* rather than *documented*:
//!
//! * [`CutMode`] has no variant that can express "copy a body that does not begin on a
//!   keyframe" ([`plan::plan_cut`] refuses it).
//! * [`PlanInvariant`] is checked on every `CutPlan` a caller can obtain, and
//!   [`CutPlan::verify_invariants`] re-checks the arithmetic the executor depends on.
//! * Frame arithmetic is exact integer arithmetic end to end. Seconds exist only where a
//!   command line demands them, in [`MediaInfo::seconds_of`], and the conversion is
//!   rounded to microseconds at that boundary only.
//!
//! Because the crate is pure, it is also *freely testable against the V1 engine*: the
//! Python implementation is kept as a differential oracle (`tests/oracle.rs`), so every
//! decision this crate makes can be refuted by the implementation that was validated on
//! real broadcast material. Purity is what makes that comparison possible at all.
//!
//! ## Module map
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`timecode`] | `HH:MM:SS:FF` ⇄ frame numbers, drop-frame included |
//! | [`domain`] | The vocabulary: [`MediaInfo`], [`Segment`], [`Project`], [`ProjectId`] |
//! | [`plan`] | The cut decision, and the invariants that make it safe |
//! | [`caption`] | SRT parsing, retiming onto a segment, and the clamp rules |
//! | [`transcript`] | Word-level transcript index: search, and cut-by-text |
//! | [`delivery`] | Delivery presets: container, codec, loudness, aspect |
//! | [`error`] | One error type per failure the domain can actually produce |
//!
//! [`docs/DESIGN.md`]: ../../docs/DESIGN.md

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic)]

// --- Lint policy, stated here so it is visible next to the code it governs -------------
//
// Clippy runs at pedantic strength. The exemptions below are deliberate and narrow.
//
// Frame counts and seconds genuinely have to meet each other in this crate: ffmpeg takes
// seconds on its command line, while every decision the domain makes is in exact integer
// frames. The conversions are confined to `FrameRate::seconds_of`, `FrameRate::frames_in` and
// the two report formatters, and each one is reasoned about at its definition. Marking that
// fact once, here, is clearer than an `#[allow]` on every cast site — which would hide the
// same information in a dozen places.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
// The standard preset table is data, not logic. Splitting it to satisfy a line count would
// make the library harder to read and harder to extend.
#![allow(clippy::too_many_lines)]
// The doc comments are prose. They name types in backticks where that helps and in plain
// words where backticks would be noise, and they are checked by hand rather than by lint.
#![allow(clippy::doc_markdown, clippy::missing_panics_doc, clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]
// Small helpers returned by value where a borrow would be less readable at the call site,
// and a nested `fn` declared after the bindings it uses — both readable in context.
#![allow(clippy::ref_option, clippy::items_after_statements, clippy::redundant_closure)]

pub mod caption;
pub mod delivery;
pub mod domain;
pub mod error;
pub mod plan;
pub mod timecode;
pub mod transcript;

// The public surface is flattened here deliberately: a consumer writing
// `trimmer_core::MediaInfo` should not have to know which module it lives in. The module
// paths remain public for a caller that prefers them, and for `cargo doc`.
pub use caption::{Cue, RetimeResult, Transcript};
pub use delivery::{
    AspectFit, AudioTreatment, Container, DeliveryPreset, Geometry, LoudnessTarget,
    VideoTreatment,
};
pub use domain::{
    AudioFormat, CutMode, KeyframeGrid, MediaInfo, MediaPath, Project, ProjectId, Segment,
    SegmentId, SegmentSource, Timescale, VerifyPolicy,
};
pub use error::{CoreError, CoreResult};
pub use plan::{
    apply_calibration, plan_cut, preset_forces_full_encode, resolve_range, CutPlan, PlanInvariant,
};
pub use timecode::{format_seconds, format_timecode, FrameRate, Position, Timecode};
pub use transcript::{Grouping, Hit, Sentence, TranscriptIndex};

//! # trimmer-verify
//!
//! What it means for a finished cut to be right — and the record that proves it was checked.
//!
//! ## Verdicts are separated from measurements, on purpose
//!
//! This crate never runs ffmpeg, and that is an architectural constraint rather than an
//! omission. It defines the vocabulary a measurement is reported in ([`CutFacts`],
//! [`FrameHashes`], [`Similarity`]), the trait a measuring implementation satisfies
//! ([`MediaMeasurer`]), and every rule that turns those facts into a verdict ([`verify_cut`],
//! [`verify_cut_with`]). Running ffprobe and ffmpeg is `trimmer-media`'s job: that crate is the
//! single adapter in the workspace that starts a process, and `trimmer-app`'s `MediaAdapter` is
//! what joins the two together.
//!
//! The split is not tidiness. Of the two halves, only one has to be *provably* right, and
//! only one can be. A verdict is a pure function of numbers: given the same facts it decides
//! the same thing, on any machine, in any year, and it can therefore be tested exhaustively —
//! every boundary, every skip path, every combination of policy — with no media at all. A
//! measurement cannot be: it depends on the version of ffprobe, on the container, on a codec,
//! and on a file that has to exist and be enormous. Keeping the two apart means the part that
//! must not be wrong is the part that costs nothing to test, and the part that costs a
//! terabyte of master to test is one trait wide. [`NoMeasurer`] is that trait implemented
//! with values a test hands in, which is what makes the claim above checkable rather than
//! merely stated.
//!
//! ## Why overshoot is a warning and never a failure
//!
//! A stream copy cannot stop between packets. The out point lands inside a packet, so the
//! copy runs to the end of it and the delivered file typically holds one to three frames
//! more than were asked for. Those extra frames are a real difference from the request and
//! the report says so — [`Check::Overshoot`] carries [`CheckStatus::Warning`], with the
//! count and the reason in the detail. But every frame that was asked for is present, in the
//! right place, so the file is not wrong, and [`VerifyReport::ok`] stays true.
//!
//! The rule is written that way because the alternative is worse. Failing a file because the
//! intended method did exactly what the intended method does would reject every correct
//! output of a stream copy, and a verifier that fails correct files is one people learn to
//! ignore — which loses the failures that matter. A file that is *short* is the opposite
//! case and does fail: a frame that was asked for is missing. **[`Check::Frames`] fails,
//! [`Check::Overshoot`] warns, and neither can do the other's job.**
//!
//! ## The three checks a diff of the output cannot make
//!
//! Three of the checks exist because they were earned on real material, in V1, by finding
//! bugs that exit `0`:
//!
//! * [`Check::TimescalePreserved`] — a re-encoded head at the wrong timescale makes the
//!   muxer rescale the copied body, and the clip plays in slow motion.
//! * [`Check::FrameAlignment`] — two files cut from the same source can start a fraction of
//!   a frame apart, which makes an exact frame-hash comparison report every frame as
//!   different (V1 ADR-003), so the comparison tolerates a one-frame shift and records
//!   which shift matched.
//! * [`Check::Captions`] — a caption file that was retimed wrongly is invisible in the
//!   picture and obvious to a viewer, so every written cue is compared against the source
//!   cue retimed with [`trimmer_core::caption::retime`] itself rather than a second
//!   implementation of the clamp rules.
//!
//! Every statement above is a property of a pure function, so every one of them is a test in
//! this crate's own suite. See the module docs of [`check`] for the reasoning behind the
//! individual rules, and [`audit`] for the record that carries a report into a delivery.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::doc_markdown,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::ref_option,
    clippy::items_after_statements,
    clippy::redundant_closure
)]

pub mod audit;
pub mod check;
pub mod facts;
pub mod measure;

pub use audit::{constant_time_eq, AuditEntry, AuditManifest};
pub use check::{
    policy_label, verify_cut, verify_cut_with, Check, CheckResult, CheckStatus, Evidence,
    VerifyReport, AUDIO_LEVEL_TOLERANCE_FRAMES, HEAD_FIDELITY_MIN, VERIFICATION_OFF,
};
pub use facts::{CutFacts, FrameHashes, Similarity};
pub use measure::{MediaMeasurer, NoMeasurer};

//! # trimmer-export
//!
//! Turning a set of cut segments into a timeline document an editor can import.
//!
//! An editor does not accept a list of frame ranges; it accepts a *document* — a Premiere
//! `xmeml` sequence, an `fcpxml` library, a CMX3600 edit decision list, or a spreadsheet.
//! This crate writes all four, from one [`ExportRequest`], so that the four can never
//! disagree about what the timeline is.
//!
//! ## Why this is a pure string builder
//!
//! Nothing here reads a frame, launches a process, or looks at a filesystem — with one
//! deliberate exception: the `write_*` helpers, which do nothing but hand an already-built
//! string to `std::fs::write`. Two things follow, and both are the reason for the design.
//!
//! **It is testable without a filesystem.** Every format is a total function from a request
//! to a `String`, so a test asserts the document itself — the `<in>` of the third clip, the
//! exact rational `1001/30000s`, the quoting of a name with a comma in it — instead of
//! asserting on a file that had to be written somewhere first and cleaned up afterwards.
//! The test suite in this crate does exactly that.
//!
//! **The caller decides where media lives.** A path inside a document is not a fact about
//! the media; it is a fact about the machine that will open the document. A studio whose
//! masters live on `\\nas-01\masters` while its editors work from a mirrored `M:\` has to
//! be able to export the same project for both, and neither document is wrong. So the
//! resolver is [`ExportRequest::media_path_for`], a closure the caller supplies, and the
//! export layer never asks the filesystem what a path means. It cannot: it has no
//! filesystem access to ask with.
//!
//! ## What it refuses to do
//!
//! * **It does not silently rescale.** A segment whose source runs at a different rate from
//!   the timeline is written on its own source rate and reported as a warning. Guessing
//!   which conversion the editor wanted is how a cut ends up a frame out.
//! * **It does not fail because one source is offline.** A segment whose source has never
//!   been probed is skipped, with a warning, and the rest of the timeline still exports. A
//!   project has to survive a drive being unplugged.
//! * **It does not let a control character into XML.** XML 1.0 has no way to carry one —
//!   `&#1;` is not a legal character reference — so they are stripped, and the caller is
//!   told which text they came out of.
//! * **It does not round FCPXML times through a float.** Times are rational seconds built by
//!   integer arithmetic from the frame count and the rate's numerator and denominator, so a
//!   hundred frames at 25 fps is exactly `4/1s` and one frame at 29.97 is exactly
//!   `1001/30000s`.
//!
//! ## Module map
//!
//! | Module | Responsibility |
//! |---|---|
//! | `request` | `ExportRequest`, `ExportProduct`, `ExportFormat` |
//! | `clips` | Resolving segments into the timeline every writer draws from |
//! | `premiere` | FCP7 `xmeml` version 4, which Premiere Pro and Resolve both import |
//! | `fcpxml` | FCPXML 1.11, with exact rational seconds |
//! | `edl` | CMX3600, inclusive out points and all |
//! | `csv` | One RFC 4180 row per segment |
//! | `xml` | Escaping, forbidden characters, and URL percent-encoding |
//! | `write` | The only filesystem call in the crate |

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

mod clips;
mod csv;
mod edl;
mod fcpxml;
mod premiere;
mod request;
mod write;
mod xml;

pub use csv::export_csv;
pub use edl::export_edl;
pub use fcpxml::export_fcpxml;
pub use premiere::export_premiere_xml;
pub use request::{ExportFormat, ExportProduct, ExportRequest};
pub use write::{write_csv, write_edl, write_fcpxml, write_premiere_xml, ExportWriteError};

use trimmer_core::CoreResult;

/// Build the timeline document `format` names, from one request.
///
/// This is the entry point a caller that has a format in a variable wants; the four
/// `export_*` functions are the same work with the choice already made.
///
/// # Errors
///
/// Returns `CoreError::NotFound` when a segment names a source the project does not hold.
/// Everything else that is wrong with the request — an unprobed source, a rate mismatch, a
/// mark outside its source — is reported in [`ExportProduct::warnings`] and does not stop
/// the document being written.
pub fn export(request: &ExportRequest<'_>, format: ExportFormat) -> CoreResult<ExportProduct> {
    match format {
        ExportFormat::PremiereXml => export_premiere_xml(request),
        ExportFormat::Fcpxml => export_fcpxml(request),
        ExportFormat::Edl => export_edl(request),
        ExportFormat::Csv => export_csv(request),
    }
}

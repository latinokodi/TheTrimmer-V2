//! The project as a readable document.
//!
//! ## Why this exists beside a database
//!
//! A SQLite file is the right place to *keep* a project and the wrong place to *inspect* one.
//! `sqlite3 projects.sqlite "select * from segments"` is a query, not a document: it scatters
//! one project across five tables, renders a `MediaInfo` as an escaped JSON string inside a
//! column, and says nothing at all about which fields are optional. An editor who has been
//! sent a `.trimmerproj` and wants to know what is in it, a support engineer reading a
//! customer's file, and a studio archiving a job for ten years all want the same thing: the
//! project as one readable text.
//!
//! So both exist, and they are not two sources of truth. The database is authoritative for
//! the application; [`to_json`] and [`from_json`] are the interchange and inspection form, and
//! the round trip between them is a test in this crate rather than an assumption. Export a
//! project with `document::to_json`, keep it, and `from_json` gives back a value that compares
//! equal to the one that was exported.
//!
//! ## The shape
//!
//! The document is `serde_json::to_string_pretty` of [`trimmer_core::Project`] — the domain
//! type itself, not a parallel DTO. A DTO would be a second definition of what a project is,
//! and the two would drift; the field names in the file are therefore exactly the field names
//! in the domain (`camelCase`, one object per source keyed by path, one array of segments in
//! running order).

use trimmer_core::{CoreError, CoreResult, Project};

/// The project as indented JSON, for a person to read and a tool to diff.
///
/// # Errors
///
/// Returns [`CoreError::Invariant`] when the project cannot be serialised, which cannot happen
/// for a plain-data type and is therefore a bug rather than a runtime condition.
pub fn to_json(project: &Project) -> CoreResult<String> {
    serde_json::to_string_pretty(project).map_err(|error| CoreError::Invariant(error.to_string()))
}

/// The project as a single line of JSON, for a log or a wire body.
///
/// # Errors
///
/// As [`to_json`].
pub fn to_compact_json(project: &Project) -> CoreResult<String> {
    serde_json::to_string(project).map_err(|error| CoreError::Invariant(error.to_string()))
}

/// Read a project back from a document.
///
/// # Errors
///
/// Returns [`CoreError::Invariant`] when the text is not a project document. The error carries
/// `serde_json`'s message, which names the line and column, because "this is not a project"
/// with no position in it is a dead end for the person holding the file.
pub fn from_json(text: &str) -> CoreResult<Project> {
    serde_json::from_str(text).map_err(|error| CoreError::Invariant(error.to_string()))
}

//! The one place this crate touches a filesystem.
//!
//! Every document in this crate is built as a `String` and returned to the caller. Saving it
//! is a separate, explicit act, and these four helpers are all there is of it: each takes a
//! path and a body that has already been written and hands the bytes to `std::fs::write`.
//!
//! They are deliberately not one generic `write(path, body)`. A caller that has an
//! [`ExportProduct`](crate::ExportProduct) in hand knows which format it is, and a helper
//! named for the format is the difference between a typo that fails to compile and a `.csv`
//! file containing XML.

use std::path::Path;

/// Why an export could not be saved.
///
/// Separate from `CoreError` on purpose: nothing here is a fact about the domain. A failed
/// `write` is a fact about a disk, and folding it into the domain's error type would make
/// every caller of the pure functions handle a case those functions cannot produce.
#[derive(Debug, thiserror::Error)]
pub enum ExportWriteError {
    /// The document could not be written to the path.
    #[error("could not write {path}: {reason}")]
    Write {
        /// The path that was attempted, as the caller gave it.
        path: String,
        /// What the operating system said.
        reason: String,
    },
}

/// Save an already-built Premiere XML document.
///
/// # Errors
///
/// Returns [`ExportWriteError::Write`] when the file cannot be written.
pub fn write_premiere_xml(path: &Path, body: &str) -> Result<(), ExportWriteError> {
    write_body(path, body)
}

/// Save an already-built FCPXML document.
///
/// # Errors
///
/// Returns [`ExportWriteError::Write`] when the file cannot be written.
pub fn write_fcpxml(path: &Path, body: &str) -> Result<(), ExportWriteError> {
    write_body(path, body)
}

/// Save an already-built edit decision list.
///
/// # Errors
///
/// Returns [`ExportWriteError::Write`] when the file cannot be written.
pub fn write_edl(path: &Path, body: &str) -> Result<(), ExportWriteError> {
    write_body(path, body)
}

/// Save an already-built CSV.
///
/// # Errors
///
/// Returns [`ExportWriteError::Write`] when the file cannot be written.
pub fn write_csv(path: &Path, body: &str) -> Result<(), ExportWriteError> {
    write_body(path, body)
}

/// Hand the bytes to the filesystem.
///
/// No directory is created and no existing file is consulted: a helper that quietly made
/// parent directories would turn a mistyped output path into a directory tree nobody asked
/// for, and the caller that wants one can create it.
fn write_body(path: &Path, body: &str) -> Result<(), ExportWriteError> {
    std::fs::write(path, body).map_err(|error| ExportWriteError::Write {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_that_cannot_be_written_reports_the_path_and_the_reason() {
        // An interior NUL cannot be handed to the operating system at all, so this fails on
        // every platform without depending on file permissions or on a directory not existing.
        let impossible = Path::new("trimmer-export\0.xml");
        let error = write_premiere_xml(impossible, "<xmeml/>").expect_err("must refuse");
        assert!(matches!(error, ExportWriteError::Write { .. }));
        assert!(!error.to_string().is_empty());
    }
}

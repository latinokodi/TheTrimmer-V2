//! A spreadsheet row per segment, quoted to RFC 4180.
//!
//! The CSV is the one export that is not a timeline. It carries no positions on a sequence
//! and no format declaration; it is the answer to "which cuts did we make, from where, and
//! how long are they", which is what a producer asks for and what a script reads back in.
//!
//! Quoting is not optional here. A segment is named by a person, and people put commas and
//! quotation marks in names. A row whose name field contained a bare comma would silently
//! become two columns, and every column after it would describe the wrong thing.

use std::borrow::Cow;

use trimmer_core::timecode::format_timecode;
use trimmer_core::CoreResult;

use crate::clips::{prepare, ExportClip, Rational};
use crate::request::{ExportFormat, ExportProduct, ExportRequest};

/// The header row, in the order the columns are written.
const HEADER: &str = "index,name,source,source_in,source_out,in_timecode,out_timecode,frames,\
                      duration_seconds,preset,tags,note";

/// What separates one tag from the next inside the tags column.
///
/// A semicolon rather than a comma, so that the tags column reads the same in a spreadsheet
/// as it does in the file — a comma would be legal, quoted, and confusing.
const TAG_SEPARATOR: &str = ";";

/// Build a comma-separated row per segment.
///
/// # Errors
///
/// Returns `CoreError::NotFound` when a segment names a source the project does not hold.
pub fn export_csv(request: &ExportRequest<'_>) -> CoreResult<ExportProduct> {
    let prepared = prepare(request)?;
    Ok(ExportProduct {
        format: ExportFormat::Csv,
        suggested_extension: ExportFormat::Csv.extension(),
        body: document(&prepared.clips),
        warnings: prepared.warnings,
        clip_count: prepared.clips.len(),
        total_frames: prepared.total_frames,
    })
}

/// The whole table, as text.
fn document(clips: &[ExportClip]) -> String {
    let mut out = String::with_capacity(128 + clips.len() * 160);
    out.push_str(HEADER);
    out.push_str("\r\n");

    for (index, clip) in clips.iter().enumerate() {
        let frames = clip.frames();
        let row = [
            (index + 1).to_string(),
            clip.name.clone(),
            clip.import_path.clone(),
            clip.source_in.to_string(),
            clip.source_out.to_string(),
            format_timecode(clip.source_in, clip.rate, None),
            format_timecode(clip.source_out, clip.rate, None),
            frames.to_string(),
            format!("{:.3}", Rational::from_frames(frames, clip.rate).as_seconds()),
            clip.preset.clone(),
            clip.tags.join(TAG_SEPARATOR),
            clip.note.clone(),
        ];
        for (column, value) in row.iter().enumerate() {
            if column > 0 {
                out.push(',');
            }
            out.push_str(&field(value));
        }
        out.push_str("\r\n");
    }
    out
}

/// One RFC 4180 field.
///
/// A field is quoted when it contains a comma, a double quote, a carriage return or a line
/// feed, and an embedded double quote is doubled. Nothing else is touched: quoting a field
/// that does not need it is legal, but it makes every row harder to read and hides the rows
/// that genuinely needed it.
fn field(raw: &str) -> Cow<'_, str> {
    if !raw.contains([',', '"', '\r', '\n']) {
        return Cow::Borrowed(raw);
    }
    let mut quoted = String::with_capacity(raw.len() + 2);
    quoted.push('"');
    for character in raw.chars() {
        if character == '"' {
            quoted.push('"');
        }
        quoted.push(character);
    }
    quoted.push('"');
    Cow::Owned(quoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_field_is_left_alone() {
        assert_eq!(field("cold open"), "cold open");
        assert_eq!(field(""), "");
    }

    #[test]
    fn a_comma_forces_quoting() {
        assert_eq!(field("begging, part two"), "\"begging, part two\"");
    }

    #[test]
    fn an_embedded_quote_is_doubled_inside_a_quoted_field() {
        assert_eq!(field("the \"real\" take"), "\"the \"\"real\"\" take\"");
    }

    #[test]
    fn a_bare_quote_forces_quoting_even_without_a_comma() {
        assert_eq!(field("5\" nail"), "\"5\"\" nail\"");
    }

    #[test]
    fn a_line_feed_forces_quoting_rather_than_breaking_the_row() {
        assert_eq!(field("first\nsecond"), "\"first\nsecond\"");
        assert_eq!(field("first\r\nsecond"), "\"first\r\nsecond\"");
    }

    #[test]
    fn the_header_has_the_columns_the_documents_writers_fill_in() {
        let columns: Vec<&str> = HEADER.split(',').collect();
        assert_eq!(columns.len(), 12);
        assert_eq!(columns[0], "index");
        assert_eq!(columns[11], "note");
    }
}

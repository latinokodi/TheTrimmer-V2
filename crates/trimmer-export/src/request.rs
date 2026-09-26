//! The vocabulary of an export: what was asked for, and what came back.
//!
//! [`ExportRequest`] is deliberately a borrowed value. An export is a description of an
//! existing project, not a new owner of it, and the caller keeps the project afterwards.

use core::fmt;

use serde::{Deserialize, Serialize};
use trimmer_core::FrameRate;
use trimmer_core::{MediaPath, Project, Segment};

/// Which document an export produces.
///
/// The four are not interchangeable, and the differences are not cosmetic: `xmeml` carries a
/// sequence in frames, `fcpxml` carries rational seconds, an EDL carries timecode labels on
/// an inclusive out point, and a CSV carries neither a timeline nor a schema. A caller picks
/// by what the next tool in the chain can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    /// Final Cut Pro 7 `xmeml` version 4. Imported by Premiere Pro, Resolve and Avid.
    PremiereXml,
    /// Final Cut Pro X `fcpxml` version 1.11.
    Fcpxml,
    /// CMX3600 edit decision list.
    Edl,
    /// A spreadsheet row per segment, quoted to RFC 4180.
    Csv,
}

impl ExportFormat {
    /// Every format, in the order a picker should offer them.
    pub const ALL: [Self; 4] = [Self::PremiereXml, Self::Fcpxml, Self::Edl, Self::Csv];

    /// The file extension, without the dot.
    ///
    /// Carried into [`ExportProduct::suggested_extension`] so that a caller saving the body
    /// does not have to hold a second table of its own.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::PremiereXml => "xml",
            Self::Fcpxml => "fcpxml",
            Self::Edl => "edl",
            Self::Csv => "csv",
        }
    }

    /// A short name for a menu.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PremiereXml => "Premiere Pro (xmeml)",
            Self::Fcpxml => "Final Cut Pro (fcpxml)",
            Self::Edl => "CMX3600 edit decision list",
            Self::Csv => "Spreadsheet (CSV)",
        }
    }
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Everything an export needs, borrowed from the caller.
pub struct ExportRequest<'a> {
    /// The project the sources and the preset table come from.
    pub project: &'a Project,
    /// The cuts to place on the timeline, in order.
    ///
    /// Taken explicitly rather than read from `project.segments` so that a caller can export
    /// a selection, a folder, or a reordered arrangement without first editing the project.
    pub segments: Vec<&'a Segment>,
    /// Timeline start timecode, in frames. Usually 0.
    pub timeline_start_frame: i64,
    /// Name of the sequence or timeline in the exported document.
    pub sequence_name: String,
    /// Frame rate of the timeline.
    ///
    /// A segment on another rate is a warning, never an error, and its frames are written on
    /// its own source rate rather than rescaled onto this one.
    pub timeline_rate: FrameRate,
    /// Where a media file should resolve to on import, given its source path.
    ///
    /// The caller supplies this, and the export layer never touches the filesystem to decide
    /// it: the same project exported on two machines should be able to name the same media
    /// two different ways, and only the caller knows which is right.
    pub media_path_for: &'a dyn Fn(&MediaPath) -> String,
}

impl fmt::Debug for ExportRequest<'_> {
    /// Reports the shape of the request. The resolver closure is not printable, and the
    /// project is summarised rather than dumped — this is a log line, not a serialisation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExportRequest")
            .field("project", &self.project.name)
            .field("segments", &self.segments.len())
            .field("timeline_start_frame", &self.timeline_start_frame)
            .field("sequence_name", &self.sequence_name)
            .field("timeline_rate", &self.timeline_rate)
            .field("media_path_for", &"<closure>")
            .finish()
    }
}

/// A finished export.
///
/// The body is the document; the rest is what a caller needs in order to save it, report it,
/// and tell the user what was left out.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExportProduct {
    /// Which document this is.
    pub format: ExportFormat,
    /// The extension to save it under, without the dot.
    pub suggested_extension: &'static str,
    /// The document itself.
    pub body: String,
    /// Things that are true and worth saying: skipped segments, rate mismatches, text that
    /// had to be cleaned. An empty list means the export was exactly what was asked for.
    pub warnings: Vec<String>,
    /// How many segments made it into the document.
    pub clip_count: usize,
    /// The total length of the timeline those clips occupy, in timeline frames.
    pub total_frames: i64,
}

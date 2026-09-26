//! Errors the domain can actually produce.
//!
//! Every variant here is a condition a caller is expected to *handle*, not a bug. The
//! variants carry the numbers that produced them so a UI can show the user a sentence
//! and a log can record the truth without re-deriving either.

use serde::{Deserialize, Serialize};

/// The crate's result alias.
pub type CoreResult<T> = Result<T, CoreError>;

/// Everything that can go wrong inside the pure domain.
///
/// Serialised without an internal tag: several variants carry a bare string, which an
/// internally-tagged representation cannot express. The variants are already unambiguous by
/// name, and `serde_json`'s external tagging keeps them readable in a log or a project file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreError {
    /// A timecode string could not be read at the given frame rate.
    #[error("{text:?} cannot be read as HH:MM:SS:FF at {rate} fps: {reason}")]
    Timecode {
        /// The text as the user typed or pasted it.
        text: String,
        /// The rate it was read against, as `num/den`.
        rate: String,
        /// Why it was refused.
        reason: String,
    },

    /// A frame rate is not something timecode can be counted on.
    #[error("{0:?} is not a usable frame rate")]
    FrameRate(String),

    /// The out point is not after the in point.
    #[error(
        "the out point (frame {out_frame}) is not after the in point (frame {in_frame})"
    )]
    EmptyRange {
        /// First frame of the requested range.
        in_frame: i64,
        /// One past the last frame of the requested range.
        out_frame: i64,
    },

    /// The in point is before the first frame of the file.
    #[error("the in point (frame {in_frame}) is before the start of the file")]
    BeforeStart {
        /// The offending in point.
        in_frame: i64,
    },

    /// The out point is past the last frame of the file.
    #[error(
        "the out point (frame {out_frame}) is past the end of the file \
         ({frame_count} frames, last frame {last_frame})"
    )]
    PastEnd {
        /// The offending out point.
        out_frame: i64,
        /// How many frames the source has.
        frame_count: i64,
        /// The last frame a caller may name.
        last_frame: i64,
    },

    /// The source codec cannot carry the head-patch method.
    ///
    /// The method needs the re-encoded head and the copied body to share one codec; a
    /// source in any other codec is refused rather than silently mangled.
    #[error(
        "the source is {codec}; the head-patch method needs the head and the body to share \
         one codec, and only H.264 and HEVC sources are supported. Re-encode the source \
         first, or cut it with a plain re-encode."
    )]
    UnsupportedCodec {
        /// The codec name as ffprobe reported it.
        codec: String,
    },

    /// A keyframe listing was needed but none was usable.
    #[error("no usable keyframe was found between frames {from} and {to}")]
    NoKeyframe {
        /// Start of the window that was searched.
        from: i64,
        /// End of the window that was searched.
        to: i64,
    },

    /// A plan's arithmetic does not hold. This is a bug in the caller or in `plan`, and
    /// it is checked rather than assumed because the executor's correctness depends on it.
    #[error("cut plan invariant violated: {0}")]
    Invariant(String),

    /// A caption file could not be read as subtitles.
    #[error("caption file {path} could not be read: {reason}")]
    Caption {
        /// The file that failed.
        path: String,
        /// Why it failed.
        reason: String,
    },

    /// A transcript index could not be built or queried.
    #[error("transcript: {0}")]
    Transcript(String),

    /// A delivery preset cannot be satisfied.
    #[error("delivery preset {preset}: {reason}")]
    Delivery {
        /// The preset that could not be satisfied.
        preset: String,
        /// Why it could not.
        reason: String,
    },

    /// A project, segment or source was referenced by an id that is not present.
    #[error("{entity} {id} is not in this project")]
    NotFound {
        /// What kind of thing was missing.
        entity: String,
        /// The id that was looked up.
        id: String,
    },
}

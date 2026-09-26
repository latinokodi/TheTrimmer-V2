//! # trimmer-app
//!
//! The application layer: it knows *what the product does* and nothing about how any of it is
//! stored, drawn, or run.
//!
//! `trimmer-core` decides what a cut is. `trimmer-media` performs one. This crate is what turns
//! those into the things a person actually uses:
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`workspace`] | Loading a project, probing what is missing, and the derived view the UI reads |
//! | [`transcript`] | Searching a transcript and turning a result into a segment |
//! | [`queue`] | The batch: sequential cuts with progress, cancellation and per-item outcomes |
//! | [`watch`] | Watch-folder rules: what to do when a file and a marker list appear |
//! | [`ports`] | The traits this layer depends on, so it can be tested with no media and no disk |
//!
//! ## Why the seams are traits
//!
//! Every capability this layer needs from the outside world is a trait in [`ports`]:
//! [`ports::MediaEngine`] performs cuts, [`ports::TranscriptSource`] reads captions,
//! [`ports::ProjectStore`] persists a project, and [`ports::Clock`] tells the time.
//!
//! That is not ceremony. It is what lets the batch queue be tested *exhaustively* — including
//! the paths that matter most and are hardest to reach in production: a cut that fails halfway,
//! a cancellation between two items, a source that has gone missing since the project was
//! written, a preset that turns out to need a re-encode. With a real ffmpeg behind the queue,
//! testing those means generating media and waiting; with a fake engine they are three lines
//! each, and they run in milliseconds. The queue is the part of this product most likely to
//! lose a studio's afternoon, so it is the part most worth testing properly.
//!
//! ## The queue is sequential, deliberately
//!
//! A batch runs one segment at a time. The temptation is to run four at once because the machine
//! has eight cores. The problem is that a cut is not CPU-bound, it is **disk-bound**: a head
//! patch reads a multi-gigabyte master and writes a segment. Four concurrent cuts on one spindle
//! do not go four times as fast; they make every one of them slower and make the progress display
//! meaningless. Sequential also means `Cancel` has an unambiguous meaning — finish this one, stop.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::doc_markdown,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::ref_option,
    clippy::items_after_statements,
    clippy::redundant_closure
)]
// The futures here are large because the adapter nests four levels deep into the executor, not
// because any one of them holds a large buffer; see the same note in `trimmer-media`.
#![allow(clippy::large_futures)]

pub mod ports;
pub mod queue;
pub mod transcript;
pub mod watch;
pub mod workspace;

pub use ports::{
    cut_request_with, Clock, FileTranscripts, MediaAdapter, MediaEngine, ProjectStore,
    SegmentCutRequest, SystemClock, TestClock, TranscriptSource,
};
pub use queue::{
    mode_word, quiet_sink, BatchOutcome, CollectingQueueSink, JobId, JobState, JobStatus, Queue,
    QueueEvent, QueueEventKind, QueueOptions, QueueSink, QuietQueueSink,
};
pub use transcript::cues_to_text;
pub use transcript::{TranscriptService, TranscriptSummary, TranscriptView};
pub use watch::{
    compound_extension, parse_marks, WatchAction, WatchFolder, WatchMark, WatchPlan, WatchPolicy,
    WatchTrigger,
};
pub use workspace::sanitise_name;
pub use workspace::{QueuePreview, SegmentView, SourceView, Workspace, WorkspaceSummary};

use trimmer_core::CoreError;

/// Everything the application layer can refuse or fail at.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The domain refused something.
    #[error(transparent)]
    Core(#[from] CoreError),

    /// The media layer failed.
    #[error(transparent)]
    Media(#[from] trimmer_media::MediaError),

    /// A source named by the project is not readable.
    #[error("source {path} is not available: {reason}")]
    SourceUnavailable {
        /// The file.
        path: String,
        /// Why it is not readable.
        reason: String,
    },

    /// A caption file could not be read or has no usable cues.
    #[error("transcript for {path}: {reason}")]
    Transcript {
        /// The video the transcript belonged to.
        path: String,
        /// What was wrong with it.
        reason: String,
    },

    /// A project could not be loaded or saved.
    #[error("project: {0}")]
    Store(String),

    /// The requested segment is not in the project.
    #[error("segment {0} is not in this project")]
    UnknownSegment(String),

    /// A batch was asked to start while one was already running.
    #[error("a batch is already running ({running} of {total} done)")]
    BatchInProgress {
        /// Items already finished.
        running: usize,
        /// Items in the batch.
        total: usize,
    },
}

/// The crate's result alias.
pub type AppResult<T> = Result<T, AppError>;

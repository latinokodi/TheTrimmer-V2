//! # trimmer-media
//!
//! The only crate in the workspace that starts a process. Everything else decides; this crate
//! *does*.
//!
//! That boundary is deliberate and it is enforced by the dependency graph rather than by
//! convention: `trimmer-core` is a pure domain with no I/O, `trimmer-verify` reasons over
//! measurements it is handed, and this crate is the single adapter that turns a decision into
//! an ffmpeg command line. There is exactly one place to look when a command is wrong, and
//! exactly one place to audit when the question is "could this program do something it was not
//! asked to".
//!
//! ## What "no shell" means here
//!
//! Every process is spawned with `tokio::process::Command` and an **argument array**. No
//! command line is ever assembled as a string and handed to a shell, so a source file called
//! `interview; rm -rf ~.mp4` is a file name and nothing else. Paths are passed as
//! [`std::ffi::OsStr`], so a path that is not valid UTF-8 still works.
//!
//! On Windows the child is given `CREATE_NO_WINDOW`. Without it, Windows hands every console
//! child of a windowed process its own console window, and a trim — a dozen ffmpeg and ffprobe
//! calls — looks like the application opening and closing command windows instead of working.
//!
//! ## The three failure modes this crate exists to avoid
//!
//! [`CutExecutor::cut`] implements the head-patch method, and it carries forward the three
//! defects that V1 found the expensive way. Each has a comment at the site that prevents it,
//! and each is checked by [`trimmer-core`]'s plan invariants before a process is started:
//!
//! 1. **Timescale** — the re-encoded head is written at the source's own
//!    `-video_track_timescale`. Left to itself libx264 chooses `1/15360`, the muxer rescales
//!    the *copied* body to match, and the deliverable plays in slow motion with frozen
//!    stretches **while ffmpeg exits 0**.
//! 2. **Codec** — the head is encoded with the encoder that matches the body's codec family. A
//!    track holding two kinds of sample description is refused by players and by Premiere.
//! 3. **Where the body starts** — the picture and the sound are copied in **two separate
//!    passes**. One pass makes ffmpeg seek the *audio* to its own sync point, up to four AAC
//!    frames before the keyframe, and `-avoid_negative_ts make_zero` then rebases the file on
//!    that earlier audio packet: the body's video begins 80 ms into its own file and the whole
//!    segment lands about three frames early.
//!
//! ## Cancellation and progress
//!
//! A stream copy of a long body prints nothing for minutes, and silence is indistinguishable
//! from a hang. [`ProcessRunner`] therefore emits a progress event on an interval *and*
//! notices cancellation between polls, so Cancel is felt while ffmpeg is still copying rather
//! than after it finishes.

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
// An async command builder genuinely needs the media, the plan, the preset, the config, the
// destination and the run options. Bundling them into a struct would move the argument list,
// not shorten it, and would make the call sites harder to read than the list does.
#![allow(clippy::too_many_arguments)]
// `large_futures` is a stack-size lint. The futures here are large because the executor's own
// state machine nests four levels deep, not because any single future holds a large buffer;
// boxing them all would trade a stack cost for a heap allocation on every call. The depth is
// bounded and the tests run on the default 2 MiB thread stack, so this is measured rather than
// assumed — see the async tests in `executor.rs`, which run the real nesting.
#![allow(clippy::large_futures)]

pub mod executor;
pub mod probe;
pub mod process;
pub mod tool;

pub use executor::{CutConfig, CutExecutor, CutOutcome, ExecutionStep, Prepared};
pub use probe::{FactsMeasurer, Prober};
pub use process::{
    CancelFlag, CollectingSink, NullSink, PollPolicy, ProcessRunner, Progress, ProgressSink,
    RunOptions,
};
pub use tool::{ToolPaths, ToolSet};

use trimmer_core::CoreError;

/// Everything this crate can fail at.
///
/// Distinct from [`CoreError`] on purpose. `CoreError` describes a decision that cannot be
/// made; `MediaError` describes a machine that would not cooperate. A caller shows them
/// differently — one is "your marks are wrong", the other is "this machine has no H.264
/// encoder" — and conflating them makes both messages worse.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// ffmpeg or ffprobe could not be located.
    #[error(
        "{tool} was not found. Install it (winget install Gyan.FFmpeg), or point \
         THE_TRIMMER_{env} at it."
    )]
    ToolNotFound {
        /// `ffmpeg` or `ffprobe`.
        tool: String,
        /// The environment variable that would override the search.
        env: String,
    },

    /// A configured tool path does not exist.
    #[error("THE_TRIMMER_{env} points at {path}, which is not there")]
    ToolOverrideMissing {
        /// The environment variable that was set.
        env: String,
        /// The path it named.
        path: String,
    },

    /// A process could not be started.
    #[error("could not start {tool}: {source}")]
    Spawn {
        /// The tool that would not start.
        tool: String,
        /// Why it would not.
        #[source]
        source: std::io::Error,
    },

    /// A process exited non-zero.
    #[error("{tool} failed ({status}):\n{tail}")]
    ProcessFailed {
        /// The tool that failed.
        tool: String,
        /// How it exited.
        status: String,
        /// The command line, for the log.
        command: String,
        /// The last lines of its error output.
        tail: String,
    },

    /// ffprobe returned something that is not JSON, or JSON of the wrong shape.
    #[error("ffprobe returned unreadable output: {0}")]
    BadProbe(String),

    /// The source has no video stream.
    #[error("{path} has no video stream")]
    NoVideoStream {
        /// The file that was probed.
        path: String,
    },

    /// The run was cancelled by the user.
    #[error("cancelled")]
    Cancelled,

    /// A working file could not be produced or read.
    #[error("working file {path}: {reason}")]
    WorkingFile {
        /// The file.
        path: String,
        /// What went wrong.
        reason: String,
    },

    /// The domain refused the request.
    #[error(transparent)]
    Core(#[from] CoreError),
}

/// The crate's result alias.
pub type MediaResult<T> = Result<T, MediaError>;

impl MediaError {
    /// True when the failure is a cancellation, so a caller can distinguish "the user stopped
    /// this" from "this went wrong".
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

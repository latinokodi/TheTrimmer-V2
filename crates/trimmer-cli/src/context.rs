//! Everything a command needs from the machine, resolved once.

use std::path::PathBuf;
use std::sync::Arc;

use trimmer_media::{CancelFlag, CutConfig, CutExecutor, PollPolicy, Prepared, Progress, ProgressSink, RunOptions};
use trimmer_store::SqliteStore;

use crate::cli::Cli;

/// It worked.
pub const OK: i32 = 0;
/// A check failed: `doctor` found something missing, or `verify` said no.
pub const CHECK_FAILED: i32 = 1;
/// A usage error, or the domain refused the request.
pub const REFUSED: i32 = 2;
/// Interrupted.
pub const INTERRUPTED: i32 = 130;

/// A failure with the exit code it deserves.
#[derive(Debug)]
pub struct Failure {
    /// What the process should exit with.
    pub code: i32,
    /// What to print on stderr.
    pub message: String,
}

impl Failure {
    /// The request was refused, or the command line was wrong.
    #[must_use]
    pub fn refused(message: impl Into<String>) -> Self {
        Self {
            code: REFUSED,
            message: message.into(),
        }
    }

    /// A check did not pass.
    #[must_use]
    pub fn checked(message: impl Into<String>) -> Self {
        Self {
            code: CHECK_FAILED,
            message: message.into(),
        }
    }

    /// Something on this side failed.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: CHECK_FAILED,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self::internal(error.to_string())
    }
}

/// The result every command returns: an exit code, or a failure to print.
pub type Outcome = Result<i32, Failure>;

/// The resolved environment.
pub struct Context {
    /// Where ffmpeg and ffprobe are.
    pub tools: trimmer_media::ToolPaths,
    /// Where the project database is.
    pub store_path: PathBuf,
    /// Whether the caller asked for more detail.
    pub verbose: bool,
    /// A flag a Ctrl-C sets, shared with any run in flight.
    pub cancel: CancelFlag,
}

impl Context {
    /// Resolve everything the command line implies.
    ///
    /// # Errors
    ///
    /// Returns a refusal when ffmpeg or ffprobe cannot be found, naming the environment
    /// variables that would point at them.
    pub fn new(cli: &Cli) -> Result<Self, Failure> {
        let tools = trimmer_media::ToolPaths::resolve()
            .map_err(|error| Failure::refused(error.to_string()))?;
        let store_path = match &cli.store {
            Some(path) => path.clone(),
            None => trimmer_store::default_store_path()
                .map_err(|error| Failure::internal(error.to_string()))?,
        };
        Ok(Self {
            tools,
            store_path,
            verbose: cli.verbose,
            cancel: CancelFlag::new(),
        })
    }

    /// The media engine.
    #[must_use]
    pub fn engine(&self) -> Arc<dyn trimmer_app::MediaEngine> {
        Arc::new(trimmer_app::MediaAdapter::new(
            CutExecutor::new(self.tools.clone()),
            self.config(),
        ))
    }

    /// The encoding configuration.
    #[must_use]
    pub fn config(&self) -> CutConfig {
        CutConfig::default()
    }

    /// The prober.
    #[must_use]
    pub fn prober(&self) -> trimmer_media::Prober {
        trimmer_media::Prober::new(self.tools.clone())
    }

    /// Open the project store.
    ///
    /// # Errors
    ///
    /// Returns an internal failure when the database cannot be opened.
    pub fn store(&self) -> Result<SqliteStore, Failure> {
        SqliteStore::open(&self.store_path).map_err(|error| Failure::internal(error.to_string()))
    }

    /// Where a licence file lives: `THE_TRIMMER_LICENCE`, else beside the store.
    #[must_use]
    pub fn licence_path(&self) -> PathBuf {
        if let Ok(path) = std::env::var("THE_TRIMMER_LICENCE") {
            if !path.trim().is_empty() {
                return PathBuf::from(path.trim());
            }
        }
        self.store_path
            .parent()
            .map_or_else(|| PathBuf::from("licence.key"), |dir| dir.join("licence.key"))
    }

    /// The run options a cut uses, with progress going to stderr.
    #[must_use]
    pub fn run_options(&self, label: impl Into<String>) -> RunOptions {
        RunOptions {
            policy: PollPolicy::long(),
            cancel: self.cancel.clone(),
            sink: Arc::new(StderrSink {
                verbose: self.verbose,
            }),
            label: label.into(),
        }
    }
}

/// Progress on stderr, so stdout stays pipeable.
///
/// The distinction matters for the one thing people actually do with this program: pipe
/// `probe --json` or `batch --json` into something. Progress written to stdout would corrupt
/// every one of those, and the corruption would be silent until a parser failed.
struct StderrSink {
    verbose: bool,
}

impl ProgressSink for StderrSink {
    fn report(&self, progress: Progress) {
        match progress {
            Progress::Step { label } => eprintln!("  · {label}"),
            Progress::Command { text, .. } => {
                if self.verbose {
                    eprintln!("  $ {text}");
                }
            }
            Progress::Elapsed { seconds } => eprintln!("  … {seconds:.0}s"),
            Progress::Finished {
                label,
                seconds,
                ok,
            } => eprintln!(
                "  {} {label} ({seconds:.1}s)",
                if ok { "ok" } else { "FAILED" }
            ),
            Progress::Message { text } => eprintln!("  {text}"),
        }
    }
}

/// A sink that says nothing, for a caller that renders its own report.
#[derive(Debug, Default, Clone, Copy)]
pub struct QuietSink;

impl ProgressSink for QuietSink {
    fn report(&self, _progress: Progress) {}
}

/// Print a prepared command line the way a shell would take it.
///
/// Quoted, because a Windows path with a space in it is otherwise a command that cannot be
/// copied out of the terminal and run — which is the only reason to print it at all.
#[must_use]
pub fn render_command(prepared: &Prepared, program: &std::path::Path) -> String {
    let mut parts = vec![quote(&program.display().to_string())];
    for arg in &prepared.args {
        parts.push(quote(arg));
    }
    parts.join(" ")
}

/// Quote one argument when it needs it.
#[must_use]
pub fn quote(text: &str) -> String {
    if !text.is_empty() && !text.contains([' ', '\t', '"']) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

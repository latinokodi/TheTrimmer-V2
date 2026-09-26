//! # trimmer-cli
//!
//! The command line: `thetrimmer`, and `ttrim` as the same program under a shorter name.
//!
//! ## What this crate is, and is not
//!
//! It is an *adapter*, and nothing else. Every decision it makes was already made by a crate
//! underneath it: [`trimmer_core`] decides what a cut is, `trimmer-media` performs it,
//! [`trimmer_verify`] judges it, `trimmer_export` writes the timeline, `trimmer_app` holds the
//! workspace and the batch, and `trimmer-store` keeps the project. This crate parses a command
//! line, calls those, and prints the answer.
//!
//! That is a rule rather than a description, and the place it is easiest to break is the dry
//! run: `cut --dry-run` must print the commands the executor *would* run, and the tempting
//! shortcut is to build them here. It does not, because a preview built twice drifts, and the
//! drift is discovered by a wrong file rather than by a failing test. The commands come from
//! [`trimmer_app::MediaEngine::preview`], which calls the executor's own argument builders.
//!
//! ## stdout is for output, stderr is for progress
//!
//! This is what makes `thetrimmer probe --json | jq` and `thetrimmer batch --json | jq` work.
//! Every progress line, every warning and every confirmation goes to stderr; only the thing the
//! command was asked for goes to stdout.
//!
//! ## Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | `0` | It worked |
//! | `1` | A check failed: `doctor` found something missing, or `verify` failed a cut |
//! | `2` | A usage error, or the domain refused the request |
//! | `130` | Interrupted |
//!
//! The distinction between `1` and `2` is the one a script cares about: `1` means the machine or
//! the material is not good enough, `2` means the request was wrong.
//!
//! ## Where the store lives
//!
//! `--store <path>` on any command, or `THE_TRIMMER_STORE`, or this machine's data directory
//! for TheTrimmer. The flag wins, because a test and a script both need to say so explicitly.

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
    clippy::redundant_closure,
    clippy::large_futures
)]

pub mod cli;
pub mod commands;
pub mod context;
pub mod project;

pub use cli::Cli;
pub use context::{Failure, Outcome, CHECK_FAILED, INTERRUPTED, OK, REFUSED};

use clap::Parser;

/// Run the program and return the exit code.
///
/// The whole of `main` is this one call, so that the two binaries — `thetrimmer` and `ttrim` —
/// are genuinely the same program rather than two programs that will drift.
///
/// A `Ctrl-C` during a command returns [`INTERRUPTED`] rather than the platform's default,
/// which on Windows is an abrupt termination with no chance to flush and on Unix is a signal
/// that a shell reports as 130 anyway. Handling it here makes the two agree, and gives a run in
/// flight a chance to stop between steps rather than mid-write.
#[must_use]
pub fn run() -> i32 {
    let cli = Cli::parse();
    let context = match context::Context::new(&cli) {
        Ok(context) => context,
        Err(failure) => {
            eprintln!("thetrimmer: {}", failure.message);
            return failure.code;
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("thetrimmer: could not start a runtime: {error}");
            return REFUSED;
        }
    };

    let cancel = context.cancel.clone();
    runtime.block_on(async move {
        tokio::select! {
            outcome = dispatch(context, cli) => outcome,
            _ = tokio::signal::ctrl_c() => {
                // Set the flag as well as returning, so a run that is already inside the queue
                // notices and stops between steps rather than at the end.
                cancel.cancel();
                eprintln!("thetrimmer: interrupted");
                INTERRUPTED
            }
        }
    })
}

/// Route a command to its implementation and turn a failure into an exit code.
async fn dispatch(context: context::Context, cli: Cli) -> i32 {
    use cli::{Command, ProjectCommand};

    let outcome = match &cli.command {
        Command::Doctor => commands::doctor(&context).await,
        Command::Probe(args) => commands::probe(&context, args).await,
        Command::Cut(args) => commands::cut(&context, args).await,
        Command::Batch(args) => commands::batch(&context, args).await,
        Command::Project(args) => match &args.command {
            ProjectCommand::New(new) => project::new_project(&context, new),
            ProjectCommand::List => project::list_projects(&context),
            ProjectCommand::Show(show) => project::show_project(&context, show),
            ProjectCommand::AddSource(add) => project::add_source(&context, add).await,
            ProjectCommand::AddSegment(add) => project::add_segment(&context, add),
            ProjectCommand::RemoveSegment(remove) => project::remove_segment(&context, remove),
            ProjectCommand::Export(export) => project::export_document(&context, export),
        },
        Command::Export(args) => project::export_timeline(&context, args),
        Command::Verify(args) => commands::verify(&context, args),
        Command::Daemon(args) => commands::daemon(&context, args).await,
        Command::Watch(args) => commands::watch(&context, args).await,
    };

    match outcome {
        Ok(code) => code,
        Err(failure) => {
            eprintln!("thetrimmer: {}", failure.message);
            failure.code
        }
    }
}

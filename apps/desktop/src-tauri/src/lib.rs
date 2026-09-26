//! TheTrimmer's desktop shell.
//!
//! # The shape of this crate
//!
//! It is a **bridge**, and nothing more. Every decision is made by a crate below it —
//! `trimmer-core` decides what a cut is, `trimmer-media` performs one, `trimmer-app` owns the
//! workspace and the batch queue, `trimmer-store` persists, `trimmer-verify` judges — and this
//! crate turns those into typed, named IPC commands the interface can call.
//!
//! ## Why the interface holds no capability
//!
//! Tauri's security model is that the webview is untrusted. The page is web technology, it renders
//! text that came from a subtitle file or a file name, and the correct assumption is that anything
//! it can do, something will eventually make it do. So:
//!
//! * the capability file grants the page a file picker and nothing else;
//! * **no** filesystem, process or licence operation is reachable from JavaScript;
//! * every command below takes named arguments, validates them, and returns either a typed value
//!   or a typed error — never a raw path, never a command line, never a file handle.
//!
//! The one place that is easy to get wrong is a path: the interface sends a *string*, and a string
//! is untrusted. Every command that accepts one turns it into a [`trimmer_core::MediaPath`] and
//! lets the domain decide what is legal, rather than joining it to anything itself.
//!
//! ## Blocking work
//!
//! A cut takes minutes and a probe takes a moment, and neither may run on the thread that draws the
//! window. `#[tauri::command]` functions here are `async` and the engine they call is `async`, so
//! the work lands on Tauri's runtime; the one genuinely blocking thing — the state lock — is held
//! for the shortest possible span, and never across an `await` that runs a process. The workspace
//! is behind an `Arc<Mutex<..>>` from [`parking_lot`] rather than a `std` mutex because a poisoned
//! lock taking the window down is a worse outcome than a slightly stale view.

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
    clippy::large_futures,
    // An `async fn` that does not await is a lint worth having in ordinary code and wrong here: a
    // Tauri command marked `async` runs on the runtime rather than on the thread that draws the
    // window, so the keyword is a statement about *where the work happens* rather than about whether
    // it awaits. Some commands only take the workspace lock and answer — and they still must not do
    // it on the UI thread, because the lock is held by whatever command is currently cutting.
    clippy::unused_async
)]

pub mod commands;
pub mod state;

pub use commands::*;
pub use state::AppState;

/// The application version, from the crate that was built.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every command the interface may call, in **one** list.
///
/// ## Why this is a macro and not two lists
///
/// It was two. `main.rs` registered the real handler and `tests/ipc_contract.rs` registered a copy,
/// with a comment arguing that the duplication was deliberate: a test that reused a shared list, it
/// said, "would still pass if the binary forgot to register one of them". That reasoning is backwards.
/// Two hand-maintained lists drift, and the direction they drift in is the dangerous one — a command
/// added to the test's list and not to the binary's passes every test in this crate while being a dead
/// button in the shipped window. The failure the comment was worried about is exactly the failure the
/// duplication makes possible.
///
/// Both now build from this, so "registered in the test" and "registered in the window" are the same
/// statement, and a missing registration is a compile error at the call site rather than a runtime
/// discovery.
///
/// The command names are checked against the interface's own `COMMAND_NAMES` by
/// `tests/ipc_contract.rs::every_command_the_interface_names_is_registered`.
#[macro_export]
macro_rules! trimmer_commands {
    () => {
        tauri::generate_handler![
            // environment
            $crate::commands::doctor,
            // projects
            $crate::commands::list_projects,
            $crate::commands::create_project,
            $crate::commands::open_project,
            $crate::commands::delete_project,
            $crate::commands::current_project,
            $crate::commands::save_project,
            // sources
            $crate::commands::add_source,
            $crate::commands::refresh_sources,
            $crate::commands::remove_source,
            $crate::commands::sources,
            // segments
            $crate::commands::segments,
            $crate::commands::summary,
            $crate::commands::presets,
            $crate::commands::add_segment,
            $crate::commands::update_segment,
            $crate::commands::remove_segment,
            $crate::commands::reorder_segment,
            $crate::commands::parse_timecode,
            // planning and cutting
            $crate::commands::preview,
            $crate::commands::preview_all,
            $crate::commands::cut_segment,
            $crate::commands::run_batch,
            $crate::commands::cancel_batch,
            // transcripts
            $crate::commands::search_transcript,
            $crate::commands::transcript_lines,
            // export
            $crate::commands::export_timeline,
            // automation
            $crate::commands::plan_watch_folder,
            // verification policy
            $crate::commands::get_verify_policy,
            $crate::commands::set_verify_policy,
            // shell integration
            $crate::commands::reveal,
        ]
    };
}

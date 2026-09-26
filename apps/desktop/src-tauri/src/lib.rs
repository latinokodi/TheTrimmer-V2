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

//! The desktop binary.
//!
//! `main` does four things and nothing else: build the application state, register the commands the
//! interface may call, register the one plugin it needs, and run the window. Everything else lives in
//! [`thetrimmer_desktop_lib`], which is a library so the commands can be unit-tested without opening
//! a window.
//!
//! ## Why the state is built before the window
//!
//! [`AppState::bootstrap`] resolves ffmpeg and ffprobe and opens the project store, and either can
//! fail on a machine that has not been set up. Failing *before* a window exists means the user gets a
//! dialog with `winget install Gyan.FFmpeg` in it, rather than an application that opens and then
//! fails on the first cut. Windows has no console attached to a GUI process, so a panic here would
//! otherwise be invisible.

// The Windows subsystem attribute is what stops a console window appearing behind the application.
// Without it a GUI executable gets one, and it is the first thing a user sees.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use thetrimmer_desktop_lib::{state::AppState, trimmer_commands, VERSION};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let state = match AppState::bootstrap() {
        Ok(state) => state,
        Err(reason) => {
            // A sentence, not a stack trace: this is the first thing a user sees on a machine that
            // is not set up, and it has to say what to do about it.
            let message = format!(
                "TheTrimmer {VERSION} cannot start.\n\n{reason}\n\n\
                 ffmpeg and ffprobe must be installed and on PATH, or pointed at with \
                 THE_TRIMMER_FFMPEG and THE_TRIMMER_FFPROBE."
            );
            tracing::error!("{message}");
            show_startup_failure(&message);
            std::process::exit(1);
        }
    };

    tauri::Builder::default()
        // The only plugin. The file picker is the single capability the page holds, and a dialog is
        // the only way to ask the operating system for a path.
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        // One list, shared with the contract test. See `trimmer_commands!` in `lib.rs` for why this is
        // not written out here.
        .invoke_handler(trimmer_commands!())
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| {
            tracing::error!("the window failed to start: {error}");
            std::process::exit(1);
        });
}

/// Put the startup failure in front of the user.
///
/// A GUI process on Windows has no console, so writing to stderr is writing to nowhere. The message
/// box is the only channel left, and it is the one that matters: it is the difference between
/// "nothing happened" and "install ffmpeg".
fn show_startup_failure(message: &str) {
    #[cfg(windows)]
    {
        // `msg` is part of a default Windows install and takes the text as an argument, so there is
        // no shell, no quoting to get wrong, and no dependency to add for one dialog.
        let _ = std::process::Command::new("mshta")
            .arg(format!(
                "javascript:alert('{}');close()",
                message.replace('\'', "\\'").replace('\n', "\\n")
            ))
            .spawn();
    }
    #[cfg(not(windows))]
    {
        eprintln!("{message}");
    }
}

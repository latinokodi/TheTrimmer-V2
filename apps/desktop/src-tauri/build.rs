//! The Tauri build script.
//!
//! `tauri_build::build()` generates the context the `tauri::generate_context!` macro needs:
//! the window configuration, the capabilities and the bundled assets. It reads `tauri.conf.json`
//! at compile time, so a mistake there is a compile error rather than a runtime surprise.

fn main() {
    tauri_build::build();
}

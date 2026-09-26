//! The build script, and the reason the test binaries need one.
//!
//! # The usual job
//!
//! [`tauri_build::build`] reads `tauri.conf.json` and generates the application's context, its
//! capabilities and — on Windows — an application manifest for the binary.
//!
//! # The extra job, and why it is not optional
//!
//! That generated manifest goes on the **binary** target only. The test targets get nothing, and
//! that is invisible until a test links something that needs a manifest. This crate does: the Tauri
//! dependency tree reaches `muda`, which imports `TaskDialogIndirect` from `comctl32.dll`.
//!
//! `TaskDialogIndirect` exists only in Common Controls **version 6**, and the loader chooses which
//! `comctl32` to bind from the application's manifest. With no manifest, Windows binds the v5.82
//! shim in the system directory, which has no such export, and the process is killed before `main`
//! runs with `STATUS_ENTRYPOINT_NOT_FOUND` (`0xC0000139`). Nothing is printed and no test runs:
//! `cargo test` reports a bare failure with no output at all.
//!
//! So `tests/manifest.xml` requests Common Controls v6, exactly as the generated manifest does, and
//! `/MANIFESTINPUT` merges it into the single manifest an executable may carry.
//!
//! The alternative is worse than the fix: two of the commands the interface leans on hardest —
//! `cut_segment` and `run_batch` — are the only ones that take an `AppHandle`, so without this the
//! most important actions in the product would be the only ones no automated test could reach.

fn main() {
    tauri_build::build();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = std::path::Path::new("tests/manifest.xml");
    if !manifest.exists() {
        // An incomplete checkout is not a reason to fail a build; the tests that need the manifest
        // simply behave as they did before it existed.
        println!(
            "cargo:warning=tests/manifest.xml is missing: test binaries will carry no manifest"
        );
        return;
    }
    println!("cargo:rerun-if-changed=tests/manifest.xml");

    // Built from `CARGO_MANIFEST_DIR` rather than `std::fs::canonicalize`, which on Windows returns
    // a `\\?\`-prefixed verbatim path. `mt.exe` cannot open a verbatim path and reports it as a
    // malformed file name, which reads like a quoting bug rather than a path one.
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest = std::path::Path::new(&root)
        .join("tests")
        .join("manifest.xml");
    // Forward slashes and no quoting. Rustc passes a `/MANIFESTINPUT:` value to `mt.exe` itself, and
    // it hands over a `\\?\`-prefixed path when the argument is quoted — which `mt.exe` rejects as a
    // malformed file name. An unquoted path with forward slashes survives that trip intact, and a
    // path containing a space was never going to survive it either way.
    let manifest = manifest.to_string_lossy().replace('\\', "/");
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{manifest}");
}

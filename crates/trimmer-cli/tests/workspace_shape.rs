//! Guards on the shape of the workspace itself.
//!
//! These are not tests of behaviour; they are tests of a *build fact* that broke the product in a way
//! nobody would think to look for, and that no behavioural test could have caught.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Two crates must never produce a binary of the same name.
///
/// Cargo resolves that collision **silently**: both binaries are written to `target/<profile>/`, one
/// overwrites the other, and which one survives depends on build order. That is exactly what happened
/// here — `apps/desktop` declared a binary called `thetrimmer`, the same name as the command line's —
/// so `thetrimmer doctor` opened a Tauri window instead of printing a report, and the window showed a
/// connection error because it was looking for a frontend dev server.
///
/// A product whose command-line tool opens a GUI, and whose GUI cannot find its own assets, is a bug
/// with no obvious cause. The fix was to rename the desktop binary
/// `thetrimmer-desktop`; this test is what stops it coming back.
#[test]
fn no_two_crates_produce_a_binary_of_the_same_name() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives at <workspace>/crates/trimmer-cli");

    let output = Command::new(env!("CARGO"))
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .current_dir(workspace)
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let meta: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");

    // Binary name -> the crate that declared it.
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let packages = meta["packages"].as_array().expect("packages");
    for package in packages {
        let crate_name = package["name"].as_str().unwrap_or("?").to_owned();
        for target in package["targets"].as_array().into_iter().flatten() {
            let kinds: Vec<&str> = target["kind"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .collect();
            if !kinds.iter().any(|kind| matches!(*kind, "bin")) {
                continue;
            }
            if let Some(name) = target["name"].as_str() {
                owners
                    .entry(name.to_owned())
                    .or_default()
                    .push(crate_name.clone());
            }
        }
    }
    assert!(!owners.is_empty(), "no binaries were found at all");

    let clashes: Vec<String> = owners
        .iter()
        .filter(|(_, crates)| crates.len() > 1)
        .map(|(name, crates)| format!("`{name}` is produced by {}", crates.join(" and ")))
        .collect();

    assert!(
        clashes.is_empty(),
        "two crates produce a binary of the same name, which Cargo resolves silently by letting one \
         overwrite the other:\n  {}\nRename one of them.",
        clashes.join("\n  ")
    );

    // And the two names this product depends on are the ones it documents.
    for expected in ["thetrimmer", "ttrim", "trimmer-daemon"] {
        assert!(
            owners.contains_key(expected),
            "`{expected}` is documented but no crate produces it; the workspace produces {:?}",
            owners.keys().collect::<Vec<_>>()
        );
    }
}

/// The desktop binary is not called `thetrimmer`.
///
/// Stated separately from the collision check because this is the specific mistake: the name that
/// belongs to the command line.
#[test]
fn the_desktop_binary_does_not_take_the_command_lines_name() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives at <workspace>/crates/trimmer-cli");
    let desktop_manifest = workspace.join("apps/desktop/src-tauri/Cargo.toml");
    let text =
        std::fs::read_to_string(&desktop_manifest).expect("the desktop manifest is readable");
    assert!(
        !text.contains("name = \"thetrimmer\""),
        "the desktop crate declares a binary called `thetrimmer`, which belongs to the command line"
    );
    assert!(
        text.contains("name = \"thetrimmer-desktop\""),
        "the desktop binary should be `thetrimmer-desktop`"
    );
}

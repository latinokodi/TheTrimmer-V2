//! Identifying the machine, offline, without a network and without `unsafe`.
//!
//! ## What [`machine_id`] hashes, exactly
//!
//! In this order, separated by a unit separator so no two fields can be confused with one:
//!
//! 1. `COMPUTERNAME` (on Windows), or the hostname the platform exposes.
//! 2. `USERNAME`, or `USER` where that is the name the platform uses.
//! 3. [`std::env::consts::OS`] and [`std::env::consts::ARCH`].
//! 4. The system drive root — `%SystemDrive%` (`C:`) on Windows, `/` elsewhere.
//! 5. `%SystemRoot%` on Windows, which distinguishes two installations of the same build on
//!    two different volumes of one machine.
//!
//! The hash is SHA-256, and only the first 16 bytes are kept, rendered as 32 lowercase hex
//! characters. It is computed once per process and then cached, so it cannot change under a
//! caller that has already recorded it.
//!
//! ## What it deliberately does *not* use
//!
//! The brief for this crate asked for **the volume serial number of the system drive**. It is
//! not read, and the reason is stated here rather than discovered later:
//!
//! * Reading it on Windows means calling `GetVolumeInformationW`, which is an `unsafe` FFI
//!   call or a new dependency (`windows-sys`, `winapi`) that exists to expose `unsafe` calls.
//!   This crate is `#![forbid(unsafe_code)]`, which is not negotiable, and the workspace's
//!   only process runner belongs to `trimmer-media` — a `cmd /c vol` would be both a shell and
//!   a second place that starts a process.
//! * The volume serial is a 32-bit value assigned when a volume is formatted. It is not a
//!   secret, it changes if the drive is reformatted, and on a virtual machine it is cloned
//!   along with the disk. Adding it would make the id *more* brittle without making it
//!   meaningful.
//!
//! ## The limit of any machine binding, stated plainly
//!
//! **A cloned virtual machine produces the same id, and so does a machine image restored onto
//! new hardware.** That is not a defect in this implementation; it is the property of every
//! machine binding that does not consult a network. Anything stronger needs either a
//! TPM-backed key or a server that has seen the machine before, and this product is used in
//! studios that are sometimes air-gapped on purpose. So the binding is a speed bump against
//! casual sharing, and it is documented as one rather than sold as a lock.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

/// This machine's identity, stable for the life of the installation.
///
/// See the module documentation for exactly what is hashed, and for the limitation that a
/// cloned virtual machine shares an id.
#[must_use]
pub fn machine_id() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(compute).clone()
}

/// The machine identity before caching, so a test can see the raw derivation.
#[must_use]
pub fn machine_id_uncached() -> String {
    compute()
}

/// Hash the local facts documented at the top of this module.
fn compute() -> String {
    // A unit separator, so `"AB" + "C"` and `"A" + "BC"` cannot collide.
    const SEP: char = '\u{1f}';

    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown-host".to_owned());
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown-user".to_owned());
    let system_root = std::env::var("SystemRoot").unwrap_or_default();
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| default_drive());

    let mut seed = String::with_capacity(64);
    for field in [
        host.as_str(),
        user.as_str(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        system_drive.as_str(),
        system_root.as_str(),
    ] {
        seed.push_str(field);
        seed.push(SEP);
    }

    let digest = Sha256::digest(seed.as_bytes());
    let mut out = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
    out
}

/// The drive a Windows machine boots from when the variable that names it is unset.
fn default_drive() -> String {
    if cfg!(windows) {
        "C:".to_owned()
    } else {
        "/".to_owned()
    }
}

/// One lowercase hexadecimal digit.
fn hex_digit(nibble: u8) -> char {
    char::from(b"0123456789abcdef"[usize::from(nibble & 0x0f)])
}

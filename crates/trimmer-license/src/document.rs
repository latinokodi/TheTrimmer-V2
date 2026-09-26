//! Signing, reading, writing and parsing a licence as a document.
//!
//! ## The canonical payload
//!
//! [`canonical_payload`] is what the signature covers, and it is public because a signing
//! scheme whose input cannot be inspected is a scheme nobody can debug. Two properties are
//! load-bearing:
//!
//! 1. **The features are sorted and deduplicated first.** A licence written by a sales tool
//!    that appends `batch` before `export`, and the same licence written with the list the
//!    other way round, are the same licence and must sign identically. Without the sort, a
//!    customer re-saving a key through a form would invalidate it.
//! 2. **The serialisation goes through [`serde_json::Value`].** A `Value`'s object map is a
//!    `BTreeMap`, so its keys come out sorted by name; a struct serialised straight to a
//!    string comes out in field-declaration order. Going through `Value` is what makes the
//!    payload a function of the *content* rather than of the declaration order of a Rust
//!    struct that someone may one day reorder for readability.
//!
//! ## The signature is obfuscation, not proof
//!
//! Read the crate documentation before relying on this. In short: HMAC means the verifying key
//! is the signing key, the key is in the binary, and therefore anyone who can run the program
//! can mint a licence. It stops a customer editing a text field. It does not stop an attacker.

use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::licence::{Licence, LicenceError};

/// The largest a licence file may be, in bytes.
///
/// A real licence is under a kilobyte. The cap is here so that pointing `install` at a
/// multi-gigabyte file — by typo, or by an attacker who would rather the program read until it
/// runs out of memory — is refused from the file's own metadata **before** a byte of it is
/// read. See [`read_file`].
pub const MAX_LICENCE_BYTES: u64 = 64 * 1024;

/// The first line of the armoured form.
pub const ARMOUR_BEGIN: &str = "-----BEGIN THE TRIMMER LICENCE-----";

/// The last line of the armoured form.
pub const ARMOUR_END: &str = "-----END THE TRIMMER LICENCE-----";

/// A licence and the signature over it.
///
/// The two travel together and are stored together because a licence without its signature is
/// not a licence — it is a claim. Keeping them in one type means there is no code path that
/// can hold one and forget the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedLicence {
    /// What was sold.
    pub licence: Licence,
    /// The base64 HMAC-SHA256 over [`canonical_payload`], as lowercase hex is *not* used here
    /// so that the signature is half the length in the email it arrives in.
    pub signature: String,
}

/// The key this build ships with.
///
/// Public because the command line has to verify an installed licence without the user
/// supplying a key, and because hiding it would be theatre: it is in the binary either way.
/// What that means is stated in the crate documentation — with HMAC, this key signs as well as
/// verifies.
#[must_use]
pub fn embedded_key() -> &'static [u8] {
    // Not a secret. A placeholder a distributor replaces when they build for sale; the value
    // is fixed so that two builds of this source verify each other's licences.
    b"TheTrimmer-V2-offline-licence-key/2024-11::replace-me-at-build-time"
}

/// The exact bytes a signature covers.
///
/// # Errors
///
/// Returns [`LicenceError::Crypto`] when the licence cannot be serialised, which cannot happen
/// for a plain-data type and is therefore a bug rather than a runtime condition.
pub fn canonical_payload(licence: &Licence) -> Result<String, LicenceError> {
    let mut canonical = licence.clone();
    canonical.features.sort();
    canonical.features.dedup();
    // Through `Value`, so object keys are emitted in the map's own (sorted) order rather than
    // in the order the struct declares its fields.
    let value = serde_json::to_value(&canonical).map_err(crypto)?;
    serde_json::to_string(&value).map_err(crypto)
}

/// Sign a licence, producing the pair that gets stored or pasted.
///
/// # Errors
///
/// Returns [`LicenceError::Crypto`] when the payload cannot be built.
pub fn sign(licence: &Licence, key: &[u8]) -> Result<SignedLicence, LicenceError> {
    let payload = canonical_payload(licence)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(crypto)?;
    mac.update(payload.as_bytes());
    let signature = mac.finalize().into_bytes();
    Ok(SignedLicence {
        licence: licence.clone(),
        signature: BASE64.encode(signature),
    })
}

/// Sign with the key this build ships with.
///
/// # Errors
///
/// As [`sign`].
pub fn sign_with_embedded(licence: &Licence) -> Result<SignedLicence, LicenceError> {
    sign(licence, embedded_key())
}

/// Check a signature against a key.
///
/// # Errors
///
/// Returns [`LicenceError::BadSignature`] when the signature is not valid base64, is the wrong
/// length, or does not match the contents. All three are the same refusal to a caller: this
/// document is not the one that was signed.
pub fn verify(signed: &SignedLicence, key: &[u8]) -> Result<(), LicenceError> {
    let supplied = BASE64
        .decode(signed.signature.trim())
        .map_err(|_| LicenceError::BadSignature)?;
    let payload = canonical_payload(&signed.licence)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(crypto)?;
    mac.update(payload.as_bytes());
    // `verify_slice` is the constant-time comparison: it does not stop at the first differing
    // byte, so the time a refusal takes says nothing about how much of a guess was right.
    mac.verify_slice(&supplied)
        .map_err(|_| LicenceError::BadSignature)
}

/// Verify against the key this build ships with.
///
/// # Errors
///
/// As [`verify`].
pub fn verify_with_embedded(signed: &SignedLicence) -> Result<(), LicenceError> {
    verify(signed, embedded_key())
}

/// Read a licence from a file, refusing anything that is not a plausible-size regular file.
///
/// The order matters and is the point of the function: the length is checked from the file's
/// **metadata**, before the file is opened for reading. A check made after `read_to_string`
/// would already have allocated whatever the path pointed at.
///
/// # Errors
///
/// Returns [`LicenceError::Malformed`] when the path does not exist, is a directory, is larger
/// than [`MAX_LICENCE_BYTES`], is not UTF-8, or does not hold a licence.
pub fn read_file(path: &Path) -> Result<SignedLicence, LicenceError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| LicenceError::Malformed(format!("{}: {error}", path.display())))?;
    if !metadata.is_file() {
        return Err(LicenceError::Malformed(format!(
            "{} is not a file",
            path.display()
        )));
    }
    if metadata.len() > MAX_LICENCE_BYTES {
        return Err(LicenceError::Malformed(format!(
            "{} is {} bytes; a licence file may be at most {MAX_LICENCE_BYTES}",
            path.display(),
            metadata.len()
        )));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| LicenceError::Malformed(format!("{}: {error}", path.display())))?;
    parse(&text)
}

/// Write a licence to a file, creating the directory it goes in.
///
/// # Errors
///
/// Returns [`LicenceError::Malformed`] when the directory or the file cannot be written.
pub fn write_file(path: &Path, signed: &SignedLicence) -> Result<(), LicenceError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| LicenceError::Malformed(format!("{}: {error}", parent.display())))?;
        }
    }
    std::fs::write(path, render(signed))
        .map_err(|error| LicenceError::Malformed(format!("{}: {error}", path.display())))
}

/// The pasteable form: an armoured block holding base64 of the JSON envelope.
///
/// Documented shape, and asserted by a test:
///
/// ```text
/// -----BEGIN THE TRIMMER LICENCE-----
/// <base64 of the JSON envelope, wrapped at 64 characters>
/// -----END THE TRIMMER LICENCE-----
/// ```
///
/// Base64 rather than the raw JSON because an email client is entitled to re-wrap, and a
/// JSON document that has been re-wrapped is still valid — but one that has been re-wrapped
/// *and* had a line ending changed inside a string is not, and the failure looks like a
/// corrupt key rather than a mangled message.
#[must_use]
pub fn render(signed: &SignedLicence) -> String {
    let json = serde_json::to_vec(signed).unwrap_or_default();
    let encoded = BASE64.encode(json);
    let mut out = String::with_capacity(encoded.len() + 96);
    out.push_str(ARMOUR_BEGIN);
    out.push('\n');
    for chunk in encoded.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out.push_str(ARMOUR_END);
    out.push('\n');
    out
}

/// Read a licence from text, in any of the three forms it may arrive in.
///
/// * The armoured block [`render`] produces.
/// * A bare base64 blob — the same bytes with the markers stripped by a mail client.
/// * Raw JSON, for a hand-written file and for a test.
///
/// # Errors
///
/// Returns [`LicenceError::Malformed`] when none of the three reads.
pub fn parse(text: &str) -> Result<SignedLicence, LicenceError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(LicenceError::Malformed("the text is empty".to_owned()));
    }

    // The armoured form, and the same form with the markers lost. Everything that is
    // whitespace is dropped, because a mail client is entitled to re-wrap a long line and a
    // base64 decoder is not entitled to forgive it.
    let body: String = trimmed
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with("-----")
        })
        .collect::<Vec<_>>()
        .join("")
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();

    // JSON first: a JSON document is also valid base64 alphabet for its punctuation-free
    // stretches, so trying base64 first would turn a clear parse error into a decode error.
    if let Ok(signed) = serde_json::from_str::<SignedLicence>(trimmed) {
        return Ok(signed);
    }
    if let Ok(bytes) = BASE64.decode(body.as_bytes()) {
        if let Ok(signed) = serde_json::from_slice::<SignedLicence>(&bytes) {
            return Ok(signed);
        }
    }
    if let Ok(signed) = serde_json::from_str::<SignedLicence>(&body) {
        return Ok(signed);
    }
    Err(LicenceError::Malformed(
        "it is neither an armoured licence block, nor base64, nor a licence document".to_owned(),
    ))
}

/// Turn a library failure into this crate's error.
fn crypto(error: impl std::fmt::Display) -> LicenceError {
    LicenceError::Crypto(error.to_string())
}

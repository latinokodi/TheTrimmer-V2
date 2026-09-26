//! # trimmer-license
//!
//! Selling a desktop tool to studios that often have no internet connection on the machine
//! doing the work. A licence therefore has to be *verifiable offline*, which means the whole
//! decision — is this valid, on this machine, at this instant, for this feature — has to be a
//! pure function of a small signed document and the local clock.
//!
//! ## Read this first: what the signature does and does not prove
//!
//! **[`sign`] and [`verify`] use HMAC-SHA256 with a key embedded in the binary.** That has a
//! consequence which must not be glossed over:
//!
//! > **With HMAC, the key that verifies is the same key that signs.** Anyone who can run the
//! > program can extract that key from it, and anyone with the key can mint a licence for any
//! > name, any edition and any expiry. So the signature is not a proof of authorship. It is
//! > **obfuscation**: it stops a person editing a licence file in Notepad and it stops a
//! > customer sharing a licence with a colleague across a machine binding. It does not stop a
//! > determined attacker, and this crate does not claim that it does.
//!
//! Asymmetric signing (Ed25519, or RSA) is the right answer, because a public key can be
//! embedded safely: it verifies without being able to sign. It is not used here because adding
//! a public-key crate is a build-and-supply-chain decision that this crate cannot make on its
//! own — see the crate report — and because a half-verified claim ("signed" without saying
//! with what) would be worse than an honest HMAC. If `ed25519-dalek` is later adopted, the
//! shape of this crate does not change: [`sign`] and [`verify`] take the key as a parameter,
//! [`Licence::is_valid_at`] does not care how the signature was made, and the embedded key
//! becomes a public key instead of a shared secret.
//!
//! Two things follow, and both are enforced rather than merely intended:
//!
//! * The **canonical payload** that is signed is deterministic: the features are sorted and
//!   the licence is serialised through [`serde_json::Value`], whose object map is
//!   `BTreeMap`-backed and therefore key-sorted. Two licences that differ only in the order
//!   their fields were written produce the same signature — [`canonical_payload`] is public so
//!   a caller can see exactly what was signed.
//! * The **machine binding** is a hash of local facts and never touches the network. What it
//!   hashes, and the limit of what that can achieve, is documented on [`machine_id`].
//!
//! ## Expiry is inclusive
//!
//! A licence expires *at* its `expires_at` instant, not one second before it. A licence whose
//! expiry is `1_700_000_000` is valid when `now == 1_700_000_000` and expired at
//! `1_700_000_001`. That is stated here because the two conventions are one character apart in
//! the code and worlds apart in an argument with a customer.
//!
//! ## The pasteable form
//!
//! [`render`] produces a small armoured block; [`parse`] accepts that block, a bare base64
//! blob, or raw JSON. A studio that buys a licence receives an email, not a file transfer, so
//! the form has to survive being pasted into a text field without losing a byte.

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

pub mod capability;
pub mod document;
pub mod licence;
pub mod machine;

pub use capability::{capabilities, edition_allows, features_of, Capability, CAPABILITIES};
pub use document::{
    canonical_payload, embedded_key, parse, read_file, render, sign, sign_with_embedded, verify,
    verify_with_embedded, write_file, SignedLicence, ARMOUR_BEGIN, ARMOUR_END, MAX_LICENCE_BYTES,
};
pub use licence::{new_serial, Edition, Licence, LicenceError};
pub use machine::{machine_id, machine_id_uncached};

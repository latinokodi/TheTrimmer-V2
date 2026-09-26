//! The audit manifest: a tamper-evident record of what a run did.
//!
//! A cut is delivered with a claim about how it was made. The manifest is that claim as
//! data: who ran what, when, and with which arguments, appended in order and hashed as a
//! whole. Editing any entry after the fact changes the digest, and a digest that has been
//! signed with a key the studio holds cannot be reproduced without the key.
//!
//! The hashing is done over a *canonical* form of the data rather than over the bytes of the
//! file as written, so that a manifest which has been pretty-printed, re-indented or had its
//! object keys reordered still digests to the same value. That is what makes the digest a
//! statement about the content and not about the formatting.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use trimmer_core::{CoreError, CoreResult, ProjectId};
use uuid::Uuid;

/// One thing that happened, and who made it happen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    /// Seconds since the Unix epoch.
    pub at_unix_seconds: i64,
    /// Who ran it.
    pub actor: String,
    /// What they ran: `"cut"`, `"export"`, `"verify"`, `"open"`.
    pub action: String,
    /// The arguments, as JSON.
    pub detail: Value,
}

impl AuditEntry {
    /// An entry.
    #[must_use]
    pub fn new(
        actor: impl Into<String>,
        action: impl Into<String>,
        detail: Value,
        at_unix_seconds: i64,
    ) -> Self {
        Self {
            at_unix_seconds,
            actor: actor.into(),
            action: action.into(),
            detail,
        }
    }

    /// The entry's time as a UTC stamp, for a human reading the trail.
    ///
    /// A time that cannot be represented — far outside the range a signed 64-bit count of
    /// seconds can name as a date — is printed as the raw count rather than dropped, because
    /// an entry with no time at all is worse than an ugly one.
    #[must_use]
    pub fn at_utc(&self) -> String {
        match time::OffsetDateTime::from_unix_timestamp(self.at_unix_seconds) {
            Ok(moment) => moment.to_string(),
            Err(_) => self.at_unix_seconds.to_string(),
        }
    }
}

/// A run's record: its identity, the environment it ran in, and every entry in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditManifest {
    /// This run's identity, minted when the manifest is opened.
    pub run_id: Uuid,
    /// The project the run belongs to.
    pub project_id: ProjectId,
    /// Every entry, oldest first.
    pub entries: Vec<AuditEntry>,
    /// The version of the program that wrote it.
    pub app_version: String,
    /// The machine it ran on, as the operator named it.
    pub machine: String,
}

impl AuditManifest {
    /// Open a manifest for a project, recording the opening as its first entry.
    ///
    /// Recording the opening is what puts the environment in the trail: a reader of the
    /// entries a month later can see which build and which machine the other entries came
    /// from without trusting the fields at the top of the file.
    #[must_use]
    pub fn new(
        project_id: ProjectId,
        actor: impl Into<String>,
        app_version: impl Into<String>,
        machine: impl Into<String>,
        now_unix: i64,
    ) -> Self {
        let actor = actor.into();
        let app_version = app_version.into();
        let machine = machine.into();
        let run_id = Uuid::now_v7();
        let detail = serde_json::json!({
            "runId": run_id,
            "appVersion": app_version,
            "machine": machine,
        });
        let mut manifest = Self {
            run_id,
            project_id,
            entries: Vec::new(),
            app_version,
            machine,
        };
        manifest.record(&actor, "open", detail, now_unix);
        manifest
    }

    /// Append an entry.
    pub fn record(&mut self, actor: &str, action: &str, detail: Value, now_unix: i64) {
        self.entries
            .push(AuditEntry::new(actor, action, detail, now_unix));
    }

    /// A stable digest over the manifest, in lowercase hex.
    ///
    /// The digest covers the entries *and* the identity fields — the run id, the project,
    /// the program version and the machine — because a manifest edited after the fact is one
    /// where any of those could have been edited. Hashing only the entries would leave the
    /// environment of the run rewritable, which is exactly the claim a studio needs to be
    /// able to make.
    ///
    /// The entries are serialised in a canonical form, so the digest depends on the content
    /// and not on the order the keys of a `detail` object happened to be written in.
    #[must_use]
    pub fn digest(&self) -> String {
        let canonical = CanonicalManifest {
            run_id: &self.run_id,
            project_id: &self.project_id,
            app_version: &self.app_version,
            machine: &self.machine,
            entries: &self.entries,
        };
        let value = serde_json::to_value(&canonical)
            .expect("a manifest of plain data always serialises to JSON");
        let mut hasher = Sha256::new();
        hasher.update(canonical_json(&value).as_bytes());
        let digest = hasher.finalize();
        hex_lower(&digest[..])
    }

    /// Sign the digest with a key, returning a lowercase hex HMAC-SHA256.
    ///
    /// The key is not stored anywhere: verification is done by someone who already holds it,
    /// which is what makes the signature worth having.
    #[must_use]
    pub fn sign(&self, key: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(key)
            .expect("HMAC accepts a key of any length, including none");
        mac.update(self.digest().as_bytes());
        let signature = mac.finalize().into_bytes();
        hex_lower(&signature[..])
    }

    /// Verify a signature against this manifest's current digest.
    ///
    /// A manifest whose entries were edited after signing has a different digest, so it
    /// cannot be made to verify without the key — which is the whole claim the signature
    /// makes. The signature may be given in either case, and with surrounding whitespace,
    /// since it will have travelled through a log or an email to get here.
    #[must_use]
    pub fn verify_signature(&self, key: &[u8], signature_hex: &str) -> bool {
        let expected = self.sign(key);
        let supplied = signature_hex.trim().to_lowercase();
        constant_time_eq(expected.as_bytes(), supplied.as_bytes())
    }

    /// The manifest as JSON, for the audit trail.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Invariant`] when the value cannot be serialised, which cannot
    /// happen for plain data and is therefore a bug rather than a runtime condition.
    pub fn to_json(&self) -> CoreResult<String> {
        serde_json::to_string_pretty(self).map_err(|error| json_error(&error))
    }

    /// Read a manifest back from JSON.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Invariant`] when the text is not an audit manifest.
    pub fn from_json(text: &str) -> CoreResult<Self> {
        serde_json::from_str(text).map_err(|error| json_error(&error))
    }
}

/// The manifest as it is hashed.
///
/// A separate view rather than the type itself, so that a field added to [`AuditManifest`]
/// for a future purpose does not silently change the digest of every manifest already
/// written. Changing what a digest covers has to be a deliberate edit here.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalManifest<'a> {
    run_id: &'a Uuid,
    project_id: &'a ProjectId,
    app_version: &'a str,
    machine: &'a str,
    entries: &'a [AuditEntry],
}

/// Compare two byte strings without stopping at the first difference.
///
/// A comparison that returns as soon as two bytes differ takes a different amount of time
/// depending on *where* they differ, and that is enough to recover a digest byte by byte
/// with a stopwatch. Comparing every byte and folding the differences into one accumulator
/// takes the same path whatever the inputs are. The length is folded in first, so two
/// strings of different lengths can never compare equal — including a short signature
/// against a long digest.
///
/// This is not a defence against a determined local attacker, who has better tools than
/// timing; it is here because it costs nothing, and because a signature check that can be
/// walked one character at a time is not a signature check.
#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

/// A digest or signature as lowercase hex.
fn hex_lower(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

/// A value as JSON with every object's keys in sorted order.
///
/// `serde_json`'s `Map` is backed by a `BTreeMap` by default, so it already prints keys in
/// sorted order — but only while the `preserve_order` feature is off, and that is a feature
/// any crate in a workspace can switch on for every crate in it. Sorting here makes the
/// property explicit and independent of feature unification, which is what lets the digest
/// be called stable rather than merely usually stable.
fn canonical_json(value: &Value) -> String {
    serde_json::to_string(&sorted(value)).expect("a JSON value always serialises")
}

/// The same value with every object rebuilt in sorted key order.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let ordered: BTreeMap<&String, Value> = object
                .iter()
                .map(|(key, value)| (key, sorted(value)))
                .collect();
            Value::Object(
                ordered
                    .into_iter()
                    .map(|(key, value)| (key.clone(), value))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// A manifest that could not be read or written as JSON, which for plain data is a bug.
fn json_error(error: &serde_json::Error) -> CoreError {
    CoreError::Invariant(format!(
        "the audit manifest could not be read as JSON: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> AuditManifest {
        let mut manifest = AuditManifest::new(
            ProjectId::new(),
            "fernando",
            "2.0.0",
            "edit-01",
            1_700_000_000,
        );
        manifest.record(
            "fernando",
            "cut",
            json!({ "segment": "begging 01", "frames": 600 }),
            1_700_000_100,
        );
        manifest.record(
            "fernando",
            "verify",
            json!({ "policy": "strict", "ok": true }),
            1_700_000_200,
        );
        manifest
    }

    #[test]
    fn a_new_manifest_records_its_own_opening() {
        let manifest = AuditManifest::new(ProjectId::new(), "fernando", "2.0.0", "edit-01", 42);
        assert_eq!(manifest.entries.len(), 1);
        let entry = &manifest.entries[0];
        assert_eq!(entry.action, "open");
        assert_eq!(entry.actor, "fernando");
        assert_eq!(entry.at_unix_seconds, 42);
        assert_eq!(entry.detail["appVersion"], json!("2.0.0"));
        assert_eq!(entry.detail["machine"], json!("edit-01"));
        assert_eq!(manifest.app_version, "2.0.0");
        assert_eq!(manifest.machine, "edit-01");
    }

    #[test]
    fn recording_appends_the_actor_the_action_and_the_time() {
        let manifest = manifest();
        assert_eq!(manifest.entries.len(), 3);
        assert_eq!(manifest.entries[1].actor, "fernando");
        assert_eq!(manifest.entries[1].action, "cut");
        assert_eq!(manifest.entries[1].at_unix_seconds, 1_700_000_100);
        assert_eq!(manifest.entries[2].detail["policy"], json!("strict"));
        // Two runs are two identities.
        assert_ne!(
            manifest.run_id,
            AuditManifest::new(ProjectId::new(), "x", "y", "z", 0).run_id
        );
    }

    #[test]
    fn an_entry_renders_its_time_in_utc() {
        let entry = AuditEntry::new("fernando", "cut", json!({}), 0);
        assert!(
            entry.at_utc().starts_with("1970-01-01"),
            "{}",
            entry.at_utc()
        );
        let entry = AuditEntry::new("fernando", "cut", json!({}), 1_700_000_000);
        assert!(
            entry.at_utc().starts_with("2023-11-14"),
            "{}",
            entry.at_utc()
        );
    }

    #[test]
    fn the_digest_is_stable_for_the_same_manifest() {
        let manifest = manifest();
        let digest = manifest.digest();
        assert_eq!(digest.len(), 64);
        assert!(digest
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
        assert_eq!(manifest.clone().digest(), digest);
        assert_eq!(
            manifest.digest(),
            digest,
            "hashing does not consume the manifest"
        );
    }

    #[test]
    fn the_digest_changes_when_an_entry_changes() {
        let manifest = manifest();

        let mut actor = manifest.clone();
        actor.entries[1].actor = "someone else".to_owned();
        assert_ne!(actor.digest(), manifest.digest());

        let mut action = manifest.clone();
        action.entries[1].action = "export".to_owned();
        assert_ne!(action.digest(), manifest.digest());

        let mut time = manifest.clone();
        time.entries[1].at_unix_seconds += 1;
        assert_ne!(time.digest(), manifest.digest());

        let mut detail = manifest.clone();
        detail.entries[1].detail = json!({ "segment": "begging 01", "frames": 601 });
        assert_ne!(detail.digest(), manifest.digest());

        let mut added = manifest.clone();
        added.record("fernando", "cut", json!({}), 1_700_000_300);
        assert_ne!(added.digest(), manifest.digest());
    }

    #[test]
    fn the_digest_covers_the_identity_fields_too() {
        let manifest = manifest();

        let mut version = manifest.clone();
        version.app_version = "2.0.1".to_owned();
        assert_ne!(version.digest(), manifest.digest());

        let mut machine = manifest.clone();
        machine.machine = "edit-02".to_owned();
        assert_ne!(machine.digest(), manifest.digest());

        let mut run = manifest.clone();
        run.run_id = Uuid::now_v7();
        assert_ne!(run.digest(), manifest.digest());

        let mut project = manifest.clone();
        project.project_id = ProjectId::new();
        assert_ne!(project.digest(), manifest.digest());
    }

    #[test]
    fn the_digest_does_not_depend_on_the_order_detail_keys_were_written_in() {
        // The same manifest, so that only the detail objects differ.
        let base = manifest();
        let mut one = base.clone();
        let mut two = base.clone();
        one.entries[1].detail =
            json!({ "b": 2, "a": 1, "nested": { "z": 3, "a": { "q": 4, "b": 5 } } });
        two.entries[1].detail =
            json!({ "a": 1, "b": 2, "nested": { "a": { "b": 5, "q": 4 }, "z": 3 } });
        assert_eq!(one.digest(), two.digest());

        // But the content still matters, at any depth.
        let mut three = base.clone();
        three.entries[1].detail =
            json!({ "a": 1, "b": 2, "nested": { "a": { "b": 5, "q": 6 }, "z": 3 } });
        assert_ne!(one.digest(), three.digest());
    }

    #[test]
    fn an_empty_manifest_has_a_stable_digest_and_round_trips() {
        let empty = AuditManifest {
            run_id: Uuid::now_v7(),
            project_id: ProjectId::new(),
            entries: Vec::new(),
            app_version: "2.0.0".to_owned(),
            machine: "edit-01".to_owned(),
        };
        assert!(empty.entries.is_empty());
        assert_eq!(empty.digest(), empty.clone().digest());
        assert_eq!(empty.digest().len(), 64);
        assert_eq!(
            empty.sign(b"key"),
            empty.clone().sign(b"key"),
            "signing is pure"
        );

        let json = empty.to_json().expect("serialises");
        let back = AuditManifest::from_json(&json).expect("deserialises");
        assert_eq!(back, empty);
        assert_eq!(back.digest(), empty.digest());
    }

    #[test]
    fn a_manifest_round_trips_through_json() {
        let manifest = manifest();
        let json = manifest.to_json().expect("serialises");
        assert!(json.contains("\"runId\""), "{json}");
        assert!(json.contains("\"atUnixSeconds\""), "{json}");
        let back = AuditManifest::from_json(&json).expect("deserialises");
        assert_eq!(back, manifest);
        assert_eq!(
            back.digest(),
            manifest.digest(),
            "a round trip must not change the digest"
        );
    }

    #[test]
    fn text_that_is_not_a_manifest_is_refused() {
        assert!(AuditManifest::from_json("not json at all").is_err());
        assert!(AuditManifest::from_json("{}").is_err());
    }

    #[test]
    fn a_signature_verifies_against_the_key_that_made_it() {
        let manifest = manifest();
        let key = b"a studio's own key";
        let signature = manifest.sign(key);
        assert_eq!(signature.len(), 64);
        assert!(signature
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
        assert!(manifest.verify_signature(key, &signature));
        // An empty key is a key.
        let signature = manifest.sign(b"");
        assert!(manifest.verify_signature(b"", &signature));
    }

    #[test]
    fn a_signature_does_not_verify_a_tampered_manifest() {
        let manifest = manifest();
        let key = b"a studio's own key";
        let signature = manifest.sign(key);

        let mut tampered = manifest.clone();
        tampered.entries[1].action = "export".to_owned();
        assert!(!tampered.verify_signature(key, &signature));

        let mut rewritten = manifest.clone();
        rewritten.machine = "somewhere else".to_owned();
        assert!(!rewritten.verify_signature(key, &signature));

        let mut extended = manifest.clone();
        extended.record("someone", "cut", json!({}), 1_700_000_400);
        assert!(!extended.verify_signature(key, &signature));
    }

    #[test]
    fn a_signature_does_not_verify_under_a_wrong_key() {
        let manifest = manifest();
        let signature = manifest.sign(b"the right key");
        assert!(!manifest.verify_signature(b"the wrong key", &signature));
        assert!(!manifest.verify_signature(b"the right ke", &signature));
        assert!(!manifest.verify_signature(b"the right key ", &signature));
    }

    #[test]
    fn a_signature_may_arrive_in_any_case_or_with_whitespace() {
        let manifest = manifest();
        let signature = manifest.sign(b"key");
        assert!(manifest.verify_signature(b"key", &signature.to_uppercase()));
        assert!(manifest.verify_signature(b"key", &format!("  {signature}\n")));
    }

    #[test]
    fn a_truncated_or_mangled_signature_does_not_verify() {
        let manifest = manifest();
        let signature = manifest.sign(b"key");
        assert!(!manifest.verify_signature(b"key", &signature[..63]));
        assert!(!manifest.verify_signature(b"key", &signature[1..]));
        assert!(!manifest.verify_signature(b"key", ""));
        assert!(!manifest.verify_signature(b"key", "not a signature at all"));
    }

    #[test]
    fn constant_time_comparison_answers_correctly() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abcdef", b"abcdef"));
        assert!(constant_time_eq(&[0, 1, 2], &[0, 1, 2]));
        // The first byte differs: a short-circuiting comparison returns here, this one does
        // not, and the answer is still no.
        assert!(!constant_time_eq(b"abcdef", b"bbcdef"));
        assert!(!constant_time_eq(b"abcdef", b"abcdee"));
        assert!(!constant_time_eq(&[0, 1, 2], &[0, 1, 3]));
    }

    #[test]
    fn constant_time_comparison_refuses_different_lengths() {
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"ab", b"abc"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(!constant_time_eq(b"a", b""));
        // A zero byte is not the same as no byte at all, which is what padding must not say.
        assert!(!constant_time_eq(&[0], b""));
    }
}

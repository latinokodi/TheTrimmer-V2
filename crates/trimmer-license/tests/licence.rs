//! The licence suite: validity boundaries, capability gating, the signature and its refusals,
//! the pasteable form, and the file reader's size cap.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use trimmer_license::{
    canonical_payload, capabilities, edition_allows, embedded_key, machine_id, machine_id_uncached,
    new_serial, parse, read_file, render, sign, verify, write_file, Edition, Licence, LicenceError,
    SignedLicence, ARMOUR_BEGIN, ARMOUR_END, MAX_LICENCE_BYTES,
};

/// A scratch directory that removes itself.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "trimmer-licence-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory can be made");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    fn dir(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A licence that is valid, unbound and expiring.
fn base() -> Licence {
    Licence {
        licensee: "Rollup Studios Ltd".to_owned(),
        email: "accounts@rollup.example".to_owned(),
        edition: Edition::Studio,
        seats: 3,
        issued_at: 1_000,
        expires_at: Some(2_000_000_000),
        machine_id: None,
        features: vec!["cut".to_owned(), "export".to_owned(), "batch".to_owned()],
        serial: "TRIM-AAAA-BBBB-CCCC".to_owned(),
    }
}

/// A licence that is valid forever.
fn perpetual() -> Licence {
    Licence {
        expires_at: None,
        ..base()
    }
}

// --- validity ----------------------------------------------------------------------------

#[test]
fn a_valid_perpetual_licence_is_accepted() {
    assert_eq!(perpetual().is_valid_at(1_500_000_000, None), Ok(()));
}

#[test]
fn a_valid_expiring_licence_is_accepted_before_its_expiry() {
    assert_eq!(base().is_valid_at(1_999_999_999, None), Ok(()));
}

#[test]
fn an_expired_licence_is_refused_and_names_the_instant() {
    let error = base().is_valid_at(2_000_000_001, None).expect_err("refused");
    assert_eq!(
        error,
        LicenceError::Expired {
            expired_at: 2_000_000_000
        }
    );
    assert!(error.to_string().contains("expired"), "{error}");
}

#[test]
fn exactly_at_expiry_is_still_valid() {
    // Inclusive, and stated in the crate docs: valid *through* the expiry instant.
    assert_eq!(base().is_valid_at(2_000_000_000, None), Ok(()));
}

#[test]
fn one_second_past_expiry_is_expired() {
    assert!(base().is_valid_at(2_000_000_001, None).is_err());
    assert!(base().is_expired_at(2_000_000_001));
    assert!(!base().is_expired_at(2_000_000_000));
}

#[test]
fn a_perpetual_licence_is_never_expired() {
    assert!(!perpetual().is_expired_at(i64::MAX));
    assert_eq!(perpetual().is_valid_at(i64::MAX, None), Ok(()));
}

#[test]
fn a_licence_issued_in_the_future_is_not_yet_valid() {
    let error = base().is_valid_at(999, None).expect_err("refused");
    assert_eq!(error, LicenceError::NotYetValid { issued_at: 1_000 });
    assert!(error.to_string().contains("1000"), "{error}");
}

#[test]
fn a_licence_bound_to_another_machine_is_refused() {
    let mut licence = base();
    licence.machine_id = Some("aaaa1111".to_owned());
    let error = licence
        .is_valid_at(1_500_000_000, Some("bbbb2222"))
        .expect_err("refused");
    assert_eq!(
        error,
        LicenceError::WrongMachine {
            expected: "aaaa1111".to_owned(),
            found: "bbbb2222".to_owned(),
        }
    );
}

#[test]
fn a_licence_bound_to_this_machine_is_accepted_case_insensitively() {
    let mut licence = base();
    licence.machine_id = Some("AAAA1111".to_owned());
    assert_eq!(licence.is_valid_at(1_500_000_000, Some("aaaa1111")), Ok(()));
}

#[test]
fn a_bound_licence_checked_without_a_machine_is_refused() {
    // "I could not tell which machine this is" is not evidence that it is the right one.
    let mut licence = base();
    licence.machine_id = Some("aaaa1111".to_owned());
    let error = licence
        .is_valid_at(1_500_000_000, None)
        .expect_err("refused");
    assert!(matches!(error, LicenceError::WrongMachine { .. }));
}

#[test]
fn an_any_machine_licence_is_accepted_anywhere() {
    let licence = base();
    assert_eq!(licence.is_valid_at(1_500_000_000, Some("whatever")), Ok(()));
    assert_eq!(licence.is_valid_at(1_500_000_000, None), Ok(()));
}

#[test]
fn a_licence_naming_nobody_is_refused() {
    let mut licence = base();
    licence.licensee = "   ".to_owned();
    assert!(matches!(
        licence.is_valid_at(1_500_000_000, None),
        Err(LicenceError::Malformed(_))
    ));
}

#[test]
fn an_expiry_before_the_issue_date_is_refused_as_malformed() {
    let mut licence = base();
    licence.expires_at = Some(500);
    assert!(matches!(
        licence.is_valid_at(1_500_000_000, None),
        Err(LicenceError::Malformed(_))
    ));
}

// --- capability gating --------------------------------------------------------------------

#[test]
fn every_edition_and_feature_pair_agrees_with_the_table() {
    let table = capabilities();
    assert!(!table.is_empty());
    for edition in Edition::ALL {
        for capability in &table {
            assert_eq!(
                edition_allows(edition, capability.name),
                capability.editions.contains(&edition),
                "{edition} / {}",
                capability.name
            );
        }
    }
}

#[test]
fn an_unknown_feature_is_allowed_to_nobody() {
    for edition in Edition::ALL {
        assert!(!edition_allows(edition, "teleport"));
        assert!(!edition_allows(edition, ""));
        // A typo denies rather than grants, which is the safe direction.
        assert!(!edition_allows(edition, "batcch"));
    }
}

#[test]
fn the_editions_form_a_strict_progression() {
    // Every feature a smaller edition has, a larger one has too. If that ever stops being
    // true, the price list stops being a ladder and the report should say so.
    for pair in Edition::ALL.windows(2) {
        for capability in capabilities() {
            if edition_allows(pair[0], capability.name) {
                assert!(
                    edition_allows(pair[1], capability.name),
                    "{} has {} but {} does not",
                    pair[0],
                    capability.name,
                    pair[1]
                );
            }
        }
    }
    // And at least one feature is unique to the largest, so the ladder is not flat.
    assert!(edition_allows(Edition::Enterprise, "api"));
    assert!(!edition_allows(Edition::Studio, "api"));
}

#[test]
fn has_feature_needs_both_the_edition_and_the_list() {
    let mut licence = base();
    licence.features = vec!["cut".to_owned(), "batch".to_owned()];
    assert!(licence.has_feature("cut"));
    assert!(licence.has_feature("CUT"));
    assert!(!licence.has_feature("export"), "it is not in the list");

    // In the list, but the edition does not gate it.
    licence.features.push("api".to_owned());
    assert!(!licence.has_feature("api"));

    // Gated by the edition, but not sold.
    let mut studio = Licence {
        edition: Edition::Studio,
        features: vec!["watch".to_owned()],
        ..base()
    };
    assert!(studio.has_feature("watch"));
    studio.features.clear();
    assert!(!studio.has_feature("watch"));
}

#[test]
fn has_feature_is_false_for_a_feature_that_does_not_exist() {
    assert!(!base().has_feature("does-not-exist"));
}

#[test]
fn a_new_licence_is_granted_exactly_its_editions_features() {
    let licence = Licence::new("X", "x@y.z", Edition::Personal, 1, 0, None);
    for capability in capabilities() {
        assert_eq!(
            licence.has_feature(capability.name),
            edition_allows(Edition::Personal, capability.name)
        );
    }
}

// --- signing ------------------------------------------------------------------------------

#[test]
fn a_signed_licence_verifies_with_its_key() {
    let key = b"a-key-for-a-test";
    let signed = sign(&base(), key).expect("signed");
    assert!(!signed.signature.is_empty());
    assert_eq!(verify(&signed, key), Ok(()));
}

#[test]
fn a_signature_from_another_key_is_refused() {
    let signed = sign(&base(), b"key-one").expect("signed");
    assert_eq!(
        verify(&signed, b"key-two"),
        Err(LicenceError::BadSignature)
    );
}

#[test]
fn tampering_with_the_licensee_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.licence.licensee = "Someone Else".to_owned();
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn tampering_with_the_edition_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.licence.edition = Edition::Enterprise;
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn tampering_with_the_seats_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.licence.seats = 500;
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn tampering_with_the_features_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.licence.features.push("api".to_owned());
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn tampering_with_the_expiry_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.licence.expires_at = None;
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn tampering_with_the_machine_id_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.licence.machine_id = Some("the-machine-i-want".to_owned());
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn tampering_with_the_serial_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.licence.serial = "TRIM-9999-9999-9999".to_owned();
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn a_truncated_signature_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.signature.truncate(20);
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn a_garbage_signature_is_refused() {
    let key = b"a-key-for-a-test";
    let mut signed = sign(&base(), key).expect("signed");
    signed.signature = "not base64 at all !!!".to_owned();
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));

    signed.signature = String::new();
    assert_eq!(verify(&signed, key), Err(LicenceError::BadSignature));
}

#[test]
fn an_empty_signature_is_refused() {
    let signed = SignedLicence {
        licence: base(),
        signature: String::new(),
    };
    assert_eq!(verify(&signed, embedded_key()), Err(LicenceError::BadSignature));
}

#[test]
fn the_feature_order_does_not_change_the_signature() {
    let key = b"a-key-for-a-test";
    let mut one = base();
    one.features = vec!["batch".to_owned(), "cut".to_owned(), "export".to_owned()];
    let mut two = base();
    two.features = vec!["export".to_owned(), "batch".to_owned(), "cut".to_owned()];

    assert_eq!(
        canonical_payload(&one).expect("a payload"),
        canonical_payload(&two).expect("a payload")
    );
    assert_eq!(
        sign(&one, key).expect("signed").signature,
        sign(&two, key).expect("signed").signature
    );
}

#[test]
fn a_duplicated_feature_does_not_change_the_signature() {
    let key = b"a-key-for-a-test";
    let one = base();
    let mut two = base();
    two.features.push("cut".to_owned());
    two.features.push("cut".to_owned());
    assert_eq!(
        sign(&one, key).expect("signed").signature,
        sign(&two, key).expect("signed").signature
    );
}

#[test]
fn a_licence_rebuilt_with_its_fields_in_another_order_signs_identically() {
    // The property the `Value` round trip exists for: the payload is a function of the
    // *content*, not of the order a struct declares its fields or a tool writes its keys.
    let key = b"a-key-for-a-test";
    let mut licence = base();
    // Already in canonical order, so the only difference the rebuild below introduces is the
    // order the object's keys are written in.
    licence.features = vec!["batch".to_owned(), "cut".to_owned(), "export".to_owned()];
    let payload = canonical_payload(&licence).expect("a payload");

    // The same object, with its keys inserted in the opposite order.
    let value = serde_json::to_value(&licence).expect("a value");
    let serde_json::Value::Object(map) = value else {
        panic!("a licence serialises to an object");
    };
    let mut reversed = serde_json::Map::new();
    let mut pairs: Vec<(String, serde_json::Value)> = map.into_iter().collect();
    pairs.reverse();
    for (key, value) in pairs {
        reversed.insert(key, value);
    }
    let rebuilt = serde_json::Value::Object(reversed);
    assert_eq!(
        serde_json::to_string(&rebuilt).expect("a string"),
        payload,
        "serde_json's map is BTreeMap-backed, so keys come out sorted"
    );

    let signature = sign(&licence, key).expect("signed").signature;
    let reparsed: Licence = serde_json::from_str(&payload).expect("the payload is a licence");
    assert_eq!(sign(&reparsed, key).expect("signed").signature, signature);
}

// --- the pasteable form -------------------------------------------------------------------

#[test]
fn render_produces_the_documented_armoured_form() {
    let signed = sign(&base(), b"k").expect("signed");
    let text = render(&signed);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.first().copied(), Some(ARMOUR_BEGIN));
    assert_eq!(lines.last().copied(), Some(ARMOUR_END));
    assert!(lines.len() >= 3);
    for line in &lines[1..lines.len() - 1] {
        assert!(line.len() <= 64, "a wrapped line is at most 64 wide: {line}");
        assert!(!line.trim().is_empty());
    }
    assert!(text.ends_with('\n'));
}

#[test]
fn parse_and_render_round_trip() {
    let signed = sign(&base(), embedded_key()).expect("signed");
    let parsed = parse(&render(&signed)).expect("parsed");
    assert_eq!(parsed, signed);
}

#[test]
fn parse_accepts_a_bare_base64_blob() {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;
    let signed = sign(&base(), b"k").expect("signed");
    let json = serde_json::to_vec(&signed).expect("json");
    let blob = BASE64.encode(json);
    assert_eq!(parse(&blob).expect("parsed"), signed);
}

#[test]
fn parse_accepts_raw_json() {
    let signed = sign(&base(), b"k").expect("signed");
    let json = serde_json::to_string(&signed).expect("json");
    assert_eq!(parse(&json).expect("parsed"), signed);
}

#[test]
fn parse_tolerates_a_paste_that_added_whitespace_and_lost_the_markers() {
    let signed = sign(&base(), b"k").expect("signed");
    let body = render(&signed)
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("  \r\n  ");
    assert_eq!(parse(&body).expect("parsed"), signed);
}

#[test]
fn parse_refuses_text_that_is_not_a_licence() {
    let error = parse("hello, this is not a licence").expect_err("refused");
    assert!(matches!(error, LicenceError::Malformed(_)));
    assert!(parse("").is_err());
    assert!(parse("   \n  ").is_err());
}

// --- serials ------------------------------------------------------------------------------

#[test]
fn a_serial_has_the_documented_shape() {
    for _ in 0..32 {
        let serial = new_serial();
        assert_eq!(serial.len(), 19, "{serial}");
        assert!(serial.starts_with("TRIM-"), "{serial}");
        let groups: Vec<&str> = serial.split('-').collect();
        assert_eq!(groups.len(), 4, "{serial}");
        assert_eq!(groups[0], "TRIM");
        for group in &groups[1..] {
            assert_eq!(group.len(), 4, "{serial}");
            for ch in group.chars() {
                assert!(
                    "23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(ch),
                    "{serial} holds {ch}, which is not in the alphabet"
                );
            }
        }
    }
}

#[test]
fn serials_minted_together_differ() {
    let first = new_serial();
    let second = new_serial();
    assert_ne!(first, second);
}

// --- the machine --------------------------------------------------------------------------

#[test]
fn machine_id_is_stable_and_non_empty() {
    let first = machine_id();
    let second = machine_id();
    assert_eq!(first, second, "it is cached, so it cannot drift under a caller");
    assert!(!first.is_empty());
    assert_eq!(first.len(), 32, "sixteen bytes of SHA-256, in hex");
    assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()), "{first}");
    assert_eq!(first, machine_id_uncached());
}

// --- files --------------------------------------------------------------------------------

#[test]
fn a_licence_written_to_a_file_reads_back() {
    let scratch = Scratch::new("roundtrip");
    let signed = sign(&base(), embedded_key()).expect("signed");
    let path = scratch.join("nested").join("licence.key");
    write_file(&path, &signed).expect("written");
    assert!(path.is_file());
    assert_eq!(read_file(&path).expect("read"), signed);
}

#[test]
fn read_file_refuses_a_missing_file() {
    let scratch = Scratch::new("missing");
    let error = read_file(&scratch.join("nope.key")).expect_err("refused");
    assert!(matches!(error, LicenceError::Malformed(_)));
    assert!(error.to_string().contains("not a readable licence"), "{error}");
}

#[test]
fn read_file_refuses_a_directory() {
    let scratch = Scratch::new("directory");
    let error = read_file(scratch.dir()).expect_err("refused");
    assert!(error.to_string().contains("is not a file"), "{error}");
}

#[test]
fn read_file_refuses_a_file_over_the_cap_before_reading_it() {
    let scratch = Scratch::new("oversized");
    let path = scratch.join("huge.key");
    std::fs::write(&path, vec![b'x'; MAX_LICENCE_BYTES as usize + 1]).expect("written");
    let error = read_file(&path).expect_err("refused");
    let message = error.to_string();
    assert!(message.contains("65536"), "{message}");
    assert!(
        message.contains(&(MAX_LICENCE_BYTES + 1).to_string()),
        "the message names the size it found: {message}"
    );
}

#[test]
fn read_file_accepts_a_file_exactly_at_the_cap() {
    let scratch = Scratch::new("atcap");
    let signed = sign(&base(), b"k").expect("signed");
    let rendered = render(&signed);
    let mut padded = rendered.clone();
    // Whitespace to the cap: a legal licence with a lot of trailing blank lines.
    while (padded.len() as u64) < MAX_LICENCE_BYTES {
        padded.push('\n');
    }
    assert_eq!(padded.len() as u64, MAX_LICENCE_BYTES);
    let path = scratch.join("at-cap.key");
    std::fs::write(&path, padded).expect("written");
    assert_eq!(read_file(&path).expect("read"), signed);
}

#[test]
fn read_file_refuses_a_file_that_is_not_a_licence() {
    let scratch = Scratch::new("notalicence");
    let path = scratch.join("readme.txt");
    std::fs::write(&path, "this is a readme, not a licence\n").expect("written");
    assert!(read_file(&path).is_err());
}

// --- days remaining -----------------------------------------------------------------------

#[test]
fn days_remaining_is_none_for_a_perpetual_licence() {
    assert_eq!(perpetual().days_remaining(1_500_000_000), None);
}

#[test]
fn days_remaining_rounds_up_and_saturates_at_zero() {
    let licence = base();
    assert_eq!(licence.days_remaining(2_000_000_000), Some(0));
    assert_eq!(licence.days_remaining(2_000_000_000 - 1), Some(1), "an instant left is one day");
    assert_eq!(licence.days_remaining(2_000_000_000 - 86_400), Some(1));
    assert_eq!(licence.days_remaining(2_000_000_000 - 86_401), Some(2));
    assert_eq!(licence.days_remaining(2_000_000_001), Some(0), "never negative");
    assert_eq!(licence.days_remaining(0), Some(23_149));
}

// --- odds and ends ------------------------------------------------------------------------

#[test]
fn an_edition_reads_from_the_words_a_price_list_uses() {
    use std::str::FromStr;
    assert_eq!(Edition::from_str("Studio"), Ok(Edition::Studio));
    assert_eq!(Edition::from_str(" enterprise "), Ok(Edition::Enterprise));
    assert!(Edition::from_str("platinum").is_err());
    assert_eq!(Edition::Studio.to_string(), "studio");
}

#[test]
fn a_licence_summary_says_what_a_person_needs_to_know() {
    let summary = base().summary();
    assert!(summary.contains("Rollup Studios Ltd"), "{summary}");
    assert!(summary.contains("studio"), "{summary}");
    assert!(summary.contains("3 seat"), "{summary}");
    assert!(summary.contains("any machine"), "{summary}");

    let bound = Licence {
        machine_id: Some("deadbeef".to_owned()),
        expires_at: None,
        ..base()
    };
    assert!(bound.summary().contains("deadbeef"));
    assert!(bound.summary().contains("perpetual"));
}

#[test]
fn the_embedded_key_is_not_empty_and_signs_what_it_verifies() {
    assert!(!embedded_key().is_empty());
    let signed = trimmer_license::sign_with_embedded(&base()).expect("signed");
    assert_eq!(trimmer_license::verify_with_embedded(&signed), Ok(()));
}

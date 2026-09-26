//! The licence itself: what was sold, to whom, and whether it is good right now.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

/// Which product a customer bought.
///
/// The editions are ordered from the smallest to the largest, and [`capability`] is the table
/// that says which features each one gates. An edition is a *commercial* fact, not a runtime
/// one: the program checks it, and a customer who edits the file to say `Enterprise` changes
/// nothing, because the same edit breaks the signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edition {
    /// One editor, one machine, no automation.
    Personal,
    /// A studio: batch runs, watch folders, transcript search.
    Studio,
    /// A pipeline: the local API and scheduling priority as well.
    Enterprise,
}

impl Edition {
    /// Every edition, smallest first, in the order a price list would show them.
    pub const ALL: [Self; 3] = [Self::Personal, Self::Studio, Self::Enterprise];

    /// The word a person writes and a file holds.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Studio => "studio",
            Self::Enterprise => "enterprise",
        }
    }
}

impl std::fmt::Display for Edition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.word())
    }
}

impl std::str::FromStr for Edition {
    type Err = LicenceError;

    /// Read an edition from text, in any case.
    ///
    /// # Errors
    ///
    /// Returns [`LicenceError::Malformed`] when the word names no edition.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "personal" => Ok(Self::Personal),
            "studio" => Ok(Self::Studio),
            "enterprise" => Ok(Self::Enterprise),
            other => Err(LicenceError::Malformed(format!(
                "{other:?} is not an edition; expected personal, studio or enterprise"
            ))),
        }
    }
}

/// What a customer bought, and for how long.
///
/// Every field is public because a licence is a document: the program reads it, and a support
/// engineer reads it in a bug report. There is no invariant between the fields that a
/// constructor could enforce and a deserialiser could not break, so there is no constructor:
/// [`Licence::is_valid_at`] is where the fields are checked against each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Licence {
    /// The person or company the licence was sold to.
    pub licensee: String,
    /// Where to reach them.
    pub email: String,
    /// Which edition.
    pub edition: Edition,
    /// How many machines may run it at once.
    pub seats: u32,
    /// Seconds since the Unix epoch, when it was issued.
    pub issued_at: i64,
    /// Seconds since the Unix epoch, or `None` for a perpetual licence.
    pub expires_at: Option<i64>,
    /// The machine it is bound to, or `None` to run anywhere.
    pub machine_id: Option<String>,
    /// The features it unlocks, e.g. `batch`, `watch`, `api`.
    ///
    /// Kept sorted by [`crate::canonical_payload`] before signing, so the order this vector is
    /// built in never changes the signature.
    pub features: Vec<String>,
    /// The human-readable serial, e.g. `TRIM-XXXX-XXXX-XXXX`.
    pub serial: String,
}

impl Licence {
    /// A licence with the serial minted and every field at its most permissive.
    ///
    /// A convenience for a sales tool and for a test, not a validation bypass: it still has to
    /// be signed, and [`Licence::is_valid_at`] still applies.
    #[must_use]
    pub fn new(
        licensee: impl Into<String>,
        email: impl Into<String>,
        edition: Edition,
        seats: u32,
        issued_at: i64,
        expires_at: Option<i64>,
    ) -> Self {
        Self {
            licensee: licensee.into(),
            email: email.into(),
            edition,
            seats,
            issued_at,
            expires_at,
            machine_id: None,
            features: crate::capability::CAPABILITIES
                .iter()
                .filter(|capability| capability.editions.contains(&edition))
                .map(|capability| capability.name.to_owned())
                .collect(),
            serial: new_serial(),
        }
    }

    /// Whether the licence is good at an instant, on a machine.
    ///
    /// `machine_id` is what [`crate::machine_id`] returns on the machine asking. Pass `None`
    /// only when the machine genuinely cannot be identified; a licence that is bound to a
    /// machine treats `None` as a mismatch, because "I could not tell which machine this is"
    /// is not evidence that it is the right one.
    ///
    /// # Errors
    ///
    /// Returns [`LicenceError::NotYetValid`] when the clock is before the issue date — which
    /// on a correctly set clock means the licence is a forgery or the clock is wrong, and
    /// either way it must not be honoured. Returns [`LicenceError::Expired`] when the instant
    /// is past the expiry, [`LicenceError::WrongMachine`] when the binding does not match, and
    /// [`LicenceError::Malformed`] when a field cannot mean anything — a blank licensee, an
    /// expiry before the issue date, or a machine binding that is empty.
    pub fn is_valid_at(&self, now_unix: i64, machine_id: Option<&str>) -> Result<(), LicenceError> {
        if self.licensee.trim().is_empty() {
            return Err(LicenceError::Malformed(
                "the licence names nobody".to_owned(),
            ));
        }
        if let Some(expires_at) = self.expires_at {
            if expires_at < self.issued_at {
                return Err(LicenceError::Malformed(format!(
                    "it expires at {expires_at}, before it was issued at {}",
                    self.issued_at
                )));
            }
        }
        if now_unix < self.issued_at {
            return Err(LicenceError::NotYetValid {
                issued_at: self.issued_at,
            });
        }
        if let Some(expires_at) = self.expires_at {
            // Inclusive: valid *through* the expiry instant. See the crate docs.
            if now_unix > expires_at {
                return Err(LicenceError::Expired { expired_at: expires_at });
            }
        }
        if let Some(expected) = &self.machine_id {
            if expected.trim().is_empty() {
                return Err(LicenceError::Malformed(
                    "the licence binds an empty machine id".to_owned(),
                ));
            }
            match machine_id {
                Some(found) if found.eq_ignore_ascii_case(expected) => {}
                Some(found) => {
                    return Err(LicenceError::WrongMachine {
                        expected: expected.clone(),
                        found: found.to_owned(),
                    })
                }
                None => {
                    return Err(LicenceError::WrongMachine {
                        expected: expected.clone(),
                        found: "(this machine could not be identified)".to_owned(),
                    })
                }
            }
        }
        Ok(())
    }

    /// True when the edition gates this feature *and* the licence lists it.
    ///
    /// Both halves matter. The licence's list is what was sold — a customer on a Studio
    /// licence who did not buy the watch-folder add-on must not get it — and the edition table
    /// is what the product will do at all. An unknown feature is false, never true, so a
    /// typo in a caller is a missing feature rather than an accidental grant.
    #[must_use]
    pub fn has_feature(&self, feature: &str) -> bool {
        crate::capability::edition_allows(self.edition, feature)
            && self
                .features
                .iter()
                .any(|listed| listed.eq_ignore_ascii_case(feature))
    }

    /// True when the licence has run out at an instant.
    ///
    /// A perpetual licence is never expired, which is the whole meaning of perpetual.
    #[must_use]
    pub fn is_expired_at(&self, now_unix: i64) -> bool {
        self.expires_at.is_some_and(|expires_at| now_unix > expires_at)
    }

    /// Whole days until expiry, rounded up, or `None` for a perpetual licence.
    ///
    /// Rounded up rather than down because the number is shown to a person deciding whether to
    /// renew: with four hours left, "0 days" is alarming and "1 day" is true enough to act on.
    /// An expired licence reports `0`, not a negative number.
    #[must_use]
    pub fn days_remaining(&self, now_unix: i64) -> Option<i64> {
        let expires_at = self.expires_at?;
        let remaining = expires_at.saturating_sub(now_unix);
        Some(if remaining <= 0 {
            0
        } else {
            remaining / 86_400 + i64::from(remaining % 86_400 != 0)
        })
    }

    /// A one-line description for a status report.
    #[must_use]
    pub fn summary(&self) -> String {
        let term = match self.expires_at {
            Some(expires_at) => format!("expires {}", stamp(expires_at)),
            None => "perpetual".to_owned(),
        };
        let machine = self
            .machine_id
            .as_ref()
            .map_or_else(|| "any machine".to_owned(), |id| format!("bound to {id}"));
        format!(
            "{} ({}) — {} seat(s), {term}, {machine}, {} feature(s)",
            self.licensee,
            self.edition,
            self.seats,
            self.features.len()
        )
    }
}

/// Everything a licence can be refused for.
///
/// Each variant carries the numbers that produced it, so a message can be written without
/// re-deriving them and a log records the truth rather than a summary of it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LicenceError {
    /// The licence ran out.
    #[error("this licence expired on {expired_at}")]
    Expired {
        /// The instant it expired, in Unix seconds.
        expired_at: i64,
    },

    /// It is bound to a different machine.
    #[error("this licence is bound to machine {expected}, but this machine is {found}")]
    WrongMachine {
        /// The machine the licence names.
        expected: String,
        /// The machine asking.
        found: String,
    },

    /// The clock is before the issue date.
    #[error("this licence was not issued until {issued_at}, and the clock says it is earlier")]
    NotYetValid {
        /// The issue instant, in Unix seconds.
        issued_at: i64,
    },

    /// A feature was asked for that the licence does not grant.
    #[error("this licence does not include the {0} feature")]
    MissingFeature(String),

    /// The signature does not match the contents.
    #[error("the licence signature does not match its contents")]
    BadSignature,

    /// The licence text cannot be read as a licence at all.
    #[error("this is not a readable licence: {0}")]
    Malformed(String),

    /// A cryptographic operation failed.
    ///
    /// Included because HMAC accepts a key of any length — including none — so this is only
    /// reachable if a future key type refuses one, and a variant that says so is better than
    /// an `expect` that would panic in a customer's hands.
    #[error("the licence signature could not be computed: {0}")]
    Crypto(String),
}

/// One instant as a UTC stamp, falling back to the raw count for a value no date can name.
fn stamp(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix).map_or_else(
        |_| unix.to_string(),
        |moment| moment.to_string(),
    )
}

/// The characters a serial may use.
///
/// `0`, `O`, `1` and `I` are absent deliberately: a serial is read aloud down a telephone and
/// typed from a screenshot, and the four characters that look like each other are the four
/// that get a support ticket.
const SERIAL_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// A counter mixed into the serial, so two serials minted in the same nanosecond differ.
static SERIAL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh serial: `TRIM-XXXX-XXXX-XXXX`.
///
/// Built from the clock, the process id and a counter, hashed rather than used directly — the
/// point is a short string a person can read out, not an unguessable token. Uniqueness comes
/// from the counter, and a serial is not a secret: the signature is what makes a licence
/// unforgeable, and the serial is what makes it referable in an email.
#[must_use]
pub fn new_serial() -> String {
    let mut seed = Vec::with_capacity(32);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    seed.extend_from_slice(&nanos.to_le_bytes());
    seed.extend_from_slice(
        &SERIAL_COUNTER
            .fetch_add(1, Ordering::SeqCst)
            .to_le_bytes(),
    );
    seed.extend_from_slice(&u64::from(std::process::id()).to_le_bytes());

    let digest = Sha256::digest(&seed);
    let chars: Vec<char> = digest
        .iter()
        .take(12)
        .map(|byte| char::from(SERIAL_ALPHABET[*byte as usize % SERIAL_ALPHABET.len()]))
        .collect();
    let group = |offset: usize| -> String { (0..4).map(|index| chars[offset + index]).collect() };
    format!("TRIM-{}-{}-{}", group(0), group(4), group(8))
}

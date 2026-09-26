//! Which edition gates which feature.
//!
//! One table, and everything else derives from it. That matters more than it looks: the answer
//! to "can this customer do X" is asked in three places — the licence, the command line and
//! the daemon — and three hand-written `match` statements would disagree within a release.
//! [`edition_allows`] is the only implementation, and the table is public so a licence report
//! can print the whole product matrix rather than the four rows a given customer happens to
//! own.
//!
//! A feature that is not in the table is allowed to nobody. That is the safe direction for a
//! typo: `edition_allows(edition, "batcch")` is `false`, so a misspelt gate denies rather than
//! grants.

use crate::licence::Edition;

/// One feature, and the editions that include it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    /// The feature's name, as it appears in a licence's `features` list.
    pub name: &'static str,
    /// The editions that include it, smallest first.
    pub editions: &'static [Edition],
}

/// Every feature the product gates, in the order a price list would show them.
pub const CAPABILITIES: &[Capability] = &[
    Capability {
        name: "cut",
        editions: &[Edition::Personal, Edition::Studio, Edition::Enterprise],
    },
    Capability {
        name: "export",
        editions: &[Edition::Personal, Edition::Studio, Edition::Enterprise],
    },
    Capability {
        name: "batch",
        editions: &[Edition::Studio, Edition::Enterprise],
    },
    Capability {
        name: "watch",
        editions: &[Edition::Studio, Edition::Enterprise],
    },
    Capability {
        name: "transcript",
        editions: &[Edition::Studio, Edition::Enterprise],
    },
    Capability {
        name: "api",
        editions: &[Edition::Enterprise],
    },
    Capability {
        name: "priority",
        editions: &[Edition::Enterprise],
    },
];

/// The table, as an owned list, for a caller that wants to iterate it.
///
/// A `Vec` rather than the constant because that is the shape a caller can hold while it
/// builds a report, and the allocation happens once per `doctor` rather than per keystroke.
#[must_use]
pub fn capabilities() -> Vec<Capability> {
    CAPABILITIES.to_vec()
}

/// True when an edition includes a feature.
///
/// Case-insensitive, because the name travels through a licence file, a command line and a
/// JSON body before it gets here and none of them is the authority on capitalisation.
#[must_use]
pub fn edition_allows(edition: Edition, feature: &str) -> bool {
    let wanted = feature.trim();
    CAPABILITIES.iter().any(|capability| {
        capability.name.eq_ignore_ascii_case(wanted) && capability.editions.contains(&edition)
    })
}

/// The feature names an edition includes, for a report.
#[must_use]
pub fn features_of(edition: Edition) -> Vec<&'static str> {
    CAPABILITIES
        .iter()
        .filter(|capability| capability.editions.contains(&edition))
        .map(|capability| capability.name)
        .collect()
}

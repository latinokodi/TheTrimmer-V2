//! # trimmer-store
//!
//! Where a project, its runs and their audit manifests actually live: one SQLite file.
//!
//! ## Why a database and not a folder of JSON
//!
//! A project is not one document. It is a header, a set of sources, an ordered list of
//! segments, a preset table and a growing history of runs, and the operations a studio
//! performs on it are *relational*: "every segment of this project, in order", "every run of
//! this project, newest first", "remove this project and everything it owns". A file per
//! project makes the first two a directory scan and the third a rule that some future code
//! path will forget to follow. `ON DELETE CASCADE` cannot forget.
//!
//! What a database is *worse* at is being read by a person, and that matters too: an editor
//! who wants to know what a `.trimmerproj` holds should be able to open it. So [`document`]
//! exists beside the database and is the other half of the answer — see its module docs.
//!
//! ## What is stored, and what is not
//!
//! Two things in a [`trimmer_core::Project`] are *decisions* rather than *facts* and are
//! stored twice on purpose:
//!
//! * The preset table. Presets are named rows; a project that carries a house preset it
//!   edited must still carry it after a round trip, so the table is written out rather than
//!   rebuilt from `standard_presets()` on load. `projects.default_preset` names one of them.
//! * The verification policy, as a word (`off`, `standard`, `strict`, `forensic`) rather than
//!   as a serialised enum, because a human reading the database with `sqlite3` should be able
//!   to see at a glance how hard a project checks its own output.
//!
//! Media facts are stored as JSON in `sources.media_json`, because they are a probe's answer
//! rather than a queryable property: nothing ever asks "which sources are 1920 wide".
//!
//! ## Saving is all-or-nothing
//!
//! [`SqliteStore::save`] replaces the project's row, its sources, its segments and its presets
//! inside **one transaction**. A crash between two of those statements would otherwise leave a
//! project holding half its segments and no way to tell that it was truncated — which is a
//! worse outcome than losing the save, because a truncated project still opens and still runs.
//! `tests/` proves the rollback with a save that is made to fail on a duplicate segment id.
//!
//! ## Migrations are forward-only, and that is the point
//!
//! [`schema`] holds one statement block per version. Opening a database applies every block
//! whose version is greater than the one recorded in `schema_version`, each inside its own
//! transaction, and never looks backwards. There is no `down`: a schema that can go backwards
//! is a schema two versions of the program can disagree about, and the version that is wrong
//! is always the older one. Adding version 2 is adding a block.

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

pub mod document;
pub mod schema;

mod store;

pub use store::{default_store_path, RunSummary, SqliteStore, StoreError, StoreResult};

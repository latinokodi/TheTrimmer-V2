//! The schema, and the forward-only migrations that reach it.
//!
//! One [`Migration`] per version, applied in ascending order, each inside its own
//! transaction. A database whose recorded version is already the newest has nothing applied
//! to it at all — see [`migrate`], which is a loop that does nothing in that case, and the
//! `opening_a_current_database_applies_nothing` test, which checks it.
//!
//! ## Indexes
//!
//! Version 1 creates exactly two, and the reason is worth stating where the SQL is:
//!
//! * `segments(project_id, ordinal)` — the query the workspace makes on every open.
//! * `runs(project_id, started_at)` — the query the run history makes on every open.
//!
//! Everything else in this schema is reached by primary key or by a full scan of a table with
//! one project's rows in it. More indexes would be speculative: each one costs write
//! throughput on every save and is a guess about a query nobody has written yet. Adding one is
//! a version 2 statement block and a measurement, not a hunch.
//!
//! ## The preset table
//!
//! The brief for this crate lists the tables it wants, and `presets` is not among them. It is
//! here anyway, and the reason is that [`trimmer_core::Project::presets`] is a real map with a
//! real content: a project that carries a house preset must still carry it after a round trip,
//! and `projects.default_preset` only records the *name* of the default. There is nowhere else
//! for the other presets to go, and dropping them on save would silently change what a batch
//! delivers.

use rusqlite::Connection;

use crate::{StoreError, StoreResult};

/// The newest schema version this build knows how to produce.
pub const LATEST_VERSION: i64 = 1;

/// One version's worth of statements.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// The version this block brings the database *to*.
    pub version: i64,
    /// The statements to run, in order. Each may itself be a multi-statement batch.
    pub statements: &'static [&'static str],
}

/// Every migration, in ascending version order.
///
/// Adding a version is adding an entry here. Nothing else in this crate needs to change.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    statements: &[
        // The project header. `default_preset` is a name, resolved against `presets`.
        "CREATE TABLE projects (
            id             TEXT PRIMARY KEY,
            name           TEXT NOT NULL,
            created_by     TEXT NOT NULL,
            created_at     INTEGER NOT NULL,
            updated_at     INTEGER NOT NULL,
            default_preset TEXT NOT NULL,
            verify         TEXT NOT NULL,
            output_dir     TEXT
        )",
        // A source belongs to exactly one project. `media_json` is the last probe's answer,
        // or NULL when the file has never been read.
        "CREATE TABLE sources (
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            path       TEXT NOT NULL,
            available  INTEGER NOT NULL,
            label      TEXT,
            media_json TEXT,
            PRIMARY KEY (project_id, path)
        )",
        // Segments are ordered within a project by `ordinal`, which is the running order the
        // user arranged. `end_frame` is NULL for a segment that runs to the end of its source.
        "CREATE TABLE segments (
            id            TEXT PRIMARY KEY,
            project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            ordinal       INTEGER NOT NULL,
            source_path   TEXT NOT NULL,
            name          TEXT NOT NULL,
            start_frame   INTEGER NOT NULL,
            end_frame     INTEGER,
            note          TEXT,
            tags_json     TEXT NOT NULL,
            preset        TEXT,
            handle_frames INTEGER NOT NULL,
            enabled       INTEGER NOT NULL
        )",
        // The house preset table, so an edited preset survives a round trip.
        "CREATE TABLE presets (
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            name        TEXT NOT NULL,
            preset_json TEXT NOT NULL,
            PRIMARY KEY (project_id, name)
        )",
        // One row per batch run, with the audit manifest exactly as it was signed.
        "CREATE TABLE runs (
            id            TEXT PRIMARY KEY,
            project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            started_at    INTEGER NOT NULL,
            finished_at   INTEGER,
            actor         TEXT NOT NULL,
            status        TEXT NOT NULL,
            manifest_json TEXT NOT NULL
        )",
        // One row per job inside a run. The counts a run summary reports are derived from
        // these rows rather than stored beside them, so the two can never disagree.
        "CREATE TABLE runs_items (
            run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
            ordinal     INTEGER NOT NULL,
            segment_id  TEXT NOT NULL,
            status      TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            PRIMARY KEY (run_id, ordinal)
        )",
        // The two real access paths, and deliberately no others. See the module docs.
        "CREATE INDEX segments_by_project ON segments(project_id, ordinal)",
        "CREATE INDEX runs_by_project ON runs(project_id, started_at)",
    ],
}];

/// Bring a connection up to [`LATEST_VERSION`], applying only what is missing.
///
/// # Errors
///
/// Returns [`StoreError::Schema`] when a migration fails, with the version that failed named
/// so an operator can see how far the file got.
pub fn migrate(conn: &mut Connection) -> StoreResult<i64> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")?;

    let recorded: Option<i64> = conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get(0)
        })
        .ok();
    let mut version = if let Some(version) = recorded {
        version
    } else {
        conn.execute("INSERT INTO schema_version (version) VALUES (0)", [])?;
        0
    };

    for migration in MIGRATIONS {
        if migration.version <= version {
            continue;
        }
        let transaction = conn.transaction()?;
        for statement in migration.statements {
            transaction
                .execute_batch(statement)
                .map_err(|error| StoreError::Schema {
                    version: migration.version,
                    reason: error.to_string(),
                })?;
        }
        transaction.execute(
            "UPDATE schema_version SET version = ?1",
            [migration.version],
        )?;
        transaction.commit()?;
        version = migration.version;
    }
    Ok(version)
}

/// The version recorded in a connection's `schema_version` table, or `None` when the table
/// has never been created.
///
/// # Errors
///
/// Returns [`StoreError::Sqlite`] when the read fails for a reason other than a missing table.
pub fn recorded_version(conn: &Connection) -> StoreResult<Option<i64>> {
    let table_exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
        [],
        |row| row.get::<_, i64>(0).map(|count| count > 0),
    )?;
    if !table_exists {
        return Ok(None);
    }
    let version: Option<i64> = conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get(0)
        })
        .ok();
    Ok(version)
}

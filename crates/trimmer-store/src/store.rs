//! The store itself: opening, migrating, and every read and write a project needs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use trimmer_app::BatchOutcome;
use trimmer_core::{MediaInfo, MediaPath, Project, ProjectId, Segment, SegmentId, SegmentSource, VerifyPolicy};
use uuid::Uuid;

use crate::schema;

/// Everything the store can fail at.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The database file could not be opened, or its directory could not be created.
    #[error("could not open the project store at {path}: {reason}")]
    Open {
        /// The file that would not open.
        path: String,
        /// Why not.
        reason: String,
    },

    /// SQLite refused a statement.
    #[error("the project store refused a statement: {0}")]
    Sqlite(String),

    /// A migration failed.
    #[error("schema migration to version {version} failed: {reason}")]
    Schema {
        /// The version that was being applied.
        version: i64,
        /// What went wrong.
        reason: String,
    },

    /// A row holds a value that cannot be read back into the domain.
    #[error("project store: {0}")]
    Decode(String),

    /// The project the caller asked for is not in the store.
    #[error("no project {0} in this store")]
    NotFound(ProjectId),

    /// A lock was poisoned by a panic in another thread.
    #[error("the project store lock was poisoned by a panic")]
    Poisoned,

    /// There is nowhere on this machine that a standard data directory can be found.
    #[error("no data directory is known on this machine")]
    NoDataDirectory,
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error.to_string())
    }
}

/// The crate's result alias.
pub type StoreResult<T> = Result<T, StoreError>;

/// One run of the batch queue, as the run history lists it.
///
/// The four counts are derived from `runs_items` on every read rather than stored beside the
/// run, so a summary can never disagree with the items it summarises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    /// The run's identity.
    pub id: Uuid,
    /// Seconds since the Unix epoch.
    pub started_at: i64,
    /// Seconds since the Unix epoch, or `None` for a run that never finished.
    pub finished_at: Option<i64>,
    /// The person or suite that started it.
    pub actor: String,
    /// `clean`, `unverified`, `failed` or `cancelled`.
    pub status: String,
    /// How many segments came out certified.
    pub succeeded: usize,
    /// How many came out with a failed check.
    pub unverified: usize,
    /// How many did not come out at all.
    pub failed: usize,
    /// How many were never attempted.
    pub skipped: usize,
}

impl RunSummary {
    /// The start time as a UTC stamp, for a report a person reads.
    #[must_use]
    pub fn started_utc(&self) -> String {
        stamp(self.started_at)
    }

    /// The finish time as a UTC stamp, or a dash for a run that is still going.
    #[must_use]
    pub fn finished_utc(&self) -> String {
        self.finished_at.map_or_else(|| "-".to_owned(), stamp)
    }
}

/// One instant as a UTC stamp, falling back to the raw count for a value no date can name.
fn stamp(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix).map_or_else(
        |_| unix.to_string(),
        |moment| moment.to_string(),
    )
}

/// Where a project store lives when the caller does not say.
///
/// # Errors
///
/// Returns [`StoreError::NoDataDirectory`] when the platform has no data directory to offer,
/// which means `$HOME` and the equivalent are both unset — a container with no passwd entry.
pub fn default_store_path() -> StoreResult<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", "TheTrimmer", "TheTrimmer")
        .ok_or(StoreError::NoDataDirectory)?;
    Ok(dirs.data_dir().join("projects.sqlite"))
}

/// A project store backed by one SQLite database.
///
/// A `Mutex` around the connection rather than a pool: a studio has one user, the operations
/// are single-digit milliseconds, and a pool would be a second thing to configure and get
/// wrong. The `Mutex` is what makes the type `Sync`, which [`trimmer_app::ProjectStore`]
/// requires.
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("locked", &self.conn.try_lock().is_err())
            .finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// Open a database, creating the file and its parent directories when they are absent,
    /// and applying every migration the file is missing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Open`] when the directory or file cannot be created, and
    /// [`StoreError::Schema`] when a migration fails.
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| StoreError::Open {
                    path: path.display().to_string(),
                    reason: error.to_string(),
                })?;
            }
        }
        let conn = Connection::open(path).map_err(|error| StoreError::Open {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        Self::from_connection(conn)
    }

    /// A database in memory, for a test or a throwaway session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Sqlite`] when the in-memory database cannot be created, which is
    /// a bug in SQLite rather than a condition a caller can act on.
    pub fn open_in_memory() -> StoreResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Set the pragmas, migrate, and wrap the connection.
    fn from_connection(mut conn: Connection) -> StoreResult<Self> {
        // Foreign keys are off by default in SQLite and have been since before this schema
        // existed. Without this line every `ON DELETE CASCADE` above is a comment.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // WAL is what lets a reader see the project while a batch is writing a run record.
        // It is a no-op for an in-memory database, which has no file to journal to.
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        schema::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// The schema version recorded in this database.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Poisoned`] when another thread panicked while holding the lock.
    pub fn schema_version(&self) -> StoreResult<Option<i64>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        schema::recorded_version(&conn)
    }

    /// The underlying connection, for a caller that needs a query this API does not expose.
    ///
    /// Handing out the lock rather than the connection is deliberate: the pragmas and the
    /// schema are invariants of this type, and a caller that could reach the raw connection
    /// could break both.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Poisoned`] when another thread panicked while holding the lock.
    pub fn with_connection<T>(&self, body: impl FnOnce(&Connection) -> T) -> StoreResult<T> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(body(&conn))
    }

    /// Record a finished batch as a run, and return its identity.
    ///
    /// `started_at` is passed in rather than read from the clock so that a test can pin it,
    /// and the finish time is derived from the outcome's own elapsed seconds — the number the
    /// batch measured — rather than from a second look at a clock that may have moved.
    ///
    /// # Errors
    ///
    /// Returns the error as a sentence, because this is called from the queue's reporting
    /// path where a run that cannot be recorded is worth a line in a log and not a new error
    /// type at every call site.
    pub fn record_run(
        &self,
        project_id: ProjectId,
        actor: &str,
        started_at: i64,
        outcome: &BatchOutcome,
    ) -> Result<Uuid, String> {
        let id = Uuid::now_v7();
        let finished_at = started_at + outcome.elapsed_seconds.max(0.0) as i64;
        let status = run_status(outcome);
        let manifest = outcome
            .audit
            .to_json()
            .map_err(|error| error.to_string())?;
        let items: Vec<(i64, String, String, String)> = outcome
            .jobs
            .iter()
            .map(|(job, segment, name, job_status)| {
                let detail = serde_json::json!({
                    "name": name,
                    "status": job_status.summary(),
                    "output": job_status.output().map(ToString::to_string),
                });
                (
                    ordinal_of(job.0),
                    segment.to_string(),
                    item_status(job_status).to_owned(),
                    detail.to_string(),
                )
            })
            .collect();

        let mut conn = self.conn.lock().map_err(|_| StoreError::Poisoned.to_string())?;
        let transaction = conn
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO runs (id, project_id, started_at, finished_at, actor, status, \
                 manifest_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id.to_string(),
                    project_id.to_string(),
                    started_at,
                    finished_at,
                    actor,
                    status,
                    manifest,
                ],
            )
            .map_err(|error| error.to_string())?;
        for (ordinal, segment_id, item, detail) in &items {
            transaction
                .execute(
                    "INSERT INTO runs_items (run_id, ordinal, segment_id, status, detail_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id.to_string(), ordinal, segment_id, item, detail],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(id)
    }

    /// Every run of a project, newest first.
    ///
    /// # Errors
    ///
    /// Returns the error as a sentence; see [`SqliteStore::record_run`].
    pub fn list_runs(&self, project_id: ProjectId) -> Result<Vec<RunSummary>, String> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned.to_string())?;
        let mut statement = conn
            .prepare(
                "SELECT r.id, r.started_at, r.finished_at, r.actor, r.status, \
                 COALESCE(SUM(CASE WHEN i.status = 'succeeded'  THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN i.status = 'unverified' THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN i.status = 'failed'     THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN i.status = 'skipped'    THEN 1 ELSE 0 END), 0) \
                 FROM runs r LEFT JOIN runs_items i ON i.run_id = r.id \
                 WHERE r.project_id = ?1 \
                 GROUP BY r.id, r.started_at, r.finished_at, r.actor, r.status \
                 ORDER BY r.started_at DESC, r.id DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([project_id.to_string()], |row| {
                Ok(RunSummary {
                    id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_default(),
                    started_at: row.get(1)?,
                    finished_at: row.get(2)?,
                    actor: row.get(3)?,
                    status: row.get(4)?,
                    succeeded: row.get::<_, i64>(5)? as usize,
                    unverified: row.get::<_, i64>(6)? as usize,
                    failed: row.get::<_, i64>(7)? as usize,
                    skipped: row.get::<_, i64>(8)? as usize,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    /// One run's audit manifest, exactly as it was written.
    ///
    /// # Errors
    ///
    /// Returns the error as a sentence; see [`SqliteStore::record_run`].
    pub fn load_run(&self, id: Uuid) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned.to_string())?;
        conn.query_row(
            "SELECT manifest_json FROM runs WHERE id = ?1",
            [id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())
    }

    /// The raw text of the `runs_items` detail for a run, for a caller that wants more than
    /// the summary counts.
    ///
    /// # Errors
    ///
    /// Returns the error as a sentence; see [`SqliteStore::record_run`].
    pub fn run_items(&self, id: Uuid) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned.to_string())?;
        let mut statement = conn
            .prepare("SELECT detail_json FROM runs_items WHERE run_id = ?1 ORDER BY ordinal")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([id.to_string()], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    // --- the reads and writes the trait needs, in a form that returns a real error --------

    fn save_project(&self, project: &Project) -> StoreResult<()> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let transaction = conn.transaction()?;

        transaction.execute(
            "INSERT INTO projects (id, name, created_by, created_at, updated_at, \
             default_preset, verify, output_dir) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(id) DO UPDATE SET name = ?2, created_by = ?3, created_at = ?4, \
             updated_at = ?5, default_preset = ?6, verify = ?7, output_dir = ?8",
            params![
                project.id.to_string(),
                project.name,
                project.created_by,
                project.created_at,
                project.updated_at,
                project.default_preset,
                verify_word(project.verify),
                project.output_dir.as_ref().map(ToString::to_string),
            ],
        )?;

        // Replace rather than merge. A merge would leave a segment the user deleted, and a
        // segment that is still there after being deleted is indistinguishable from one that
        // was never deleted — which is exactly the class of bug a project store must not have.
        transaction.execute(
            "DELETE FROM sources WHERE project_id = ?1",
            [project.id.to_string()],
        )?;
        transaction.execute(
            "DELETE FROM segments WHERE project_id = ?1",
            [project.id.to_string()],
        )?;
        transaction.execute(
            "DELETE FROM presets WHERE project_id = ?1",
            [project.id.to_string()],
        )?;

        for source in project.sources.values() {
            transaction.execute(
                "INSERT INTO sources (project_id, path, available, label, media_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    project.id.to_string(),
                    source.path.to_string(),
                    i64::from(source.available),
                    source.label,
                    media_json(source.media.as_ref())?,
                ],
            )?;
        }

        for (ordinal, segment) in project.segments.iter().enumerate() {
            transaction.execute(
                "INSERT INTO segments (id, project_id, ordinal, source_path, name, start_frame, \
                 end_frame, note, tags_json, preset, handle_frames, enabled) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    segment.id.to_string(),
                    project.id.to_string(),
                    ordinal_of(ordinal),
                    segment.source.to_string(),
                    segment.name,
                    segment.start_frame,
                    segment.end_frame,
                    segment.note,
                    serde_json::to_string(&segment.tags).map_err(decode)?,
                    segment.preset,
                    segment.handle_frames,
                    i64::from(segment.enabled),
                ],
            )?;
        }

        for (name, preset) in &project.presets {
            transaction.execute(
                "INSERT INTO presets (project_id, name, preset_json) VALUES (?1, ?2, ?3)",
                params![
                    project.id.to_string(),
                    name,
                    serde_json::to_string(preset).map_err(decode)?,
                ],
            )?;
        }

        transaction.commit()?;
        Ok(())
    }

    fn load_project(&self, id: ProjectId) -> StoreResult<Project> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let header = conn
            .query_row(
                "SELECT name, created_by, created_at, updated_at, default_preset, verify, \
                 output_dir FROM projects WHERE id = ?1",
                [id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StoreError::NotFound(id))?;

        let mut sources = BTreeMap::new();
        {
            let mut statement = conn.prepare(
                "SELECT path, available, label, media_json FROM sources WHERE project_id = ?1 \
                 ORDER BY path",
            )?;
            let rows = statement.query_map([id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            for row in rows {
                let (path, available, label, media) = row?;
                let path = MediaPath::new(path);
                sources.insert(
                    path.clone(),
                    SegmentSource {
                        path,
                        media: parse_media(media.as_deref())?,
                        available,
                        label,
                    },
                );
            }
        }

        let mut segments = Vec::new();
        {
            let mut statement = conn.prepare(
                "SELECT id, source_path, name, start_frame, end_frame, note, tags_json, preset, \
                 handle_frames, enabled FROM segments WHERE project_id = ?1 ORDER BY ordinal",
            )?;
            let rows = statement.query_map([id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)? != 0,
                ))
            })?;
            for row in rows {
                let (
                    segment_id,
                    source_path,
                    name,
                    start_frame,
                    end_frame,
                    note,
                    tags_json,
                    preset,
                    handle_frames,
                    enabled,
                ) = row?;
                segments.push(Segment {
                    id: SegmentId(uuid_of(&segment_id)?),
                    source: MediaPath::new(source_path),
                    name,
                    start_frame,
                    end_frame,
                    note,
                    tags: serde_json::from_str(&tags_json).map_err(decode)?,
                    preset,
                    handle_frames,
                    enabled,
                });
            }
        }

        let mut presets = BTreeMap::new();
        {
            let mut statement =
                conn.prepare("SELECT name, preset_json FROM presets WHERE project_id = ?1")?;
            let rows = statement.query_map([id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (name, json) = row?;
                presets.insert(name, serde_json::from_str(&json).map_err(decode)?);
            }
        }

        Ok(Project {
            id,
            name: header.0,
            sources,
            segments,
            presets,
            default_preset: header.4,
            verify: verify_policy(&header.5)?,
            output_dir: header.6.map(MediaPath::new),
            created_by: header.1,
            created_at: header.2,
            updated_at: header.3,
        })
    }

    fn list_projects(&self) -> StoreResult<Vec<(ProjectId, String, i64)>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let mut statement =
            conn.prepare("SELECT id, name, updated_at FROM projects ORDER BY updated_at DESC, id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut listed = Vec::new();
        for row in rows {
            let (id, name, updated_at) = row?;
            listed.push((
                ProjectId(uuid_of(&id)?),
                name,
                updated_at,
            ));
        }
        Ok(listed)
    }

    fn delete_project(&self, id: ProjectId) -> StoreResult<()> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute("DELETE FROM projects WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }
}

/// The word stored in `projects.verify`.
fn verify_word(policy: VerifyPolicy) -> &'static str {
    match policy {
        VerifyPolicy::Off => "off",
        VerifyPolicy::Standard => "standard",
        VerifyPolicy::Strict => "strict",
        VerifyPolicy::Forensic => "forensic",
    }
}

/// Read a stored verification policy back.
fn verify_policy(word: &str) -> StoreResult<VerifyPolicy> {
    match word {
        "off" => Ok(VerifyPolicy::Off),
        "standard" => Ok(VerifyPolicy::Standard),
        "strict" => Ok(VerifyPolicy::Strict),
        "forensic" => Ok(VerifyPolicy::Forensic),
        other => Err(StoreError::Decode(format!(
            "{other:?} is not a verification policy this build knows"
        ))),
    }
}

/// The overall word for a run.
fn run_status(outcome: &BatchOutcome) -> &'static str {
    if outcome.cancelled {
        "cancelled"
    } else if outcome.failed() > 0 {
        "failed"
    } else if outcome.unverified() > 0 {
        "unverified"
    } else {
        "clean"
    }
}

/// The word for one job inside a run.
fn item_status(status: &trimmer_app::JobStatus) -> &'static str {
    match status {
        trimmer_app::JobStatus::Succeeded { .. } => "succeeded",
        trimmer_app::JobStatus::Unverified { .. } => "unverified",
        trimmer_app::JobStatus::Failed { .. } => "failed",
        trimmer_app::JobStatus::Skipped { .. } => "skipped",
    }
}

/// Serialise the probe result, or `None` when the source was never probed.
fn media_json(media: Option<&MediaInfo>) -> StoreResult<Option<String>> {
    match media {
        Some(media) => serde_json::to_string(media).map(Some).map_err(decode),
        None => Ok(None),
    }
}

/// Read a stored probe result back.
fn parse_media(json: Option<&str>) -> StoreResult<Option<MediaInfo>> {
    match json {
        Some(text) => serde_json::from_str(text).map(Some).map_err(decode),
        None => Ok(None),
    }
}

/// Turn a failure from a decoding library into a decode error.
///
/// Generic over the error rather than taking one concrete type: a corrupt JSON column, a
/// damaged UUID and an unreadable verification word are all "the database holds something this
/// version cannot read", and all three arrive as a `Display`.
fn decode(error: impl std::fmt::Display) -> StoreError {
    StoreError::Decode(error.to_string())
}

/// Read an identity back out of its text form.
fn uuid_of(text: &str) -> StoreResult<Uuid> {
    Uuid::parse_str(text).map_err(decode)
}

/// A list position as a storable integer.
///
/// Saturating rather than wrapping: a project with more than `i64::MAX` segments is not a case
/// worth a panic, and a wrapped negative ordinal would sort the running order wrongly.
fn ordinal_of(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

impl trimmer_app::ProjectStore for SqliteStore {
    fn save(&self, project: &Project) -> Result<(), String> {
        self.save_project(project).map_err(|error| error.to_string())
    }

    fn load(&self, id: ProjectId) -> Result<Project, String> {
        self.load_project(id).map_err(|error| error.to_string())
    }

    fn list(&self) -> Result<Vec<(ProjectId, String, i64)>, String> {
        self.list_projects().map_err(|error| error.to_string())
    }

    fn delete(&self, id: ProjectId) -> Result<(), String> {
        self.delete_project(id).map_err(|error| error.to_string())
    }
}

//! The store's suite: round trips, the transaction, the migrations, and the two pragmas the
//! schema's own correctness depends on.
//!
//! Every test that touches a file uses its own scratch directory, so the suite can run in
//! parallel and a failure in one leaves nothing behind for the next.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use trimmer_app::{BatchOutcome, JobId, JobStatus, ProjectStore};
use trimmer_core::timecode::FrameRate;
use trimmer_core::{
    MediaInfo, MediaPath, Project, ProjectId, Segment, SegmentId, SegmentSource, Timescale,
    VerifyPolicy,
};
use trimmer_store::{document, schema, SqliteStore, StoreError};
use trimmer_verify::AuditManifest;
use uuid::Uuid;

/// A scratch directory that removes itself.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A fresh, empty directory unique to this test.
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "trimmer-store-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory can be made");
        Self { path }
    }

    /// A path inside the scratch directory.
    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A store in memory, which is all any test that is not about files needs.
fn memory() -> SqliteStore {
    SqliteStore::open_in_memory().expect("an in-memory database can be made")
}

/// Probe facts a source might have.
fn media(path: &str) -> MediaInfo {
    MediaInfo {
        path: MediaPath::new(path),
        codec: "h264".to_owned(),
        pix_fmt: "yuv420p".to_owned(),
        width: 1920,
        height: 1080,
        rate: FrameRate::FPS_29_97,
        average_rate: Some(FrameRate::FPS_29_97),
        timebase: Timescale::NINETY_KHZ,
        frame_count: 216_000,
        audio: Some(trimmer_core::AudioFormat {
            codec: "aac".to_owned(),
            sample_rate: 48_000,
            channels: 2,
        }),
        size_bytes: 6_000_000_000,
        start_time: 0.125,
    }
}

/// A project with one source, one segment and the standard preset library.
fn project(name: &str, now: i64) -> Project {
    let mut project = Project::new(name, "Fernando", now);
    project.upsert_source(SegmentSource {
        path: MediaPath::new(r"H:\masters\andy.mp4"),
        media: Some(media(r"H:\masters\andy.mp4")),
        available: true,
        label: Some("Andy, day one".to_owned()),
    });
    project
        .add_segment(Segment::new(
            r"H:\masters\andy.mp4",
            "cold open",
            1_000,
            3_000,
        ))
        .expect("the source is in the project");
    project.segments[0].note = Some("starts on the laugh".to_owned());
    project.segments[0].tags = vec!["interview".to_owned(), "act one".to_owned()];
    project.segments[0].preset = Some("master".to_owned());
    project.segments[0].handle_frames = 12;
    project.output_dir = Some(MediaPath::new(r"H:\delivery\andy"));
    project
}

/// A batch outcome with one skipped and one failed job, for the run history tests.
fn outcome(project_id: ProjectId, now: i64) -> BatchOutcome {
    let first = SegmentId::new();
    let second = SegmentId::new();
    BatchOutcome {
        jobs: vec![
            (
                JobId(0),
                first,
                "cold open".to_owned(),
                JobStatus::Skipped {
                    reason: "the source is not on disk".to_owned(),
                },
            ),
            (
                JobId(1),
                second,
                "the bit about custody".to_owned(),
                JobStatus::Failed {
                    reason: "ffmpeg exited 1".to_owned(),
                    cancelled: false,
                },
            ),
        ],
        delivered_frames: 0,
        delivered_seconds: 0.0,
        elapsed_seconds: 12.5,
        cancelled: false,
        audit: Box::new(AuditManifest::new(
            project_id,
            "Fernando",
            "2.0.0-test",
            "STUDIO-01",
            now,
        )),
    }
}

// --- round trips -------------------------------------------------------------------------

#[test]
fn a_project_round_trips_with_sources_segments_presets_and_an_output_dir() {
    let store = memory();
    let original = project("Rollup", 1_700_000_000);
    store.save(&original).expect("saved");

    let loaded = store.load(original.id).expect("loaded");
    assert_eq!(loaded, original);
    assert_eq!(loaded.segments.len(), 1);
    assert_eq!(loaded.sources.len(), 1);
    assert_eq!(loaded.default_preset, "master");
    assert!(loaded.presets.contains_key("master"));
    assert_eq!(
        loaded.output_dir.as_ref().map(ToString::to_string),
        Some(r"H:\delivery\andy".to_owned())
    );
}

#[test]
fn a_project_with_no_sources_round_trips() {
    let store = memory();
    let original = Project::new("Empty", "Fernando", 42);
    store.save(&original).expect("saved");

    let loaded = store.load(original.id).expect("loaded");
    assert_eq!(loaded, original);
    assert!(loaded.sources.is_empty());
    assert!(loaded.segments.is_empty());
    // The preset library is still there: it is part of what a project *is*.
    assert!(loaded.presets.contains_key("master"));
}

#[test]
fn an_open_ended_segment_keeps_its_null_end_frame() {
    let store = memory();
    let mut original = project("Open", 10);
    let mut open = Segment::new(r"H:\masters\andy.mp4", "to the end", 5_000, 0);
    open.end_frame = None;
    let id = original.add_segment(open).expect("added");
    store.save(&original).expect("saved");

    let loaded = store.load(original.id).expect("loaded");
    let segment = loaded.segment(id).expect("the segment is there");
    assert!(segment.is_open_ended());
    assert_eq!(segment.end_frame, None);
    assert_eq!(loaded, original);
}

#[test]
fn unicode_and_quote_characters_survive_a_round_trip() {
    let store = memory();
    let mut original = Project::new("Röllup — «le montage»", "Fernandö", 1);
    original.upsert_source(SegmentSource::unprobed(
        "H:\\masters\\日本語 'quoted' \"and\" double.mp4",
    ));
    original
        .add_segment(Segment::new(
            "H:\\masters\\日本語 'quoted' \"and\" double.mp4",
            "il dit « bonjour » ; puis — rien",
            0,
            48,
        ))
        .expect("added");
    original.segments[0].note = Some("emoji: 🎬 and a semicolon;".to_owned());
    store.save(&original).expect("saved");

    let loaded = store.load(original.id).expect("loaded");
    assert_eq!(loaded, original);
    assert!(loaded.name.contains('—'));
    assert!(loaded.segments[0].note.as_deref().unwrap().contains('🎬'));
}

#[test]
fn a_windows_path_with_backslashes_survives_a_round_trip() {
    let store = memory();
    let mut original = Project::new(r"C:\work\rollup", "Fernando", 1);
    let path = r"\\nas-01\masters\2024\Andy Ross\master 01.mp4";
    original.upsert_source(SegmentSource::unprobed(path));
    original
        .add_segment(Segment::new(path, "one", 0, 100))
        .expect("added");
    store.save(&original).expect("saved");

    let loaded = store.load(original.id).expect("loaded");
    assert_eq!(loaded, original);
    assert_eq!(
        loaded.segments[0].source.as_path(),
        Path::new(r"\\nas-01\masters\2024\Andy Ross\master 01.mp4")
    );
}

#[test]
fn source_media_label_and_availability_round_trip() {
    let store = memory();
    let mut original = Project::new("Probed", "Fernando", 1);
    original.upsert_source(SegmentSource {
        path: MediaPath::new(r"H:\masters\andy.mp4"),
        media: Some(media(r"H:\masters\andy.mp4")),
        available: false,
        label: Some("offline since Tuesday".to_owned()),
    });
    store.save(&original).expect("saved");

    let loaded = store.load(original.id).expect("loaded");
    let source = loaded
        .source(&MediaPath::new(r"H:\masters\andy.mp4"))
        .expect("the source is there");
    assert!(!source.available);
    assert_eq!(source.label.as_deref(), Some("offline since Tuesday"));
    assert_eq!(source.media.as_ref().map(|m| m.frame_count), Some(216_000));
    assert_eq!(loaded, original);
}

#[test]
fn tags_notes_and_handles_round_trip() {
    let store = memory();
    let original = project("Tagged", 1);
    store.save(&original).expect("saved");

    let loaded = store.load(original.id).expect("loaded");
    assert_eq!(loaded.segments[0].tags, ["interview", "act one"]);
    assert_eq!(loaded.segments[0].handle_frames, 12);
    assert_eq!(
        loaded.segments[0].note.as_deref(),
        Some("starts on the laugh")
    );
    assert_eq!(loaded.segments[0].preset.as_deref(), Some("master"));
    assert!(loaded.segments[0].enabled);
}

#[test]
fn every_verification_policy_round_trips() {
    for policy in [
        VerifyPolicy::Off,
        VerifyPolicy::Standard,
        VerifyPolicy::Strict,
        VerifyPolicy::Forensic,
    ] {
        let store = memory();
        let mut original = Project::new("Verify", "Fernando", 1);
        original.verify = policy;
        store.save(&original).expect("saved");
        assert_eq!(store.load(original.id).expect("loaded").verify, policy);
    }
}

#[test]
fn a_segment_that_is_switched_off_stays_switched_off() {
    let store = memory();
    let mut original = project("Disabled", 1);
    original.segments[0].enabled = false;
    store.save(&original).expect("saved");

    let loaded = store.load(original.id).expect("loaded");
    assert!(!loaded.segments[0].enabled);
    assert!(loaded.runnable().is_empty());
}

// --- listing, deleting, replacing ---------------------------------------------------------

#[test]
fn list_is_ordered_by_updated_at_descending() {
    let store = memory();
    let mut old = Project::new("Oldest", "Fernando", 100);
    old.updated_at = 100;
    let mut middle = Project::new("Middle", "Fernando", 200);
    middle.updated_at = 200;
    let newest = Project::new("Newest", "Fernando", 300);
    store.save(&old).expect("saved");
    store.save(&middle).expect("saved");
    store.save(&newest).expect("saved");

    let listed = store.list().expect("listed");
    let names: Vec<&str> = listed.iter().map(|(_, name, _)| name.as_str()).collect();
    assert_eq!(names, ["Newest", "Middle", "Oldest"]);
    assert_eq!(listed[0].2, 300);
}

#[test]
fn delete_removes_sources_and_segments_by_cascade() {
    let store = memory();
    let original = project("Doomed", 1);
    store.save(&original).expect("saved");
    store.delete(original.id).expect("deleted");

    assert!(store.list().expect("listed").is_empty());
    match store.load(original.id) {
        Err(error) => assert!(error.contains("no project"), "{error}"),
        Ok(_) => panic!("the project should be gone"),
    }

    // The children are gone too, and the only thing that removed them was the cascade.
    let (sources, segments, presets): (i64, i64, i64) = store
        .with_connection(|conn| {
            let count = |table: &str| -> i64 {
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("a count")
            };
            (count("sources"), count("segments"), count("presets"))
        })
        .expect("the connection is available");
    assert_eq!((sources, segments, presets), (0, 0, 0));
}

#[test]
fn saving_replaces_rather_than_merges() {
    let store = memory();
    let mut original = project("Replaced", 1);
    store.save(&original).expect("saved");

    let removed = original.segments[0].id;
    original
        .add_segment(Segment::new(r"H:\masters\andy.mp4", "second", 4_000, 4_100))
        .expect("added");
    store.save(&original).expect("saved again");
    assert_eq!(store.load(original.id).expect("loaded").segments.len(), 2);

    original.remove_segment(removed).expect("removed");
    store.save(&original).expect("saved a third time");
    let loaded = store.load(original.id).expect("loaded");
    assert_eq!(loaded.segments.len(), 1);
    assert!(loaded.segment(removed).is_none());
}

#[test]
fn upserting_a_source_replaces_it_rather_than_duplicating_it() {
    let store = memory();
    let mut original = Project::new("Upsert", "Fernando", 1);
    let path = MediaPath::new(r"H:\masters\andy.mp4");
    original.upsert_source(SegmentSource::unprobed(path.clone()));
    original.upsert_source(SegmentSource {
        path: path.clone(),
        media: Some(media(r"H:\masters\andy.mp4")),
        available: true,
        label: Some("probed".to_owned()),
    });
    store.save(&original).expect("saved");

    let rows: i64 = store
        .with_connection(|conn| {
            conn.query_row("SELECT COUNT(*) FROM sources", [], |row| row.get(0))
                .expect("a count")
        })
        .expect("the connection is available");
    assert_eq!(rows, 1);
    assert_eq!(
        store
            .load(original.id)
            .expect("loaded")
            .source(&path)
            .and_then(|source| source.label.clone())
            .as_deref(),
        Some("probed")
    );
}

#[test]
fn loading_a_project_that_is_not_there_is_a_named_refusal() {
    let store = memory();
    let error = store
        .load(ProjectId::new())
        .expect_err("nothing is stored under that id");
    assert!(error.contains("no project"), "{error}");
    assert!(error.contains("in this store"), "{error}");
}

// --- the transaction, and the schema ------------------------------------------------------

#[test]
fn a_failed_save_leaves_the_previous_state_intact() {
    let store = memory();
    let original = project("Atomic", 1);
    store.save(&original).expect("saved");

    // A project whose two segments share one identity. The row is deleted and the first
    // insert succeeds, so the second is what aborts the transaction — which is exactly the
    // "half the segments are missing" state the transaction exists to prevent.
    let mut broken = original.clone();
    broken.name = "Atomic, renamed".to_owned();
    let duplicate = broken.segments[0].clone();
    broken.segments.push(duplicate);
    let error = store
        .save(&broken)
        .expect_err("the duplicate identity is refused");
    assert!(
        error.to_lowercase().contains("unique") || error.contains("constraint"),
        "{error}"
    );

    let loaded = store
        .load(original.id)
        .expect("the old state is still readable");
    assert_eq!(loaded, original, "the rollback restored every row");
    assert_eq!(loaded.name, "Atomic");
    assert_eq!(loaded.segments.len(), 1);
}

#[test]
fn a_fresh_database_reports_the_latest_schema_version() {
    let scratch = Scratch::new("version");
    let store = SqliteStore::open(scratch.join("db.sqlite")).expect("opened");
    assert_eq!(
        store.schema_version().expect("readable"),
        Some(schema::LATEST_VERSION)
    );
}

#[test]
fn opening_a_current_database_applies_nothing() {
    let scratch = Scratch::new("idempotent");
    let path = scratch.join("db.sqlite");

    let original = project("Idempotent", 1);
    {
        let store = SqliteStore::open(&path).expect("opened");
        store.save(&original).expect("saved");
    }

    // Open it again: the migration loop must find nothing to do, and must not damage a row.
    let reopened = SqliteStore::open(&path).expect("reopened");
    assert_eq!(
        reopened.schema_version().expect("readable"),
        Some(schema::LATEST_VERSION)
    );
    assert_eq!(reopened.load(original.id).expect("loaded"), original);

    // And a third time, so a migration that was not idempotent has two chances to show it.
    drop(reopened);
    let third = SqliteStore::open(&path).expect("reopened again");
    assert_eq!(third.load(original.id).expect("loaded"), original);
    assert_eq!(
        third
            .with_connection(|conn| {
                conn.query_row("SELECT COUNT(*) FROM schema_version", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("a count")
            })
            .expect("the connection is available"),
        1,
        "the version table holds exactly one row"
    );
}

#[test]
fn open_creates_the_parent_directory() {
    let scratch = Scratch::new("parents");
    let nested = scratch.join("a").join("b").join("db.sqlite");
    assert!(!nested.parent().expect("a parent").exists());
    let store = SqliteStore::open(&nested).expect("opened");
    assert!(nested.is_file());
    assert_eq!(store.list().expect("listed").len(), 0);
}

#[test]
fn journal_mode_is_wal_for_a_file_database() {
    let scratch = Scratch::new("wal");
    let store = SqliteStore::open(scratch.join("db.sqlite")).expect("opened");
    let mode: String = store
        .with_connection(|conn| {
            conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .expect("a mode")
        })
        .expect("the connection is available");
    assert_eq!(mode.to_lowercase(), "wal");
}

#[test]
fn foreign_keys_are_on() {
    let store = memory();
    let enabled: i64 = store
        .with_connection(|conn| {
            conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))
                .expect("a flag")
        })
        .expect("the connection is available");
    assert_eq!(
        enabled, 1,
        "without this every ON DELETE CASCADE is a comment"
    );
}

#[test]
fn the_two_declared_indexes_exist_and_no_others() {
    let store = memory();
    let indexes: Vec<String> = store
        .with_connection(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'index' AND name NOT LIKE \
                     'sqlite_autoindex%' ORDER BY name",
                )
                .expect("a statement");
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .expect("rows");
            rows.map(|row| row.expect("a name")).collect::<Vec<_>>()
        })
        .expect("the connection is available");
    assert_eq!(indexes, ["runs_by_project", "segments_by_project"]);
}

// --- runs ---------------------------------------------------------------------------------

#[test]
fn a_run_round_trips_including_the_manifest_json() {
    let store = memory();
    let original = project("Ran", 1);
    store.save(&original).expect("saved");

    let batch = outcome(original.id, 1_700_000_000);
    let id = store
        .record_run(original.id, "Fernando", 1_700_000_000, &batch)
        .expect("recorded");

    let manifest = store
        .load_run(id)
        .expect("readable")
        .expect("the run is there");
    let parsed = AuditManifest::from_json(&manifest).expect("the manifest parses");
    assert_eq!(parsed.project_id, original.id);
    assert_eq!(parsed.machine, "STUDIO-01");
    assert_eq!(parsed.app_version, "2.0.0-test");

    let items = store.run_items(id).expect("items");
    assert_eq!(items.len(), 2);
    assert!(items[0].contains("cold open"));
    assert!(items[1].contains("ffmpeg exited 1"));
}

#[test]
fn a_run_an_unknown_id_names_is_absent_rather_than_an_error() {
    let store = memory();
    assert_eq!(store.load_run(Uuid::now_v7()).expect("readable"), None);
}

#[test]
fn run_counts_are_derived_from_the_items() {
    let store = memory();
    let original = project("Counted", 1);
    store.save(&original).expect("saved");
    store
        .record_run(
            original.id,
            "Fernando",
            1_700_000_000,
            &outcome(original.id, 5),
        )
        .expect("recorded");

    let runs = store.list_runs(original.id).expect("listed");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].succeeded, 0);
    assert_eq!(runs[0].unverified, 0);
    assert_eq!(runs[0].failed, 1);
    assert_eq!(runs[0].skipped, 1);
    assert_eq!(runs[0].status, "failed");
    assert_eq!(runs[0].actor, "Fernando");
    assert_eq!(runs[0].finished_at, Some(1_700_000_000 + 12));
    assert!(!runs[0].started_utc().is_empty());
    assert!(!runs[0].finished_utc().is_empty());
}

#[test]
fn runs_are_listed_newest_first() {
    let store = memory();
    let original = project("History", 1);
    store.save(&original).expect("saved");
    let first = store
        .record_run(original.id, "Fernando", 1_000, &outcome(original.id, 1_000))
        .expect("recorded");
    let second = store
        .record_run(original.id, "Fernando", 2_000, &outcome(original.id, 2_000))
        .expect("recorded");

    let runs = store.list_runs(original.id).expect("listed");
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].id, second);
    assert_eq!(runs[1].id, first);
    assert!(runs[0].started_at > runs[1].started_at);
}

#[test]
fn a_clean_run_is_recorded_as_clean() {
    let store = memory();
    let original = project("Clean", 1);
    store.save(&original).expect("saved");
    let mut batch = outcome(original.id, 7);
    batch.jobs.clear();
    let id = store
        .record_run(original.id, "Fernando", 7, &batch)
        .expect("recorded");
    assert!(store.load_run(id).expect("readable").is_some());
    assert_eq!(
        store.list_runs(original.id).expect("listed")[0].status,
        "clean"
    );
}

#[test]
fn a_cancelled_run_is_recorded_as_cancelled() {
    let store = memory();
    let original = project("Cancelled", 1);
    store.save(&original).expect("saved");
    let mut batch = outcome(original.id, 7);
    batch.cancelled = true;
    store
        .record_run(original.id, "Fernando", 7, &batch)
        .expect("recorded");
    assert_eq!(
        store.list_runs(original.id).expect("listed")[0].status,
        "cancelled"
    );
}

#[test]
fn a_run_for_a_project_that_is_not_there_is_refused() {
    let store = memory();
    let batch = outcome(ProjectId::new(), 1);
    let error = store
        .record_run(ProjectId::new(), "Fernando", 1, &batch)
        .expect_err("the foreign key refuses it");
    assert!(error.to_lowercase().contains("foreign key"), "{error}");
}

#[test]
fn deleting_a_project_takes_its_runs_with_it() {
    let store = memory();
    let original = project("RunsDoomed", 1);
    store.save(&original).expect("saved");
    store
        .record_run(original.id, "Fernando", 1, &outcome(original.id, 1))
        .expect("recorded");
    store.delete(original.id).expect("deleted");

    let (runs, items): (i64, i64) = store
        .with_connection(|conn| {
            let count = |table: &str| -> i64 {
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("a count")
            };
            (count("runs"), count("runs_items"))
        })
        .expect("the connection is available");
    assert_eq!((runs, items), (0, 0));
    assert!(store.list_runs(original.id).expect("listed").is_empty());
}

// --- the document -------------------------------------------------------------------------

#[test]
fn the_document_round_trips_to_the_same_project() {
    let original = project("Documented", 1_700_000_000);
    let text = document::to_json(&original).expect("serialised");
    assert!(text.contains('\n'), "the readable form is indented");
    assert!(text.contains("cold open"));
    let parsed = document::from_json(&text).expect("parsed");
    assert_eq!(parsed, original);
}

#[test]
fn the_compact_document_is_one_line_and_parses() {
    let original = project("Compact", 1);
    let text = document::to_compact_json(&original).expect("serialised");
    assert!(!text.contains('\n'));
    assert_eq!(document::from_json(&text).expect("parsed"), original);
}

#[test]
fn a_document_that_is_not_a_project_is_refused_with_a_position() {
    let error = document::from_json("{\"nope\": true}").expect_err("refused");
    let message = error.to_string();
    assert!(
        message.contains("line") || message.contains("missing field"),
        "{message}"
    );
}

#[test]
fn a_document_carries_the_unicode_a_project_holds() {
    let mut original = Project::new("Ünïcode", "Fernando", 1);
    original.upsert_source(SegmentSource::unprobed(r"H:\masters\日本語.mp4"));
    let text = document::to_json(&original).expect("serialised");
    // `serde_json` escapes nothing here: the characters are valid UTF-8 and are written as
    // themselves, which is what makes the file readable by a person.
    assert!(text.contains("日本語"), "{text}");
    assert_eq!(document::from_json(&text).expect("parsed"), original);
}

// --- odds and ends ------------------------------------------------------------------------

#[test]
fn the_default_store_path_is_below_a_data_directory() {
    let path = trimmer_store::default_store_path().expect("this machine has a data directory");
    assert!(path.to_string_lossy().contains("TheTrimmer"), "{path:?}");
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("sqlite"));
}

#[test]
fn a_store_error_prints_a_sentence_a_person_can_act_on() {
    let error = StoreError::Schema {
        version: 3,
        reason: "no such table".to_owned(),
    };
    let message = error.to_string();
    assert!(message.contains("version 3"), "{message}");
    assert!(message.contains("no such table"), "{message}");
}

#[test]
fn a_house_preset_survives_a_round_trip_under_its_own_name() {
    // The reason `presets` is a table. A project whose edited preset vanished on the way out
    // would export a document that delivers something other than what the studio asked for.
    let mut presets = BTreeMap::new();
    let mut house = trimmer_core::delivery::standard_preset("master").expect("the standard master");
    house.description = "the house look".to_owned();
    presets.insert("house".to_owned(), house.clone());

    let mut original = Project::new("Presets", "Fernando", 1);
    original.presets = presets;
    original.default_preset = "house".to_owned();

    let store = memory();
    store.save(&original).expect("saved");
    let loaded = store.load(original.id).expect("loaded");
    assert_eq!(loaded.presets.len(), 1);
    assert_eq!(
        loaded
            .presets
            .get("house")
            .map(|preset| preset.description.clone()),
        Some("the house look".to_owned())
    );
    assert_eq!(loaded.default_preset, "house");
    assert_eq!(loaded, original);
}

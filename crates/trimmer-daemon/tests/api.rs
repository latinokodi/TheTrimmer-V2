//! The API's suite: the token, every route, the status codes, and the shapes a client depends on.
//!
//! The router is driven with `tower::ServiceExt::oneshot`, so no socket is opened and the whole
//! suite runs in-process. That matters for more than speed: a test that binds a port collides
//! with a parallel run of itself, and a port is the one thing in this crate a test cannot own.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;
use trimmer_core::{Project, ProjectId, SegmentId};
use trimmer_daemon::{
    constant_time_eq, openapi, router, DaemonConfig, DaemonState, RunRecord, RunState, MAX_RUNS,
};
use trimmer_media::CancelFlag;
use trimmer_store::SqliteStore;
use uuid::Uuid;

/// The token every test uses. Long enough to pass `validate`, and obviously not a secret.
const TOKEN: &str = "test-token-0123456789";

/// A scratch directory that removes itself.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "trimmer-daemon-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory can be made");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A daemon in a scratch directory, with its router ready to drive.
struct Harness {
    app: Router,
    state: Arc<DaemonState>,
    token: String,
    _scratch: Scratch,
}

impl Harness {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let config = DaemonConfig::new(8787, TOKEN, scratch.join("projects.sqlite"));
        config.validate().expect("the test configuration is valid");
        let state = DaemonState::new(config).expect("the store opens");
        Self {
            app: router(Arc::clone(&state)),
            state,
            token: TOKEN.to_owned(),
            _scratch: scratch,
        }
    }

    /// A request with the right token.
    async fn ask(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        send(&self.app, method, uri, Some(&self.token), body).await
    }
}

/// Drive one request through the router.
async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .expect("a request"),
        None => builder.body(Body::empty()).expect("a request"),
    };
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("a body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Create a project and return its id.
async fn make_project(harness: &Harness, name: &str) -> String {
    let (status, body) = harness
        .ask(
            Method::POST,
            "/v1/projects",
            Some(json!({ "name": name, "created_by": "Fernando" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().expect("an id").to_owned()
}

// --- the token ----------------------------------------------------------------------------

#[test]
fn a_token_shorter_than_the_minimum_is_refused() {
    let mut config = DaemonConfig::new(8787, "short", PathBuf::from("x.sqlite"));
    let error = config.validate().expect_err("refused");
    assert!(error.contains("16"), "{error}");

    config.token = "sixteen-chars-ok".to_owned();
    assert!(config.validate().is_ok());
}

#[test]
fn a_token_with_whitespace_is_refused() {
    let config = DaemonConfig::new(8787, "a token with spaces", PathBuf::from("x.sqlite"));
    let error = config.validate().expect_err("refused");
    assert!(error.contains("whitespace"), "{error}");
}

#[test]
fn a_bind_address_that_is_not_loopback_is_refused() {
    for address in ["0.0.0.0", "192.168.1.10", "::"] {
        let mut config = DaemonConfig::new(8787, TOKEN, PathBuf::from("x.sqlite"));
        config.bind = address.to_owned();
        let error = config.validate().expect_err("refused");
        assert!(
            error.contains("local control surface"),
            "{address}: {error}"
        );
    }
    for address in ["127.0.0.1", "::1"] {
        let mut config = DaemonConfig::new(8787, TOKEN, PathBuf::from("x.sqlite"));
        config.bind = address.to_owned();
        assert!(config.validate().is_ok(), "{address} is loopback");
    }
}

#[test]
fn serve_refuses_to_start_with_a_short_token() {
    let scratch = Scratch::new("serve-token");
    let config = DaemonConfig::new(8787, "tiny", scratch.join("db.sqlite"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let error = runtime
        .block_on(trimmer_daemon::serve(config))
        .expect_err("refused");
    assert!(error.to_string().contains("16"), "{error}");
}

#[test]
fn the_constant_time_comparison_agrees_with_equality() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(constant_time_eq(b"", b""));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    assert!(!constant_time_eq(b"ab", b"abc"));
    assert!(!constant_time_eq(b"", b"a"));
}

#[tokio::test]
async fn health_without_a_token_is_401() {
    let harness = Harness::new("no-token");
    let (status, body) = send(&harness.app, Method::GET, "/v1/health", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
    assert!(body["detail"].is_string());
}

#[tokio::test]
async fn health_with_a_wrong_token_is_401() {
    let harness = Harness::new("wrong-token");
    let (status, body) = send(
        &harness.app,
        Method::GET,
        "/v1/health",
        Some("not-the-token-000000"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
}

#[tokio::test]
async fn health_with_the_right_token_is_200_and_a_json_body() {
    let harness = Harness::new("health");
    let (status, body) = harness.ask(Method::GET, "/v1/health", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "thetrimmer-daemon");
    assert!(body["version"].is_string());
    assert!(body["ffmpeg"].is_boolean());
    assert!(body["licensed"].is_boolean());
}

#[tokio::test]
async fn an_openapi_request_without_a_token_is_401_too() {
    // The document is not secret, but an unauthenticated route is a route that was forgotten.
    let harness = Harness::new("openapi-auth");
    let (status, _) = send(&harness.app, Method::GET, "/v1/openapi.json", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// --- projects -----------------------------------------------------------------------------

#[tokio::test]
async fn a_project_is_created_listed_read_and_deleted() {
    let harness = Harness::new("project-crud");
    let id = make_project(&harness, "Rollup").await;

    let (status, listed) = harness.ask(Method::GET, "/v1/projects", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed.as_array().expect("a list").len(), 1);
    assert_eq!(listed[0]["name"], "Rollup");
    assert_eq!(listed[0]["id"], id);

    let (status, read) = harness
        .ask(Method::GET, &format!("/v1/projects/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["name"], "Rollup");
    assert_eq!(read["createdBy"], "Fernando");
    assert_eq!(read["defaultPreset"], "master");
    assert!(read["presets"].is_object());

    let (status, deleted) = harness
        .ask(Method::DELETE, &format!("/v1/projects/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["deleted"], id);

    let (status, _) = harness
        .ask(Method::GET, &format!("/v1/projects/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, listed) = harness.ask(Method::GET, "/v1/projects", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed.as_array().expect("a list").is_empty());
}

#[tokio::test]
async fn a_missing_project_is_404_with_the_error_shape() {
    let harness = Harness::new("missing-project");
    let (status, body) = harness
        .ask(Method::GET, &format!("/v1/projects/{}", Uuid::now_v7()), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "notFound");
    assert!(body["detail"].as_str().expect("a detail").contains("project"));
}

#[tokio::test]
async fn an_id_that_is_not_a_uuid_is_a_bad_request() {
    let harness = Harness::new("bad-id");
    let (status, body) = harness.ask(Method::GET, "/v1/projects/not-a-uuid", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "badId");
}

#[tokio::test]
async fn a_malformed_body_is_400_with_the_error_shape() {
    let harness = Harness::new("malformed-body");
    let (status, body) = harness
        .ask(Method::POST, "/v1/projects", Some(json!({ "name": 12 })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "malformed");
    assert!(body["detail"].as_str().expect("a detail").contains("name"));

    let (status, body) = harness.ask(Method::POST, "/v1/projects", Some(json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "malformed");
}

// --- sources and segments ------------------------------------------------------------------

#[tokio::test]
async fn a_source_that_is_not_on_disk_is_still_recorded() {
    let harness = Harness::new("source-missing");
    let id = make_project(&harness, "Offline").await;
    let missing = r"H:\nowhere\does-not-exist.mp4";

    let (status, body) = harness
        .ask(
            Method::POST,
            &format!("/v1/projects/{id}/sources"),
            Some(json!({ "path": missing })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["available"], false);
    assert_eq!(body["probed"], false);
    assert_eq!(body["path"], missing);

    let (_, read) = harness
        .ask(Method::GET, &format!("/v1/projects/{id}"), None)
        .await;
    assert!(
        read["sources"][missing].is_object(),
        "the source is in the project even though the file is not: {read}"
    );
}

#[tokio::test]
async fn adding_a_source_to_a_missing_project_is_404() {
    let harness = Harness::new("source-missing-project");
    let (status, _) = harness
        .ask(
            Method::POST,
            &format!("/v1/projects/{}/sources", Uuid::now_v7()),
            Some(json!({ "path": "x.mp4" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_segment_with_a_bad_range_is_400() {
    let harness = Harness::new("bad-range");
    let id = make_project(&harness, "Ranges").await;
    let source = r"H:\masters\andy.mp4";
    harness
        .ask(
            Method::POST,
            &format!("/v1/projects/{id}/sources"),
            Some(json!({ "path": source })),
        )
        .await;

    let (status, body) = harness
        .ask(
            Method::POST,
            &format!("/v1/projects/{id}/segments"),
            Some(json!({
                "source": source,
                "name": "backwards",
                "start_frame": 500,
                "end_frame": 100,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "badRange");
}

#[tokio::test]
async fn a_segment_for_a_source_that_is_not_in_the_project_is_400() {
    let harness = Harness::new("unknown-source");
    let id = make_project(&harness, "Unknown").await;
    let (status, body) = harness
        .ask(
            Method::POST,
            &format!("/v1/projects/{id}/segments"),
            Some(json!({
                "source": r"H:\masters\nowhere.mp4",
                "name": "x",
                "start_frame": 0,
                "end_frame": 10,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "unknownSource");
}

#[tokio::test]
async fn segments_are_added_listed_and_deleted() {
    let harness = Harness::new("segments");
    let id = make_project(&harness, "Segments").await;
    let source = r"H:\masters\andy.mp4";
    harness
        .ask(
            Method::POST,
            &format!("/v1/projects/{id}/sources"),
            Some(json!({ "path": source })),
        )
        .await;

    let mut created = Vec::new();
    for (name, start, end) in [("one", 0_i64, 48_i64), ("two", 100, 200), ("three", 300, 400)] {
        let (status, body) = harness
            .ask(
                Method::POST,
                &format!("/v1/projects/{id}/segments"),
                Some(json!({
                    "source": source,
                    "name": name,
                    "start_frame": start,
                    "end_frame": end,
                    "handle_frames": 4,
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["name"], name);
        assert_eq!(body["handleFrames"], 4);
        assert_eq!(body["enabled"], true);
        created.push(body["id"].as_str().expect("an id").to_owned());
    }

    let (status, listed) = harness
        .ask(Method::GET, &format!("/v1/projects/{id}/segments"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed.as_array().expect("a list").len(), 3);

    let (status, _) = harness
        .ask(
            Method::DELETE,
            &format!("/v1/projects/{id}/segments/{}", created[1]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (_, listed) = harness
        .ask(Method::GET, &format!("/v1/projects/{id}/segments"), None)
        .await;
    let names: Vec<&str> = listed
        .as_array()
        .expect("a list")
        .iter()
        .map(|segment| segment["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["one", "three"]);
}

#[tokio::test]
async fn deleting_a_segment_that_is_not_there_is_404() {
    let harness = Harness::new("no-segment");
    let id = make_project(&harness, "NoSegment").await;
    let (status, body) = harness
        .ask(
            Method::DELETE,
            &format!("/v1/projects/{id}/segments/{}", Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "notFound");
}

// --- preview and runs ----------------------------------------------------------------------

#[tokio::test]
async fn preview_of_an_empty_project_is_an_empty_list() {
    let harness = Harness::new("preview-empty");
    let id = make_project(&harness, "Empty").await;
    let (status, body) = harness
        .ask(Method::GET, &format!("/v1/projects/{id}/preview"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn preview_of_a_missing_project_is_404() {
    let harness = Harness::new("preview-missing");
    let (status, _) = harness
        .ask(
            Method::GET,
            &format!("/v1/projects/{}/preview", Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_on_a_project_with_no_segments_finishes_immediately() {
    let harness = Harness::new("run-empty");
    let id = make_project(&harness, "Nothing to do").await;

    let (status, body) = harness
        .ask(Method::POST, &format!("/v1/projects/{id}/run"), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().expect("a run id").to_owned();
    assert_eq!(body["state"], "queued");

    let mut record = Value::Null;
    for _ in 0..200 {
        let (status, body) = harness
            .ask(Method::GET, &format!("/v1/runs/{run_id}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let state = body["state"].as_str().unwrap_or_default().to_owned();
        record = body;
        if state == "finished" || state == "cancelled" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(record["state"], "finished", "{record}");
    assert!(record["finishedAt"].is_number(), "{record}");
    if record["outcome"].is_object() {
        assert_eq!(record["outcome"]["succeeded"], 0);
        assert_eq!(record["outcome"]["failed"], 0);
        assert_eq!(record["outcome"]["items"], json!([]));
        assert!(record["outcome"]["report"].is_string());
    } else {
        // Only reachable on a machine with no ffmpeg, and it says so.
        assert!(record["error"].is_string(), "{record}");
    }
}

#[tokio::test]
async fn starting_a_run_while_one_is_going_is_409() {
    let harness = Harness::new("run-conflict");
    let id = make_project(&harness, "Busy").await;
    let project_id = ProjectId(Uuid::parse_str(&id).expect("a uuid"));

    harness
        .state
        .insert_run(RunRecord {
            id: Uuid::now_v7(),
            project_id,
            state: RunState::Running,
            started_at: 1,
            finished_at: None,
            total: 1,
            outcome: None,
            error: None,
            cancel: CancelFlag::new(),
        })
        .expect("there is room");

    let (status, body) = harness
        .ask(Method::POST, &format!("/v1/projects/{id}/run"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "runInProgress");
}

#[tokio::test]
async fn an_unknown_run_is_404() {
    let harness = Harness::new("run-unknown");
    let (status, body) = harness
        .ask(Method::GET, &format!("/v1/runs/{}", Uuid::now_v7()), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "notFound");
}

#[tokio::test]
async fn cancelling_an_unknown_run_is_404() {
    let harness = Harness::new("cancel-unknown");
    let (status, body) = harness
        .ask(
            Method::POST,
            &format!("/v1/runs/{}/cancel", Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "notFound");
}

#[tokio::test]
async fn cancelling_a_finished_run_is_404_because_there_is_nothing_to_stop() {
    let harness = Harness::new("cancel-finished");
    let id = Uuid::now_v7();
    harness
        .state
        .insert_run(RunRecord {
            id,
            project_id: ProjectId::new(),
            state: RunState::Finished,
            started_at: 1,
            finished_at: Some(2),
            total: 0,
            outcome: None,
            error: None,
            cancel: CancelFlag::new(),
        })
        .expect("there is room");

    let (status, _) = harness
        .ask(Method::POST, &format!("/v1/runs/{id}/cancel"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn the_registry_is_bounded_and_evicts_the_oldest_finished_run() {
    let scratch = Scratch::new("registry-bound");
    let config = DaemonConfig::new(8787, TOKEN, scratch.join("db.sqlite"));
    let state = DaemonState::new(config).expect("the store opens");

    let record = |index: usize, state_word: RunState| RunRecord {
        id: Uuid::from_u128(index as u128 + 1),
        project_id: ProjectId::new(),
        state: state_word,
        started_at: index as i64,
        finished_at: Some(index as i64),
        total: 0,
        outcome: None,
        error: None,
        cancel: CancelFlag::new(),
    };

    for index in 0..MAX_RUNS {
        state
            .insert_run(record(index, RunState::Finished))
            .expect("there is room");
    }
    assert_eq!(state.run_count(), MAX_RUNS);

    // One more: the oldest finished run goes, and the newest stays.
    state
        .insert_run(record(MAX_RUNS, RunState::Finished))
        .expect("the oldest finished run made room");
    assert_eq!(state.run_count(), MAX_RUNS);
    assert!(state.run(Uuid::from_u128(1)).is_none(), "the oldest went");
    assert!(
        state.run(Uuid::from_u128(MAX_RUNS as u128 + 1)).is_some(),
        "the newest stayed"
    );
}

#[test]
fn the_registry_refuses_a_new_run_when_every_record_is_still_going() {
    let scratch = Scratch::new("registry-full");
    let config = DaemonConfig::new(8787, TOKEN, scratch.join("db.sqlite"));
    let state = DaemonState::new(config).expect("the store opens");

    for index in 0..MAX_RUNS {
        state
            .insert_run(RunRecord {
                id: Uuid::from_u128(index as u128 + 1),
                project_id: ProjectId::new(),
                state: RunState::Running,
                started_at: index as i64,
                finished_at: None,
                total: 0,
                outcome: None,
                error: None,
                cancel: CancelFlag::new(),
            })
            .expect("there is room");
    }
    let error = state
        .insert_run(RunRecord {
            id: Uuid::from_u128(9_999),
            project_id: ProjectId::new(),
            state: RunState::Queued,
            started_at: 0,
            finished_at: None,
            total: 0,
            outcome: None,
            error: None,
            cancel: CancelFlag::new(),
        })
        .expect_err("the registry will not discard work in flight");
    assert!(error.contains("in flight"), "{error}");
}

// --- transcripts, capabilities, openapi ----------------------------------------------------

#[tokio::test]
async fn a_transcript_search_for_a_video_with_no_captions_is_200_with_an_empty_list() {
    let harness = Harness::new("transcript-none");
    let (status, body) = harness
        .ask(
            Method::POST,
            "/v1/transcripts/search",
            Some(json!({
                "video": r"H:\masters\andy.mp4",
                "phrase": "custody",
                "limit": 5,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["hits"], json!([]));
    assert!(body["reason"].is_string(), "{body}");
    assert_eq!(body["phrase"], "custody");
}

#[tokio::test]
async fn a_transcript_search_with_a_malformed_body_is_400() {
    let harness = Harness::new("transcript-bad");
    let (status, body) = harness
        .ask(Method::POST, "/v1/transcripts/search", Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "malformed");
}

#[tokio::test]
async fn capabilities_answers_with_the_edition_table_and_the_ffmpeg_report() {
    let harness = Harness::new("capabilities");
    let (status, body) = harness.ask(Method::GET, "/v1/capabilities", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["version"].is_string());
    assert!(body["licence"]["present"].is_boolean());
    let features = body["features"].as_array().expect("a table");
    assert!(!features.is_empty());
    assert!(features.iter().any(|entry| entry["name"] == "api"));
}

#[tokio::test]
async fn the_openapi_document_parses_and_names_the_health_route() {
    let harness = Harness::new("openapi");
    let (status, document) = harness.ask(Method::GET, "/v1/openapi.json", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(document["openapi"], "3.1.0");
    assert!(document["info"]["title"].is_string());
    let text = document.to_string();
    assert!(text.contains("/v1/health"), "{text}");
    assert!(text.contains("bearer"), "{text}");
}

#[test]
fn the_openapi_paths_are_exactly_the_constants_the_tests_check() {
    let document = openapi::openapi_document();
    let mut listed: Vec<&str> = document["paths"]
        .as_object()
        .expect("the document lists paths")
        .keys()
        .map(String::as_str)
        .collect();
    listed.sort_unstable();
    let mut expected: Vec<&str> = openapi::PATHS.to_vec();
    expected.sort_unstable();
    assert_eq!(listed, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_documented_path_is_a_route_the_router_knows() {
    let harness = Harness::new("routes");
    let project = make_project(&harness, "Routes").await;
    let segment = SegmentId::new().to_string();
    let run = Uuid::now_v7().to_string();

    for path in openapi::PATHS {
        let concrete = path
            .replace("{id}", &project)
            .replace("{segment_id}", &segment)
            .replace("{run_id}", &run);
        let (status, body) = harness.ask(Method::GET, &concrete, None).await;
        // An unrouted path is a 404 with an empty body; every route this API registers answers
        // either with JSON or with a 405 for the wrong method.
        assert!(
            !(status == StatusCode::NOT_FOUND && body.is_null()),
            "{path} is documented but not routed"
        );
    }
}

#[tokio::test]
async fn an_unknown_route_is_404_with_an_empty_body() {
    let harness = Harness::new("unknown-route");
    let (status, body) = harness.ask(Method::GET, "/v1/nonsense", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Not one of this API's error bodies: there is no handler to write one.
    assert!(body.is_null(), "{body}");
}

#[test]
fn a_daemon_can_be_built_over_a_store_that_already_has_projects() {
    let scratch = Scratch::new("reopen");
    let store = SqliteStore::open(scratch.join("db.sqlite")).expect("opened");
    let project = Project::new("Kept", "Fernando", 1);
    trimmer_app::ProjectStore::save(&store, &project).expect("saved");
    drop(store);

    let state = DaemonState::new(DaemonConfig::new(
        8787,
        TOKEN,
        scratch.join("db.sqlite"),
    ))
    .expect("the daemon opens the same store");
    let listed = trimmer_app::ProjectStore::list(state.store.as_ref()).expect("listed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, project.id);
}

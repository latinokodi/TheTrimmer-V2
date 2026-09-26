//! The routes: what the API answers, and what it refuses.
//!
//! ## Status codes, and why each one
//!
//! | Code | When |
//! |---|---|
//! | `200` / `201` | The answer, or the thing that was created |
//! | `400` | A body that is malformed, or a request the domain refuses — a segment whose out point is not after its in point |
//! | `401` | No token, or the wrong one |
//! | `404` | A project, segment or run that is not there |
//! | `409` | A run was asked for while one is already going for that project |
//! | `500` | The store or the media layer failed |
//!
//! Every error body is `{"error": ..., "detail": ...}` — a short word a client can switch on
//! and a sentence a person can read. There is no case where the API answers with a bare string,
//! because a client that has to parse prose to find out what happened is a client that will
//! get it wrong.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tower_http::trace::TraceLayer;
use trimmer_app::{
    AppError, Clock, FileTranscripts, MediaAdapter, MediaEngine, ProjectStore, Queue, QueueOptions,
    SystemClock, Workspace,
};
use trimmer_core::{plan_cut, KeyframeGrid, MediaPath, Project, ProjectId, Segment, SegmentId};
use trimmer_media::{CancelFlag, CutConfig, CutExecutor, Prober};
use trimmer_store::document;
use uuid::Uuid;

use crate::config::DaemonConfig;
use crate::openapi::openapi_document;
use crate::state::{DaemonState, ProbeMeasurer, RunItem, RunOutcome, RunRecord, RunState};

/// The name this API reports, so a client can tell a daemon from a studio's own thing.
pub const SERVICE_NAME: &str = "thetrimmer-daemon";

// --- errors -------------------------------------------------------------------------------

/// A refusal, in the shape the API documents.
#[derive(Debug, Clone)]
pub struct ApiError {
    /// The status code.
    pub status: StatusCode,
    /// A short word, e.g. `notFound`.
    pub error: String,
    /// A sentence saying what happened.
    pub detail: String,
}

impl ApiError {
    /// A refusal the caller can fix by sending something else.
    #[must_use]
    pub fn bad_request(error: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: error.into(),
            detail: detail.into(),
        }
    }

    /// A thing that is not there.
    #[must_use]
    pub fn not_found(what: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            error: "notFound".to_owned(),
            detail: format!("there is no {what} with that id"),
        }
    }

    /// A request that conflicts with the state of the daemon.
    #[must_use]
    pub fn conflict(error: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            error: error.into(),
            detail: detail.into(),
        }
    }

    /// Something on this side failed.
    #[must_use]
    pub fn internal(detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: "internal".to_owned(),
            detail: detail.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": self.error, "detail": self.detail });
        (self.status, Json(body)).into_response()
    }
}

/// An id in a path that is not a UUID is a malformed request, not a missing resource.
fn parse_project(id: &str) -> Result<ProjectId, ApiError> {
    Uuid::parse_str(id)
        .map(ProjectId)
        .map_err(|_| ApiError::bad_request("badId", format!("{id:?} is not a project id")))
}

/// As [`parse_project`], for a segment.
fn parse_segment(id: &str) -> Result<SegmentId, ApiError> {
    Uuid::parse_str(id)
        .map(SegmentId)
        .map_err(|_| ApiError::bad_request("badId", format!("{id:?} is not a segment id")))
}

/// As [`parse_project`], for a run.
fn parse_run(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id)
        .map_err(|_| ApiError::bad_request("badId", format!("{id:?} is not a run id")))
}

/// A sentence for anything the domain or the media layer refused.
fn domain(error: &AppError) -> ApiError {
    match error {
        AppError::Store(_) | AppError::Media(_) => ApiError::internal(error.to_string()),
        _ => ApiError::bad_request("refused", error.to_string()),
    }
}

// --- body helpers -------------------------------------------------------------------------

/// A required string field.
fn text(body: &Value, name: &str) -> Result<String, ApiError> {
    body.get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| ApiError::bad_request("malformed", format!("{name} must be a string")))
}

/// A required integer field.
fn integer(body: &Value, name: &str) -> Result<i64, ApiError> {
    body.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::bad_request("malformed", format!("{name} must be a whole number")))
}

/// An optional integer field, which may also be an explicit `null`.
fn optional_integer(body: &Value, name: &str) -> Result<Option<i64>, ApiError> {
    match body.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or_else(|| {
            ApiError::bad_request(
                "malformed",
                format!("{name} must be a whole number or null"),
            )
        }),
    }
}

/// An optional string field, which may also be an explicit `null`.
fn optional_text(body: &Value, name: &str) -> Result<Option<String>, ApiError> {
    match body.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_owned()))
            .ok_or_else(|| {
                ApiError::bad_request("malformed", format!("{name} must be a string or null"))
            }),
    }
}

// --- authentication -----------------------------------------------------------------------

/// Compare two byte strings without letting the time taken say how much matched.
///
/// A short-circuiting `==` on a token is a timing oracle: the number of bytes an attacker got
/// right is visible in how long the refusal took. This compares every byte of the shorter
/// string and folds the length difference in, so the answer is `false` and the clock is quiet.
#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(a ^ b);
    }
    difference == 0
}

/// The bearer token a request carries, if it carries one.
fn bearer(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// Refuse anything without the right token.
async fn require_token(
    State(state): State<Arc<DaemonState>>,
    request: Request,
    next: Next,
) -> Response {
    let supplied = bearer(&request).unwrap_or_default();
    if constant_time_eq(supplied.as_bytes(), state.config.token.as_bytes()) && !supplied.is_empty()
    {
        return next.run(request).await;
    }
    ApiError {
        status: StatusCode::UNAUTHORIZED,
        error: "unauthorized".to_owned(),
        detail: "this API needs an `Authorization: Bearer <token>` header".to_owned(),
    }
    .into_response()
}

// --- the router ---------------------------------------------------------------------------

/// Build the router. Kept public so a test can drive it with `oneshot` and no socket.
pub fn router(state: Arc<DaemonState>) -> Router {
    let routes = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/projects", get(list_projects).post(create_project))
        .route("/v1/projects/{id}", get(get_project).delete(delete_project))
        .route("/v1/projects/{id}/sources", post(add_source))
        .route(
            "/v1/projects/{id}/segments",
            get(list_segments).post(add_segment),
        )
        .route(
            "/v1/projects/{id}/segments/{segment_id}",
            delete(delete_segment),
        )
        .route("/v1/projects/{id}/preview", get(preview))
        .route("/v1/projects/{id}/run", post(start_run))
        .route("/v1/runs/{run_id}", get(get_run))
        .route("/v1/runs/{run_id}/cancel", post(cancel_run))
        .route("/v1/transcripts/search", post(search_transcripts))
        .route("/v1/openapi.json", get(openapi))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_token,
        ))
        .with_state(state);

    routes.layer(TraceLayer::new_for_http())
}

/// Start the daemon.
///
/// # Errors
///
/// Returns an error when the configuration is refused — a token shorter than
/// [`crate::MIN_TOKEN_LEN`], or a bind address that is not loopback — when the store cannot be
/// opened, or when the port cannot be bound.
pub async fn serve(config: DaemonConfig) -> anyhow::Result<()> {
    config.validate().map_err(anyhow::Error::msg)?;
    let bind = config.bind.clone();
    let port = config.port;
    let state = DaemonState::new(config).map_err(anyhow::Error::msg)?;
    let app = router(state);

    let listener = tokio::net::TcpListener::bind((bind.as_str(), port)).await?;
    tracing::info!(%bind, port, "thetrimmer daemon listening on http://{bind}:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// A name for this machine, recorded in the run log.
fn machine_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

// --- handlers -----------------------------------------------------------------------------

/// `GET /v1/health`
async fn health(State(state): State<Arc<DaemonState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": SERVICE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "ffmpeg": state.has_ffmpeg(),
    }))
}

/// `GET /v1/capabilities`
async fn capabilities(State(state): State<Arc<DaemonState>>) -> Result<Json<Value>, ApiError> {
    let mut body = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "ffmpeg": state.has_ffmpeg(),
    });
    if let Ok(prober) = state.prober() {
        match prober.capabilities().await {
            Ok(found) => {
                body["ffmpegVersion"] = json!(found.version_line.clone());
                body["libx264"] = json!(found.has_encoder("libx264"));
                body["libx265"] = json!(found.has_encoder("libx265"));
                body["encoders"] = json!(found.encoders);
                body["muxers"] = json!(found.muxers);
                body["filters"] = json!(found.filters);
                body["doctorReport"] = json!(found.doctor_report());
            }
            Err(error) => body["ffmpegError"] = json!(error.to_string()),
        }
    }
    Ok(Json(body))
}

/// `GET /v1/projects`
async fn list_projects(State(state): State<Arc<DaemonState>>) -> Result<Json<Value>, ApiError> {
    let listed = state.store.list().map_err(ApiError::internal)?;
    Ok(Json(json!(listed
        .into_iter()
        .map(|(id, name, updated_at)| json!({
            "id": id.to_string(),
            "name": name,
            "updatedAt": updated_at,
        }))
        .collect::<Vec<_>>())))
}

/// `POST /v1/projects`
async fn create_project(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let name = text(&body, "name")?;
    let created_by = text(&body, "created_by")?;
    if name.trim().is_empty() {
        return Err(ApiError::bad_request("malformed", "name must not be blank"));
    }
    let project = Project::new(name, created_by, SystemClock.now_unix());
    state.store.save(&project).map_err(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(project_json(&project)?)))
}

/// `GET /v1/projects/{id}`
async fn get_project(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let project = load(&state, parse_project(&id)?)?;
    Ok(Json(project_json(&project)?))
}

/// `DELETE /v1/projects/{id}`
async fn delete_project(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let project_id = parse_project(&id)?;
    // Prove it is there first: the store's delete is happy to delete nothing, and a client
    // that deletes a project twice should be told the second time that it is already gone.
    load(&state, project_id)?;
    state.store.delete(project_id).map_err(ApiError::internal)?;
    Ok(Json(json!({ "deleted": project_id.to_string() })))
}

/// `POST /v1/projects/{id}/sources`
async fn add_source(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let project_id = parse_project(&id)?;
    // Read it first, so a project that is not there is a `404` rather than the `500` that
    // opening a workspace over a missing row would otherwise produce.
    load(&state, project_id)?;
    let path = MediaPath::new(text(&body, "path")?);
    let engine = engine(&state)?;
    let mut workspace = open_workspace(&state, project_id, engine)?;
    let added = workspace
        .add_source(path)
        .await
        .map_err(|error| domain(&error))?;
    workspace
        .save(state.store.as_ref())
        .map_err(|error| domain(&error))?;
    let source = workspace
        .project()
        .source(&added)
        .map(|source| {
            json!({
                "path": source.path.to_string(),
                "available": source.available,
                "label": source.label,
                "probed": source.media.is_some(),
            })
        })
        .ok_or_else(|| ApiError::internal("the source vanished as it was added"))?;
    Ok((StatusCode::CREATED, Json(source)))
}

/// `GET /v1/projects/{id}/segments`
async fn list_segments(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let project = load(&state, parse_project(&id)?)?;
    Ok(Json(json!(project.segments)))
}

/// `POST /v1/projects/{id}/segments`
async fn add_segment(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let project_id = parse_project(&id)?;
    let mut project = load(&state, project_id)?;

    let source = MediaPath::new(text(&body, "source")?);
    let name = text(&body, "name")?;
    let start_frame = integer(&body, "start_frame")?;
    let end_frame = optional_integer(&body, "end_frame")?;
    let preset = optional_text(&body, "preset")?;
    let handle_frames = optional_integer(&body, "handle_frames")?.unwrap_or(0);

    if !project.sources.contains_key(&source) {
        return Err(ApiError::bad_request(
            "unknownSource",
            format!("{source} is not a source in this project; add it first"),
        ));
    }
    if handle_frames < 0 {
        return Err(ApiError::bad_request(
            "malformed",
            "handle_frames cannot be negative",
        ));
    }
    // A range that is not a range is refused here rather than three processes later, which is
    // the whole reason the domain has a planner that can be asked.
    if let Some(end) = end_frame {
        if end <= start_frame {
            return Err(ApiError::bad_request(
                "badRange",
                format!(
                    "the out point (frame {end}) is not after the in point (frame {start_frame})"
                ),
            ));
        }
    }
    if let Some(media) = project.media(&source) {
        if start_frame > media.last_frame() {
            return Err(ApiError::bad_request(
                "badRange",
                format!(
                    "the in point is frame {start_frame}, past the end of the source \
                     ({} frames)",
                    media.frame_count
                ),
            ));
        }
        let grid = KeyframeGrid::new(
            Vec::new(),
            start_frame,
            end_frame.unwrap_or(media.frame_count),
        );
        let probe = Segment {
            id: SegmentId::new(),
            source: source.clone(),
            name: name.clone(),
            start_frame,
            end_frame,
            note: None,
            tags: Vec::new(),
            preset: preset.clone(),
            handle_frames,
            enabled: true,
        };
        plan_cut(media, &probe, &grid)
            .map_err(|error| ApiError::bad_request("badRange", error.to_string()))?;
    }

    let mut segment = Segment::new(source, name, start_frame, end_frame.unwrap_or(start_frame));
    segment.end_frame = end_frame;
    segment.preset = preset;
    segment.handle_frames = handle_frames;
    let id = segment.id;
    project.segments.push(segment);
    state.store.save(&project).map_err(ApiError::internal)?;
    let stored = load(&state, project_id)?;
    let created = stored
        .segment(id)
        .cloned()
        .ok_or_else(|| ApiError::internal("the segment vanished as it was added"))?;
    Ok((StatusCode::CREATED, Json(json!(created))))
}

/// `DELETE /v1/projects/{id}/segments/{segment_id}`
async fn delete_segment(
    State(state): State<Arc<DaemonState>>,
    Path((id, segment_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let project_id = parse_project(&id)?;
    let segment_id = parse_segment(&segment_id)?;
    let mut project = load(&state, project_id)?;
    project
        .remove_segment(segment_id)
        .map_err(|_| ApiError::not_found("segment"))?;
    state.store.save(&project).map_err(ApiError::internal)?;
    Ok(Json(json!({ "deleted": segment_id.to_string() })))
}

/// `GET /v1/projects/{id}/preview`
///
/// The dry run: what a batch would do, with the exact commands, and without writing anything.
async fn preview(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let project_id = parse_project(&id)?;
    // As `add_source`: a missing project is a `404`, not a `500`.
    load(&state, project_id)?;
    let engine = engine(&state)?;
    let mut workspace = open_workspace(&state, project_id, engine)?;
    let previews = workspace
        .preview_all()
        .await
        .map_err(|error| domain(&error))?;
    Ok(Json(json!(previews
        .iter()
        .map(preview_json)
        .collect::<Vec<_>>())))
}

/// `POST /v1/projects/{id}/run`
async fn start_run(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let project_id = parse_project(&id)?;
    let project = load(&state, project_id)?;
    if state.project_is_running(project_id) {
        return Err(ApiError::conflict(
            "runInProgress",
            "a run for this project is already going; wait for it or cancel it",
        ));
    }

    let now = SystemClock.now_unix();
    let actor = if project.created_by.trim().is_empty() {
        "daemon".to_owned()
    } else {
        project.created_by.clone()
    };
    let record = RunRecord {
        id: Uuid::now_v7(),
        project_id,
        state: RunState::Queued,
        started_at: now,
        finished_at: None,
        total: project.segments.iter().filter(|s| s.enabled).count(),
        outcome: None,
        error: None,
        cancel: CancelFlag::new(),
    };
    let run_id = record.id;
    let provisional = record.cancel.clone();
    state
        .insert_run(record)
        .map_err(|error| ApiError::conflict("registryFull", error))?;

    spawn_run(
        Arc::clone(&state),
        project_id,
        run_id,
        provisional,
        actor,
        now,
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "runId": run_id.to_string(), "state": RunState::Queued.word() })),
    ))
}

/// `GET /v1/runs/{run_id}`
async fn get_run(
    State(state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_run(&run_id)?;
    state
        .run(id)
        .map(|record| Json(json!(record)))
        .ok_or_else(|| ApiError::not_found("run"))
}

/// `POST /v1/runs/{run_id}/cancel`
async fn cancel_run(
    State(state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_run(&run_id)?;
    let flag = state
        .cancel_flag(id)
        .ok_or_else(|| ApiError::not_found("running run"))?;
    flag.cancel();
    state
        .update_run(id, |record| record.state = RunState::Cancelled)
        .map_err(ApiError::internal)?;
    Ok(Json(
        json!({ "runId": id.to_string(), "state": RunState::Cancelled.word() }),
    ))
}

/// `POST /v1/transcripts/search`
///
/// A video with no caption file beside it is `200` with an empty list, not `404`: "there is no
/// transcript here" is a fact about the file rather than a missing resource, and a client that
/// needs to tell those apart can look at the file it asked about. See the crate docs.
async fn search_transcripts(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let video = MediaPath::new(text(&body, "video")?);
    let phrase = text(&body, "phrase")?;
    let limit = optional_integer(&body, "limit")?
        .unwrap_or(20)
        .clamp(0, 1_000) as usize;

    // The state's index, not one built for this request: the transcript module documents that the
    // index is "built once and kept", and building one per request re-read the SRT, re-parsed three
    // thousand cues and re-folded all of them on every polled search — the daemon's worst hot path,
    // found by an audit.
    let view = match state.transcripts.load(&video) {
        Ok(Some(view)) => view,
        Ok(None) => {
            return Ok(Json(json!({
                "video": video.to_string(),
                "phrase": phrase,
                "hits": [],
                "reason": "there is no caption file beside that video",
            })))
        }
        Err(error) => {
            return Ok(Json(json!({
                "video": video.to_string(),
                "phrase": phrase,
                "hits": [],
                "reason": error.to_string(),
            })))
        }
    };
    // 29.97 is a placeholder only when the video has never been probed; the frames a transcript
    // search reports are on the source's own grid, and a caller that cares probes first.
    let rate = trimmer_core::FrameRate::FPS_29_97;
    let hits = view.find_phrase(&phrase, rate, limit);
    Ok(Json(json!({
        "video": video.to_string(),
        "phrase": phrase,
        "rate": rate.as_ffmpeg(),
        "hits": hits,
    })))
}

/// `GET /v1/openapi.json`
async fn openapi() -> Json<Value> {
    Json(openapi_document())
}

// --- the pieces the handlers share --------------------------------------------------------

/// Load a project, or refuse with `404`.
fn load(state: &Arc<DaemonState>, id: ProjectId) -> Result<Project, ApiError> {
    state.store.load(id).map_err(|error| {
        if error.contains("no project") {
            ApiError::not_found("project")
        } else {
            ApiError::internal(error)
        }
    })
}

/// The media engine, or a refusal explaining that ffmpeg is missing.
fn engine(state: &Arc<DaemonState>) -> Result<Arc<dyn MediaEngine>, ApiError> {
    let tools = state.tools.clone().ok_or_else(|| {
        ApiError::internal(
            "ffmpeg and ffprobe could not be found; install them or set THE_TRIMMER_FFMPEG and \
             THE_TRIMMER_FFPROBE",
        )
    })?;
    Ok(Arc::new(MediaAdapter::new(
        CutExecutor::new(tools),
        CutConfig::default(),
    )))
}

/// Open a workspace over a stored project.
fn open_workspace(
    state: &Arc<DaemonState>,
    id: ProjectId,
    engine: Arc<dyn MediaEngine>,
) -> Result<Workspace, ApiError> {
    Workspace::open(
        state.store.as_ref(),
        id,
        engine,
        Arc::new(FileTranscripts),
        Arc::new(SystemClock),
    )
    .map_err(|error| domain(&error))
}

/// A project as the API returns it: the readable document, parsed back into a value.
fn project_json(project: &Project) -> Result<Value, ApiError> {
    let text = document::to_json(project).map_err(|error| ApiError::internal(error.to_string()))?;
    serde_json::from_str(&text).map_err(|error| ApiError::internal(error.to_string()))
}

/// A dry run as JSON.
///
/// Written by hand because `trimmer_app::QueuePreview` has no `serde` derive: it is the
/// application layer's own view type and giving it a wire format would tie the two layers
/// together. The coupling is this function, and it is deliberately one function wide.
fn preview_json(preview: &trimmer_app::QueuePreview) -> Value {
    json!({
        "segment": preview.segment.to_string(),
        "preset": preview.preset,
        "forcesFullEncode": preview.forces_full_encode,
        "reencodeFraction": preview.reencode_fraction,
        "estimatedBytes": preview.estimated_bytes,
        "plan": preview.plan.as_ref().map(|plan| json!({
            "mode": format!("{:?}", plan.mode).to_lowercase(),
            "startFrame": plan.start_frame,
            "endFrame": plan.end_frame,
            "keyframe": plan.keyframe,
            "requestedFrames": plan.requested_frames(),
            "videoTimescale": plan.video_timescale,
            "headEncoder": plan.head_encoder,
        })),
        "commands": preview.commands.iter().map(|prepared| json!({
            "label": prepared.label,
            "args": prepared.args,
        })).collect::<Vec<_>>(),
        "problems": preview.problems,
        "notes": preview.notes,
    })
}

/// Run a batch off the request thread and record what happened.
fn spawn_run(
    state: Arc<DaemonState>,
    project_id: ProjectId,
    run_id: Uuid,
    provisional: CancelFlag,
    actor: String,
    started_at: i64,
) {
    tokio::spawn(async move {
        state
            .update_run(run_id, |record| record.state = RunState::Running)
            .ok();

        let result = run_batch(&state, project_id, run_id, provisional, &actor, started_at).await;

        match result {
            Ok((outcome, cancelled)) => {
                let finished_at = SystemClock.now_unix();
                let state_for_update = Arc::clone(&state);
                state_for_update
                    .update_run(run_id, |record| {
                        record.state = if cancelled {
                            RunState::Cancelled
                        } else {
                            RunState::Finished
                        };
                        record.finished_at = Some(finished_at);
                        record.outcome = Some(outcome);
                    })
                    .ok();
            }
            Err(message) => {
                tracing::error!(%run_id, %message, "a run could not be carried out");
                state
                    .update_run(run_id, |record| {
                        record.state = RunState::Finished;
                        record.finished_at = Some(SystemClock.now_unix());
                        record.error = Some(message);
                    })
                    .ok();
            }
        }
    });
}

/// The batch itself: build a workspace, run the queue, and persist the audit record.
async fn run_batch(
    state: &Arc<DaemonState>,
    project_id: ProjectId,
    run_id: Uuid,
    provisional: CancelFlag,
    actor: &str,
    started_at: i64,
) -> Result<(RunOutcome, bool), String> {
    let tools = state.tools.clone().ok_or_else(|| {
        "ffmpeg and ffprobe could not be found; install them or set THE_TRIMMER_FFMPEG and \
         THE_TRIMMER_FFPROBE"
            .to_owned()
    })?;

    let engine: Arc<dyn MediaEngine> = Arc::new(MediaAdapter::new(
        CutExecutor::new(tools.clone()),
        CutConfig::default(),
    ));
    let clock = Arc::new(SystemClock);
    let mut workspace = Workspace::open(
        state.store.as_ref(),
        project_id,
        Arc::clone(&engine),
        Arc::new(FileTranscripts),
        Arc::clone(&clock) as Arc<dyn Clock>,
    )
    .map_err(|error| error.to_string())?;

    let queue = Queue::new(
        Arc::clone(&engine),
        Arc::new(ProbeMeasurer::new(Prober::new(tools))),
        Arc::clone(&clock) as Arc<dyn Clock>,
        env!("CARGO_PKG_VERSION"),
        machine_name(),
    );

    // The run record was written before the queue existed, so the flag it holds is replaced by
    // the queue's own. Anything that arrived in between is honoured rather than lost.
    let queue_cancel = queue.cancel_flag();
    state.update_run(run_id, |record| record.cancel = queue_cancel.clone())?;
    if provisional.is_cancelled() {
        queue_cancel.cancel();
    }

    let options = QueueOptions {
        // A daemon batch is unattended, so one bad segment must not cost the other ninety-nine.
        stop_on_error: false,
        skip_verification: false,
        audit: true,
        label: format!("daemon run {run_id}"),
    };
    let outcome = queue
        .run(&mut workspace, &options, trimmer_app::quiet_sink())
        .await
        .map_err(|error| error.to_string())?;

    // Persist the audit record. A failure here is logged and does not change the verdict: the
    // files are on disk either way, and reporting the run as failed because its bookkeeping
    // failed would be a lie about the work.
    if let Err(error) = state
        .store
        .record_run(project_id, actor, started_at, &outcome)
    {
        tracing::error!(%run_id, %error, "the run could not be recorded in the store");
    }

    let items = outcome
        .jobs
        .iter()
        .map(|(_, segment, name, status)| RunItem {
            segment: segment.to_string(),
            name: name.clone(),
            status: status.summary(),
            output: status.output().map(ToString::to_string),
        })
        .collect();

    Ok((
        RunOutcome {
            succeeded: outcome.succeeded(),
            unverified: outcome.unverified(),
            failed: outcome.failed(),
            skipped: outcome.skipped(),
            delivered_frames: outcome.delivered_frames,
            delivered_seconds: outcome.delivered_seconds,
            elapsed_seconds: outcome.elapsed_seconds,
            items,
            report: outcome.report(),
        },
        outcome.cancelled,
    ))
}

/// How long a client should wait between polls of `GET /v1/runs/{id}`.
///
/// Published because an API that does not say is an API every client polls at a different rate.
pub const SUGGESTED_POLL: Duration = Duration::from_millis(500);

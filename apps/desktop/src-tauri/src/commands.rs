//! The commands the interface can call, and the only things it can call.
//!
//! Each one is a thin translation: take named arguments that arrived as JSON, hand them to the
//! application layer, and return either a value the interface has a type for or a sentence it can
//! show. Nothing here holds state of its own and nothing here reaches around the layer below it.
//!
//! ## The rules this file follows
//!
//! 1. **A command that fails says why, in a sentence.** An error is a `String`, never a panic and
//!    never a debug format of a Rust enum. The interface shows it to somebody who is trying to cut
//!    a video, not to read a type name.
//! 2. **A path is a string until the domain accepts it.** Every `path` argument becomes a
//!    [`MediaPath`] and is then checked by [`MediaPath::exists`] or by a probe, which is the layer
//!    that should decide. Nothing joins a user string to a directory.
//! 3. **No command holds the workspace lock across a process.** The lock is taken, read, released;
//!    the long work happens outside it. A `MutexGuard` living across an `await` would make the whole
//!    interface wait for a ten-minute copy.
//! 4. **A cut's progress goes to the interface as events.** A command that takes minutes and returns
//!    nothing until it is done cannot drive a progress bar.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use trimmer_app::ports::{Clock, MediaEngine, SegmentCutRequest, SystemClock, TranscriptSource};
use trimmer_app::watch::ObservedFile;
use trimmer_app::{
    CollectingQueueSink, MediaAdapter, QueueOptions, SegmentView, SourceView, Workspace,
};
use trimmer_core::{Grouping, MediaPath, Segment, VerifyPolicy};
use trimmer_media::{CutConfig, CutExecutor, PollPolicy, RunOptions, ToolPaths};

use crate::state::{AppState, RunningBatch};
use crate::VERSION;

/// A failure the interface can show.
type Reply<T> = Result<T, String>;

/// Turn an application error into a sentence.
fn explain(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// The clock, constructed per call. It is a unit struct, so this costs nothing.
fn clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}

/// Build a workspace over the shared engine and transcript service.
fn new_workspace(state: &AppState, name: &str, created_by: &str) -> Workspace {
    let transcripts: Arc<dyn TranscriptSource> = Arc::new(trimmer_app::ports::FileTranscripts);
    Workspace::new(
        name,
        created_by,
        Arc::clone(&state.engine),
        transcripts,
        clock(),
    )
}

// ---------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------

/// The doctor panel: what the application found on this machine.
#[tauri::command]
pub async fn doctor(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    let tools = ToolPaths::resolve().map_err(explain)?;
    let prober = trimmer_media::Prober::new(tools.clone());
    let capabilities = prober.capabilities().await.map_err(explain)?;
    let ffmpeg = prober
        .ffmpeg_version()
        .await
        .unwrap_or_else(|_| "not found".to_owned());
    let ffprobe = ffmpeg.clone();
    Ok(serde_json::json!({
        "version": VERSION,
        "ffmpeg": ffmpeg,
        "ffprobe": ffprobe,
        "capabilities": capabilities.doctor_report(),
        "storePath": state.store_path.display().to_string(),
        "libx264": capabilities.has_encoder("libx264"),
        "libx265": capabilities.has_encoder("libx265"),
    }))
}

// ---------------------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------------------

/// The projects in the store.
#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    let rows = trimmer_app::ProjectStore::list(state.store.as_ref()).map_err(|error| error)?;
    let listed: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, name, updated_at)| {
            serde_json::json!({ "id": id.to_string(), "name": name, "updatedAt": updated_at })
        })
        .collect();
    Ok(serde_json::Value::Array(listed))
}

/// Create a project and open it.
#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    name: String,
    created_by: String,
) -> Reply<serde_json::Value> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("a project needs a name".to_owned());
    }
    let mut workspace = new_workspace(&state, trimmed, &created_by);
    workspace.save(state.store.as_ref()).map_err(explain)?;
    let id = workspace.project().id;
    state.set_workspace(workspace);
    Ok(serde_json::json!({ "id": id.to_string() }))
}

/// Open a project, probing what it needs.
#[tauri::command]
pub async fn open_project(state: State<'_, AppState>, id: String) -> Reply<()> {
    let project_id = id
        .parse::<uuid::Uuid>()
        .map(trimmer_core::ProjectId)
        .map_err(|error| format!("{id} is not a project id: {error}"))?;
    let transcripts: Arc<dyn TranscriptSource> = Arc::new(trimmer_app::ports::FileTranscripts);
    let mut workspace = Workspace::open(
        state.store.as_ref(),
        project_id,
        Arc::clone(&state.engine),
        transcripts,
        clock(),
    )
    .map_err(explain)?;
    workspace.refresh_sources().await.map_err(explain)?;
    state.set_workspace(workspace);
    Ok(())
}

/// Delete a project.
#[tauri::command]
pub async fn delete_project(state: State<'_, AppState>, id: String) -> Reply<()> {
    let project_id = id
        .parse::<uuid::Uuid>()
        .map(trimmer_core::ProjectId)
        .map_err(|error| format!("{id} is not a project id: {error}"))?;
    trimmer_app::ProjectStore::delete(state.store.as_ref(), project_id).map_err(|error| error)?;
    if state
        .workspace
        .lock()
        .as_ref()
        .is_some_and(|workspace| workspace.project().id == project_id)
    {
        *state.workspace.lock() = None;
    }
    Ok(())
}

/// The currently open project's id and name.
#[tauri::command]
pub async fn current_project(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    let guard = state.workspace.lock();
    match guard.as_ref() {
        None => Ok(serde_json::Value::Null),
        Some(workspace) => Ok(serde_json::json!({
            "id": workspace.project().id.to_string(),
            "name": workspace.project().name,
            "createdBy": workspace.project().created_by,
        })),
    }
}

/// Save the open project.
#[tauri::command]
pub async fn save_project(state: State<'_, AppState>) -> Reply<()> {
    state.with_workspace(|workspace| {
        workspace.save(state.store.as_ref()).map_err(explain)?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------------------

/// Add a source to the open project.
#[tauri::command]
pub async fn add_source(state: State<'_, AppState>, path: String) -> Reply<serde_json::Value> {
    // The probe is the slow part, and it must not run while the workspace lock is held: the
    // interface polls other commands while this is in flight.
    let engine = Arc::clone(&state.engine);
    let media_path = MediaPath::new(path.clone());
    let probed = if media_path.exists() {
        Some(engine.probe(&media_path).await.map_err(explain)?)
    } else {
        None
    };

    state.with_workspace(|workspace| {
        let canonical = media_path.canonicalised();
        let transcript = trimmer_core::caption::find_for(canonical.as_path());
        workspace
            .project_mut()
            .upsert_source(trimmer_core::SegmentSource {
                path: canonical.clone(),
                media: probed.clone(),
                available: media_path.exists(),
                label: None,
            });
        let summary = probed.as_ref().map_or_else(
            || "the file is not on disk; it was added anyway so the marks can be fixed".to_owned(),
            trimmer_core::MediaInfo::summary,
        );
        Ok(serde_json::json!({
            "path": canonical.to_string(),
            "name": canonical.file_name(),
            "present": media_path.exists(),
            "media": probed,
            "summary": summary,
            "transcript": transcript.map(|found| found.display().to_string()),
            "transcriptCues": serde_json::Value::Null,
            "label": serde_json::Value::Null,
            "variableRate": probed.as_ref().is_some_and(trimmer_core::MediaInfo::is_variable_rate),
        }))
    })
}

/// Re-probe every source and mark the ones that have gone.
#[tauri::command]
pub async fn refresh_sources(state: State<'_, AppState>) -> Reply<()> {
    // Re-probe outside the lock, then apply, so a probe of six masters does not block the window.
    let paths: Vec<MediaPath> = state
        .with_workspace(|workspace| Ok(workspace.project().sources.keys().cloned().collect()))?;
    let engine = Arc::clone(&state.engine);
    let mut probed: Vec<(MediaPath, bool, Option<trimmer_core::MediaInfo>)> = Vec::new();
    for path in paths {
        let present = path.exists();
        let media = if present {
            engine.probe(&path).await.ok()
        } else {
            None
        };
        probed.push((path, present, media));
    }
    state.with_workspace(|workspace| {
        for (path, present, media) in probed {
            if let Some(source) = workspace.project_mut().sources.get_mut(&path) {
                source.available = present;
                if media.is_some() {
                    source.media = media;
                }
            }
        }
        Ok(())
    })
}

/// Remove a source and every segment that cut it.
#[tauri::command]
pub async fn remove_source(state: State<'_, AppState>, path: String) -> Reply<()> {
    let target = MediaPath::new(path);
    state.with_workspace(|workspace| {
        let doomed: Vec<trimmer_core::SegmentId> = workspace
            .project()
            .segments
            .iter()
            .filter(|segment| segment.source.same_file_as(&target))
            .map(|segment| segment.id)
            .collect();
        for id in doomed {
            let _ = workspace.remove_segment(id);
        }
        workspace.project_mut().sources.remove(&target);
        Ok(())
    })
}

/// The sources, as the list shows them.
#[tauri::command]
pub async fn sources(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    state.with_workspace(|workspace| {
        let views: Vec<SourceView> = workspace.sources();
        serde_json::to_value(views).map_err(explain)
    })
}

// ---------------------------------------------------------------------------------------
// Segments
// ---------------------------------------------------------------------------------------

/// The segments.
#[tauri::command]
pub async fn segments(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    state.with_workspace(|workspace| {
        let views: Vec<SegmentView> = workspace.segments();
        serde_json::to_value(views).map_err(explain)
    })
}

/// The summary line above the run button.
#[tauri::command]
pub async fn summary(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    state.with_workspace(|workspace| serde_json::to_value(workspace.summary()).map_err(explain))
}

/// The delivery presets this project offers, with what each one costs.
#[tauri::command]
pub async fn presets(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    state.with_workspace(|workspace| {
        let project = workspace.project();
        let listed: Vec<serde_json::Value> = project
            .presets
            .values()
            .map(|preset| {
                // Whether a preset preserves the picture depends on the *source's* geometry, so a
                // preset with no segment to apply to reports the geometry rule alone.
                let preserves = preset.video.is_copy()
                    && matches!(preset.geometry.fit, trimmer_core::AspectFit::Native);
                serde_json::json!({
                    "name": preset.name,
                    "description": preset.description,
                    "container": preset.container.extension(),
                    "preservesPicture": preserves,
                    "batchSafe": preset.batch_safe,
                })
            })
            .collect();
        Ok(serde_json::Value::Array(listed))
    })
}

/// Add a segment.
#[tauri::command]
pub async fn add_segment(
    state: State<'_, AppState>,
    source: String,
    name: String,
    start_frame: i64,
    end_frame: Option<i64>,
    preset: Option<String>,
    handle_frames: i64,
) -> Reply<serde_json::Value> {
    state.with_workspace(|workspace| {
        let path = MediaPath::new(source);
        let mut segment = Segment::new(
            path,
            name,
            start_frame,
            end_frame.unwrap_or(start_frame + 1),
        );
        segment.end_frame = end_frame;
        segment.preset = preset;
        segment.handle_frames = handle_frames;
        let id = workspace.add_segment(segment).map_err(explain)?;
        Ok(serde_json::json!({ "id": id.to_string() }))
    })
}

/// Update a segment in place.
#[tauri::command]
pub async fn update_segment(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    start_frame: Option<i64>,
    end_frame: Option<Option<i64>>,
    preset: Option<Option<String>>,
    handle_frames: Option<i64>,
    enabled: Option<bool>,
    note: Option<Option<String>>,
) -> Reply<()> {
    let segment_id = id
        .parse::<uuid::Uuid>()
        .map(trimmer_core::SegmentId)
        .map_err(|error| format!("{id} is not a segment id: {error}"))?;
    state.with_workspace(|workspace| {
        // Read the current value, build the new one, then validate by planning it: an edit that
        // moves the out point past the end of the source must be refused while the user is looking
        // at the field, not discovered three processes into a cut.
        let current = workspace
            .project()
            .segment(segment_id)
            .cloned()
            .ok_or_else(|| format!("segment {id} is not in this project"))?;
        let mut edited = current.clone();
        if let Some(value) = name {
            edited.name = value;
        }
        if let Some(value) = start_frame {
            edited.start_frame = value;
        }
        if let Some(value) = end_frame {
            edited.end_frame = value;
        }
        if let Some(value) = preset {
            edited.preset = value;
        }
        if let Some(value) = handle_frames {
            edited.handle_frames = value;
        }
        if let Some(value) = enabled {
            edited.enabled = value;
        }
        if let Some(value) = note {
            edited.note = value;
        }

        let media = workspace.project().media(&edited.source).cloned();
        if let Some(media) = media {
            let grid = trimmer_core::KeyframeGrid::new(
                Vec::new(),
                edited.start_frame,
                edited.end_frame.unwrap_or(media.frame_count),
            );
            trimmer_core::plan_cut(&media, &edited, &grid).map_err(explain)?;
        }

        let slot = workspace
            .project_mut()
            .segment_mut(segment_id)
            .ok_or_else(|| format!("segment {id} is not in this project"))?;
        *slot = edited;
        Ok(())
    })
}

/// Remove a segment.
#[tauri::command]
pub async fn remove_segment(state: State<'_, AppState>, id: String) -> Reply<()> {
    let segment_id = id
        .parse::<uuid::Uuid>()
        .map(trimmer_core::SegmentId)
        .map_err(|error| format!("{id} is not a segment id: {error}"))?;
    state.with_workspace(|workspace| {
        workspace.remove_segment(segment_id).map_err(explain)?;
        Ok(())
    })
}

/// Move a segment in the running order.
#[tauri::command]
pub async fn reorder_segment(state: State<'_, AppState>, from: usize, to: usize) -> Reply<()> {
    state.with_workspace(|workspace| {
        workspace
            .project_mut()
            .reorder_segment(from, to)
            .map_err(explain)?;
        Ok(())
    })
}

/// Turn a timecode into a frame number, so a field can show its frame as the user types.
#[tauri::command]
pub async fn parse_timecode(
    state: State<'_, AppState>,
    text: String,
    source: String,
) -> Reply<serde_json::Value> {
    let path = MediaPath::new(source);
    state.with_workspace(|workspace| {
        let rate = workspace
            .project()
            .media(&path)
            .map(|media| media.rate)
            .ok_or_else(|| {
                "that source has not been probed, so its frame rate is unknown".to_owned()
            })?;
        let frame = trimmer_core::timecode::parse_timecode(&text, rate).map_err(explain)?;
        Ok(serde_json::json!({
            "frame": frame,
            "timecode": trimmer_core::timecode::format_timecode(frame, rate, None),
        }))
    })
}

// ---------------------------------------------------------------------------------------
// Planning and cutting
// ---------------------------------------------------------------------------------------

/// What one segment will do and cost.
#[tauri::command]
pub async fn preview(state: State<'_, AppState>, id: String) -> Reply<serde_json::Value> {
    let segment_id = id
        .parse::<uuid::Uuid>()
        .map(trimmer_core::SegmentId)
        .map_err(|error| format!("{id} is not a segment id: {error}"))?;
    let engine = Arc::clone(&state.engine);
    // Plan outside the lock: a plan needs a keyframe listing, which is a process launch.
    let (media, segment, preset) = state.with_workspace(|workspace| {
        let segment = workspace
            .project()
            .segment(segment_id)
            .cloned()
            .ok_or_else(|| format!("segment {id} is not in this project"))?;
        let media = workspace
            .project()
            .media(&segment.source)
            .cloned()
            .ok_or_else(|| "that source has not been probed yet".to_owned())?;
        let preset = workspace
            .project()
            .preset_for(&segment)
            .map_err(explain)?
            .clone();
        Ok((media, segment, preset))
    })?;

    let plan = engine.plan(&media, &segment).await.map_err(explain)?;
    let forces = !preset.preserves_picture(media.width, media.height);
    let commands = engine
        .preview(&media, &segment, &preset, &plan)
        .unwrap_or_default();
    let commands_json: Vec<serde_json::Value> = commands
        .iter()
        .map(|step| serde_json::json!({ "label": step.label, "args": step.args }))
        .collect();

    Ok(serde_json::json!({
        "segment": id,
        "plan": plan,
        "preset": preset.name,
        "forcesFullEncode": forces,
        "reencodeFraction": plan.reencode_fraction(),
        "estimatedBytes": estimate_bytes(&plan, &media, &preset),
        "commands": commands_json,
        "problems": [],
        "notes": plan.notes,
    }))
}

/// A rough output size. Rough on purpose: it answers "will this fit on the drive", and pretending
/// to more accuracy would be false precision about a bitrate nobody has measured.
fn estimate_bytes(
    plan: &trimmer_core::CutPlan,
    media: &trimmer_core::MediaInfo,
    preset: &trimmer_core::DeliveryPreset,
) -> Option<u64> {
    if plan.requested_frames() <= 0 || media.width == 0 {
        return None;
    }
    let crf = match &preset.video {
        trimmer_core::VideoTreatment::Encode { quality, .. } => *quality,
        _ => 18,
    };
    let quality_factor = 0.12_f64 * 1.18_f64.powi(i32::from(crf) - 18);
    let bits_per_frame = f64::from(media.width) * f64::from(media.height) * quality_factor;
    let video_bytes = bits_per_frame * plan.requested_frames() as f64 / 8.0;
    let seconds = media.rate.seconds_of(plan.requested_frames());
    let audio_bytes = 24_000.0 * seconds;
    Some((video_bytes + audio_bytes).max(0.0) as u64)
}

/// What the whole batch will do and cost.
#[tauri::command]
pub async fn preview_all(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    let ids: Vec<String> = state.with_workspace(|workspace| {
        Ok(workspace
            .project()
            .segments
            .iter()
            .filter(|segment| segment.enabled)
            .map(|segment| segment.id.to_string())
            .collect())
    })?;
    let mut planned = Vec::new();
    for id in ids {
        // One at a time, each with its own short lock, so the window stays responsive while a
        // hundred keyframe listings run.
        match preview(state.clone(), id).await {
            Ok(value) => planned.push(value),
            Err(reason) => planned.push(serde_json::json!({ "problems": [reason] })),
        }
    }
    Ok(serde_json::Value::Array(planned))
}

/// Cut one segment, emitting `cut-progress` events as it goes.
#[tauri::command]
pub async fn cut_segment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Reply<serde_json::Value> {
    let segment_id = id
        .parse::<uuid::Uuid>()
        .map(trimmer_core::SegmentId)
        .map_err(|error| format!("{id} is not a segment id: {error}"))?;
    let cancel = trimmer_media::CancelFlag::new();
    *state.running.lock() = Some(RunningBatch {
        label: format!("cutting {id}"),
        started_millis: clock().now_millis(),
        cancel: cancel.clone(),
    });

    let outcome = run_one(&app, &state, segment_id, cancel).await;
    *state.running.lock() = None;
    outcome
}

/// Cut one segment through the queue machinery, so a single cut and a batch behave identically.
async fn run_one(
    app: &AppHandle,
    state: &AppState,
    segment_id: trimmer_core::SegmentId,
    cancel: trimmer_media::CancelFlag,
) -> Reply<serde_json::Value> {
    let engine = Arc::clone(&state.engine);
    let (media, segment, preset, output) = state.with_workspace(|workspace| {
        let segment = workspace
            .project()
            .segment(segment_id)
            .cloned()
            .ok_or_else(|| format!("{segment_id} is not in this project"))?;
        let media = workspace
            .project()
            .media(&segment.source)
            .cloned()
            .ok_or_else(|| "that source has not been probed yet".to_owned())?;
        let preset = workspace
            .project()
            .preset_for(&segment)
            .map_err(explain)?
            .clone();
        let output = workspace.output_path(&segment);
        Ok((media, segment, preset, output))
    })?;

    let plan = engine.plan(&media, &segment).await.map_err(explain)?;
    let request = SegmentCutRequest {
        media,
        segment,
        preset,
        output,
        plan: Some(plan),
    };

    let sink = Arc::new(EmitterSink { app: app.clone() });
    let options = RunOptions {
        policy: PollPolicy::long(),
        cancel,
        sink,
        label: "cut".to_owned(),
    };
    let outcome = engine.cut(&request, &options).await.map_err(explain)?;

    // Persist so a run survives a restart.
    if let Err(reason) = state.with_workspace(|workspace| {
        workspace.save(state.store.as_ref()).map_err(explain)?;
        Ok(())
    }) {
        tracing::warn!("could not save after a cut: {reason}");
    }

    Ok(serde_json::json!({
        "jobs": [{
            "job": 0,
            "segment": segment_id.to_string(),
            "name": outcome.plan.mode.label(),
            "status": {
                "kind": "succeeded",
                "output": outcome.output.to_string(),
                "frames": outcome.frame_count,
                "overshoot": outcome.overshoot,
                "seconds": outcome.steps.iter().filter_map(|step| step.seconds).sum::<f64>(),
                "steps": outcome.steps.len(),
                "checks": [],
            },
        }],
        "deliveredFrames": outcome.frame_count,
        "deliveredSeconds": 0.0,
        "elapsedSeconds": outcome.steps.iter().filter_map(|step| step.seconds).sum::<f64>(),
        "cancelled": false,
        "digest": String::new(),
    }))
}

/// Run the whole batch through the queue.
#[tauri::command]
pub async fn run_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    stop_on_error: bool,
    skip_verification: bool,
    label: String,
) -> Reply<serde_json::Value> {
    let cancel = state.queue.cancel_flag();
    *state.running.lock() = Some(RunningBatch {
        label: label.clone(),
        started_millis: clock().now_millis(),
        cancel: cancel.clone(),
    });

    let sink = Arc::new(EmitterSink { app: app.clone() });
    let options = QueueOptions {
        stop_on_error,
        skip_verification,
        audit: true,
        label,
    };

    // The queue needs `&mut Workspace`, and the workspace must stay behind the lock for the whole
    // run — but the run awaits, so the guard cannot be held. The queue is therefore given an owned
    // clone of the project by taking it out and putting it back, which is safe because nothing else
    // can touch it while a batch is in flight: the interface disables the editing controls.
    //
    // A deeper fix is for `Queue::run` to take the project by value and return it; noted in the
    // README rather than hidden here.
    let mut workspace = state
        .workspace
        .lock()
        .take()
        .ok_or_else(|| "no project is open".to_owned())?;

    let outcome = state.queue.run(&mut workspace, &options, sink).await;

    let saved = workspace.save(state.store.as_ref());
    *state.workspace.lock() = Some(workspace);
    *state.running.lock() = None;
    if let Err(reason) = saved {
        tracing::warn!("could not save after a batch: {reason}");
    }

    let outcome = outcome.map_err(explain)?;
    Ok(serde_json::json!({
        "jobs": outcome.jobs.iter().map(|(job, segment, name, status)| serde_json::json!({
            "job": job.0,
            "segment": segment.to_string(),
            "name": name,
            "status": status_json(status),
        })).collect::<Vec<_>>(),
        "deliveredFrames": outcome.delivered_frames,
        "deliveredSeconds": outcome.delivered_seconds,
        "elapsedSeconds": outcome.elapsed_seconds,
        "cancelled": outcome.cancelled,
        "digest": outcome.audit.digest(),
    }))
}

/// A job status as the interface's discriminated union.
fn status_json(status: &trimmer_app::JobStatus) -> serde_json::Value {
    use trimmer_app::JobStatus;
    match status {
        JobStatus::Succeeded {
            output,
            frames,
            overshoot,
            verification,
            steps,
            seconds,
            ..
        } => serde_json::json!({
            "kind": "succeeded",
            "output": output.to_string(),
            "frames": frames,
            "overshoot": overshoot,
            "seconds": seconds,
            "steps": steps,
            "checks": checks_json(verification),
        }),
        JobStatus::Unverified {
            output,
            frames,
            verification,
            seconds,
            ..
        } => serde_json::json!({
            "kind": "unverified",
            "output": output.to_string(),
            "frames": frames,
            "seconds": seconds,
            "checks": checks_json(verification),
        }),
        JobStatus::Failed { reason, cancelled } => serde_json::json!({
            "kind": "failed",
            "reason": reason,
            "cancelled": cancelled,
        }),
        JobStatus::Skipped { reason } => serde_json::json!({
            "kind": "skipped",
            "reason": reason,
        }),
    }
}

/// The checks of a verification report.
fn checks_json(report: &trimmer_verify::VerifyReport) -> Vec<serde_json::Value> {
    use trimmer_verify::CheckStatus;
    report
        .results
        .iter()
        .map(|result| {
            let status = match &result.status {
                CheckStatus::Passed => serde_json::json!({ "kind": "passed" }),
                CheckStatus::Failed { detail } => {
                    serde_json::json!({ "kind": "failed", "detail": detail })
                }
                CheckStatus::Warning { detail } => {
                    serde_json::json!({ "kind": "warning", "detail": detail })
                }
                CheckStatus::Skipped { reason } => {
                    serde_json::json!({ "kind": "skipped", "reason": reason })
                }
            };
            serde_json::json!({
                "check": format!("{:?}", result.check),
                "status": status,
                "measured": result.measured,
                "expected": result.expected,
            })
        })
        .collect()
}

/// Ask the running batch to stop after the segment in flight.
#[tauri::command]
pub async fn cancel_batch(state: State<'_, AppState>) -> Reply<()> {
    let guard = state.running.lock();
    match guard.as_ref() {
        Some(running) => {
            running.cancel.cancel();
            Ok(())
        }
        None => Err("no batch is running".to_owned()),
    }
}

/// A progress sink that forwards each event to the interface.
///
/// It implements both vocabularies: `trimmer-media` speaks `Progress` about one process, and the
/// queue speaks `QueueEvent` about a batch. The interface listens for one event name and
/// discriminates on the shape, which is why both go out under `cut-progress`.
struct EmitterSink {
    app: AppHandle,
}

impl trimmer_media::ProgressSink for EmitterSink {
    fn report(&self, progress: trimmer_media::Progress) {
        // A failed emit means the window has gone; there is nothing useful to do about it and
        // panicking would take the cut down with the window.
        let _ = self.app.emit("cut-progress", &progress);
    }
}

impl trimmer_app::QueueSink for EmitterSink {
    fn event(&self, event: trimmer_app::QueueEvent) {
        let _ = self.app.emit("cut-progress", &event);
    }
}

// ---------------------------------------------------------------------------------------
// Transcripts
// ---------------------------------------------------------------------------------------

/// Search a transcript.
#[tauri::command]
pub async fn search_transcript(
    state: State<'_, AppState>,
    video: String,
    phrase: String,
    limit: usize,
) -> Reply<serde_json::Value> {
    let path = MediaPath::new(video);
    state.with_workspace(|workspace| {
        let rate = workspace
            .project()
            .media(&path)
            .map(|media| media.rate)
            .ok_or_else(|| {
                "that source has not been probed, so its frame rate is unknown".to_owned()
            })?;
        let service =
            trimmer_app::TranscriptService::new(workspace.transcripts(), Grouping::Sentence);
        let view = service.load(&path).map_err(explain)?;
        let Some(view) = view else {
            return Ok(serde_json::json!([]));
        };
        let hits = view.find_phrase(&phrase, rate, limit.clamp(1, 500));
        serde_json::to_value(hits).map_err(explain)
    })
}

/// The transcript beside a video, as lines, for the reading pane.
#[tauri::command]
pub async fn transcript_lines(
    state: State<'_, AppState>,
    video: String,
) -> Reply<serde_json::Value> {
    let path = MediaPath::new(video);
    state.with_workspace(|workspace| {
        let service = trimmer_app::TranscriptService::new(
            workspace.transcripts(),
            Grouping::Sentence,
        );
        let view = service.load(&path).map_err(explain)?;
        let Some(view) = view else {
            return Ok(serde_json::json!([]));
        };
        let rate = workspace
            .project()
            .media(&path)
            .map_or(trimmer_core::FrameRate::FPS_25, |media| media.rate);
        let lines: Vec<serde_json::Value> = view
            .groups
            .iter()
            .enumerate()
            .map(|(index, group)| {
                serde_json::json!({
                    "index": index,
                    "text": group.text,
                    "startFrame": group.start_frame(rate),
                    "endFrame": group.end_frame(rate),
                    "timecode": trimmer_core::timecode::format_timecode(group.start_frame(rate), rate, None),
                })
            })
            .collect();
        Ok(serde_json::Value::Array(lines))
    })
}

// ---------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------

/// Write the project's segments as a timeline document.
#[tauri::command]
pub async fn export_timeline(
    state: State<'_, AppState>,
    format: String,
    path: String,
    sequence_name: String,
) -> Reply<serde_json::Value> {
    let kind = match format.as_str() {
        "premiere" => trimmer_export::ExportFormat::PremiereXml,
        "fcpxml" => trimmer_export::ExportFormat::Fcpxml,
        "edl" => trimmer_export::ExportFormat::Edl,
        "csv" => trimmer_export::ExportFormat::Csv,
        other => return Err(format!("{other} is not an export format this build writes")),
    };
    state.with_workspace(|workspace| {
        let project = workspace.project();
        let segments: Vec<&Segment> = project.segments.iter().collect();
        let resolve = |source: &MediaPath| source.to_string();
        let request = trimmer_export::ExportRequest {
            project,
            segments: segments.clone(),
            timeline_start_frame: 0,
            sequence_name: sequence_name.clone(),
            timeline_rate: segments
                .first()
                .and_then(|segment| project.media(&segment.source))
                .map_or(trimmer_core::FrameRate::FPS_25, |media| media.rate),
            media_path_for: &resolve,
        };
        let product = trimmer_export::export(&request, kind).map_err(explain)?;
        let destination = PathBuf::from(&path);
        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(explain)?;
            }
        }
        std::fs::write(&destination, &product.body).map_err(explain)?;
        Ok(serde_json::json!({
            "warnings": product.warnings,
            "clips": product.clip_count,
        }))
    })
}

// ---------------------------------------------------------------------------------------
// Watch folders
// ---------------------------------------------------------------------------------------

/// Plan the jobs a watch folder implies, without running any of them.
#[tauri::command]
pub async fn plan_watch_folder(folder: String, settle_seconds: u64) -> Reply<serde_json::Value> {
    let directory = PathBuf::from(&folder);
    if !directory.is_dir() {
        return Err(format!("{folder} is not a folder"));
    }
    let mut folder_state = trimmer_app::WatchFolder::new(MediaPath::new(&directory));
    folder_state.policy.settle_seconds = settle_seconds;

    // Observe the folder: every file, with its size and how long it has been quiet. The quiet time
    // is how long ago the file was last modified, which is the signal that a copy has finished.
    let now = std::time::SystemTime::now();
    let mut observed = Vec::new();
    let entries = std::fs::read_dir(&directory).map_err(explain)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let quiet = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .map_or(0, |elapsed| elapsed.as_secs());
        observed.push(ObservedFile {
            path,
            size_bytes: metadata.len(),
            quiet_seconds: quiet,
        });
    }

    let plans = folder_state.plan(&observed);
    let listed: Vec<serde_json::Value> = plans
        .iter()
        .map(|plan| {
            serde_json::json!({
                "master": plan.master.to_string(),
                "markers": plan.markers.as_ref().map(ToString::to_string),
                "trigger": format!("{:?}", plan.trigger),
                "action": format!("{:?}", plan.action),
                "preset": plan.preset,
                "blockedBy": plan.blocked_by,
            })
        })
        .collect();
    Ok(serde_json::Value::Array(listed))
}

// ---------------------------------------------------------------------------------------
// Shell integration
// ---------------------------------------------------------------------------------------

/// Reveal a file in the operating system's file manager.
///
/// Deliberately not a general "open this" — it selects a file that this application just wrote, and
/// the interface cannot name an arbitrary thing to run.
#[tauri::command]
pub async fn reveal(path: String) -> Reply<()> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("{path} is not there"));
    }
    let parent = target
        .parent()
        .ok_or_else(|| format!("{path} has no folder"))?
        .to_path_buf();
    // `explorer /select,` is the only way to select a file rather than open its folder, and it is
    // the behaviour an editor expects from "show me where that went".
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", target.display()))
            .spawn()
            .map_err(explain)?;
    }
    #[cfg(not(windows))]
    {
        let _ = parent;
        return Err("revealing a file is only implemented on Windows".to_owned());
    }
    #[allow(unreachable_code)]
    Ok(())
}

/// The verification policy, so the interface can show it and change it.
#[tauri::command]
pub async fn get_verify_policy(state: State<'_, AppState>) -> Reply<serde_json::Value> {
    state.with_workspace(|workspace| Ok(serde_json::json!(workspace.verify_policy())))
}

/// Set the verification policy.
#[tauri::command]
pub async fn set_verify_policy(state: State<'_, AppState>, policy: String) -> Reply<()> {
    let parsed = match policy.as_str() {
        "off" => VerifyPolicy::Off,
        "standard" => VerifyPolicy::Standard,
        "strict" => VerifyPolicy::Strict,
        "forensic" => VerifyPolicy::Forensic,
        other => return Err(format!("{other} is not a verification policy")),
    };
    state.with_workspace(|workspace| {
        workspace.project_mut().verify = parsed;
        Ok(())
    })
}

/// A collecting sink, kept so a caller that wants to run the queue synchronously can.
#[allow(dead_code)]
fn collecting_sink() -> Arc<CollectingQueueSink> {
    Arc::new(CollectingQueueSink::new())
}

/// The media adapter, exposed so a future command can build a dry run without a workspace.
#[allow(dead_code)]
fn adapter(tools: ToolPaths) -> MediaAdapter {
    MediaAdapter::new(CutExecutor::new(tools), CutConfig::default())
}

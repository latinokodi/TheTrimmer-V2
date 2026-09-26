//! `project` and `export`: reading, editing and writing out what the store holds.

use std::sync::Arc;

use trimmer_app::{Clock, FileTranscripts, ProjectStore, SystemClock, Workspace};
use trimmer_core::{parse_timecode, MediaPath, Project, ProjectId, Segment, SegmentId};
use trimmer_export::{export, ExportFormat, ExportRequest};
use trimmer_store::document;
use uuid::Uuid;

use crate::cli::{
    ExportArgs, FormatArg, ProjectAddSegmentArgs, ProjectAddSourceArgs, ProjectExportArgs,
    ProjectIdArg, ProjectNewArgs, ProjectRemoveSegmentArgs,
};
use crate::context::{Context, Failure, Outcome, OK};

/// Parse a project id, or say what was wrong with it.
fn project_id(text: &str) -> Result<ProjectId, Failure> {
    Uuid::parse_str(text)
        .map(ProjectId)
        .map_err(|_| Failure::refused(format!("{text:?} is not a project id")))
}

/// Parse a segment id, or say what was wrong with it.
fn segment_id(text: &str) -> Result<SegmentId, Failure> {
    Uuid::parse_str(text)
        .map(SegmentId)
        .map_err(|_| Failure::refused(format!("{text:?} is not a segment id")))
}

/// Open a workspace over a stored project.
fn workspace(context: &Context, id: ProjectId) -> Result<Workspace, Failure> {
    let store = context.store()?;
    Workspace::open(
        &store,
        id,
        context.engine(),
        Arc::new(FileTranscripts),
        Arc::new(SystemClock) as Arc<dyn Clock>,
    )
    .map_err(|error| Failure::refused(error.to_string()))
}

/// `project new`
pub fn new_project(context: &Context, args: &ProjectNewArgs) -> Outcome {
    let store = context.store()?;
    let project = Project::new(args.name.clone(), args.by.clone(), SystemClock.now_unix());
    store
        .save(&project)
        .map_err(|error| Failure::internal(error))?;
    println!("{}", project.id);
    eprintln!(
        "created {:?} ({}) with {} presets",
        project.name,
        project.id,
        project.presets.len()
    );
    Ok(OK)
}

/// `project list`
pub fn list_projects(context: &Context) -> Outcome {
    let store = context.store()?;
    let listed = store.list().map_err(Failure::internal)?;
    if listed.is_empty() {
        eprintln!("no projects in {}", context.store_path.display());
        return Ok(OK);
    }
    for (id, name, updated_at) in listed {
        let when = time_stamp(updated_at);
        println!("{id}  {when}  {name}");
    }
    Ok(OK)
}

/// `project show`
pub fn show_project(context: &Context, args: &ProjectIdArg) -> Outcome {
    let id = project_id(&args.project)?;
    let workspace = workspace(context, id)?;
    println!("{}", workspace.describe());
    let views = workspace.segments();
    if views.is_empty() {
        println!("segments    none yet");
        return Ok(OK);
    }
    println!();
    println!("{:<4} {:<28} {:<16} {:<16} {:>8}  {}", "idx", "name", "in", "out", "frames", "problems");
    for (index, view) in views.iter().enumerate() {
        let frames = view
            .frames
            .map_or_else(|| "?".to_owned(), |frames| frames.to_string());
        let problems = if view.problems.is_empty() {
            String::new()
        } else {
            format!("!! {}", view.problems.join("; "))
        };
        println!(
            {index: <4} {:<28} {:<16} {:<16} {:>8}  {problems}",
            view.name, view.in_timeview.in_timecode, view
        );
    }
    Ok(OK)
}

/// `project add-source`
pub async fn add_source(context: &Context, args: &ProjectAddSourceArgs) -> Outcome {
    let id = project_id(&args.project)?;
    let mut workspace = workspace(context, id)?;
    let path = workspace
        .add_source(args.path.clone())
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;
    let store = context.store()?;
    workspace
        .save(&store)
        .map_err(|error| Failure::internal(error.to_string()))?;
    let source = workspace.project().source(&path);
    match source.and_then(|source| source.media.as_ref()) {
        Some(media) => println!("added {path}\n  {}", media.summary()),
        None => println!("added {path}\n  the file is not on disk; it is recorded anyway"),
    }
    Ok(OK)
}

/// `project add-segment`
pub async fn add_segment(context: &Context, args: &ProjectAddSegmentArgs) -> Outcome {
    let id = project_id(&args.project)?;
    let mut workspace = workspace(context, id)?;
    let source = MediaPath::new(args.source.clone());
    let rate = workspace
        .project()
        .rate_for(&source)
        .map_err(|error| Failure::refused(error.to_string()))?;

    let start = parse_timecode(&args.in_point, rate)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let end = match &args.out_point {
        Some(text) => {
            let frame =
                parse_timecode(text, rate).map_err(|error| Failure::refused(error.to_string()))?;
            Some(if args.out_exclusive { frame } else { frame + 1 })
        }
        None => None,
    };

    let mut segment = Segment::new(source, args.name.clone(), start, end.unwrap_or(start + 1));
    segment.end_frame = end;
    segment.preset = args.preset.clone();
    segment.handle_frames = args.handles;

    let id = workspace
        .add_segment(segment)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let store = context.store()?;
    workspace
        .save(&store)
        .map_err(|error| Failure::internal(error.to_string()))?;
    println!("{id}");
    eprintln!("added {:?}", args.name);
    Ok(OK)
}

/// `project remove-segment`
pub fn remove_segment(context: &Context, args: &ProjectRemoveSegmentArgs) -> Outcome {
    let id = project_id(&args.project)?;
    let segment = segment_id(&args.segment)?;
    let mut workspace = workspace(context, id)?;
    let removed = workspace
        .remove_segment(segment)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let store = context.store()?;
    workspace
        .save(&store)
        .map_err(|error| Failure::internal(error.to_string()))?;
    println!("removed {:?} ({})", removed.name, removed.id);
    Ok(OK)
}

/// `project export`
pub fn export_document(context: &Context, args: &ProjectExportArgs) -> Outcome {
    let id = project_id(&args.project)?;
    let store = context.store()?;
    let project = store.load(id).map_err(Failure::refused)?;
    let text = document::to_json(&project).map_err(|error| Failure::internal(error.to_string()))?;
    std::fs::write(&args.output, text)
        .map_err(|error| Failure::internal(format!("{}: {error}", args.output.display())))?;
    println!("{}", args.output.display());
    Ok(OK)
}

/// `export`
pub fn export_timeline(context: &Context, args: &ExportArgs) -> Outcome {
    let id = project_id(&args.project)?;
    let store = context.store()?;
    let project = store.load(id).map_err(Failure::refused)?;
    let format: ExportFormat = FormatArg::from(args.format).into();

    // The resolver the export layer is handed. An absolute, canonicalised path is what an
    // editor's machine can open, and the export layer never touches the filesystem to decide
    // it — see that crate's module docs.
    let resolver = |path: &MediaPath| path.canonicalised().to_string();
    let segments: Vec<&Segment> = project.segments.iter().collect();
    let request = ExportRequest {
        project: &project,
        segments,
        timeline_start_frame: 0,
        sequence_name: project.name.clone(),
        timeline_rate: timeline_rate(&project),
        media_path_for: &resolver,
    };

    let product = export(&request, format).map_err(|error| Failure::refused(error.to_string()))?;
    std::fs::write(&args.output, &product.body)
        .map_err(|error| Failure::internal(format!("{}: {error}", args.output.display())))?;

    // The warnings are the point of the return value: a segment whose source was never probed
    // is skipped, and a caller that is not told would deliver an incomplete timeline.
    for warning in &product.warnings {
        eprintln!("warning: {warning}");
    }
    eprintln!(
        "wrote {} clip(s), {} frames, to {}",
        product.clip_count,
        product.total_frames,
        args.output.display()
    );
    Ok(OK)
}

/// The rate a timeline is drawn on.
///
/// The first probed source's rate, or 25 when nothing has been probed. A timeline has to be on
/// *some* grid, and inventing one from nothing is worse than naming a conventional default and
/// letting the export layer warn about the segments that disagree.
fn timeline_rate(project: &Project) -> trimmer_core::FrameRate {
    project
        .sources
        .values()
        .find_map(|source| source.rate())
        .unwrap_or(trimmer_core::FrameRate::FPS_25)
}

/// One instant as a UTC stamp, falling back to the raw count.
fn time_stamp(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix).map_or_else(
        |_| unix.to_string(),
        |moment| moment.to_string(),
    )
}

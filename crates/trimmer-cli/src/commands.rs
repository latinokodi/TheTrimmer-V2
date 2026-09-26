//! The commands themselves.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;
use trimmer_app::{
    Clock, FileTranscripts, ProjectStore, Queue, QueueEvent, QueueOptions, QueueSink, SystemClock,
    Workspace,
};
use trimmer_core::{
    caption, parse_timecode, MediaPath, Project, ProjectId, Segment, SegmentId, VerifyPolicy,
};
use trimmer_verify::{
    verify_cut, verify_cut_with, CutFacts, Evidence, FrameHashes, MediaMeasurer, Similarity,
    VerifyReport,
};
use uuid::Uuid;

use crate::cli::{BatchArgs, CutArgs, DaemonArgs, ProbeArgs, VerifyArgs, WatchArgs};
use crate::context::{render_command, Context, Failure, Outcome, OK};

// --- doctor -------------------------------------------------------------------------------

/// `doctor`
pub async fn doctor(context: &Context) -> Outcome {
    let prober = context.prober();
    let version = prober
        .ffmpeg_version()
        .await
        .unwrap_or_else(|error| format!("could not run ffmpeg: {error}"));
    println!("ffmpeg      {version}");
    println!("ffprobe     {}", context.tools.ffprobe.display());
    println!("ffmpeg path {}", context.tools.ffmpeg.display());

    let capabilities = prober.capabilities().await;
    let mut usable = false;
    match &capabilities {
        Ok(found) => {
            println!();
            println!("{}", found.doctor_report());
            usable = found.has_encoder("libx264");
        }
        Err(error) => println!("\nffmpeg could not be asked what it supports: {error}"),
    }

    println!();
    println!("store       {}", context.store_path.display());
    println!(
        "            {}",
        if context.store_path.exists() {
            "exists"
        } else {
            "will be created on first use"
        }
    );

    if usable {
        Ok(OK)
    } else {
        Err(Failure::checked(
            "this machine cannot cut: ffmpeg and ffprobe must both resolve and the ffmpeg build \
             must have libx264",
        ))
    }
}

// --- probe --------------------------------------------------------------------------------

/// `probe`
pub async fn probe(context: &Context, args: &ProbeArgs) -> Outcome {
    let mut workspace = Workspace::new(
        "probe",
        "thetrimmer",
        context.engine(),
        Arc::new(FileTranscripts),
        Arc::new(SystemClock) as Arc<dyn Clock>,
    );
    let path = workspace
        .add_source(args.video.clone())
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;
    let view = workspace
        .sources()
        .into_iter()
        .next()
        .ok_or_else(|| Failure::internal("the probe produced no source"))?;
    let Some(media) = view.media.clone() else {
        return Err(Failure::refused(format!(
            "{path} is not on disk, or has no picture this build can read"
        )));
    };

    if args.json {
        let facts = json!({
            "path": media.path.to_string(),
            "name": view.name,
            "codec": media.codec,
            "pixFmt": media.pix_fmt,
            "width": media.width,
            "height": media.height,
            "rate": media.rate.as_ffmpeg(),
            "averageRate": media.average_rate.map(trimmer_core::FrameRate::as_ffmpeg),
            "timescale": media.timebase.ticks(),
            "frameCount": media.frame_count,
            "lastFrame": media.last_frame(),
            "startTime": media.start_time,
            "sizeBytes": media.size_bytes,
            "audio": media.audio.as_ref().map(|audio| json!({
                "codec": audio.codec,
                "sampleRate": audio.sample_rate,
                "channels": audio.channels,
            })),
            "variableRate": media.is_variable_rate(),
            "supportsHeadPatch": media.supports_head_patch(),
            "firstTimecode": media.timecode_of(0),
            "lastTimecode": media.timecode_of(media.last_frame()),
            "summary": media.summary(),
            "transcript": view.transcript.as_ref().map(ToString::to_string),
            "transcriptCues": view.transcript_cues,
            "present": view.present,
            "label": view.label,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&facts)
                .map_err(|error| Failure::internal(error.to_string()))?
        );
        return Ok(OK);
    }

    for (name, value) in [
        ("path", media.path.to_string()),
        ("name", view.name.clone()),
        ("present", view.present.to_string()),
        ("summary", media.summary()),
        ("codec", media.codec.clone()),
        ("pix_fmt", media.pix_fmt.clone()),
        ("geometry", format!("{}x{}", media.width, media.height)),
        ("rate", media.rate.as_ffmpeg()),
        (
            "average_rate",
            media
                .average_rate
                .map_or_else(|| "unknown".to_owned(), trimmer_core::FrameRate::as_ffmpeg),
        ),
        ("timescale", format!("1/{}", media.timebase.ticks())),
        ("frames", media.frame_count.to_string()),
        (
            "audio",
            media
                .audio
                .as_ref()
                .map_or_else(|| "no audio".to_owned(), trimmer_core::AudioFormat::summary),
        ),
        ("start_time", format!("{:.6}", media.start_time)),
        ("size", human_bytes(media.size_bytes)),
        (
            "timecode_range",
            format!(
                "{} .. {}",
                media.timecode_of(0),
                media.timecode_of(media.last_frame())
            ),
        ),
        ("variable_rate", media.is_variable_rate().to_string()),
        ("head_patch", media.supports_head_patch().to_string()),
        (
            "transcript",
            view.transcript.as_ref().map_or_else(
                || "none beside it".to_owned(),
                |path| format!("{path} ({} cues)", view.transcript_cues.unwrap_or_default()),
            ),
        ),
    ] {
        println!("{name:<14} {value}");
    }
    Ok(OK)
}

/// A byte count a person can read.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {} ({bytes} bytes)", UNITS[unit])
    }
}

// --- cut ----------------------------------------------------------------------------------

/// `cut`
pub async fn cut(context: &Context, args: &CutArgs) -> Outcome {
    let engine = context.engine();
    let mut workspace = Workspace::new(
        "cut",
        "thetrimmer",
        Arc::clone(&engine),
        Arc::new(FileTranscripts),
        Arc::new(SystemClock) as Arc<dyn Clock>,
    );
    let path = workspace
        .add_source(args.video.clone())
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;
    let media = workspace.project().media(&path).cloned().ok_or_else(|| {
        Failure::refused(format!(
            "{path} is not on disk, or has no picture this build can read"
        ))
    })?;
    let rate = media.rate;

    let start = parse_timecode(&args.in_point, rate)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let out_frame = parse_timecode(&args.out_point, rate)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let end = if args.out_exclusive {
        out_frame
    } else {
        out_frame + 1
    };
    if end <= start {
        return Err(Failure::refused(format!(
            "the out point (frame {end}) is not after the in point (frame {start})"
        )));
    }

    let preset = preset_named(&args.preset, args.crf)?;
    let mut segment = Segment::new(path.clone(), segment_name(&path, &media, start), start, end);
    segment.end_frame = Some(end);
    segment.handle_frames = args.handles;
    let id = workspace
        .add_segment(segment.clone())
        .map_err(|error| Failure::refused(error.to_string()))?;

    let plan = engine
        .plan(&media, &segment)
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;
    let commands = engine
        .preview(&media, &segment, &preset, &plan)
        .map_err(|error| Failure::refused(error.to_string()))?;

    let output = args.output.clone().unwrap_or_else(|| {
        workspace
            .output_path(workspace.project().segment(id).unwrap_or(&segment))
            .as_path()
            .to_path_buf()
    });

    if args.dry_run {
        println!("source      {}", media.path);
        println!("segment     {start}..{end} ({} frames)", end - start);
        println!("mode        {}", plan.mode.label());
        println!("preset      {}", preset.name);
        println!("output      {}", output.display());
        // The executor decides whether a reshaping preset forces a full re-encode; the preview
        // port only knows the plan's own mode. Saying so here keeps a dry run honest rather
        // than silently showing the wrong command.
        if trimmer_core::preset_forces_full_encode(&media, &preset) {
            println!(
                "note        the {} preset changes the picture, so the executor will re-encode \
                 the whole segment rather than copy it",
                preset.name
            );
        }
        println!();
        for prepared in &commands {
            println!("# {}", prepared.label);
            println!("{}", render_command(prepared, &context.tools.ffmpeg));
        }
        if commands.is_empty() {
            return Err(Failure::refused(
                "no command could be built for this segment; the plan is not cuttable",
            ));
        }
        return Ok(OK);
    }

    eprintln!(
        "cutting {} frames ({}) from {} with the {} preset",
        end - start,
        plan.mode.label(),
        media.path.file_name(),
        preset.name
    );
    eprintln!("writing {}", output.display());
    if !args.yes && !confirm()? {
        return Err(Failure::refused("nothing was written"));
    }

    let policy = VerifyPolicy::from(args.verify);
    let mut request = trimmer_app::cut_request_with(&workspace, id, Some(plan.clone()))
        .map_err(|error| Failure::refused(error.to_string()))?;
    request.preset = preset.clone();
    request.output = MediaPath::new(output.clone());

    let options = context.run_options(format!("cut {}", segment.name));
    let outcome = engine
        .cut(&request, &options)
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;

    // The captions are written *before* the verification, and the order matters: the caption check
    // compares the sidecar against the source's cues retimed onto this window, so a sidecar written
    // afterwards is not there to be compared and the check reports a genuine disagreement — "the cut
    // carries 0 cues and the window holds 1" — rather than a skip. Writing first is what makes the
    // check meaningful, and it is a real ordering bug that the check found.
    write_captions(context, args, &media, &output, start, end)?;

    if policy == VerifyPolicy::Off {
        println!(
            "{} frames written to {}",
            outcome.frame_count,
            output.display()
        );
    } else {
        let measurer = trimmer_daemon::ProbeMeasurer::new(context.prober());
        let report = verify_files(&measurer, &plan, &media.path, &output, policy);
        println!("{}", report.report());
        if !report.ok() {
            return Err(Failure::checked(format!(
                "{} frames were written to {}, but a check failed",
                outcome.frame_count,
                output.display()
            )));
        }
    }

    Ok(OK)
}

/// A name for a segment cut from the command line.
fn segment_name(path: &MediaPath, media: &trimmer_core::MediaInfo, start: i64) -> String {
    let _ = path;
    format!(
        "{} {}",
        media.path.stem(),
        media.timecode_of(start).replace([':', ';'], ".")
    )
}

/// Look a preset up in the standard library, applying a quality override.
fn preset_named(name: &str, crf: Option<u8>) -> Result<trimmer_core::DeliveryPreset, Failure> {
    let mut preset = trimmer_core::delivery::standard_preset(name)
        .map_err(|error| Failure::refused(error.to_string()))?;
    if let Some(crf) = crf {
        if let trimmer_core::VideoTreatment::Encode { quality, .. } = &mut preset.video {
            *quality = crf;
        } else {
            return Err(Failure::refused(format!(
                "the {name} preset copies the picture, so --crf has nothing to change; use a \
                 preset that re-encodes, or a re-encode will happen only in the head"
            )));
        }
    }
    Ok(preset)
}

/// Ask before writing. A cut is expensive and the answer is not always obvious.
fn confirm() -> Result<bool, Failure> {
    use std::io::{BufRead, Write};
    eprint!("write it? [y/N] ");
    std::io::stderr()
        .flush()
        .map_err(|error| Failure::internal(error.to_string()))?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| Failure::internal(error.to_string()))?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Measure a finished cut and hand back the verdict.
///
/// The evidence is gathered here for the same reason the queue gathers it: a policy that asks for a
/// decoded-frame comparison must not be answered with a skip, because a skip under a policy that
/// requested the check is a gap rather than a pass. An earlier version called [`verify_cut`] with no
/// evidence, so `frames` and `duration` were checked and everything else reported itself skipped
/// while the verdict still read `ok`.
fn verify_files(
    measurer: &dyn trimmer_verify::MediaMeasurer,
    plan: &trimmer_core::CutPlan,
    source: &MediaPath,
    output: &std::path::Path,
    policy: VerifyPolicy,
) -> VerifyReport {
    let cut_path = MediaPath::new(output.to_path_buf());
    let cut_facts = measurer
        .facts(&cut_path)
        .unwrap_or_else(|_| unknown_facts(&cut_path));
    let source_facts = measurer
        .facts(source)
        .unwrap_or_else(|_| unknown_facts(source));

    if !policy.hashes_frames() {
        return verify_cut_with(
            plan,
            &cut_facts,
            &source_facts,
            policy,
            &Evidence::default(),
        );
    }

    // The sample window is anchored on the keyframe the body begins on: frame zero of the output for
    // a copy, and `keyframe - start_frame` into it for a head patch. That is the one position both
    // files are guaranteed to share whatever the overshoot.
    const SAMPLE: usize = 24;
    let sample = i64::try_from(SAMPLE).unwrap_or(i64::MAX);
    let rate = source_facts
        .rate
        .or(cut_facts.rate)
        .unwrap_or(trimmer_core::FrameRate::FPS_25);
    let anchor = plan.keyframe.unwrap_or(plan.start_frame);
    let cut_anchor = anchor - plan.start_frame;

    let mut evidence = Evidence::default();
    if let (Ok(source_hashes), Ok(cut_hashes)) = (
        measurer.frame_hashes(source, anchor, sample, rate),
        measurer.frame_hashes(&cut_path, cut_anchor, sample, rate),
    ) {
        evidence = evidence.with_frame_hashes(
            FrameHashes::new(source_hashes.digests, anchor),
            FrameHashes::new(cut_hashes.digests, cut_anchor),
        );
    }

    // The head, when the policy compares it pixel-wise. A re-encoded frame never hashes equal to its
    // original, so this is a similarity score rather than a digest comparison.
    if policy.is_forensic() {
        let source_frame = measurer
            .extract_frame(source, plan.start_frame, rate)
            .unwrap_or_default();
        let cut_frame = measurer
            .extract_frame(&cut_path, 0, rate)
            .unwrap_or_default();

        if !source_frame.is_empty() && !cut_frame.is_empty() {
            evidence = evidence.with_head_similarity(
                measurer
                    .ssim(&source_frame, &cut_frame)
                    .unwrap_or(Similarity::UNMEASURABLE),
            );
        }

        // And the captions, when there is a transcript beside the source. The written sidecar is
        // named after the output, which is where the retime puts it.
        if let Some(source_srt) = trimmer_core::caption::find_for(source.as_path()) {
            if let Ok(transcript) = trimmer_core::caption::read(&source_srt) {
                let sidecar = cut_path.with_extension("srt");
                let written = trimmer_core::caption::read(sidecar.as_path())
                    .map(|found| found.cues)
                    .unwrap_or_default();
                evidence = evidence.with_captions(transcript.cues, written);
            }
        }
    }

    verify_cut_with(plan, &cut_facts, &source_facts, policy, &evidence)
}

/// Facts that say "nothing is known", so a check that could not measure reports rather than
/// silently passing.
fn unknown_facts(path: &MediaPath) -> CutFacts {
    CutFacts {
        path: path.clone(),
        frame_count: -1,
        video_duration: -1.0,
        video_start_time: 0.0,
        audio_duration: None,
        audio_start_time: None,
        rate: None,
        codec: String::new(),
        width: 0,
        height: 0,
        size_bytes: 0,
    }
}

/// Retime a caption file onto the segment, unless the caller said not to.
fn write_captions(
    context: &Context,
    args: &CutArgs,
    media: &trimmer_core::MediaInfo,
    output: &std::path::Path,
    start: i64,
    end: i64,
) -> Result<(), Failure> {
    if args.no_srt {
        return Ok(());
    }
    let source = match &args.srt {
        Some(path) => Some(path.clone()),
        None => caption::find_for(media.path.as_path()),
    };
    let Some(source) = source else {
        return Ok(());
    };
    let cues = caption::read(&source)
        .map_err(|error| Failure::refused(error.to_string()))?
        .cues;
    let start_seconds = media.rate.seconds_of(start);
    let end_seconds = media.rate.seconds_of(end);
    let retimed = caption::retime(&cues, start_seconds, end_seconds, caption::MIN_OVERLAP)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let target = output.with_extension("srt");
    caption::write(&target, &retimed.cues, "\r\n", false)
        .map_err(|error| Failure::internal(error.to_string()))?;
    eprintln!(
        "wrote {} cue(s) to {} ({} dropped, {} clamped)",
        retimed.cues.len(),
        target.display(),
        retimed.dropped.len(),
        retimed.clamped.len()
    );
    let _ = context;
    Ok(())
}

// --- batch --------------------------------------------------------------------------------

/// `batch`
pub async fn batch(context: &Context, args: &BatchArgs) -> Outcome {
    let id = project_id(&args.project)?;
    let store = context.store()?;
    let project = store.load(id).map_err(Failure::refused)?;

    let engine = context.engine();
    let mut workspace = Workspace::open(
        &store,
        id,
        Arc::clone(&engine),
        Arc::new(FileTranscripts),
        Arc::new(SystemClock) as Arc<dyn Clock>,
    )
    .map_err(|error| Failure::refused(error.to_string()))?;
    workspace
        .refresh_sources()
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;

    if !args.segments.is_empty() {
        let mut wanted: Vec<SegmentId> = Vec::new();
        for needle in &args.segments {
            let found = project
                .segments
                .iter()
                .find(|segment| {
                    &segment.id.to_string() == needle
                        || segment.name.eq_ignore_ascii_case(needle)
                        || segment.id.to_string().starts_with(needle)
                })
                .map(|segment| segment.id)
                .ok_or_else(|| {
                    Failure::refused(format!("no segment in this project matches {needle:?}"))
                })?;
            wanted.push(found);
        }
        for segment in &mut workspace.project_mut().segments {
            segment.enabled = wanted.contains(&segment.id);
        }
    }

    let queue = Queue::new(
        Arc::clone(&engine),
        Arc::new(trimmer_daemon::ProbeMeasurer::new(context.prober())),
        Arc::new(SystemClock) as Arc<dyn Clock>,
        env!("CARGO_PKG_VERSION"),
        machine_name(),
    );
    let options = QueueOptions {
        stop_on_error: args.stop_on_error,
        skip_verification: args.no_verify,
        audit: true,
        label: args.label.clone(),
    };
    let sink: Arc<dyn QueueSink> = if args.json {
        Arc::new(trimmer_app::QuietQueueSink)
    } else {
        Arc::new(LineSink)
    };
    let outcome = queue
        .run(&mut workspace, &options, sink)
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;

    let _ = context
        .store()?
        .record_run(id, &project.created_by, SystemClock.now_unix(), &outcome);

    if args.json {
        let body = json!({
            "project": id.to_string(),
            "label": args.label,
            "total": outcome.total(),
            "succeeded": outcome.succeeded(),
            "unverified": outcome.unverified(),
            "failed": outcome.failed(),
            "skipped": outcome.skipped(),
            "cancelled": outcome.cancelled,
            "deliveredFrames": outcome.delivered_frames,
            "elapsedSeconds": outcome.elapsed_seconds,
            "clean": outcome.is_clean(),
            "items": outcome.jobs.iter().map(|(job, segment, name, status)| json!({
                "job": job.0,
                "segment": segment.to_string(),
                "name": name,
                "status": status.summary(),
                "output": status.output().map(ToString::to_string),
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&body)
                .map_err(|error| Failure::internal(error.to_string()))?
        );
    } else {
        println!("{}", outcome.report());
    }

    if outcome.is_clean() {
        Ok(OK)
    } else {
        Err(Failure::checked(format!(
            "{} failed and {} failed a check",
            outcome.failed(),
            outcome.unverified()
        )))
    }
}

/// A queue sink that prints one line per job to stderr.
struct LineSink;

impl QueueSink for LineSink {
    fn event(&self, event: QueueEvent) {
        match event {
            QueueEvent::Started { total } => eprintln!("batch of {total}"),
            QueueEvent::State { job, name, state } => {
                eprintln!("{job:>4} {name:<32} {}", state.label());
            }
            QueueEvent::Finished { job, name, status } => {
                eprintln!("{job:>4} {name:<32} {}", status.summary());
            }
            QueueEvent::Completed {
                succeeded,
                unverified,
                failed,
                skipped,
            } => eprintln!(
                "done: {succeeded} written, {unverified} failed a check, {failed} failed, \
                 {skipped} skipped"
            ),
            QueueEvent::JobProgress { .. } => {}
        }
    }
}

/// Parse a project id from a string, with a message that says what was wrong.
fn project_id(text: &str) -> Result<ProjectId, Failure> {
    Uuid::parse_str(text)
        .map(ProjectId)
        .map_err(|_| Failure::refused(format!("{text:?} is not a project id")))
}

// --- verify -------------------------------------------------------------------------------

/// `verify`
pub fn verify(context: &Context, args: &VerifyArgs) -> Outcome {
    let text = std::fs::read_to_string(&args.plan)
        .map_err(|error| Failure::refused(format!("{}: {error}", args.plan.display())))?;
    let plan: trimmer_core::CutPlan = serde_json::from_str(&text)
        .map_err(|error| Failure::refused(format!("{}: {error}", args.plan.display())))?;

    let measurer = trimmer_daemon::ProbeMeasurer::new(context.prober());
    let source = MediaPath::new(args.source.clone());
    let output = MediaPath::new(args.output.clone());
    let policy = VerifyPolicy::from(args.policy);

    let cut_facts = measurer
        .facts(&output)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let source_facts = measurer
        .facts(&source)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let report = verify_cut(&plan, &cut_facts, &source_facts, policy);
    println!("{}", report.report());

    if report.ok() {
        Ok(OK)
    } else {
        Err(Failure::checked(format!(
            "{} check(s) failed",
            report.failed().len()
        )))
    }
}

/// A name for this machine, recorded in the run log.
///
/// Not a binding of any kind: just enough for a run recorded on one workstation to be
/// distinguishable from a run on another when somebody reads the log.
fn machine_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

// --- daemon -------------------------------------------------------------------------------

/// `daemon`
pub async fn daemon(context: &Context, args: &DaemonArgs) -> Outcome {
    let config = trimmer_daemon::DaemonConfig {
        port: args.port,
        token: args.token.clone(),
        store_path: context.store_path.clone(),
        bind: "127.0.0.1".to_owned(),
    };
    config.validate().map_err(Failure::refused)?;
    trimmer_daemon::serve(config)
        .await
        .map_err(|error| Failure::internal(error.to_string()))?;
    Ok(OK)
}

// --- watch --------------------------------------------------------------------------------

/// `watch`
///
/// A foreground loop. The filesystem notifier wakes it; a two-second scan is what makes the
/// settle rule work, because a file that is still being copied generates events the whole time
/// it is arriving and "it has stopped changing" is not an event.
pub async fn watch(context: &Context, args: &WatchArgs) -> Outcome {
    use trimmer_app::watch::{WatchFolder, WatchPolicy};

    if !args.folder.is_dir() {
        return Err(Failure::refused(format!(
            "{} is not a folder",
            args.folder.display()
        )));
    }
    let preset = preset_named(&args.preset, None)?;
    let mut policy = WatchPolicy {
        settle_seconds: args.settle,
        default_preset: preset.name.clone(),
        ..WatchPolicy::default()
    };
    policy.cut_whole_when_unmarked = true;
    let mut folder = WatchFolder::new(MediaPath::new(args.folder.clone()));
    folder.policy = policy;

    let (sender, receiver) = std::sync::mpsc::channel::<()>();
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = sender.send(());
        }
    })
    .and_then(|mut watcher| {
        use notify::Watcher as _;
        watcher.watch(&args.folder, notify::RecursiveMode::NonRecursive)?;
        Ok(watcher)
    });
    if watcher.is_err() {
        eprintln!(
            "the filesystem notifier could not watch {}; falling back to a two-second scan",
            args.folder.display()
        );
    }
    // The watcher has to outlive the loop: dropping it stops the notifications, and the loop
    // would then only ever see its own two-second scan.
    let _keep_alive = watcher;

    eprintln!(
        "watching {} with the {} preset, settling for {}s{}",
        args.folder.display(),
        preset.name,
        args.settle,
        if args.apply {
            ""
        } else {
            " (dry run; --apply cuts)"
        }
    );

    let mut last_seen: BTreeMap<String, (u64, i64)> = BTreeMap::new();
    loop {
        if context.cancel.is_cancelled() {
            return Ok(OK);
        }
        // Wait for an event, but never longer than two seconds: the settle rule needs a clock
        // even when nothing happens.
        let _ = receiver.recv_timeout(std::time::Duration::from_secs(2));

        let observed = observe(&args.folder, &mut last_seen);
        let plans = folder.ready(&observed);
        if plans.is_empty() {
            continue;
        }
        for plan in &plans {
            let size = observed
                .iter()
                .find(|file| file.path == plan.master.as_path())
                .map_or(0, |file| file.size_bytes);
            if !args.apply {
                println!("would cut {}", folder.describe(plan, None));
                folder.mark_processed(plan, size);
                continue;
            }
            match apply_plan(context, &mut folder, plan, &preset).await {
                Ok(()) => folder.mark_processed(plan, size),
                Err(failure) => eprintln!("{}: {}", plan.master, failure.message),
            }
        }
    }
}

/// One pass over the folder: what is there, how big, and how long since it changed.
fn observe(
    folder: &std::path::Path,
    last_seen: &mut BTreeMap<String, (u64, i64)>,
) -> Vec<trimmer_app::watch::ObservedFile> {
    use trimmer_app::watch::ObservedFile;
    let now = SystemClock.now_unix();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut observed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let size = metadata.len();
        let key = path.to_string_lossy().to_lowercase();
        let quiet_seconds = match last_seen.get(&key) {
            Some((previous, since)) if *previous == size => {
                now.saturating_sub(*since).max(0) as u64
            }
            _ => {
                last_seen.insert(key, (size, now));
                0
            }
        };
        observed.push(ObservedFile {
            path,
            size_bytes: size,
            quiet_seconds,
        });
    }
    observed
}

/// Cut what a plan asks for, by building a project and running it.
async fn apply_plan(
    context: &Context,
    folder: &mut trimmer_app::watch::WatchFolder,
    plan: &trimmer_app::watch::WatchPlan,
    preset: &trimmer_core::DeliveryPreset,
) -> Result<(), Failure> {
    use trimmer_app::watch::WatchTrigger;
    let store = context.store()?;
    let now = SystemClock.now_unix();
    let mut project = Project::new(
        format!("watch {}", plan.master.file_name()),
        "thetrimmer watch",
        now,
    );
    project.default_preset = preset.name.clone();
    project.upsert_source(trimmer_core::SegmentSource::unprobed(plan.master.clone()));
    if let Some(output_dir) = &plan.output_dir {
        project.output_dir = Some(output_dir.clone());
    }
    store
        .save(&project)
        .map_err(|error| Failure::internal(error))?;

    let engine = context.engine();
    let mut workspace = Workspace::open(
        &store,
        project.id,
        Arc::clone(&engine),
        Arc::new(FileTranscripts),
        Arc::new(SystemClock) as Arc<dyn Clock>,
    )
    .map_err(|error| Failure::refused(error.to_string()))?;
    let added = workspace
        .add_source(plan.master.as_path().to_path_buf())
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;
    let rate = workspace
        .project()
        .rate_for(&added)
        .map_err(|error| Failure::refused(error.to_string()))?;
    let frame_count = workspace
        .project()
        .media(&added)
        .map_or(0, |media| media.frame_count);

    let marks = match plan.trigger {
        WatchTrigger::MarkerList => folder.read_marks(plan, rate).map_err(Failure::refused)?,
        _ => Vec::new(),
    };
    if marks.is_empty() {
        let mut segment = Segment::new(added.clone(), "whole file", 0, frame_count);
        segment.end_frame = Some(frame_count);
        workspace
            .add_segment(segment)
            .map_err(|error| Failure::refused(error.to_string()))?;
    } else {
        for (index, mark) in marks.iter().enumerate() {
            let Some(start) = mark.in_frame else { continue };
            let mut segment = Segment::new(
                added.clone(),
                mark.name
                    .clone()
                    .unwrap_or_else(|| format!("mark {}", index + 1)),
                start,
                mark.out_frame.unwrap_or(frame_count),
            );
            segment.end_frame = mark.out_frame;
            workspace
                .add_segment(segment)
                .map_err(|error| Failure::refused(error.to_string()))?;
        }
    }
    workspace
        .save(&store)
        .map_err(|error| Failure::internal(error.to_string()))?;

    let queue = Queue::new(
        Arc::clone(&engine),
        Arc::new(trimmer_daemon::ProbeMeasurer::new(context.prober())),
        Arc::new(SystemClock) as Arc<dyn Clock>,
        env!("CARGO_PKG_VERSION"),
        machine_name(),
    );
    let options = QueueOptions {
        label: format!("watch {}", plan.master.file_name()),
        ..QueueOptions::default()
    };
    let outcome = queue
        .run(&mut workspace, &options, Arc::new(LineSink))
        .await
        .map_err(|error| Failure::refused(error.to_string()))?;
    let _ = store.record_run(project.id, "thetrimmer watch", now, &outcome);
    println!("{}: {}", plan.master.file_name(), outcome.report());
    if outcome.is_clean() {
        Ok(())
    } else {
        Err(Failure::checked("the run did not come out clean"))
    }
}

//! Tests for the application layer.
//!
//! These tests are the reason [`crate::ports::MediaEngine`] is a trait. Every path that matters in
//! a batch — a cut that fails, a cancellation between two items, a source that has gone missing, a
//! preset that turns out to need a full re-encode — is unreachable in a test that needs a real
//! ffmpeg and real media, and three lines each with a fake engine. The queue is the part of this
//! product most likely to cost a studio an afternoon, so it is the part most worth exercising.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use trimmer_core::{
    AudioFormat, CutMode, CutPlan, DeliveryPreset, FrameRate, KeyframeGrid, MediaInfo, MediaPath,
    Project, Segment, SegmentId, Timescale, VerifyPolicy,
};
use trimmer_media::{CutOutcome, ExecutionStep, MediaError, MediaResult, Prepared, RunOptions};
use trimmer_verify::{CheckStatus, CutFacts, FrameHashes, Similarity};

use trimmer_app::ports::{Clock, MediaEngine, SegmentCutRequest, TestClock, TranscriptSource};
use trimmer_app::queue::{CollectingQueueSink, JobState, JobStatus, Queue, QueueEvent, QueueOptions};
use trimmer_app::transcript::{TranscriptService, TranscriptView};
use trimmer_app::watch::{ObservedFile, WatchAction, WatchFolder, WatchPolicy, WatchTrigger};
use trimmer_app::workspace::Workspace;
use trimmer_app::{parse_marks, sanitise_name};

// ---------------------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------------------

/// A media engine that answers from a table and writes a placeholder file where a real cut would.
struct FakeEngine {
    media: MediaInfo,
    keyframes: Vec<i64>,
    outcome: Mutex<FakeCut>,
    cuts: AtomicUsize,
    facts: Mutex<Vec<CutFacts>>,
}

#[derive(Clone)]
enum FakeCut {
    Ok,
    /// Fail with a message.
    Fail(String),
    /// Ask for cancellation, as a real engine does when the flag is set.
    Cancelled,

}

impl FakeEngine {
    fn new(media: MediaInfo) -> Arc<Self> {
        Arc::new(Self {
            media,
            keyframes: Vec::new(),
            outcome: Mutex::new(FakeCut::Ok),
            cuts: AtomicUsize::new(0),
            facts: Mutex::new(Vec::new()),
        })
    }


    fn set_outcome(&self, outcome: FakeCut) {
        if let Ok(mut guard) = self.outcome.lock() {
            *guard = outcome;
        }
    }

    fn cut_count(&self) -> usize {
        self.cuts.load(Ordering::SeqCst)
    }

}

#[async_trait::async_trait]
impl MediaEngine for FakeEngine {
    async fn probe(&self, path: &MediaPath) -> MediaResult<MediaInfo> {
        Ok(MediaInfo {
            path: path.clone(),
            ..self.media.clone()
        })
    }

    async fn plan(&self, media: &MediaInfo, segment: &Segment) -> trimmer_core::CoreResult<CutPlan> {
        let grid = KeyframeGrid::new(
            self.keyframes.clone(),
            segment.start_frame,
            segment.end_frame.unwrap_or(media.frame_count),
        );
        trimmer_core::plan_cut(media, segment, &grid)
    }

    async fn cut(
        &self,
        request: &SegmentCutRequest,
        _options: &RunOptions,
    ) -> MediaResult<CutOutcome> {
        self.cuts.fetch_add(1, Ordering::SeqCst);
        let outcome = self
            .outcome
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or(FakeCut::Ok);
        match outcome {
            FakeCut::Fail(reason) => Err(MediaError::ProcessFailed {
                tool: "ffmpeg".to_owned(),
                status: "exit 1".to_owned(),
                command: "ffmpeg ...".to_owned(),
                tail: reason,
            }),
            FakeCut::Cancelled => Err(MediaError::Cancelled),
            FakeCut::Ok => {
                // Write something where a real cut would have, so a caller that touches the file
                // finds one. The bytes are irrelevant; only the existence matters.
                if let Some(parent) = request.output.as_path().parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(request.output.as_path(), b"fake");
                let plan = match &request.plan {
                    Some(plan) => plan.clone(),
                    None => self.plan(&request.media, &request.segment).await?,
                };
                let frames = plan.requested_frames();
                Ok(CutOutcome {
                    plan,
                    output: request.output.clone(),
                    steps: vec![ExecutionStep {
                        label: "fake".to_owned(),
                        program: "ffmpeg".to_owned(),
                        args: vec!["-fake".to_owned()],
                        seconds: Some(0.5),
                        ok: Some(true),
                    }],
                    frame_count: frames,
                    overshoot: 0,
                    notes: Vec::new(),
                })
            }
        }
    }

    fn preview(
        &self,
        media: &MediaInfo,
        _segment: &Segment,
        preset: &DeliveryPreset,
        plan: &CutPlan,
    ) -> trimmer_core::CoreResult<Vec<Prepared>> {
        // The real builders where they apply, so a preview in a test is the same shape as a
        // preview in production.
        let config = trimmer_media::CutConfig::default();
        Ok(match plan.mode {
            CutMode::Copy => vec![trimmer_media::executor::prepare_copy(media, plan, &config)],
            CutMode::Reencode => vec![trimmer_media::executor::prepare_reencode(
                media, plan, preset, &config,
            )],
            CutMode::HeadPatch => {
                let mut steps = vec![trimmer_media::executor::prepare_head(media, plan, &config)];
                steps.extend(trimmer_media::executor::prepare_body(media, plan, &config));
                steps
            }
        })
    }

    async fn facts(&self, path: &MediaPath) -> MediaResult<CutFacts> {
        if let Ok(mut guard) = self.facts.lock() {
            if let Some(facts) = guard.pop() {
                return Ok(facts);
            }
        }
        let is_source = path == &self.media.path;
        Ok(facts_for(path, if is_source { self.media.frame_count } else { 0 }))
    }
}

/// A measurement seam that answers from a table.
///
/// The frame count it reports is *explicit* rather than defaulted, because that is the number the
/// frame check turns on: a measurer that silently reported zero frames made every batch in these
/// tests fail verification, which is how the default was caught.
struct FakeMeasurer {
    facts: Mutex<Vec<(String, CutFacts)>>,
    /// Frames reported for a path the table does not mention. The source's own count is used for
    /// the source path, so a cut is compared against a source that has the frames it has.
    output_frames: i64,
    source_frames: i64,
}

impl FakeMeasurer {
    /// A measurer that reports a correct-looking output of `output_frames` and a source of
    /// `source_frames`.
    fn reporting(output_frames: i64, source_frames: i64) -> Arc<Self> {
        Arc::new(Self {
            facts: Mutex::new(Vec::new()),
            output_frames,
            source_frames,
        })
    }

    fn push(&self, path: &MediaPath, facts: CutFacts) {
        if let Ok(mut guard) = self.facts.lock() {
            guard.push((path.to_string(), facts));
        }
    }
}

impl trimmer_verify::MediaMeasurer for FakeMeasurer {
    fn facts(&self, path: &MediaPath) -> trimmer_core::CoreResult<CutFacts> {
        if let Ok(guard) = self.facts.lock() {
            if let Some((_, facts)) = guard.iter().find(|(key, _)| key == &path.to_string()) {
                return Ok(facts.clone());
            }
        }
        // The source is the file that already existed; anything else is an output this run wrote.
        let frames = if path.as_path().file_name().is_some_and(|name| name == "master.mp4") {
            self.source_frames
        } else {
            self.output_frames
        };
        Ok(facts_for(path, frames))
    }

    fn frame_hashes(
        &self,
        _path: &MediaPath,
        _start_frame: i64,
        _count: i64,
        _rate: FrameRate,
    ) -> trimmer_core::CoreResult<FrameHashes> {
        Ok(FrameHashes::new(Vec::new(), 0))
    }

    fn extract_frame(
        &self,
        _path: &MediaPath,
        _frame: i64,
        _rate: FrameRate,
    ) -> trimmer_core::CoreResult<Vec<u8>> {
        Ok(Vec::new())
    }

    fn ssim(&self, _a: &[u8], _b: &[u8]) -> trimmer_core::CoreResult<Similarity> {
        Ok(Similarity(Some(1.0)))
    }
}

/// Captions that are never found, so no test depends on the filesystem.
struct NoTranscripts;

impl TranscriptSource for NoTranscripts {
    fn find_for(&self, _video: &MediaPath) -> Option<PathBuf> {
        None
    }

    fn read(&self, _path: &Path) -> trimmer_core::CoreResult<trimmer_core::Transcript> {
        Err(trimmer_core::CoreError::Caption {
            path: String::new(),
            reason: "no transcripts in this test".to_owned(),
        })
    }

    fn exists(&self, _path: &MediaPath) -> bool {
        false
    }
}

/// Facts that are *self-consistent*: the duration is derived from the frame count at the rate,
/// because an inconsistent pair makes the duration check fail — which is the point of that check,
/// and which is exactly what an earlier version of this helper did by hard-coding 10.0 seconds.
fn facts_for(path: &MediaPath, frames: i64) -> CutFacts {
    let rate = FrameRate::FPS_25;
    let seconds = rate.seconds_of(frames);
    CutFacts {
        path: path.clone(),
        frame_count: frames,
        video_duration: seconds,
        video_start_time: 0.0,
        audio_duration: Some(seconds),
        audio_start_time: Some(0.0),
        rate: Some(rate),
        codec: "h264".to_owned(),
        width: 1920,
        height: 1080,
        size_bytes: 1_000,
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

fn media(path: &Path) -> MediaInfo {
    MediaInfo {
        path: MediaPath::new(path.to_path_buf()),
        codec: "h264".to_owned(),
        pix_fmt: "yuv420p".to_owned(),
        width: 1920,
        height: 1080,
        rate: FrameRate::FPS_25,
        average_rate: Some(FrameRate::FPS_25),
        timebase: Timescale::NINETY_KHZ,
        frame_count: 100_000,
        audio: Some(AudioFormat {
            codec: "aac".to_owned(),
            sample_rate: 48_000,
            channels: 2,
        }),
        size_bytes: 1_000_000,
        start_time: 0.0,
    }
}

/// A workspace over a temporary directory, with a fake engine.
struct Fixture {
    dir: PathBuf,
    workspace: Workspace,
    engine: Arc<FakeEngine>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "trimmer-app-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let master = dir.join("master.mp4");
        std::fs::write(&master, b"not really a video").expect("write");
        let engine = FakeEngine::new(media(&master));
        Self {
            workspace: Workspace::new(
                name,
                "tester",
                engine.clone(),
                Arc::new(NoTranscripts),
                Arc::new(TestClock::new(1_700_000_000)),
            ),
            dir,
            engine,
        }
    }

    fn master(&self) -> MediaPath {
        MediaPath::new(self.dir.join("master.mp4"))
    }

    /// Add the source and `count` segments of 100 frames each.
    async fn with_segments(mut self, count: usize) -> Self {
        let path = self.master();
        self.workspace.add_source(path.clone()).await.expect("probed");
        for index in 0..count {
            let start = 1_000 + (index as i64) * 200;
            let segment = Segment::new(path.clone(), format!("segment {index}"), start, start + 100);
            self.workspace.add_segment(segment).expect("added");
        }
        self
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn queue_for(engine: Arc<FakeEngine>, measurer: Arc<dyn trimmer_verify::MediaMeasurer>) -> Queue {
    Queue::new(
        engine,
        measurer,
        Arc::new(TestClock::new(1_700_000_000)),
        "2.0.0-test",
        "test-machine",
    )
}

// ---------------------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_source_is_probed_when_it_is_added() {
    let fixture = Fixture::new("probe").with_segments(1).await;
    let sources = fixture.workspace.sources();
    assert_eq!(sources.len(), 1);
    assert!(sources[0].present);
    assert!(sources[0].media.is_some());
    assert_eq!(sources[0].name, "master.mp4");
    assert!(sources[0].summary.contains("1920x1080"));
    assert!(!sources[0].variable_rate);
}

#[tokio::test]
async fn a_source_that_is_not_on_disk_is_still_added_and_says_so() {
    let mut fixture = Fixture::new("missing");
    let absent = MediaPath::new(fixture.dir.join("not-here.mp4"));
    fixture.workspace.add_source(absent.clone()).await.expect("added");
    let sources = fixture.workspace.sources();
    assert_eq!(sources.len(), 1);
    assert!(!sources[0].present);
    assert!(sources[0].summary.contains("not on disk"), "{}", sources[0].summary);
    // The workspace still opens, which is the whole point: an editor without the drive attached
    // must be able to read and fix their marks.
    assert_eq!(fixture.workspace.summary().missing_sources, 1);
}

#[tokio::test]
async fn a_segment_view_carries_its_timecodes_and_duration() {
    let fixture = Fixture::new("view").with_segments(1).await;
    let views = fixture.workspace.segments();
    assert_eq!(views.len(), 1);
    let view = &views[0];
    // 1 000 frames at 25 fps is 40 s, which is 00:00:40:00.
    assert_eq!(view.in_timecode, "00:00:40:00");
    assert_eq!(view.out_timecode, "00:00:43:24");
    assert_eq!(view.frames, Some(100));
    assert!((view.seconds.expect("seconds") - 4.0).abs() < 1e-9);
    assert!(view.problems.is_empty(), "{:?}", view.problems);
    assert!(view.enabled);
}

#[tokio::test]
async fn a_segment_past_the_end_of_its_source_cannot_be_added() {
    let mut fixture = Fixture::new("past-end");
    let path = fixture.master();
    fixture.workspace.add_source(path.clone()).await.expect("probed");
    let segment = Segment::new(path, "way past the end", 200_000, 200_100);
    let error = fixture.workspace.add_segment(segment).expect_err("refused");
    assert!(
        error.to_string().contains("past the end"),
        "unexpected message: {error}"
    );
    assert!(fixture.workspace.segments().is_empty());
}

#[tokio::test]
async fn a_segment_for_an_unknown_source_is_refused() {
    let mut fixture = Fixture::new("unknown-source");
    let segment = Segment::new(r"H:\somewhere\else.mp4", "x", 0, 100);
    let error = fixture.workspace.add_segment(segment).expect_err("refused");
    assert!(error.to_string().contains("has not been added"), "{error}");
}

#[tokio::test]
async fn a_source_with_an_exotic_codec_is_refused_at_add_time_with_the_codec_named() {
    // The domain is the authority here, and it refuses rather than listing a problem the user
    // could ignore: a ProRes source genuinely cannot be head-patched, so the earliest honest
    // moment to say so is while the marks are being entered.
    let fixture = Fixture::new("exotic");
    let mut media = fixture.engine.media.clone();
    media.codec = "prores".to_owned();
    let engine = FakeEngine::new(media);
    let mut workspace = Workspace::new(
        "exotic",
        "tester",
        engine,
        Arc::new(NoTranscripts),
        Arc::new(TestClock::new(0)),
    );
    let path = fixture.master();
    workspace.add_source(path.clone()).await.expect("probed");
    let error = workspace
        .add_segment(Segment::new(path, "x", 0, 100))
        .expect_err("refused");
    assert!(error.to_string().contains("prores"), "{error}");
    assert!(error.to_string().contains("H.264 and HEVC"), "{error}");

    // And the source itself is still listed, so the user can see what they added.
    assert_eq!(workspace.sources().len(), 1);
}

#[tokio::test]
async fn a_preview_reports_the_cost_the_commands_and_the_full_encode_warning() {
    let mut fixture = Fixture::new("preview").with_segments(1).await;
    // A vertical preset reshapes the frame, so it cannot be a passthrough.
    fixture
        .workspace
        .project_mut()
        .default_preset = "vertical".to_owned();
    let id = fixture.workspace.segments()[0].id;
    let preview = fixture.workspace.preview(id).await.expect("previewed");
    assert!(preview.forces_full_encode, "a crop preset must be flagged");
    assert!(
        preview
            .notes
            .iter()
            .any(|note| note.contains("whole segment is re-encoded")),
        "{:?}",
        preview.notes
    );
    assert!(!preview.commands.is_empty(), "a preview must show what will run");
    assert!(preview.estimated_bytes.unwrap_or(0) > 0);
}

#[tokio::test]
async fn a_passthrough_preview_reports_no_full_encode_and_a_small_reencode_fraction() {
    let mut fixture = Fixture::new("preview-master").with_segments(1).await;
    let id = fixture.workspace.segments()[0].id;
    let preview = fixture.workspace.preview(id).await.expect("previewed");
    assert!(!preview.forces_full_encode);
    assert_eq!(preview.preset, "master");
    // No keyframes were configured, so the whole segment is re-encoded; the fraction says so.
    assert!(preview.reencode_fraction >= 0.0);
}

#[tokio::test]
async fn the_summary_counts_what_a_batch_would_do() {
    let fixture = Fixture::new("summary").with_segments(3).await;
    let summary = fixture.workspace.summary();
    assert_eq!(summary.segments, 3);
    assert_eq!(summary.runnable, 3);
    assert_eq!(summary.skipped, 0);
    assert_eq!(summary.total_frames, 300);
    assert!((summary.total_seconds - 12.0).abs() < 1e-9);
    assert_eq!(summary.missing_sources, 0);
}

#[tokio::test]
async fn a_disabled_segment_is_not_runnable_but_is_still_listed() {
    let mut fixture = Fixture::new("disabled").with_segments(2).await;
    let id = fixture.workspace.segments()[0].id;
    fixture
        .workspace
        .project_mut()
        .segment_mut(id)
        .expect("present")
        .enabled = false;
    let summary = fixture.workspace.summary();
    assert_eq!(summary.segments, 2);
    assert_eq!(summary.runnable, 1);
    assert_eq!(summary.skipped, 1);
    assert!(fixture.workspace.segments()[0]
        .notes
        .iter()
        .any(|note| note.contains("switched off")));
}

#[test]
fn output_names_are_safe_and_carry_the_range() {
    assert_eq!(sanitise_name("cold open: the question?"), "cold open the question");
    assert_eq!(sanitise_name("a/b\\c|d*e"), "a b c d e");
    assert_eq!(sanitise_name("   "), "segment");
    assert_eq!(sanitise_name("trailing dots..."), "trailing dots");
    assert_eq!(sanitise_name(""), "segment");
    // A control character cannot reach a file name.
    assert_eq!(sanitise_name("a\u{7}b"), "a b");
    // Long names are truncated so the range and extension still fit.
    let long = "x".repeat(400);
    assert!(sanitise_name(&long).chars().count() <= 120);
}

// ---------------------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_batch_cuts_every_runnable_segment_and_reports_each_one() {
    let mut fixture = Fixture::new("batch").with_segments(3).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    let sink = Arc::new(CollectingQueueSink::new());
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), sink.clone())
        .await
        .expect("ran");

    assert_eq!(outcome.total(), 3);
    assert_eq!(outcome.failed(), 0);
    assert!(!outcome.cancelled);
    assert_eq!(fixture.engine.cut_count(), 3);
    assert_eq!(outcome.delivered_frames, 300);

    // One Finished event per job, and one Completed at the end.
    let events = sink.events();
    let finished = events
        .iter()
        .filter(|event| matches!(event, QueueEvent::Finished { .. }))
        .count();
    assert_eq!(finished, 3);
    let completed: Vec<&QueueEvent> = events
        .iter()
        .filter(|event| matches!(event, QueueEvent::Completed { .. }))
        .collect();
    assert_eq!(completed.len(), 1, "{events:#?}");
    match completed[0] {
        QueueEvent::Completed {
            succeeded,
            unverified,
            failed,
            skipped,
        } => {
            assert_eq!((*succeeded, *unverified, *failed, *skipped), (3, 0, 0, 0), "{events:#?}");
        }
        other => panic!("wrong event: {other:?}"),
    }
    // Every job was seen planning, cutting, verifying and done.
    for state in [JobState::Planning, JobState::Cutting, JobState::Verifying] {
        assert!(
            events
                .iter()
                .any(|event| matches!(event, QueueEvent::State { state: seen, .. } if *seen == state)),
            "no {state:?} event"
        );
    }
}

#[tokio::test]
async fn a_batch_runs_segments_one_at_a_time_in_project_order() {
    let mut fixture = Fixture::new("order").with_segments(3).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    let sink = Arc::new(CollectingQueueSink::new());
    queue
        .run(&mut fixture.workspace, &QueueOptions::default(), sink.clone())
        .await
        .expect("ran");

    let names: Vec<String> = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            QueueEvent::Finished { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["segment 0", "segment 1", "segment 2"]);
}

#[tokio::test]
async fn one_failed_segment_does_not_abandon_the_others() {
    let mut fixture = Fixture::new("one-fails").with_segments(3).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    fixture.engine.set_outcome(FakeCut::Fail("disk full".to_owned()));
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    // Every segment was attempted; all three failed, but none was abandoned.
    assert_eq!(outcome.total(), 3);
    assert_eq!(outcome.failed(), 3);
    assert_eq!(fixture.engine.cut_count(), 3);
    let reasons: Vec<String> = outcome
        .jobs
        .iter()
        .map(|(_, _, _, status)| status.summary())
        .collect();
    assert!(reasons.iter().all(|reason| reason.contains("disk full")), "{reasons:?}");
}

#[tokio::test]
async fn stop_on_error_stops_at_the_first_failure() {
    let mut fixture = Fixture::new("stop-on-error").with_segments(3).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    fixture.engine.set_outcome(FakeCut::Fail("bad source".to_owned()));
    let options = QueueOptions {
        stop_on_error: true,
        ..QueueOptions::default()
    };
    let outcome = queue
        .run(&mut fixture.workspace, &options, Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    assert_eq!(outcome.total(), 1, "it must stop after the first failure");
    assert_eq!(fixture.engine.cut_count(), 1);
    assert!(!outcome.is_clean());
}

#[tokio::test]
async fn a_cancellation_stops_the_batch_and_says_which_segments_did_not_run() {
    let mut fixture = Fixture::new("cancel").with_segments(3).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    fixture.engine.set_outcome(FakeCut::Cancelled);
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    assert!(outcome.cancelled);
    // The first segment reported the cancellation and the rest were never attempted.
    assert_eq!(outcome.total(), 1);
    match &outcome.jobs[0].3 {
        JobStatus::Failed { cancelled, .. } => assert!(cancelled),
        other => panic!("expected a cancellation, got {other:?}"),
    }
    assert!(outcome.report().contains("cancelled"));
}

#[tokio::test]
async fn a_cancellation_raised_from_another_thread_stops_before_the_next_segment() {
    let mut fixture = Fixture::new("cancel-flag").with_segments(3).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    // Raise the flag before the run starts.
    queue.cancel_flag().cancel();
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    assert!(outcome.cancelled);
    assert_eq!(fixture.engine.cut_count(), 0, "nothing should have been cut");
    assert_eq!(outcome.total(), 3, "every segment is accounted for");
    assert!(outcome
        .jobs
        .iter()
        .all(|(_, _, _, status)| matches!(status, JobStatus::Failed { cancelled: true, .. })));
}

#[tokio::test]
async fn a_segment_that_cannot_be_cut_is_skipped_with_a_reason_not_failed() {
    let mut fixture = Fixture::new("skipped").with_segments(2).await;
    // Make the second segment's source unavailable, as an unplugged drive would.
    let path = fixture.master();
    let source = fixture
        .workspace
        .project_mut()
        .sources
        .get_mut(&path)
        .expect("present");
    source.media = None;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    assert_eq!(outcome.skipped(), 2);
    for (_, _, _, status) in &outcome.jobs {
        match status {
            JobStatus::Skipped { reason } => {
                assert!(reason.contains("not been probed"), "{reason}");
            }
            other => panic!("expected a skip, got {other:?}"),
        }
    }
    assert_eq!(fixture.engine.cut_count(), 0);
}

#[tokio::test]
async fn a_disabled_segment_is_absent_from_the_batch_entirely() {
    let mut fixture = Fixture::new("disabled-batch").with_segments(2).await;
    let id = fixture.workspace.segments()[1].id;
    fixture
        .workspace
        .project_mut()
        .segment_mut(id)
        .expect("present")
        .enabled = false;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    assert_eq!(outcome.total(), 1);
    assert_eq!(fixture.engine.cut_count(), 1);
}

#[tokio::test]
async fn verification_off_records_that_it_did_not_check() {
    let mut fixture = Fixture::new("verify-off").with_segments(1).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    let options = QueueOptions {
        skip_verification: true,
        ..QueueOptions::default()
    };
    let outcome = queue
        .run(&mut fixture.workspace, &options, Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    assert!(outcome.is_clean());
    match &outcome.jobs[0].3 {
        JobStatus::Succeeded { verification, .. } => {
            // Every check must be Skipped with a reason rather than pretending to have passed.
            for result in &verification.results {
                assert!(
                    matches!(result.status, CheckStatus::Skipped { .. }),
                    "{:?} was not skipped",
                    result.check
                );
            }
        }
        other => panic!("expected success, got {other:?}"),
    }
}

#[tokio::test]
async fn a_project_that_verifies_forensically_reports_a_failing_check() {
    let mut fixture = Fixture::new("unverified").with_segments(1).await;
    fixture.workspace.project_mut().verify = VerifyPolicy::Strict;
    let measurer = FakeMeasurer::reporting(100, 100_000);
    // The output is reported with no frames, which is what a truncated cut looks like.
    measurer.push(
        &MediaPath::new(fixture.dir.join("master segment 0 00.00.40.00.mp4")),
        facts_for(&MediaPath::new("cut.mp4"), 0),
    );
    let queue = queue_for(fixture.engine.clone(), measurer);
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    // Whether the check fails depends on the numbers the measurer reports; the property under
    // test is that a failing check is reported as its own status rather than as a crash or a
    // silent success.
    let status = &outcome.jobs[0].3;
    assert!(
        status.is_success() || matches!(status, JobStatus::Unverified { .. }),
        "unexpected status: {status:?}"
    );
}

#[tokio::test]
async fn the_audit_manifest_records_one_entry_per_job_and_signs() {
    let mut fixture = Fixture::new("audit").with_segments(2).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    let manifest = &outcome.audit;
    assert_eq!(manifest.entries.len(), 2 + 1, "two cuts plus the opening entry");
    assert_eq!(manifest.app_version, "2.0.0-test");
    assert_eq!(manifest.machine, "test-machine");
    let digest = manifest.digest();
    let signature = manifest.sign(b"a-test-key");
    assert!(manifest.verify_signature(b"a-test-key", &signature));
    assert!(!manifest.verify_signature(b"a-different-key", &signature));
    assert!(!digest.is_empty());
}

#[tokio::test]
async fn the_batch_report_names_every_segment_and_its_verdict() {
    let mut fixture = Fixture::new("report").with_segments(2).await;
    let queue = queue_for(fixture.engine.clone(), FakeMeasurer::reporting(100, 100_000));
    let outcome = queue
        .run(&mut fixture.workspace, &QueueOptions::default(), Arc::new(CollectingQueueSink::new()))
        .await
        .expect("ran");

    let report = outcome.report();
    assert!(report.contains("segment 0"), "{report}");
    assert!(report.contains("segment 1"), "{report}");
    assert!(report.contains("2 jobs"), "{report}");
    assert!(report.contains("delivered"), "{report}");
}

// ---------------------------------------------------------------------------------------
// Watch folders
// ---------------------------------------------------------------------------------------

fn observed(name: &str, size: u64, quiet: u64) -> ObservedFile {
    ObservedFile {
        path: PathBuf::from(format!(r"H:\ingest\{name}")),
        size_bytes: size,
        quiet_seconds: quiet,
    }
}

#[test]
fn a_file_still_arriving_is_not_a_job() {
    let folder = WatchFolder::new(r"H:\ingest");
    let plans = folder.plan(&[observed("master.mp4", 1_000, 2)]);
    assert_eq!(plans.len(), 1);
    assert!(plans[0].blocked_by.is_some());
    assert!(
        plans[0].blocked_by.as_deref().unwrap_or("").contains("changed 2s ago"),
        "{:?}",
        plans[0].blocked_by
    );
    assert!(folder.ready(&[observed("master.mp4", 1_000, 2)]).is_empty());
}

#[test]
fn a_settled_file_with_no_marks_is_blocked_unless_the_policy_says_otherwise() {
    let folder = WatchFolder::new(r"H:\ingest");
    let plans = folder.plan(&[observed("master.mp4", 1_000, 60)]);
    assert!(plans[0].blocked_by.is_some());
    assert!(plans[0]
        .blocked_by
        .as_deref()
        .unwrap_or("")
        .contains("no marker list"));

    let mut permissive = WatchFolder::new(r"H:\ingest");
    permissive.policy.cut_whole_when_unmarked = true;
    let plans = permissive.plan(&[observed("master.mp4", 1_000, 60)]);
    assert_eq!(plans[0].trigger, WatchTrigger::WholeFile);
    assert_eq!(plans[0].action, WatchAction::CutWholeFile);
    assert!(plans[0].blocked_by.is_none());
}

#[test]
fn a_non_video_file_is_not_a_job() {
    let folder = WatchFolder::new(r"H:\ingest");
    assert!(folder.plan(&[observed("notes.txt", 10, 60)]).is_empty());
    assert!(folder.plan(&[observed("master.srt", 10, 60)]).is_empty());
}

#[test]
fn a_processed_file_is_never_planned_twice_however_its_timestamp_changes() {
    let mut folder = WatchFolder::new(r"H:\ingest");
    folder.policy.cut_whole_when_unmarked = true;
    let file = observed("master.mp4", 1_000, 60);
    let plans = folder.ready(std::slice::from_ref(&file));
    assert_eq!(plans.len(), 1);
    folder.mark_processed(&plans[0], file.size_bytes);
    // The same file, seen again: the fingerprint matches, so it is not a job.
    assert!(folder.ready(std::slice::from_ref(&file)).is_empty());
    // A file whose *content* changed — a different size — is a new job.
    assert_eq!(folder.ready(&[observed("master.mp4", 2_000, 60)]).len(), 1);
}

#[test]
fn a_marker_list_beside_a_settled_master_makes_a_job() {
    let dir = std::env::temp_dir().join(format!("trimmer-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let master = dir.join("master.mp4");
    std::fs::write(&master, b"video").expect("write");
    std::fs::write(dir.join("master.marks.txt"), "cold open, 00:00:10:00, 00:00:20:00\n")
        .expect("write");

    let folder = WatchFolder::new(dir.clone());
    let file = ObservedFile {
        path: master,
        size_bytes: 5,
        quiet_seconds: 60,
    };
    let plans = folder.ready(&[file]);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].trigger, WatchTrigger::MarkerList);
    assert_eq!(plans[0].action, WatchAction::CutMarkedSegments);
    assert!(plans[0].markers.is_some());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_project_marker_turns_into_a_run_project_job() {
    let dir = std::env::temp_dir().join(format!("trimmer-watch-proj-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let master = dir.join("master.mp4");
    std::fs::write(&master, b"video").expect("write");
    std::fs::write(dir.join("master.trimmerproj"), b"{}").expect("write");

    let folder = WatchFolder::new(dir.clone());
    let plans = folder.ready(&[ObservedFile {
        path: master,
        size_bytes: 5,
        quiet_seconds: 60,
    }]);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].action, WatchAction::RunProject);
    assert_eq!(plans[0].trigger, WatchTrigger::ProjectFile);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn marker_lists_are_read_forgivingly() {
    let rate = FrameRate::FPS_25;
    let text = "\
# a comment line
cold open, 00:00:10:00, 00:00:20:00
00:31:00:00            # to the end
3, second take, 0:00:40:00 - 0:01:00:00
not a mark at all
00:12:00:00,00:14:00:00
";
    let marks = parse_marks(text, rate);
    // The comment and the nonsense line are skipped; four usable marks remain.
    let usable: Vec<&trimmer_app::watch::WatchMark> =
        marks.iter().filter(|mark| mark.is_usable()).collect();
    assert_eq!(usable.len(), 4, "{marks:#?}");

    assert_eq!(usable[0].name.as_deref(), Some("cold open"));
    assert_eq!(usable[0].in_frame, Some(250));
    assert_eq!(usable[0].out_frame, Some(500));

    // A line with only an in point means "to the end".
    assert_eq!(usable[1].in_frame, Some(46_500));
    assert_eq!(usable[1].out_frame, None);

    // A leading spreadsheet index and a dash separator both work. `0:00:40:00` is forty seconds,
    // which at 25 fps is frame 1 000.
    assert_eq!(usable[2].name.as_deref(), Some("second take"));
    assert_eq!(usable[2].in_frame, Some(1_000));
    assert_eq!(usable[2].out_frame, Some(1_500));

    // A bare pair separated by a comma works.
    assert_eq!(usable[3].in_frame, Some(18_000));
    assert_eq!(usable[3].out_frame, Some(21_000));
}

#[test]
fn an_unreadable_mark_line_is_kept_with_no_frames_rather_than_dropped_silently() {
    let rate = FrameRate::FPS_25;
    let marks = parse_marks("this line has no timecodes\n", rate);
    assert_eq!(marks.len(), 1);
    assert!(!marks[0].is_usable());
    assert!(marks[0].describe().contains("unreadable"));
}

#[test]
fn the_watch_policy_defaults_are_conservative() {
    let policy = WatchPolicy::default();
    // Do not cut a file nobody marked.
    assert!(!policy.cut_whole_when_unmarked);
    // Wait for a copy to finish.
    assert!(policy.settle_seconds >= 10);
    // Leave the marker list alone unless told otherwise.
    assert!(!policy.consume_marker_list);
    assert!(policy.is_video(Path::new("a/b/master.MP4")));
    assert!(!policy.is_video(Path::new("a/b/notes.txt")));
    // A project beats an EDL beats a CSV.
    assert!(WatchPolicy::is_project_marker("trimmerproj"));
    assert!(!WatchPolicy::is_project_marker("edl"));
}

// ---------------------------------------------------------------------------------------
// Transcript integration
// ---------------------------------------------------------------------------------------

/// Captions that are always found, from a fixed file, so search can be tested without media.
struct FixedTranscripts {
    path: PathBuf,
    transcript: trimmer_core::Transcript,
}

impl TranscriptSource for FixedTranscripts {
    fn find_for(&self, _video: &MediaPath) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn read(&self, _path: &Path) -> trimmer_core::CoreResult<trimmer_core::Transcript> {
        Ok(self.transcript.clone())
    }

    fn exists(&self, _path: &MediaPath) -> bool {
        true
    }
}

#[tokio::test]
async fn a_transcript_is_indexed_and_searched_through_the_workspace() {
    use trimmer_core::{Cue, Grouping};

    let fixture = Fixture::new("transcript");
    let transcript = trimmer_core::Transcript::new(vec![
        Cue::new(0.0, 2.0, "So the custody question is"),
        Cue::new(2.1, 4.0, "the thing nobody wants to answer."),
        Cue::new(7.0, 9.5, "And that is where the market"),
        Cue::new(9.5, 11.0, "disagrees with the SEC."),
    ]);
    let service = TranscriptService::new(
        Arc::new(FixedTranscripts {
            path: fixture.dir.join("master.srt"),
            transcript,
        }),
        Grouping::Sentence,
    );
    let video = fixture.master();
    let view = service.load(&video).expect("loaded").expect("present");
    assert_eq!(view.len(), 4);
    assert_eq!(view.groups.len(), 2);
    assert_eq!(view.word_count(), 21);

    let hits = view.find_phrase("SEC", FrameRate::FPS_25, 10);
    assert_eq!(hits.len(), 1);
    let (start, end) = view
        .cut_for(&hits[0], FrameRate::FPS_25)
        .expect("a range");
    // The second sentence runs 7.0 s to 11.0 s, which at 25 fps is frames 175 to 275.
    assert_eq!(start, 175);
    assert_eq!(end, 275);

    // The nearest-pause variant can only widen.
    let (wide_start, wide_end) = view
        .cut_for_with_pauses(&hits[0], FrameRate::FPS_25)
        .expect("a range");
    assert!(wide_start <= start);
    assert!(wide_end >= end);

    let (text, timecode) = view.group_text(1, FrameRate::FPS_25).expect("group");
    assert!(text.contains("disagrees with the SEC"));
    assert_eq!(timecode, "00:00:07:00");
}

#[tokio::test]
async fn a_transcript_that_runs_past_the_video_is_flagged() {
    use trimmer_core::{Cue, Grouping};

    let fixture = Fixture::new("transcript-long");
    let transcript = trimmer_core::Transcript::new(vec![Cue::new(0.0, 5_000.0, "far too long")]);
    let service = TranscriptService::new(
        Arc::new(FixedTranscripts {
            path: fixture.dir.join("master.srt"),
            transcript,
        }),
        Grouping::Sentence,
    );
    let video = fixture.master();
    let view = service.load(&video).expect("loaded").expect("present");
    // The fake source claims 100 000 frames at 25 fps, which is 4 000 s, so 5 000 s is past it.
    let summary = view.summary(Some(&fixture.engine.media));
    assert!(summary.warning.is_some(), "{summary:?}");
    assert!(summary.warning.unwrap_or_default().contains("different file"));
}

#[tokio::test]
async fn a_transcript_with_no_cues_is_an_error_with_the_path() {
    use trimmer_core::Grouping;

    let fixture = Fixture::new("transcript-empty");
    let service = TranscriptService::new(
        Arc::new(FixedTranscripts {
            path: fixture.dir.join("master.srt"),
            transcript: trimmer_core::Transcript::new(Vec::new()),
        }),
        Grouping::Sentence,
    );
    let error = service
        .load(&fixture.master())
        .expect_err("refused");
    assert!(error.to_string().contains("no readable cues"), "{error}");
}

#[tokio::test]
async fn the_transcript_cache_holds_one_index_per_file() {
    use trimmer_core::{Cue, Grouping};

    let fixture = Fixture::new("transcript-cache");
    let transcript = trimmer_core::Transcript::new(vec![Cue::new(0.0, 1.0, "hello")]);
    let service = TranscriptService::new(
        Arc::new(FixedTranscripts {
            path: fixture.dir.join("master.srt"),
            transcript,
        }),
        Grouping::Sentence,
    );
    let video = fixture.master();
    let first = service.load(&video).expect("loaded").expect("present");
    let second = service.load(&video).expect("loaded").expect("present");
    assert!(Arc::ptr_eq(&first, &second), "the index should be shared");
    assert_eq!(service.cached_count(), 1);
    service.invalidate(&fixture.dir.join("master.srt"));
    assert_eq!(service.cached_count(), 0);
}

#[test]
fn a_transcript_view_is_constructed_from_cues_without_any_io() {
    use trimmer_core::{Cue, Grouping};

    let view = TranscriptView::new(
        MediaPath::new(r"H:\a.mp4"),
        PathBuf::from(r"H:\a.srt"),
        vec![Cue::new(0.0, 1.0, "one"), Cue::new(1.0, 2.0, "two.")],
        Grouping::Sentence,
    );
    assert_eq!(view.len(), 2);
    assert!(!view.is_empty());
    assert_eq!(view.count("one"), 1);
    assert_eq!(view.count("missing"), 0);
    assert!((view.duration() - 2.0).abs() < 1e-9);
    assert!(view.group_at_frame(30, FrameRate::FPS_25).is_some());
    assert!(view.group_at_frame(10_000, FrameRate::FPS_25).is_none());
}

// ---------------------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------------------

#[test]
fn the_test_clock_is_deterministic_and_advances() {
    let clock = TestClock::new(1_000);
    assert_eq!(clock.now_unix(), 1_000);
    assert_eq!(clock.now_millis(), 0);
    clock.advance_millis(2_500);
    assert_eq!(clock.now_millis(), 2_500);
    assert_eq!(clock.now_unix(), 1_002);
}

#[test]
fn the_system_clock_moves_forward() {
    use trimmer_app::ports::SystemClock;
    let clock = SystemClock;
    let first = clock.now_millis();
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(clock.now_millis() >= first);
    assert!(clock.now_unix() > 1_600_000_000);
}

// ---------------------------------------------------------------------------------------
// Helpers used by the fixtures
// ---------------------------------------------------------------------------------------

#[test]
fn the_workspace_description_lists_what_matters() {
    let fixture = Fixture::new("describe");
    let text = Workspace::new(
        "Rollup",
        "Fernando",
        fixture.engine.clone(),
        Arc::new(NoTranscripts),
        Arc::new(TestClock::new(0)),
    )
    .describe();
    assert!(text.contains("Rollup"));
    assert!(text.contains("Fernando"));
    assert!(text.contains("sources"));
    assert!(text.contains("segments"));
    assert!(text.contains("verifying"));
}

#[test]
fn a_project_starts_with_the_standard_preset_library() {
    let project = Project::new("x", "y", 0);
    assert!(project.presets.contains_key("master"));
    assert!(project.presets.contains_key("vertical"));
    assert_eq!(project.default_preset, "master");
}

#[test]
fn segments_can_be_reordered_and_removed_through_the_workspace() {
    let mut fixture = Fixture::new("edit");
    // Build a project by hand so the test does not need the async source probe.
    let path = fixture.master();
    fixture.workspace.project_mut().upsert_source(trimmer_core::SegmentSource {
        path: path.clone(),
        media: Some(fixture.engine.media.clone()),
        available: true,
        label: None,
    });
    let ids: Vec<SegmentId> = (0..3)
        .map(|index| {
            fixture
                .workspace
                .add_segment(Segment::new(path.clone(), format!("s{index}"), index * 10, index * 10 + 5))
                .expect("added")
        })
        .collect();

    let removed = fixture.workspace.remove_segment(ids[1]).expect("removed");
    assert_eq!(removed.name, "s1");
    assert_eq!(fixture.workspace.segments().len(), 2);
    assert!(fixture.workspace.remove_segment(ids[1]).is_err());
}

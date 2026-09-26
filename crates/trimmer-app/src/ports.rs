//! The seams between this layer and the outside world.
//!
//! Each trait is here because there is a path through the product that is important to test and
//! impossible to reach without it. The doc comment on each one names that path, so a future
//! change that removes a trait has to argue with the reason it existed.

use std::path::PathBuf;

use trimmer_core::{
    CoreError, CoreResult, CutPlan, DeliveryPreset, MediaInfo, MediaPath, Project, Segment,
};
use trimmer_media::{CutOutcome, MediaResult, Prepared, RunOptions};
use trimmer_verify::{CutFacts, FrameHashes, MediaMeasurer, Similarity};

/// One segment, ready to cut.
///
/// Assembled by the application layer from the project, the segment and the resolved preset, so
/// an engine implementation never has to know how a project is organised.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentCutRequest {
    /// The source, already probed.
    pub media: MediaInfo,
    /// The segment to cut, with handles already resolved against the source.
    pub segment: Segment,
    /// The delivery preset to satisfy.
    pub preset: DeliveryPreset,
    /// Where the cut should be written.
    pub output: MediaPath,
    /// The plan, when the caller has already made one. `None` asks the engine to plan.
    pub plan: Option<CutPlan>,
}

/// Everything the application layer needs in order to cut.
///
/// Implemented by `trimmer-media`'s executor. Faked in tests.
#[async_trait::async_trait]
/// # Thread safety
///
/// `Send + Sync` because an engine is shared behind an `Arc` between the workspace, the batch queue
/// and every command that plans or cuts. The bound lives here so an implementation that cannot be
/// shared is refused where it is written rather than at each use.
pub trait MediaEngine: Send + Sync {
    /// Probe a file.
    ///
    /// # Errors
    ///
    /// Propagates a probe failure.
    async fn probe(&self, path: &MediaPath) -> MediaResult<MediaInfo>;

    /// Work out how a segment would be cut, without cutting it.
    ///
    /// # Errors
    ///
    /// Returns the domain's refusal when the segment cannot be cut.
    async fn plan(&self, media: &MediaInfo, segment: &Segment) -> CoreResult<CutPlan>;

    /// Cut a segment.
    ///
    /// # Errors
    ///
    /// Returns [`trimmer_media::MediaError::Cancelled`] when the run options ask it to stop.
    async fn cut(
        &self,
        request: &SegmentCutRequest,
        options: &RunOptions,
    ) -> MediaResult<CutOutcome>;

    /// The arguments that *would* be run for a plan, without running anything.
    ///
    /// This is what makes a dry run honest: a "preview" that reimplements the argument builder
    /// would drift from the real one, and the drift would be discovered by a wrong file rather
    /// than by a failing test.
    ///
    /// # Errors
    ///
    /// Returns the domain's refusal when the segment cannot be cut.
    fn preview(
        &self,
        media: &MediaInfo,
        segment: &Segment,
        preset: &DeliveryPreset,
        plan: &CutPlan,
    ) -> CoreResult<Vec<Prepared>>;

    /// The facts about a finished file, for verification.
    ///
    /// # Errors
    ///
    /// Propagates a probe failure.
    async fn facts(&self, path: &MediaPath) -> MediaResult<CutFacts>;
}

/// Reading captions, and telling whether a source is on disk.
pub trait TranscriptSource: Send + Sync {
    /// The caption file for a video: `<video>.srt`, or `<video>.<lang>.srt`.
    fn find_for(&self, video: &MediaPath) -> Option<PathBuf>;

    /// Read a caption file.
    ///
    /// # Errors
    ///
    /// Returns the domain's caption error when the file cannot be read.
    fn read(&self, path: &std::path::Path) -> CoreResult<trimmer_core::Transcript>;

    /// True when a file exists and is readable.
    fn exists(&self, path: &MediaPath) -> bool;
}

/// Persisting a project.
///
/// Deliberately small. A store is a place to put a project and get it back; anything cleverer
/// belongs in the application layer, where it can be tested without a database.
pub trait ProjectStore: Send + Sync {
    /// Save a project, creating or replacing it.
    ///
    /// # Errors
    ///
    /// Returns a store error when the write fails.
    fn save(&self, project: &Project) -> Result<(), String>;

    /// Load a project by id.
    ///
    /// # Errors
    ///
    /// Returns a store error when the project is absent or unreadable.
    fn load(&self, id: trimmer_core::ProjectId) -> Result<Project, String>;

    /// Every project the store holds, as `(id, name, updated_at)`.
    ///
    /// # Errors
    ///
    /// Returns a store error when the listing fails.
    fn list(&self) -> Result<Vec<(trimmer_core::ProjectId, String, i64)>, String>;

    /// Remove a project.
    ///
    /// # Errors
    ///
    /// Returns a store error when the delete fails.
    fn delete(&self, id: trimmer_core::ProjectId) -> Result<(), String>;
}

/// Telling the time.
///
/// A port rather than a call to `SystemTime::now` because the queue's *durations* and a project's
/// timestamps end up in the audit log, and an audit log whose numbers cannot be reproduced is not
/// much of an audit log. A test clock makes every record in it deterministic.
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> i64;

    /// Milliseconds since an arbitrary fixed point, for measuring how long something took.
    fn now_millis(&self) -> u64;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            // Saturating rather than wrapping: a clock set past 2262 should read as a very large
            // timestamp, not as a negative one that sorts before the epoch.
            Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
            Err(_before_epoch) => 0,
        }
    }

    fn now_millis(&self) -> u64 {
        // `Instant` is monotonic, which is what a duration wants: `SystemTime` can step
        // backwards when the machine's clock is corrected, and a negative duration in a progress
        // bar is a worse outcome than a slightly wrong one.
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        let start = START.get_or_init(Instant::now);
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// A clock a test controls, so every timestamp in a report is reproducible.
#[derive(Debug, Default)]
pub struct TestClock {
    unix: std::sync::atomic::AtomicI64,
    millis: std::sync::atomic::AtomicU64,
}

impl TestClock {
    /// A clock starting at a fixed instant.
    #[must_use]
    pub fn new(unix: i64) -> Self {
        Self {
            unix: std::sync::atomic::AtomicI64::new(unix),
            millis: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Move the clock forward.
    pub fn advance_millis(&self, millis: u64) {
        self.millis
            .fetch_add(millis, std::sync::atomic::Ordering::SeqCst);
        self.unix.fetch_add(
            i64::try_from(millis / 1000).unwrap_or(i64::MAX),
            std::sync::atomic::Ordering::SeqCst,
        );
    }
}

impl Clock for TestClock {
    fn now_unix(&self) -> i64 {
        self.unix.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn now_millis(&self) -> u64 {
        self.millis.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Adapts `trimmer-media`'s executor to [`MediaEngine`].
///
/// The adapter is this thin on purpose: a fat adapter is a place for behaviour to hide where no
/// test looks for it.
#[derive(Debug, Clone)]
pub struct MediaAdapter {
    executor: trimmer_media::CutExecutor,
    config: trimmer_media::CutConfig,
}

impl MediaAdapter {
    /// Wrap an executor.
    #[must_use]
    pub fn new(executor: trimmer_media::CutExecutor, config: trimmer_media::CutConfig) -> Self {
        Self { executor, config }
    }

    /// The executor underneath, for a caller that needs a method this trait does not expose.
    #[must_use]
    pub const fn executor(&self) -> &trimmer_media::CutExecutor {
        &self.executor
    }
}

#[async_trait::async_trait]
impl MediaEngine for MediaAdapter {
    async fn probe(&self, path: &MediaPath) -> MediaResult<MediaInfo> {
        self.executor.prober().probe(path.as_path()).await
    }

    async fn plan(&self, media: &MediaInfo, segment: &Segment) -> CoreResult<CutPlan> {
        let to = segment.end_frame.unwrap_or(media.frame_count);
        // A keyframe listing that fails is not a reason to refuse the cut. The planner's worst
        // case when it finds no keyframe is a whole-segment re-encode — a valid answer, just a
        // more expensive one — whereas refusing would lose a cut the user asked for. So a failed
        // listing degrades to an empty grid rather than propagating.
        let grid = self
            .executor
            .prober()
            .keyframes(media, segment.start_frame, to)
            .await
            .unwrap_or_else(|_| {
                trimmer_core::KeyframeGrid::new(Vec::new(), segment.start_frame, to)
            });
        trimmer_core::plan_cut(media, segment, &grid)
    }

    async fn cut(
        &self,
        request: &SegmentCutRequest,
        options: &RunOptions,
    ) -> MediaResult<CutOutcome> {
        match &request.plan {
            Some(plan) => {
                self.executor
                    .cut_with_plan(
                        &request.media,
                        plan,
                        &request.preset,
                        &self.config,
                        request.output.as_path(),
                        options,
                    )
                    .await
            }
            None => {
                self.executor
                    .cut(
                        &request.media,
                        &request.segment,
                        &request.preset,
                        &self.config,
                        request.output.as_path(),
                        options,
                    )
                    .await
            }
        }
    }

    fn preview(
        &self,
        media: &MediaInfo,
        segment: &Segment,
        preset: &DeliveryPreset,
        plan: &CutPlan,
    ) -> CoreResult<Vec<Prepared>> {
        // The real argument builders, so a preview cannot drift from what will run.
        let config = &self.config;
        let _ = segment;
        let steps = match plan.mode {
            trimmer_core::CutMode::Copy => {
                vec![trimmer_media::executor::prepare_copy(media, plan, config)]
            }
            trimmer_core::CutMode::Reencode => {
                vec![trimmer_media::executor::prepare_reencode(
                    media, plan, preset, config,
                )]
            }
            trimmer_core::CutMode::HeadPatch => {
                let mut steps = vec![trimmer_media::executor::prepare_head(media, plan, config)];
                // A preview shows the commands, not a run, so the body's two halves are named rather
                // than located — angle-bracketed, so nobody mistakes them for files.
                steps.extend(trimmer_media::executor::prepare_body(
                    media,
                    plan,
                    config,
                    &trimmer_media::executor::BodyHalves::placeholders(),
                ));
                // The listing names two files and nothing in it says how long they are together, so the
                // expectation comes from the plan — the same frames-over-rate the head and body use.
                let rate = if plan.rate_numerator == 0 {
                    0.0
                } else {
                    plan.requested_frames() as f64 * plan.rate_denominator as f64
                        / plan.rate_numerator as f64
                };
                steps.push(trimmer_media::executor::prepare_join(
                    std::path::Path::new("concat.txt"),
                    rate,
                ));
                steps
            }
        };
        Ok(steps)
    }

    async fn facts(&self, path: &MediaPath) -> MediaResult<CutFacts> {
        let media = self.executor.prober().probe(path.as_path()).await?;
        Ok(CutFacts {
            path: media.path,
            frame_count: media.frame_count,
            video_duration: media.rate.seconds_of(media.frame_count),
            video_start_time: media.start_time,
            audio_duration: media
                .audio
                .as_ref()
                .map(|_| media.rate.seconds_of(media.frame_count)),
            audio_start_time: media.audio.as_ref().map(|_| media.start_time),
            rate: Some(media.rate),
            codec: media.codec,
            width: media.width,
            height: media.height,
            size_bytes: media.size_bytes,
        })
    }
}

/// A measurement seam re-exported so a caller does not have to depend on `trimmer-verify`
/// directly to write a fake.
pub use trimmer_verify::MediaMeasurer as Measurer;

/// Build the request a cut needs, resolving the preset and the output path.
///
/// Lives here rather than in [`crate::workspace`] so the queue can call it without the workspace
/// module having to know about the queue. The plan is passed in rather than looked up, so a
/// caller that has already made one — which the queue always has — does not pay for a second.
///
/// # Errors
///
/// Returns [`crate::AppError::SourceUnavailable`] when the source has not been probed, and the
/// domain's refusal when the preset does not exist.
pub fn cut_request_with(
    workspace: &crate::workspace::Workspace,
    id: trimmer_core::SegmentId,
    plan: Option<CutPlan>,
) -> Result<SegmentCutRequest, crate::AppError> {
    let segment = workspace
        .project()
        .segment(id)
        .cloned()
        .ok_or_else(|| crate::AppError::UnknownSegment(id.to_string()))?;
    let media = workspace
        .project()
        .media(&segment.source)
        .cloned()
        .ok_or_else(|| crate::AppError::SourceUnavailable {
            path: segment.source.to_string(),
            reason: "it has not been probed".to_owned(),
        })?;
    let preset = workspace.project().preset_for(&segment)?.clone();
    let output = workspace.output_path(&segment);
    Ok(SegmentCutRequest {
        media,
        segment,
        preset,
        output,
        plan,
    })
}

/// The captions that ship with the crate, reading real files.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileTranscripts;

impl TranscriptSource for FileTranscripts {
    fn find_for(&self, video: &MediaPath) -> Option<PathBuf> {
        trimmer_core::caption::find_for(video.as_path())
    }

    fn read(&self, path: &std::path::Path) -> CoreResult<trimmer_core::Transcript> {
        trimmer_core::caption::read(path)
    }

    fn exists(&self, path: &MediaPath) -> bool {
        path.exists()
    }
}

/// A measurement seam that answers from a table, for tests.
#[derive(Debug, Default, Clone)]
pub struct FakeMeasurer {
    /// Facts keyed by path.
    pub facts: std::collections::BTreeMap<String, CutFacts>,
    /// Hashes keyed by path.
    pub hashes: std::collections::BTreeMap<String, FrameHashes>,
    /// Similarity scores, in order.
    pub similarity: Vec<Option<f64>>,
}

impl MediaMeasurer for FakeMeasurer {
    fn facts(&self, path: &MediaPath) -> CoreResult<CutFacts> {
        self.facts
            .get(&path.to_string())
            .cloned()
            .ok_or_else(|| CoreError::Caption {
                path: path.to_string(),
                reason: "the fake measurer has no facts for this file".to_owned(),
            })
    }

    fn frame_hashes(
        &self,
        path: &MediaPath,
        _start_frame: i64,
        _count: i64,
        _rate: trimmer_core::FrameRate,
    ) -> CoreResult<FrameHashes> {
        self.hashes
            .get(&path.to_string())
            .cloned()
            .ok_or_else(|| CoreError::Caption {
                path: path.to_string(),
                reason: "the fake measurer has no hashes for this file".to_owned(),
            })
    }

    fn extract_frame(
        &self,
        _path: &MediaPath,
        _frame: i64,
        _rate: trimmer_core::FrameRate,
    ) -> CoreResult<Vec<u8>> {
        Ok(Vec::new())
    }

    fn ssim(&self, _a: &[u8], _b: &[u8]) -> CoreResult<Similarity> {
        Ok(Similarity(
            self.similarity.first().copied().unwrap_or(Some(1.0)),
        ))
    }
}

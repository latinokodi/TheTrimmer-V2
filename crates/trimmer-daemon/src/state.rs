//! Shared state: the store, the tools, and the bounded registry of runs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use trimmer_core::{CoreError, CoreResult, MediaPath, ProjectId};
use trimmer_media::{CancelFlag, CutConfig, CutExecutor, MediaError, Prober, ToolPaths};
use trimmer_store::SqliteStore;
use trimmer_verify::{CutFacts, FrameHashes, MediaMeasurer, Similarity};
use uuid::Uuid;

use crate::config::DaemonConfig;

/// How many run records the registry will hold before it evicts one.
///
/// A daemon runs for weeks. An unbounded map of run records is a leak that shows up in week
/// three and is blamed on something else, so the bound is here and the eviction is deliberate:
/// the **oldest finished** run goes first, because the newest is the one a client is polling.
pub const MAX_RUNS: usize = 100;

/// Where a run has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    /// Accepted, and waiting for its task to be scheduled.
    Queued,
    /// Cutting.
    Running,
    /// Finished, successfully or not. See the outcome.
    Finished,
    /// A client asked for it to stop.
    Cancelled,
}

impl RunState {
    /// The word a client sees.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One run, and everything a client polling it needs to see.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    /// The run's identity, minted when it was accepted.
    pub id: Uuid,
    /// The project it is cutting.
    pub project_id: ProjectId,
    /// Where it has got to.
    pub state: RunState,
    /// Seconds since the Unix epoch.
    pub started_at: i64,
    /// Seconds since the Unix epoch, once it has finished.
    pub finished_at: Option<i64>,
    /// How many segments the batch will process.
    pub total: usize,
    /// The per-item outcome, once there is one.
    pub outcome: Option<RunOutcome>,
    /// A sentence saying what went wrong when the *run itself* could not be carried out — as
    /// opposed to a segment failing, which is in the outcome.
    pub error: Option<String>,
    /// Set by `POST /v1/runs/{id}/cancel`.
    ///
    /// Skipped by serde: a client has no business seeing the flag, only the state it produces,
    /// and a flag is a handle rather than a fact about the run.
    #[serde(skip)]
    pub cancel: CancelFlag,
}

/// A finished run's counts and its report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    /// How many segments came out certified.
    pub succeeded: usize,
    /// How many came out with a failed check.
    pub unverified: usize,
    /// How many did not come out at all.
    pub failed: usize,
    /// How many were never attempted.
    pub skipped: usize,
    /// Frames delivered across the batch.
    pub delivered_frames: i64,
    /// Seconds delivered across the batch.
    pub delivered_seconds: f64,
    /// Seconds the batch took.
    pub elapsed_seconds: f64,
    /// One line per segment, in order.
    pub items: Vec<RunItem>,
    /// The batch's own report, as the queue wrote it.
    pub report: String,
}

/// One segment's fate inside a run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunItem {
    /// The segment.
    pub segment: String,
    /// Its name.
    pub name: String,
    /// One line saying what happened.
    pub status: String,
    /// What was written, when something was.
    pub output: Option<String>,
}

/// Everything a request handler needs.
pub struct DaemonState {
    /// The project database.
    pub store: Arc<SqliteStore>,
    /// The configuration it was built from.
    pub config: DaemonConfig,
    /// The resolved ffmpeg and ffprobe, when they could be found.
    pub tools: Option<ToolPaths>,
    /// The runs, newest last, bounded by [`MAX_RUNS`].
    pub runs: Mutex<HashMap<Uuid, RunRecord>>,
    /// The transcript index, built once per file and kept.
    ///
    /// On the state rather than constructed per request, because building it means reading the SRT,
    /// parsing every cue and folding all of them: a client polling a search endpoint would otherwise
    /// pay that on every call, which an audit measured as the daemon's worst hot path.
    pub transcripts: trimmer_app::TranscriptService,
}

impl std::fmt::Debug for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonState")
            .field("store_path", &self.config.store_path)
            .field("bind", &self.config.bind)
            .field("port", &self.config.port)
            .field("tools", &self.tools.is_some())
            .field("runs", &self.runs.lock().map(|map| map.len()).unwrap_or(0))
            .finish_non_exhaustive()
    }
}

impl DaemonState {
    /// Build the state, resolving the media tools if they are there.
    ///
    /// A missing ffmpeg is not fatal to *starting*: `doctor` and the project routes are useful
    /// on a machine that has not been set up yet, and a health check that says so is more
    /// useful than a daemon that refuses to boot. A run on such a machine fails with a sentence.
    ///
    /// # Errors
    ///
    /// Returns a sentence when the store cannot be opened, which is fatal.
    pub fn new(config: DaemonConfig) -> Result<Arc<Self>, String> {
        let store = SqliteStore::open(&config.store_path)
            .map_err(|error| format!("could not open the project store: {error}"))?;
        let tools = ToolPaths::resolve().ok();
        if tools.is_none() {
            tracing::warn!(
                "ffmpeg and ffprobe could not be resolved; probing and cutting will fail until \
                 the tools are installed or THE_TRIMMER_FFMPEG / THE_TRIMMER_FFPROBE are set"
            );
        }
        // One transcript index for the process, built lazily per file and kept.
        let transcripts = trimmer_app::TranscriptService::new(
            Arc::new(trimmer_app::ports::FileTranscripts),
            trimmer_core::Grouping::Sentence,
        );
        Ok(Arc::new(Self {
            store: Arc::new(store),
            config,
            tools,
            runs: Mutex::new(HashMap::new()),
            transcripts,
        }))
    }

    /// The prober, when the tools could be resolved.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the environment variables that would fix it when they could
    /// not be.
    pub fn prober(&self) -> Result<Prober, String> {
        self.tools.clone().map(Prober::new).ok_or_else(|| {
            MediaError::ToolNotFound {
                tool: "ffmpeg".to_owned(),
                env: "FFMPEG".to_owned(),
            }
            .to_string()
        })
    }

    /// True when a usable ffmpeg was found.
    #[must_use]
    pub fn has_ffmpeg(&self) -> bool {
        self.tools
            .as_ref()
            .is_some_and(|tools| tools.ffmpeg.exists() && tools.ffprobe.exists())
    }

    /// Whether a run for this project is already going.
    #[must_use]
    pub fn project_is_running(&self, project_id: ProjectId) -> bool {
        self.runs.lock().is_ok_and(|runs| {
            runs.values().any(|run| {
                run.project_id == project_id
                    && matches!(run.state, RunState::Queued | RunState::Running)
            })
        })
    }

    /// Put a run in the registry, evicting the oldest finished one if it is full.
    ///
    /// # Errors
    ///
    /// Returns a sentence when the registry is full of runs that are still going, because
    /// discarding the record of work in flight would leave a client polling an id that has
    /// silently become unknown.
    pub fn insert_run(&self, record: RunRecord) -> Result<(), String> {
        let mut runs = self.runs.lock().map_err(|_| poison())?;
        if runs.len() >= MAX_RUNS {
            let oldest_finished = runs
                .values()
                .filter(|run| matches!(run.state, RunState::Finished | RunState::Cancelled))
                .min_by_key(|run| run.finished_at.unwrap_or(run.started_at))
                .map(|run| run.id);
            match oldest_finished {
                Some(id) => {
                    runs.remove(&id);
                }
                None => {
                    return Err(format!(
                        "{MAX_RUNS} runs are in flight; wait for one to finish before starting \
                         another"
                    ))
                }
            }
        }
        runs.insert(record.id, record);
        Ok(())
    }

    /// Update a run in place.
    ///
    /// # Errors
    ///
    /// Returns a sentence when the registry lock is poisoned.
    pub fn update_run(&self, id: Uuid, body: impl FnOnce(&mut RunRecord)) -> Result<(), String> {
        let mut runs = self.runs.lock().map_err(|_| poison())?;
        if let Some(record) = runs.get_mut(&id) {
            body(record);
        }
        Ok(())
    }

    /// One run, by id.
    #[must_use]
    pub fn run(&self, id: Uuid) -> Option<RunRecord> {
        self.runs
            .lock()
            .ok()
            .and_then(|runs| runs.get(&id).cloned())
    }

    /// How many runs the registry holds, for a diagnostics line and for the bound's test.
    #[must_use]
    pub fn run_count(&self) -> usize {
        self.runs.lock().map_or(0, |runs| runs.len())
    }

    /// The cancellation flag of a running run.
    #[must_use]
    pub fn cancel_flag(&self, id: Uuid) -> Option<CancelFlag> {
        self.runs.lock().ok().and_then(|runs| {
            runs.get(&id)
                .filter(|run| matches!(run.state, RunState::Queued | RunState::Running))
                .map(|run| run.cancel.clone())
        })
    }
}

/// A lock poisoned by a panic in another thread.
fn poison() -> String {
    "the run registry lock was poisoned by a panic".to_owned()
}

/// Adapts the prober to the measurement trait the verdict logic uses.
///
/// `trimmer-verify` deliberately has no implementation of its own: measuring means running a
/// process, and a rule that can only be reached through a subprocess is a rule that can only
/// be tested with media. This is the adapter, and it is the only place in this crate where the
/// two meet.
///
/// The probe is asynchronous and the trait is not, so it is driven to completion on the
/// runtime the batch is already running on. That is only possible on a multi-threaded runtime,
/// which is what [`crate::serve`] uses; on a single-threaded one this returns a refusal rather
/// than panicking, and the verdict then reports the measurement as unmeasurable rather than
/// silently passing it.
pub struct ProbeMeasurer {
    prober: Prober,
    /// The resolved tools, so a frame-hash pass does not have to resolve them again.
    tools: ToolPaths,
}

impl ProbeMeasurer {
    /// Wrap a prober.
    #[must_use]
    pub fn new(prober: Prober) -> Self {
        let tools = prober.tools().clone();
        Self { prober, tools }
    }
}

impl MediaMeasurer for ProbeMeasurer {
    fn facts(&self, path: &MediaPath) -> CoreResult<CutFacts> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| CoreError::Invariant("there is no runtime to probe on".to_owned()))?;
        if !matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        ) {
            return Err(CoreError::Invariant(
                "the daemon probes on a multi-threaded runtime; a single-threaded one cannot \
                 block on a probe"
                    .to_owned(),
            ));
        }
        let media =
            tokio::task::block_in_place(|| handle.block_on(self.prober.probe(path.as_path())))
                .map_err(|error| CoreError::Invariant(error.to_string()))?;
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

    /// Not measured here.
    ///
    /// Frame hashing needs `ffmpeg -f framemd5` over a window, which `trimmer-media` exposes
    /// through the executor rather than the prober. The queue's verification path calls
    /// [`MediaMeasurer::facts`] only, so this is unreachable from the daemon's own routes;
    /// returning a refusal rather than an empty result means that if it ever *does* become
    /// reachable, the check reports itself as unmeasurable instead of passing.
    /// A real hash pass.
    ///
    /// Refusing here — as an earlier version did, on the reasoning that the queue called only
    /// [`MediaMeasurer::facts`] — meant the frame comparison reported itself skipped while the report
    /// still called the cut verified. The queue's certification rule catches that and reports the file
    /// uncertified, which is honest, but the right answer is to make the measurement. It costs one
    /// ffmpeg run per side, which is why the *policy* decides whether it happens.
    fn frame_hashes(
        &self,
        path: &MediaPath,
        start_frame: i64,
        count: i64,
        rate: trimmer_core::FrameRate,
    ) -> CoreResult<FrameHashes> {
        if count <= 0 {
            return Ok(FrameHashes::new(Vec::new(), start_frame));
        }
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!("frame hashing has no runtime: {error}");
                return Err(CoreError::Invariant(error.to_string()));
            }
        };
        let executor = trimmer_media::CutExecutor::new(self.tools.clone());
        let options = trimmer_media::RunOptions {
            policy: trimmer_media::PollPolicy::long(),
            ..trimmer_media::RunOptions::default()
        };
        let digests = tokio::task::block_in_place(|| {
            handle.block_on(executor.frame_hashes(
                path.as_path(),
                rate.seconds_of(start_frame),
                usize::try_from(count).unwrap_or(usize::MAX),
                &options,
            ))
        })
        .map_err(|error| CoreError::Invariant(error.to_string()))?;
        Ok(FrameHashes::new(digests, start_frame))
    }

    /// The first frame of a file, as PNG bytes, so the head can be compared pixel-wise.
    ///
    /// Refusing here meant the head-fidelity check reported itself skipped even under the `Forensic`
    /// policy that asks for it — the same shape of bug as the missing hash pass: the verdict logic was
    /// right and the measurement was absent. ffmpeg writes an image to a path, so this extracts to a
    /// scratch file and reads it back, which is simpler than feeding the bytes through a pipe.
    fn extract_frame(
        &self,
        path: &MediaPath,
        frame: i64,
        rate: trimmer_core::FrameRate,
    ) -> CoreResult<Vec<u8>> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            CoreError::Invariant("there is no runtime to extract a frame on".to_owned())
        })?;
        let executor = trimmer_media::CutExecutor::new(self.tools.clone());
        let options = trimmer_media::RunOptions {
            policy: trimmer_media::PollPolicy::long(),
            ..trimmer_media::RunOptions::default()
        };
        let scratch =
            std::env::temp_dir().join(format!("trimmer-frame-{}-{frame}.png", std::process::id()));
        tokio::task::block_in_place(|| {
            handle.block_on(executor.frame_png(
                path.as_path(),
                rate.seconds_of(frame),
                &scratch,
                &options,
            ))
        })
        .map_err(|error| CoreError::Invariant(error.to_string()))?;
        let bytes = std::fs::read(&scratch).unwrap_or_default();
        let _ = std::fs::remove_file(&scratch);
        Ok(bytes)
    }

    /// Unmeasurable, rather than a number nobody took.
    /// The similarity between two PNG frames, measured with ffmpeg's `ssim` filter.
    ///
    /// The two byte slices are written to scratch files first: `ssim` takes two inputs, and a path
    /// per side is simpler than teaching this to feed ffmpeg from memory.
    fn ssim(&self, a: &[u8], b: &[u8]) -> CoreResult<Similarity> {
        if a.is_empty() || b.is_empty() {
            return Ok(Similarity::UNMEASURABLE);
        }
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| CoreError::Invariant("there is no runtime to compare on".to_owned()))?;
        let left = std::env::temp_dir().join(format!("trimmer-ssim-a-{}.png", std::process::id()));
        let right = std::env::temp_dir().join(format!("trimmer-ssim-b-{}.png", std::process::id()));
        std::fs::write(&left, a).map_err(|error| CoreError::Invariant(error.to_string()))?;
        std::fs::write(&right, b).map_err(|error| CoreError::Invariant(error.to_string()))?;
        let executor = trimmer_media::CutExecutor::new(self.tools.clone());
        let options = trimmer_media::RunOptions {
            policy: trimmer_media::PollPolicy::long(),
            ..trimmer_media::RunOptions::default()
        };
        let score =
            tokio::task::block_in_place(|| handle.block_on(executor.ssim(&left, &right, &options)));
        let _ = std::fs::remove_file(&left);
        let _ = std::fs::remove_file(&right);
        Ok(match score {
            Ok(Some(value)) => Similarity::measured(value),
            _ => Similarity::UNMEASURABLE,
        })
    }
}

/// The executor a batch uses, when the tools are there.
///
/// # Errors
///
/// Returns a sentence naming the environment variables that would fix it when the tools could
/// not be resolved.
pub fn executor(tools: &ToolPaths) -> CutExecutor {
    CutExecutor::new(tools.clone())
}

/// The encoding configuration a daemon-driven batch uses.
#[must_use]
pub fn cut_config() -> CutConfig {
    CutConfig::default()
}

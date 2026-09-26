//! The state the window shares with the commands.
//!
//! One struct, one lock, held briefly. Everything the interface needs to see lives here, so a
//! command never has to reach for a global.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use trimmer_app::ports::{Clock, FileTranscripts, MediaAdapter, SystemClock, TranscriptSource};
use trimmer_app::transcript::TranscriptService;
use trimmer_app::{MediaEngine, Queue, Workspace};
use trimmer_core::Grouping;
use trimmer_media::{CutConfig, CutExecutor, ToolPaths};
use trimmer_store::SqliteStore;

use crate::VERSION;

/// Everything the commands share.
///
/// `store`, `engine`, `queue`, `transcripts` and `store_path` are reached through [`Deref`] to
/// [`Inner`]. That is what lets a command take Tauri's `State<'_, AppState>` — which derefs to
/// `&AppState` — and still call methods that want a plain `&AppState`, so a signature stays
/// `state: State<'_, AppState>` and the body stays ordinary Rust.
pub struct AppState {
    /// The open project, when one is open.
    pub workspace: Mutex<Option<Workspace>>,
    /// When the batch currently running was started, for the progress display.
    pub running: Mutex<Option<RunningBatch>>,
    /// The shared, borrowable half.
    pub inner: Inner,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("store_path", &self.store_path)
            .finish_non_exhaustive()
    }
}

impl std::ops::Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// The borrowed half of [`AppState`].
///
/// `Debug` is written by hand rather than derived: the engine and the queue are trait objects, and
/// deriving would require them to be `Debug`, which an implementation of a media engine has no
/// reason to be.
pub struct Inner {
    /// The project store.
    pub store: Arc<SqliteStore>,
    /// The media engine, shared with the queue.
    pub engine: Arc<dyn MediaEngine>,
    /// The batch queue.
    pub queue: Arc<Queue>,
    /// Transcripts, cached across searches.
    pub transcripts: Arc<TranscriptService>,
    /// Where the store lives, for the doctor panel.
    pub store_path: PathBuf,
}

/// A batch that is in flight.
#[derive(Debug, Clone)]
pub struct RunningBatch {
    /// A label for the progress line.
    pub label: String,
    /// When it started, in milliseconds on the application clock.
    pub started_millis: u64,
    /// The flag that stops it.
    pub cancel: trimmer_media::CancelFlag,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("open_project", &self.workspace.lock().is_some())
            .field("store_path", &self.store_path)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// Build the state, resolving the tools and opening the store.
    ///
    /// # Errors
    ///
    /// Returns a sentence when ffmpeg or ffprobe cannot be found, or the store cannot be opened.
    /// A missing tool is fatal at startup on purpose: an application that opens and then fails on
    /// the first cut has taught the user nothing, whereas one that refuses to open and says
    /// `winget install Gyan.FFmpeg` has.
    pub fn bootstrap() -> Result<Self, String> {
        let tools = ToolPaths::resolve().map_err(|error| error.to_string())?;
        let store_path = default_store_path();
        let store = SqliteStore::open(&store_path).map_err(|error| error.to_string())?;

        let config = CutConfig::default();
        let executor = CutExecutor::new(tools);
        let engine: Arc<dyn MediaEngine> = Arc::new(MediaAdapter::new(executor, config));

        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let transcripts = Arc::new(TranscriptService::new(
            Arc::new(FileTranscripts) as Arc<dyn TranscriptSource>,
            Grouping::Sentence,
        ));

        let queue = Arc::new(Queue::new(
            Arc::clone(&engine),
            Arc::new(EngineMeasurer::new()),
            clock,
            VERSION,
            machine_name(),
        ));

        Ok(Self {
            workspace: Mutex::new(None),
            running: Mutex::new(None),
            inner: Inner {
                store: Arc::new(store),
                engine,
                queue,
                transcripts,
                store_path,
            },
        })
    }

    /// Run something with the workspace, failing with a sentence when none is open.
    ///
    /// # Errors
    ///
    /// Returns a sentence when no project is open.
    pub fn with_workspace<T>(
        &self,
        body: impl FnOnce(&mut Workspace) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = self.workspace.lock();
        let workspace = guard
            .as_mut()
            .ok_or_else(|| "no project is open".to_owned())?;
        body(workspace)
    }

    /// Install a workspace.
    pub fn set_workspace(&self, workspace: Workspace) {
        *self.workspace.lock() = Some(workspace);
    }

    /// True when a project is open.
    #[must_use]
    pub fn has_workspace(&self) -> bool {
        self.workspace.lock().is_some()
    }
}

/// Where the project database lives.
///
/// The operating system's per-user data directory, not beside the executable: a studio's editors may
/// share a machine, and a store inside `Program Files` would need administrator rights to write to —
/// which is exactly the kind of thing that makes an application unusable on a locked-down
/// workstation.
///
/// `THE_TRIMMER_STORE` overrides it. There are two reasons that has to exist rather than being a
/// test-only hook: an editor who keeps projects on a network share needs to say so, and the
/// integration test that drives the real IPC commands has to run against a database it can throw
/// away. An application whose store location cannot be named is an application that cannot be
/// tested without touching the user's own projects.
fn default_store_path() -> PathBuf {
    if let Some(named) = std::env::var_os("THE_TRIMMER_STORE") {
        if !named.is_empty() {
            return PathBuf::from(named);
        }
    }
    directories::ProjectDirs::from("com", "thetrimmer", "TheTrimmer").map_or_else(
        || PathBuf::from("thetrimmer.db"),
        |dirs| dirs.data_dir().join("projects.db"),
    )
}

/// A name for this machine, for the run log.
///
/// Not a binding of any kind: just enough for a run recorded on one workstation to be
/// distinguishable from a run on another when a studio reads the log.
fn machine_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

/// A measurement adapter over the prober.
///
/// `trimmer-verify`'s `MediaMeasurer` is synchronous by design, so this blocks on the async prober.
/// That is acceptable here for the reason the queue documents: a batch is sequential and there is
/// nothing else for the thread to do.
pub struct EngineMeasurer {
    prober: trimmer_media::Prober,
    /// The resolved tools, so a hash pass can be run without resolving them again per call.
    tools: ToolPaths,
}

impl EngineMeasurer {
    /// Build a measurer over the resolved tools.
    #[must_use]
    pub fn new() -> Self {
        let tools = ToolPaths::resolve().unwrap_or_else(|_| ToolPaths::new("ffmpeg", "ffprobe"));
        Self {
            prober: trimmer_media::Prober::new(tools.clone()),
            tools,
        }
    }
}

impl Default for EngineMeasurer {
    fn default() -> Self {
        Self::new()
    }
}

impl trimmer_verify::MediaMeasurer for EngineMeasurer {
    fn facts(
        &self,
        path: &trimmer_core::MediaPath,
    ) -> trimmer_core::CoreResult<trimmer_verify::CutFacts> {
        let media = block_on(probe_owned(self.prober.clone(), path.clone())).map_err(|error| {
            trimmer_core::CoreError::Caption {
                path: path.to_string(),
                reason: error.to_string(),
            }
        })?;
        Ok(trimmer_verify::CutFacts {
            path: media.path.clone(),
            frame_count: media.frame_count,
            video_duration: media.rate.seconds_of(media.frame_count),
            video_start_time: media.start_time,
            audio_duration: media
                .audio
                .as_ref()
                .map(|_| media.rate.seconds_of(media.frame_count)),
            audio_start_time: media.audio.as_ref().map(|_| media.start_time),
            rate: Some(media.rate),
            codec: media.codec.clone(),
            width: media.width,
            height: media.height,
            size_bytes: media.size_bytes,
        })
    }

    fn frame_hashes(
        &self,
        path: &trimmer_core::MediaPath,
        start_frame: i64,
        count: i64,
        rate: trimmer_core::FrameRate,
    ) -> trimmer_core::CoreResult<trimmer_verify::FrameHashes> {
        // A real hash pass: one ffmpeg run per side, which is why the *policy* decides whether it
        // happens. `Strict` and `Forensic` ask for it; `Standard` does not.
        //
        // Returning an empty list here — as an earlier version did — meant the frame comparison
        // reported itself skipped while the batch still called the cut verified. The queue's
        // certification rule now catches that and reports the file uncertified, which is honest, but
        // the right answer is to make the measurement rather than to report its absence.
        if count <= 0 {
            return Ok(trimmer_verify::FrameHashes::new(Vec::new(), start_frame));
        }
        let options = trimmer_media::RunOptions {
            policy: trimmer_media::PollPolicy::long(),
            ..trimmer_media::RunOptions::default()
        };
        let digests = block_on(measure(
            self.tools.clone(),
            path.clone(),
            rate.seconds_of(start_frame),
            usize::try_from(count).unwrap_or(usize::MAX),
            options,
        ))?;
        Ok(trimmer_verify::FrameHashes::new(digests, start_frame))
    }

    fn extract_frame(
        &self,
        _path: &trimmer_core::MediaPath,
        _frame: i64,
        _rate: trimmer_core::FrameRate,
    ) -> trimmer_core::CoreResult<Vec<u8>> {
        Ok(Vec::new())
    }

    fn ssim(&self, _a: &[u8], _b: &[u8]) -> trimmer_core::CoreResult<trimmer_verify::Similarity> {
        Ok(trimmer_verify::Similarity(None))
    }
}

/// Run a future to completion, from code that may itself be running on a runtime.
///
/// # Why a thread rather than `block_on`
///
/// [`trimmer_verify::MediaMeasurer`] is synchronous and its implementations block on `ffprobe` and
/// `ffmpeg`. The obvious way to bridge that is `Runtime::block_on`, and it works exactly until the
/// caller is *already* inside a runtime — then driving a nested one panics with "Cannot start a
/// runtime from within a runtime", which is what a batch run through Tauri's own async command
/// handler does.
///
/// The alternatives both have a catch. `Handle::block_on` needs the runtime to be multi-threaded
/// (it panics on a current-thread one), and `block_in_place` is unavailable on one too — which rules
/// out both Tauri's test runtime and any single-threaded host. A dedicated thread with its own
/// current-thread runtime has no such condition: it is correct on every host, and one thread for a
/// measurement that is about to spawn `ffmpeg` anyway is not a cost worth optimising away.
///
/// The future is moved to that thread rather than borrowed, so everything it touches has to be owned
/// by the time it gets there. That is why [`measure`] exists: it takes the values rather than
/// references to them.
fn block_on<F>(future: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("a current-thread runtime")
                    .block_on(future)
            })
            .join()
            .expect("the measurement thread does not panic")
    })
}

/// Hash a window of a file's frames, with everything the future needs owned by the future.
///
/// The arguments are taken by value so this can be moved to the runtime thread; see [`block_on`].
async fn measure(
    tools: ToolPaths,
    path: trimmer_core::MediaPath,
    start_seconds: f64,
    count: usize,
    options: trimmer_media::RunOptions,
) -> trimmer_core::CoreResult<Vec<String>> {
    let executor = trimmer_media::CutExecutor::new(tools);
    executor
        .frame_hashes(path.as_path(), start_seconds, count, &options)
        .await
        .map_err(|error| trimmer_core::CoreError::Caption {
            path: path.to_string(),
            reason: error.to_string(),
        })
}

/// Probe a file, with the prober and the path owned by the future. See [`block_on`].
async fn probe_owned(
    prober: trimmer_media::Prober,
    path: trimmer_core::MediaPath,
) -> trimmer_media::MediaResult<trimmer_core::MediaInfo> {
    prober.probe(path.as_path()).await
}

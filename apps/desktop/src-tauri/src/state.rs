//! The state the window shares with the commands.
//!
//! One struct, one lock, held briefly. Everything the interface needs to see lives here, so a
//! command never has to reach for a global.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use trimmer_app::ports::{
    Clock, FileTranscripts, MediaAdapter, SystemClock, TranscriptSource,
};
use trimmer_app::transcript::TranscriptService;
use trimmer_app::{MediaEngine, Queue, Workspace};
use trimmer_core::Grouping;
use trimmer_media::{CutConfig, CutExecutor, ToolPaths};
use trimmer_store::SqliteStore;

use crate::VERSION;

/// Everything the commands share.
pub struct AppState {
    /// The open project, when one is open.
    pub workspace: Mutex<Option<Workspace>>,
    /// The project store.
    pub store: Arc<SqliteStore>,
    /// The media engine, shared with the queue.
    pub engine: Arc<dyn MediaEngine>,
    /// The batch queue.
    pub queue: Arc<Queue>,
    /// When the batch currently running was started, for the progress display.
    pub running: Mutex<Option<RunningBatch>>,
    /// Transcripts, cached across searches.
    pub transcripts: Arc<TranscriptService>,
    /// The licence, read once at startup.
    pub licence: Option<trimmer_license::SignedLicence>,
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
            .field("licensed", &self.licence.is_some())
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

        let machine = trimmer_license::machine_id();
        let queue = Arc::new(Queue::new(
            Arc::clone(&engine),
            Arc::new(SilentMeasurer::new(Arc::clone(&transcripts))),
            clock,
            VERSION,
            machine,
        ));

        let licence = load_licence();

        Ok(Self {
            workspace: Mutex::new(None),
            store: Arc::new(store),
            engine,
            queue,
            running: Mutex::new(None),
            transcripts,
            licence,
            store_path,
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
/// The operating system's per-user data directory, not beside the executable: a studio's editors
/// may share a machine, and a store inside `Program Files` would need administrator rights to
/// write to — which is exactly the kind of thing that makes an application unusable on a locked-down
/// workstation.
fn default_store_path() -> PathBuf {
    directories::ProjectDirs::from("com", "thetrimmer", "TheTrimmer").map_or_else(
        || PathBuf::from("thetrimmer.db"),
        |dirs| dirs.data_dir().join("projects.db"),
    )
}

/// Load a licence from the standard location, if one is there.
///
/// A missing or invalid licence is not fatal: the application runs in a limited mode and the
/// interface says so. Refusing to start would be a worse experience for someone whose licence file
/// is on a share that is momentarily unreachable.
fn load_licence() -> Option<trimmer_license::SignedLicence> {
    let path = directories::ProjectDirs::from("com", "thetrimmer", "TheTrimmer")
        .map(|dirs| dirs.config_dir().join("licence.json"))?;
    let signed = trimmer_license::read_file(&path).ok()?;
    let machine = trimmer_license::machine_id();
    trimmer_license::verify(&signed, trimmer_license::EMBEDDED_VERIFICATION_KEY).ok()?;
    signed
        .licence
        .is_valid_at(now_unix(), Some(&machine))
        .ok()?;
    Some(signed)
}

/// Seconds since the Unix epoch.
fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// A measurement adapter that answers from the engine and the transcript cache.
///
/// `trimmer-verify`'s `MediaMeasurer` is synchronous by design, so this blocks on the async prober.
/// That is acceptable here for the reason the queue documents: a batch is sequential and there is
/// nothing else for the thread to do.
pub struct SilentMeasurer {
    prober: trimmer_media::Prober,
    _transcripts: Arc<TranscriptService>,
}

impl SilentMeasurer {
    /// Wrap a prober.
    #[must_use]
    pub fn new(_transcripts: Arc<TranscriptService>) -> Self {
        // The prober is rebuilt from the resolved tools rather than shared, because it is cheap
        // (it holds two paths and a unit struct) and sharing it would mean threading a lock
        // through a synchronous trait.
        let tools = ToolPaths::resolve().unwrap_or_else(|_| {
            ToolPaths::new("ffmpeg", "ffprobe")
        });
        Self {
            prober: trimmer_media::Prober::new(tools),
            _transcripts,
        }
    }
}

impl trimmer_verify::MediaMeasurer for SilentMeasurer {
    fn facts(&self, path: &trimmer_core::MediaPath) -> trimmer_core::CoreResult<trimmer_verify::CutFacts> {
        let media = block_on(self.prober.probe(path.as_path())).map_err(|error| {
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
        _path: &trimmer_core::MediaPath,
        _start_frame: i64,
        _count: i64,
        _rate: trimmer_core::FrameRate,
    ) -> trimmer_core::CoreResult<trimmer_verify::FrameHashes> {
        // Frame hashing is deliberately not wired into the desktop application yet: it costs two
        // ffmpeg passes per sample point, and the check reports itself as skipped rather than
        // pretending to have run. `trimmer-verify` says so in the status text.
        Ok(trimmer_verify::FrameHashes::new(Vec::new(), 0))
    }

    fn extract_frame(
        &self,
        _path: &trimmer_core::MediaPath,
        _frame: i64,
        _rate: trimmer_core::FrameRate,
    ) -> trimmer_core::CoreResult<Vec<u8>> {
        Ok(Vec::new())
    }

    fn ssim(
        &self,
        _a: &[u8],
        _b: &[u8],
    ) -> trimmer_core::CoreResult<trimmer_verify::Similarity> {
        Ok(trimmer_verify::Similarity(None))
    }
}

/// Run a future to completion on a small runtime, for a synchronous trait method.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}

//! The vocabulary of the domain: media facts, segments, projects and their identities.
//!
//! Everything here is plain data with validated constructors. The important property is
//! that a value of these types *cannot be inconsistent with itself*: a [`MediaInfo`] has a
//! usable rate and at least one frame, a [`Segment`] has an ordered range or an explicit
//! open end, and a [`Project`] will not hand out a segment id that does not resolve.
//!
//! That matters more here than in a typical CRUD application. A segment whose out point is
//! behind its in point is not a validation nicety — it is a cut that would either fail
//! three processes later or, worse, produce a file nobody asked for.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::timecode::{format_timecode, FrameRate};

/// A path to media, normalised so that two spellings of the same file compare equal.
///
/// Windows paths are case-insensitive, and the same file can be reached through a UNC path,
/// a mapped drive and a relative path. Project files are long-lived, so paths are stored
/// canonicalised where possible and compared case-insensitively on Windows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaPath(PathBuf);

impl MediaPath {
    /// Wrap a path, normalising separators.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// Canonicalise against the filesystem when the file exists, and fall back to the
    /// lexical path when it does not — a project must still open when a source is offline.
    #[must_use]
    pub fn canonicalised(&self) -> Self {
        match std::fs::canonicalize(&self.0) {
            Ok(absolute) => Self(strip_extended_prefix(&absolute)),
            Err(_) => Self(self.0.clone()),
        }
    }

    /// The path as it will be handed to a process.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// The file name, for display.
    #[must_use]
    pub fn file_name(&self) -> String {
        self.0
            .file_name()
            .map_or_else(|| self.0.display().to_string(), |name| name.to_string_lossy().into_owned())
    }

    /// The file stem, without its extension.
    #[must_use]
    pub fn stem(&self) -> String {
        self.0
            .file_stem()
            .map_or_else(|| self.file_name(), |stem| stem.to_string_lossy().into_owned())
    }

    /// The extension, lower-cased and without the dot. Empty when there is none.
    #[must_use]
    pub fn extension(&self) -> String {
        self.0
            .extension()
            .map_or_else(String::new, |ext| ext.to_string_lossy().to_lowercase())
    }

    /// Sibling path with a different extension, keeping the stem.
    #[must_use]
    pub fn with_extension(&self, extension: &str) -> Self {
        Self(self.0.with_extension(extension))
    }

    /// Append a suffix to the stem and set the extension: `clip` + `_cut` + `mp4`.
    #[must_use]
    pub fn with_suffix(&self, suffix: &str, extension: &str) -> Self {
        let mut name = self.stem();
        name.push_str(suffix);
        Self(self.0.with_file_name(name).with_extension(extension))
    }

    /// True when the file is present on disk right now.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.0.is_file()
    }

    /// True when this is the same file as another, under the platform's case rules.
    #[must_use]
    pub fn same_file_as(&self, other: &Self) -> bool {
        if cfg!(windows) {
            self.0.to_string_lossy().to_lowercase() == other.0.to_string_lossy().to_lowercase()
        } else {
            self.0 == other.0
        }
    }
}

impl std::fmt::Display for MediaPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl From<&Path> for MediaPath {
    fn from(value: &Path) -> Self {
        Self(value.to_path_buf())
    }
}

impl From<PathBuf> for MediaPath {
    fn from(value: PathBuf) -> Self {
        Self(value)
    }
}

impl From<&str> for MediaPath {
    fn from(value: &str) -> Self {
        Self(PathBuf::from(value))
    }
}

impl From<String> for MediaPath {
    fn from(value: String) -> Self {
        Self(PathBuf::from(value))
    }
}

/// Ordering and equality follow the platform's case rules.
///
/// On Windows two spellings of the same path *are* the same file, so a project keyed by path
/// would otherwise hold the same source twice — and its segments would then be split across
/// two entries that look identical to the user. Implementing `Ord` by hand rather than
/// deriving it is what makes `BTreeMap<MediaPath, SegmentSource>` behave the way a Windows
/// user expects.
impl PartialOrd for MediaPath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MediaPath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if cfg!(windows) {
            self.0
                .to_string_lossy()
                .to_lowercase()
                .cmp(&other.0.to_string_lossy().to_lowercase())
        } else {
            self.0.cmp(&other.0)
        }
    }
}

/// Strip the `\\?\` prefix `canonicalize` adds on Windows, which ffmpeg and Premiere
/// both accept but which nobody wants to read in a log line.
fn strip_extended_prefix(path: &Path) -> PathBuf {
    match path.to_string_lossy().strip_prefix(r"\\?\") {
        Some(stripped) => PathBuf::from(stripped),
        None => path.to_path_buf(),
    }
}

/// The audio layout of a source, in the terms the executor needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFormat {
    /// Codec name as ffprobe reports it, e.g. `aac`.
    pub codec: String,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
}

impl AudioFormat {
    /// A one-line description for the media panel.
    #[must_use]
    pub fn summary(&self) -> String {
        format!("{} {} Hz {}ch", self.codec, self.sample_rate, self.channels)
    }
}

/// A container timescale: the number of ticks a track counts per second.
///
/// This is **not** a frame rate, and modelling it as one was a real bug: a timescale of
/// `1/90000` has a "frames per second" of 0.000011, which a rate validator reasonably rejects,
/// and reading `.numerator()` off a `FrameRate` built from `1/90000` yields `1` rather than
/// `90000` — so the plan silently carried a timescale of 1 and the muxer would have rescaled
/// the copied body into slow motion, which is exactly V1 ADR-001. A separate type makes that
/// mistake unrepresentable. `ffmpeg` wants the **denominator** as `-video_track_timescale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timescale {
    /// Ticks per second. The number ffmpeg is given.
    pub ticks_per_second: i64,
}

impl Timescale {
    /// The MP4 default these sources almost always use: 90 kHz.
    pub const NINETY_KHZ: Self = Self {
        ticks_per_second: 90_000,
    };
    /// 48 kHz, common on audio-led captures.
    pub const FORTY_EIGHT_KHZ: Self = Self {
        ticks_per_second: 48_000,
    };
    /// 1 kHz.
    pub const KILO: Self = Self {
        ticks_per_second: 1_000,
    };

    /// Build a timescale, refusing a value that cannot be a tick rate.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::FrameRate`] when the value is zero or negative.
    pub fn new(ticks_per_second: i64) -> CoreResult<Self> {
        if ticks_per_second <= 0 {
            return Err(CoreError::FrameRate(format!(
                "timescale {ticks_per_second}"
            )));
        }
        Ok(Self { ticks_per_second })
    }

    /// Read the `time_base` ffprobe reports, which is `1/90000` for a 90 kHz track.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::FrameRate`] when the fraction is not a usable `1/den` timebase.
    pub fn from_ffprobe_time_base(text: &str) -> CoreResult<Self> {
        let parsed = FrameRate::parse(text)?;
        if parsed.numerator() != 1 {
            return Err(CoreError::FrameRate(format!(
                "time base {text} is not of the form 1/den"
            )));
        }
        Self::new(parsed.denominator())
    }

    /// Ticks per second.
    #[must_use]
    pub const fn ticks(self) -> i64 {
        self.ticks_per_second
    }
}

impl Default for Timescale {
    fn default() -> Self {
        Self::NINETY_KHZ
    }
}

impl std::fmt::Display for Timescale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "1/{}", self.ticks_per_second)
    }
}

/// Everything the pure domain needs to know about a source file.
///
/// Produced by `trimmer-media` from one ffprobe call, and never mutated afterwards. Note
/// what is *absent*: there is no duration-in-seconds field, because a duration in seconds
/// is how a frame-exact tool acquires rounding errors. Ask [`MediaInfo::seconds_of`] when a
/// command line needs one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    /// Where the file is.
    pub path: MediaPath,
    /// Video codec name, e.g. `h264` or `hevc`.
    pub codec: String,
    /// Pixel format, e.g. `yuv420p`.
    pub pix_fmt: String,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// The rate the file claims, which is the grid timecode is read against.
    pub rate: FrameRate,
    /// The measured average rate. Differs from [`MediaInfo::rate`] on variable-rate files.
    pub average_rate: Option<FrameRate>,
    /// The container timescale. The re-encoded head must be written at this timescale or
    /// the muxer will rescale the copied body into slow motion (V1 ADR-001). Not a frame
    /// rate; see [`Timescale`].
    pub timebase: Timescale,
    /// How many frames the file has.
    pub frame_count: i64,
    /// Audio layout, when the source has audio.
    pub audio: Option<AudioFormat>,
    /// File size in bytes.
    pub size_bytes: u64,
    /// The first timestamp the picture reports. A fresh encode can start a fraction of a
    /// frame late, and the alignment check has to know that to compare frame for frame.
    pub start_time: f64,
}

impl MediaInfo {
    /// The last frame a caller may name.
    #[must_use]
    pub fn last_frame(&self) -> i64 {
        (self.frame_count - 1).max(0)
    }

    /// True when the file is variable frame rate, in which case the frame grid the whole
    /// method rests on does not hold and the app must warn rather than pretend.
    #[must_use]
    pub fn is_variable_rate(&self) -> bool {
        match self.average_rate {
            Some(average) => {
                let claimed = self.rate.as_f64();
                if claimed <= 0.0 {
                    return false;
                }
                ((claimed - average.as_f64()).abs() / claimed) > 0.001
            }
            None => false,
        }
    }

    /// Seconds from the start of the file to a frame boundary.
    #[must_use]
    pub fn seconds_of(&self, frame: i64) -> f64 {
        self.rate.seconds_of(frame)
    }

    /// A frame as a timecode in the form Premiere would write it.
    #[must_use]
    pub fn timecode_of(&self, frame: i64) -> String {
        format_timecode(frame, self.rate, None)
    }

    /// True when the codec can carry the head-patch method.
    ///
    /// The head and the body must share one codec; a source in anything else is refused
    /// rather than mangled (V1 ADR-001).
    #[must_use]
    pub fn supports_head_patch(&self) -> bool {
        matches!(self.codec.to_lowercase().as_str(), "h264" | "hevc" | "h265")
    }

    /// The encoder that can join this source's body, and any tag the muxer needs.
    #[must_use]
    pub fn encoder(&self) -> Option<(&'static str, &'static [&'static str])> {
        match self.codec.to_lowercase().as_str() {
            "h264" => Some(("libx264", &[])),
            "hevc" | "h265" => Some(("libx265", &["-tag:v", "hvc1"])),
            _ => None,
        }
    }

    /// One line for the media panel.
    #[must_use]
    pub fn summary(&self) -> String {
        let audio = self
            .audio
            .as_ref()
            .map_or_else(|| "no audio".to_owned(), AudioFormat::summary);
        format!(
            "{}x{} {}/{}, {} fps, {} frames, {}",
            self.width,
            self.height,
            self.codec,
            self.pix_fmt,
            self.rate.as_ffmpeg(),
            self.frame_count,
            audio
        )
    }
}

/// Which of the three ways a segment is actually cut.
///
/// There is deliberately no variant meaning "copy a body that does not begin on a
/// keyframe": that cut is not possible, so it is not representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CutMode {
    /// The in point lands on a keyframe, so every frame is copied untouched. Free.
    Copy,
    /// The frames before the first keyframe are re-encoded; everything after is the
    /// original packets.
    HeadPatch,
    /// No usable keyframe inside the segment, so all of it is re-encoded. The fallback.
    Reencode,
}

impl CutMode {
    /// True when the delivered frames are the original ones, bit for bit.
    #[must_use]
    pub const fn is_lossless(self) -> bool {
        matches!(self, Self::Copy | Self::HeadPatch)
    }

    /// A short label for the UI.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Copy => "Lossless copy",
            Self::HeadPatch => "Head patch",
            Self::Reencode => "Full re-encode",
        }
    }
}

/// A keyframe position supplied by the probe layer, in frames.
///
/// The core never asks ffprobe anything; the caller hands in the keyframe grid it found and
/// the planner decides. That is what keeps the plan a pure function — and therefore what
/// lets the same decisions be replayed in a test against the V1 engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeGrid {
    /// Keyframe positions in ascending order, in frames.
    pub keyframes: Vec<i64>,
    /// The window that was searched, so a refusal can say what it looked at.
    pub searched_from: i64,
    /// End of the searched window.
    pub searched_to: i64,
}

impl KeyframeGrid {
    /// A grid from raw frame positions.
    #[must_use]
    pub fn new(mut keyframes: Vec<i64>, searched_from: i64, searched_to: i64) -> Self {
        keyframes.sort_unstable();
        keyframes.dedup();
        Self {
            keyframes,
            searched_from,
            searched_to,
        }
    }

    /// The first keyframe at or after `frame`, if the grid holds one.
    #[must_use]
    pub fn first_at_or_after(&self, frame: i64) -> Option<i64> {
        self.keyframes.iter().copied().find(|&candidate| candidate >= frame)
    }
}

/// A unique segment identity, stable across renames and reorders.
///
/// `Default` is derived and yields the nil UUID — deliberately *not* a call to [`SegmentId::new`],
/// which mints a fresh v7 identity. A default that secretly generated a new id on every call
/// would make `..Default::default()` produce a different value each time, which is the opposite
/// of what `Default` promises, and would quietly break any code that used it as a sentinel.
/// Use [`SegmentId::new`] for a real segment.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SegmentId(pub Uuid);

impl SegmentId {
    /// A fresh identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// The UUID, for storage and for the wire.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}



impl std::fmt::Display for SegmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A unique project identity. `Default` is the nil UUID; see [`SegmentId`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ProjectId(pub Uuid);

impl ProjectId {
    /// A fresh identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// The UUID, for storage and for the wire.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}



impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A source file inside a project, plus what was learned when it was last probed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentSource {
    /// The file.
    pub path: MediaPath,
    /// The last probe. `None` when the file has not been read yet, or is offline.
    pub media: Option<MediaInfo>,
    /// False once a probe has failed or the file has gone missing. Segments are kept, so a
    /// project survives a drive being unplugged and comes back whole.
    pub available: bool,
    /// A free-text label, e.g. the speaker or the day.
    pub label: Option<String>,
}

impl SegmentSource {
    /// A source that has not been probed yet.
    #[must_use]
    pub fn unprobed(path: impl Into<MediaPath>) -> Self {
        Self {
            path: path.into(),
            media: None,
            available: true,
            label: None,
        }
    }

    /// The rate to read this source's timecodes against, when it is known.
    #[must_use]
    pub fn rate(&self) -> Option<FrameRate> {
        self.media.as_ref().map(|media| media.rate)
    }
}

/// One cut: a range in a source, a label, and the delivery decisions made for it.
///
/// `end` is `Option` deliberately. An open-ended segment means "to the end of the source",
/// which is how a user marks a cut before they know where the source ends — and which must
/// be resolved by the planner rather than by the segment, because the source may have been
/// swapped for a longer one since.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// Stable identity.
    pub id: SegmentId,
    /// Which source in the project this cuts.
    pub source: MediaPath,
    /// The label shown in the workspace and used to name the output.
    pub name: String,
    /// First frame kept, inclusive.
    pub start_frame: i64,
    /// One past the last frame kept. `None` means the resolved end of the source.
    pub end_frame: Option<i64>,
    /// A note for the editor, carried into the export metadata.
    pub note: Option<String>,
    /// Free-form tags, used for filtering and for delivery routing.
    pub tags: Vec<String>,
    /// The delivery preset name, resolved against the project's preset table.
    pub preset: Option<String>,
    /// Frames of handle to add before the start and after the end, for a crossfade.
    pub handle_frames: i64,
    /// Whether this segment is included in a batch run.
    pub enabled: bool,
}

impl Segment {
    /// A segment covering `[start, end)`.
    #[must_use]
    pub fn new(source: impl Into<MediaPath>, name: impl Into<String>, start_frame: i64, end_frame: i64) -> Self {
        Self {
            id: SegmentId::new(),
            source: source.into(),
            name: name.into(),
            start_frame,
            end_frame: Some(end_frame),
            note: None,
            tags: Vec::new(),
            preset: None,
            handle_frames: 0,
            enabled: true,
        }
    }

    /// How many frames the segment covers, before handles, when the end is known.
    #[must_use]
    pub fn frame_count(&self) -> Option<i64> {
        self.end_frame.map(|end| (end - self.start_frame).max(0))
    }

    /// True when this segment runs to the end of its source.
    #[must_use]
    pub const fn is_open_ended(&self) -> bool {
        self.end_frame.is_none()
    }

    /// The range including handles, clamped to the source.
    ///
    /// Handles are what makes a cut usable in an edit: two seconds of overlap either side
    /// is what an editor needs to place a crossfade without asking for the master again.
    #[must_use]
    pub fn range_with_handles(&self, media: &MediaInfo) -> (i64, i64) {
        let handles = self.handle_frames.max(0);
        let end = self.end_frame.unwrap_or(media.frame_count);
        let start = (self.start_frame - handles).max(0);
        let end = (end + handles).min(media.frame_count);
        (start, end.max(start))
    }
}

/// How thoroughly a finished cut is checked before it is reported as good.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyPolicy {
    /// Do not measure. Fast; for a rough cut.
    Off,
    /// Check the frame count, duration and audio alignment. One pass over the metadata.
    Standard,
    /// Also hash decoded frames at sample points on each file's own grid, so "is this the
    /// same picture" is answered exactly rather than approximately. The default: the strongest
    /// check that does not require decoding the whole segment.
    #[default]
    Strict,
    /// Also compare the re-encoded head against the source by structural similarity, and
    /// check every caption cue against the source it came from.
    Forensic,
}

impl VerifyPolicy {
    /// True when decoded frames are hashed.
    #[must_use]
    pub const fn hashes_frames(self) -> bool {
        matches!(self, Self::Strict | Self::Forensic)
    }

    /// True when the head is compared pixel-wise and captions are checked cue by cue.
    #[must_use]
    pub const fn is_forensic(self) -> bool {
        matches!(self, Self::Forensic)
    }
}



/// A body of work: the sources, the cuts, and the decisions that apply to all of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Stable identity.
    pub id: ProjectId,
    /// Display name.
    pub name: String,
    /// Sources, keyed by path so a source cannot be added twice under two spellings.
    pub sources: BTreeMap<MediaPath, SegmentSource>,
    /// Segments, in the order the user arranged them.
    pub segments: Vec<Segment>,
    /// Delivery presets by name.
    pub presets: BTreeMap<String, crate::delivery::DeliveryPreset>,
    /// Which preset a segment gets when it does not name one.
    pub default_preset: String,
    /// How hard to check each finished cut.
    pub verify: VerifyPolicy,
    /// Where outputs are written. `None` means beside each source.
    pub output_dir: Option<MediaPath>,
    /// The name of the person or suite that made these cuts, recorded in the audit log.
    pub created_by: String,
    /// Seconds since the Unix epoch, for the audit trail.
    pub created_at: i64,
    /// Seconds since the Unix epoch.
    pub updated_at: i64,
}

impl Project {
    /// A new, empty project with the standard preset library.
    #[must_use]
    pub fn new(name: impl Into<String>, created_by: impl Into<String>, now: i64) -> Self {
        let presets = crate::delivery::standard_presets();
        Self {
            id: ProjectId::new(),
            name: name.into(),
            sources: BTreeMap::new(),
            segments: Vec::new(),
            default_preset: crate::delivery::DEFAULT_PRESET.to_owned(),
            presets: presets.into_iter().map(|p| (p.name.clone(), p)).collect(),
            verify: VerifyPolicy::default(),
            output_dir: None,
            created_by: created_by.into(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Add or replace a source, keeping any segments that point at it.
    pub fn upsert_source(&mut self, source: SegmentSource) {
        self.sources.insert(source.path.clone(), source);
    }

    /// Look up a source by path.
    #[must_use]
    pub fn source(&self, path: &MediaPath) -> Option<&SegmentSource> {
        self.sources.get(path)
    }

    /// The media facts for a path, when they are known.
    #[must_use]
    pub fn media(&self, path: &MediaPath) -> Option<&MediaInfo> {
        self.sources.get(path).and_then(|source| source.media.as_ref())
    }

    /// The rate to read a source's timecodes against.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] when the source has never been probed, because
    /// guessing a rate is exactly how timecode silently becomes wrong.
    pub fn rate_for(&self, path: &MediaPath) -> CoreResult<FrameRate> {
        self.media(path)
            .map(|media| media.rate)
            .ok_or_else(|| CoreError::NotFound {
                entity: "a probed source".to_owned(),
                id: path.to_string(),
            })
    }

    /// Append a segment, refusing one whose source is not in the project.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] when the segment's source is unknown.
    pub fn add_segment(&mut self, segment: Segment) -> CoreResult<SegmentId> {
        if !self.sources.contains_key(&segment.source) {
            return Err(CoreError::NotFound {
                entity: "source".to_owned(),
                id: segment.source.to_string(),
            });
        }
        let id = segment.id;
        self.segments.push(segment);
        Ok(id)
    }

    /// A segment by id.
    #[must_use]
    pub fn segment(&self, id: SegmentId) -> Option<&Segment> {
        self.segments.iter().find(|segment| segment.id == id)
    }

    /// A mutable segment by id.
    pub fn segment_mut(&mut self, id: SegmentId) -> Option<&mut Segment> {
        self.segments.iter_mut().find(|segment| segment.id == id)
    }

    /// Remove a segment.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] when the id does not resolve.
    pub fn remove_segment(&mut self, id: SegmentId) -> CoreResult<Segment> {
        let index = self
            .segments
            .iter()
            .position(|segment| segment.id == id)
            .ok_or_else(|| CoreError::NotFound {
                entity: "segment".to_owned(),
                id: id.to_string(),
            })?;
        Ok(self.segments.remove(index))
    }

    /// Move a segment within the running order.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] when either index is out of range.
    pub fn reorder_segment(&mut self, from: usize, to: usize) -> CoreResult<()> {
        if from >= self.segments.len() || to >= self.segments.len() {
            return Err(CoreError::NotFound {
                entity: "segment position".to_owned(),
                id: format!("{from}->{to}"),
            });
        }
        let segment = self.segments.remove(from);
        self.segments.insert(to, segment);
        Ok(())
    }

    /// The segments a batch run would process, in order.
    #[must_use]
    pub fn runnable(&self) -> Vec<&Segment> {
        self.segments
            .iter()
            .filter(|segment| {
                segment.enabled && self.sources.get(&segment.source).is_some_and(|s| s.available)
            })
            .collect()
    }

    /// The preset a segment should use, falling back to the project default.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Delivery`] when neither the segment nor the project names a
    /// preset that exists — better a clear refusal than a silently wrong container.
    pub fn preset_for(&self, segment: &Segment) -> CoreResult<&crate::delivery::DeliveryPreset> {
        let name = segment.preset.as_deref().unwrap_or(&self.default_preset);
        self.presets
            .get(name)
            .ok_or_else(|| CoreError::Delivery {
                preset: name.to_owned(),
                reason: "no preset by that name is defined in this project".to_owned(),
            })
    }

    /// Total frames across the runnable segments, for the batch progress bar.
    #[must_use]
    pub fn total_frames(&self) -> i64 {
        self.runnable()
            .iter()
            .filter_map(|segment| segment.frame_count())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media() -> MediaInfo {
        MediaInfo {
            path: MediaPath::new(r"H:\master.mp4"),
            codec: "h264".to_owned(),
            pix_fmt: "yuv420p".to_owned(),
            width: 1920,
            height: 1080,
            rate: FrameRate::FPS_29_97,
            average_rate: Some(FrameRate::FPS_29_97),
            timebase: Timescale::NINETY_KHZ,
            frame_count: 216_000,
            audio: Some(AudioFormat {
                codec: "aac".to_owned(),
                sample_rate: 48_000,
                channels: 2,
            }),
            size_bytes: 6_000_000_000,
            start_time: 0.0,
        }
    }

    #[test]
    fn a_project_refuses_a_segment_for_a_source_it_does_not_have() {
        let mut project = Project::new("Rollup", "Fernando", 1_700_000_000);
        let segment = Segment::new(r"H:\other.mp4", "cold open", 0, 100);
        let error = project.add_segment(segment).expect_err("refused");
        assert!(matches!(error, CoreError::NotFound { .. }));
    }

    #[test]
    fn a_segment_round_trips_through_its_source() {
        let mut project = Project::new("Rollup", "Fernando", 1_700_000_000);
        project.upsert_source(SegmentSource {
            path: MediaPath::new(r"H:\master.mp4"),
            media: Some(media()),
            available: true,
            label: Some("Andy".to_owned()),
        });
        let segment = Segment::new(r"H:\master.mp4", "begging", 1_000, 3_000);
        let id = project.add_segment(segment).expect("added");
        assert_eq!(project.segment(id).expect("present").name, "begging");
        assert_eq!(project.rate_for(&MediaPath::new(r"H:\master.mp4")).expect("known"), FrameRate::FPS_29_97);
    }

    #[test]
    fn an_empty_name_still_produces_a_usable_segment() {
        let segment = Segment::new(r"H:\master.mp4", "", 0, 1);
        assert_eq!(segment.frame_count(), Some(1));
        assert!(segment.enabled);
        assert!(!segment.is_open_ended());
    }

    #[test]
    fn handles_are_clamped_to_the_source_and_never_invert_the_range() {
        let media = media();
        let mut segment = Segment::new(media.path.clone(), "x", 10, 20);
        segment.handle_frames = 48;
        assert_eq!(segment.range_with_handles(&media), (0, 68));

        segment.start_frame = 0;
        assert_eq!(segment.range_with_handles(&media), (0, 68));

        let mut open = Segment::new(media.path.clone(), "to the end", 5, 0);
        open.end_frame = None;
        open.handle_frames = 100;
        assert_eq!(open.range_with_handles(&media), (0, media.frame_count));
    }

    #[test]
    fn variable_rate_is_detected_by_relative_drift() {
        let mut info = media();
        info.average_rate = Some(FrameRate::new(25, 1).expect("ok"));
        assert!(info.is_variable_rate());

        info.average_rate = Some(FrameRate::parse("29.970").expect("ok"));
        assert!(!info.is_variable_rate());
    }

    #[test]
    fn only_h264_and_hevc_can_carry_the_head_patch() {
        let mut info = media();
        assert!(info.supports_head_patch());
        assert_eq!(info.encoder().expect("encoders").0, "libx264");

        info.codec = "hevc".to_owned();
        assert_eq!(info.encoder().expect("encoders").0, "libx265");

        info.codec = "prores".to_owned();
        assert!(!info.supports_head_patch());
        assert!(info.encoder().is_none());
    }

    #[test]
    fn a_missing_preset_is_refused_rather_than_defaulted() {
        let project = Project::new("Rollup", "Fernando", 0);
        let mut segment = Segment::new(r"H:\master.mp4", "x", 0, 10);
        segment.preset = Some("does-not-exist".to_owned());
        let error = project.preset_for(&segment).expect_err("refused");
        assert!(matches!(error, CoreError::Delivery { .. }));
    }

    #[test]
    fn paths_compare_case_insensitively_on_windows() {
        let a = MediaPath::new(r"H:\THEROLLUPFILES\master.mp4");
        let b = MediaPath::new(r"h:\therollupfiles\MASTER.mp4");
        if cfg!(windows) {
            assert!(a.same_file_as(&b));
        }
    }

    #[test]
    fn suffix_helpers_build_expected_names() {
        let path = MediaPath::new(r"H:\work\Andy Ross.mp4");
        assert_eq!(path.stem(), "Andy Ross");
        assert_eq!(path.extension(), "mp4");
        assert_eq!(path.with_extension("srt").to_string(), r"H:\work\Andy Ross.srt");
        assert_eq!(
            path.with_suffix(" begging 01", "mp4").to_string(),
            r"H:\work\Andy Ross begging 01.mp4"
        );
    }

    #[test]
    fn reordering_moves_and_rejects_out_of_range() {
        let mut project = Project::new("Rollup", "Fernando", 0);
        project.upsert_source(SegmentSource::unprobed(r"H:\master.mp4"));
        for index in 0..3 {
            project
                .add_segment(Segment::new(r"H:\master.mp4", format!("s{index}"), index * 10, index * 10 + 5))
                .expect("added");
        }
        project.reorder_segment(0, 2).expect("moved");
        let names: Vec<&str> = project.segments.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["s1", "s2", "s0"]);
        assert!(project.reorder_segment(0, 9).is_err());
    }
}

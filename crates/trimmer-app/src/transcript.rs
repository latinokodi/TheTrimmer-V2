//! The transcript service: search a transcript and turn a sentence into a segment.
//!
//! This is the module behind the feature that changes what the tool is *for*. A three-hour
//! interview arrives with a transcript and a hundred possible moments in it. Asking an editor to
//! find the frame number of "the bit where he says the thing about custody" and then type two
//! timecodes is asking them to do the computer's job. With this, they type four words, click the
//! line, and the in and out points are already frame-exact.
//!
//! ## Indexes are built once and kept
//!
//! A three-hour transcript is around three thousand cues. Searching is fast either way, but
//! *folding* the text — lower-casing it and collapsing whitespace — on every keystroke is not,
//! and it is what a naive search box does. [`TranscriptService`] holds one
//! [`trimmer_core::TranscriptIndex`] per file, so the work happens once and the search box stays
//! instant as the user types.
//!
//! ## A search result becomes a cut
//!
//! [`TranscriptService::find_phrase`] says where a phrase was found. [`TranscriptView::cut_for`]
//! turns that into a frame range, and offers both ways an editor wants it:
//!
//! * **Whole sentence** — the default. A segment that starts on the first word of a sentence and
//!   ends after the last is what an editor wants nine times in ten.
//! * **To the nearest pause** — for when a sentence runs long, or when a cut must not clip a
//!   plosive. The transcript already knows where the gaps in the speech are, and the scissors
//!   belong in a gap.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use trimmer_core::{
    caption, format_timecode, Cue, FrameRate, Grouping, Hit, MediaPath, Sentence, TranscriptIndex,
};

use crate::ports::TranscriptSource;
use crate::{AppError, AppResult};

/// How far the "snap to the nearest pause" search will look before giving up, in seconds.
///
/// Beyond this a gap is not a pause, it is a different part of the conversation, and snapping to
/// it would silently deliver a segment many seconds longer than the one the editor marked.
pub const MAX_SNAP_SECONDS: f64 = 2.0;

/// A transcript, as the interface shows it in a list.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSummary {
    /// The video it belongs to.
    pub video: MediaPath,
    /// The caption file.
    pub path: MediaPath,
    /// How many cues.
    pub cues: usize,
    /// How many groups the cues form.
    pub groups: usize,
    /// How many words.
    pub words: usize,
    /// How long it runs, in seconds.
    pub seconds: f64,
    /// The last timecode in it, for a quick sanity check against the video.
    pub last_timecode: String,
    /// A warning when the transcript runs past the end of the video, which usually means the
    /// transcript belongs to a different file.
    pub warning: Option<String>,
}

/// One transcript, indexed and ready to search.
#[derive(Debug, Clone)]
pub struct TranscriptView {
    /// The video it belongs to.
    pub video: MediaPath,
    /// The caption file it came from.
    pub path: MediaPath,
    /// The cues, in order.
    pub cues: Vec<Cue>,
    /// The groups they form.
    pub groups: Vec<Sentence>,
    index: Arc<TranscriptIndex>,
}

impl TranscriptView {
    /// Build a view over cues.
    #[must_use]
    pub fn new(video: MediaPath, path: PathBuf, cues: Vec<Cue>, grouping: Grouping) -> Self {
        let index = TranscriptIndex::new(cues.clone(), grouping);
        let groups = index.sentences.clone();
        Self {
            video,
            path: MediaPath::new(path),
            cues,
            groups,
            index: Arc::new(index),
        }
    }

    /// How many cues.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cues.len()
    }

    /// True when the transcript holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    /// How many words.
    #[must_use]
    pub fn word_count(&self) -> usize {
        self.index.word_count()
    }

    /// How long it runs, in seconds.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.cues.iter().map(|cue| cue.end).fold(0.0, f64::max)
    }

    /// Search for a phrase.
    #[must_use]
    pub fn find_phrase(&self, phrase: &str, rate: FrameRate, limit: usize) -> Vec<Hit> {
        self.index.hits(phrase, rate, limit)
    }

    /// How many times a phrase occurs.
    #[must_use]
    pub fn count(&self, phrase: &str) -> usize {
        self.index.count(phrase)
    }

    /// The index, for a caller that wants to do something this view does not.
    #[must_use]
    pub fn index(&self) -> &TranscriptIndex {
        &self.index
    }

    /// The frame range for a hit, as whole sentences.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Transcript`] when the hit refers to a cue that is not there.
    pub fn cut_for(&self, hit: &Hit, rate: FrameRate) -> AppResult<(i64, i64)> {
        self.index
            .range_for_hit(hit, rate)
            .map_err(|error| AppError::Transcript {
                path: self.path.to_string(),
                reason: error.to_string(),
            })
    }

    /// The frame range for a hit, widened to the nearest pauses either side.
    #[must_use]
    pub fn cut_for_with_pauses(&self, hit: &Hit, rate: FrameRate) -> Option<(i64, i64)> {
        let (start, end) = self.cut_for(hit, rate).ok()?;
        Some(
            self.index
                .snap_to_silence(start, end, rate, MAX_SNAP_SECONDS),
        )
    }

    /// The group a frame falls in, as a `(first_cue, last_cue)` pair.
    #[must_use]
    pub fn group_at_frame(&self, frame: i64, rate: FrameRate) -> Option<usize> {
        self.index.sentence_at_frame(frame, rate)
    }

    /// Grow a frame range out to the sentences it touches.
    #[must_use]
    pub fn snap_to_sentence(&self, start: i64, end: i64, rate: FrameRate) -> (i64, i64) {
        self.index.snap_to_sentence(start, end, rate)
    }

    /// The text of a group, with the timecode it starts at.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Transcript`] when the group index is out of range.
    pub fn group_text(&self, index: usize, rate: FrameRate) -> AppResult<(String, String)> {
        let group = self.groups.get(index).ok_or_else(|| AppError::Transcript {
            path: self.path.to_string(),
            reason: format!("there is no group {index}"),
        })?;
        Ok((
            group.text.clone(),
            format_timecode(group.start_frame(rate), rate, None),
        ))
    }

    /// A summary for a list.
    #[must_use]
    pub fn summary(&self, media: Option<&trimmer_core::MediaInfo>) -> TranscriptSummary {
        let duration = self.duration();
        let warning = media.and_then(|media| {
            let video_seconds = media.rate.seconds_of(media.frame_count);
            if duration > video_seconds + 1.0 {
                Some(format!(
                    "the transcript runs to {duration:.0}s but the video is {video_seconds:.0}s \
                     long: it may belong to a different file"
                ))
            } else {
                None
            }
        });
        TranscriptSummary {
            video: self.video.clone(),
            path: self.path.clone(),
            cues: self.cues.len(),
            groups: self.groups.len(),
            words: self.word_count(),
            seconds: duration,
            last_timecode: self.cues.last().map_or_else(
                || "00:00:00:00".to_owned(),
                |cue| {
                    let rate = media.map_or(FrameRate::FPS_25, |media| media.rate);
                    format_timecode(rate.frames_in(cue.end), rate, None)
                },
            ),
            warning,
        }
    }
}

/// Loads and caches transcripts.
///
/// Holds one index per file. Two threads asking for the same transcript share one, which is why
/// the cache is behind a mutex rather than rebuilt per call — but the search itself takes the lock
/// only to clone an `Arc`, so two searches do not serialise against each other.
pub struct TranscriptService {
    source: Arc<dyn TranscriptSource>,
    grouping: Grouping,
    cache: Mutex<BTreeMap<PathBuf, Arc<TranscriptView>>>,
}

impl std::fmt::Debug for TranscriptService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptService")
            .field("grouping", &self.grouping)
            .field("cached", &self.cache.lock().map(|c| c.len()).unwrap_or(0))
            .finish_non_exhaustive()
    }
}

impl TranscriptService {
    /// A service over a caption source.
    #[must_use]
    pub fn new(source: Arc<dyn TranscriptSource>, grouping: Grouping) -> Self {
        Self {
            source,
            grouping,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    /// The grouping in use.
    #[must_use]
    pub const fn grouping(&self) -> Grouping {
        self.grouping
    }

    /// Change the grouping, which invalidates every cached index.
    pub fn set_grouping(&mut self, grouping: Grouping) {
        if grouping != self.grouping {
            self.grouping = grouping;
            if let Ok(mut cache) = self.cache.lock() {
                cache.clear();
            }
        }
    }

    /// Forget a cached transcript, for when its file has changed on disk.
    pub fn invalidate(&self, path: &PathBuf) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.remove(path);
        }
    }

    /// The transcript beside a video, if there is one.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Transcript`] when the file exists but holds no readable cues, which is
    /// usually a sign it is not an SRT file at all.
    pub fn load(&self, video: &MediaPath) -> AppResult<Option<Arc<TranscriptView>>> {
        let Some(path) = self.source.find_for(video) else {
            return Ok(None);
        };
        if let Ok(cache) = self.cache.lock() {
            if let Some(found) = cache.get(&path) {
                return Ok(Some(Arc::clone(found)));
            }
        }
        let transcript = self
            .source
            .read(&path)
            .map_err(|error| AppError::Transcript {
                path: video.to_string(),
                reason: error.to_string(),
            })?;
        if transcript.cues.is_empty() {
            return Err(AppError::Transcript {
                path: path.display().to_string(),
                reason: "the file holds no readable cues; is it an SRT file?".to_owned(),
            });
        }
        let view = Arc::new(TranscriptView::new(
            video.clone(),
            path.clone(),
            transcript.cues,
            self.grouping,
        ));
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(path, Arc::clone(&view));
        }
        Ok(Some(view))
    }

    /// Load a transcript from a named file rather than searching beside the video.
    ///
    /// # Errors
    ///
    /// As [`TranscriptService::load`].
    pub fn load_from(&self, video: &MediaPath, path: &PathBuf) -> AppResult<Arc<TranscriptView>> {
        if let Ok(cache) = self.cache.lock() {
            if let Some(found) = cache.get(path) {
                return Ok(Arc::clone(found));
            }
        }
        let transcript = caption::read(path).map_err(|error| AppError::Transcript {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        let view = Arc::new(TranscriptView::new(
            video.clone(),
            path.clone(),
            transcript.cues,
            self.grouping,
        ));
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(path.clone(), Arc::clone(&view));
        }
        Ok(view)
    }

    /// Summaries for every video in a list that has a transcript.
    #[must_use]
    pub fn summaries<'a>(
        &self,
        videos: impl IntoIterator<Item = &'a MediaPath>,
    ) -> Vec<TranscriptSummary> {
        videos
            .into_iter()
            .filter_map(|video| self.load(video).ok().flatten())
            .map(|view| view.summary(None))
            .collect()
    }

    /// How many transcripts are cached, for a diagnostics line.
    #[must_use]
    pub fn cached_count(&self) -> usize {
        self.cache.lock().map(|cache| cache.len()).unwrap_or(0)
    }
}

/// The combined text of a set of cues, one line per cue, for a copy-to-clipboard or an export.
#[must_use]
pub fn cues_to_text(cues: &[Cue]) -> String {
    cues.iter()
        .map(Cue::one_line)
        .collect::<Vec<_>>()
        .join("\n")
}

//! The measured facts a verdict is drawn from.
//!
//! Everything a check needs arrives here as plain data: the numbers one `ffprobe` pass
//! reports, a window of frame hashes from `ffmpeg -f framemd5`, and a similarity score taken
//! from two extracted stills. Nothing in this module measures anything — producing these
//! values is `trimmer-media`'s job, and it does not exist yet. Keeping the two apart is what
//! lets every rule in [`crate::check`] be tested without a single byte of media.

use serde::{Deserialize, Serialize};

use trimmer_core::timecode::FrameRate;
use trimmer_core::MediaPath;

/// What one ffprobe/ffmpeg pass can say about a finished file.
///
/// The fields are exactly what a check can be built from, and nothing more. There is no
/// "expected" anything here: facts are what was measured, and what was *wanted* comes from
/// the [`trimmer_core::CutPlan`] the file was made from. A verification that read its
/// expectations out of the file it is checking would agree with itself and prove nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutFacts {
    /// The file these facts describe.
    pub path: MediaPath,
    /// How many frames the video stream actually holds.
    pub frame_count: i64,
    /// The picture's content length in seconds, as the container reports it.
    pub video_duration: f64,
    /// The first timestamp the picture reports. A fresh encode can start a fraction of a
    /// frame late, so this is not always zero and is not always the same on both tracks.
    pub video_start_time: f64,
    /// The audio stream's content length, when the file has audio.
    pub audio_duration: Option<f64>,
    /// The first timestamp the audio reports, when the file has audio.
    pub audio_start_time: Option<f64>,
    /// The rate the file claims, when the probe could read one.
    pub rate: Option<FrameRate>,
    /// Video codec name as ffprobe reports it, e.g. `h264`.
    pub codec: String,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Size on disk, in bytes.
    pub size_bytes: u64,
}

impl CutFacts {
    /// Where the picture ends in the file's own timeline.
    ///
    /// The *end* is what audio alignment is about. Two tracks that start at different
    /// timestamps — which is what AAC priming does — can still end level, and it is ending
    /// level that decides whether the sound runs out before the picture or hangs over it.
    #[must_use]
    pub fn video_end(&self) -> f64 {
        self.video_start_time + self.video_duration
    }

    /// Where the audio ends in the file's own timeline, when the file has audio.
    ///
    /// An absent start time is read as zero. A track that reports a duration but no start is
    /// one whose timestamps were never written; treating it as starting at zero is the only
    /// reading that leaves the alignment check meaningful.
    #[must_use]
    pub fn audio_end(&self) -> Option<f64> {
        self.audio_duration
            .map(|duration| self.audio_start_time.unwrap_or(0.0) + duration)
    }

    /// True when the file carries an audio stream.
    #[must_use]
    pub const fn has_audio(&self) -> bool {
        self.audio_duration.is_some()
    }

    /// One frame's worth of seconds at the rate the file claims.
    ///
    /// This is the file's own rate, used for tolerances that are expressed in frames — how
    /// far a picture may be out before it is wrong. A duration *comparison* uses the plan's
    /// rate instead, because that is the grid the frames were counted on.
    #[must_use]
    pub fn seconds_per_frame(&self) -> Option<f64> {
        let rate = self.rate?;
        if rate.numerator() == 0 {
            return None;
        }
        Some(rate.denominator() as f64 / rate.numerator() as f64)
    }
}

/// MD5 of each decoded frame in a window, as `ffmpeg -f framemd5` reports it.
///
/// The digests are of *decoded* frames, so they are equal exactly when the pictures are
/// identical — a re-encode never matches, and a stream-copied body always does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameHashes {
    /// One digest per frame, in presentation order, lowercase hex.
    pub digests: Vec<String>,
    /// The frame number the first digest belongs to in *this* file.
    ///
    /// Two files that were cut from the same source count from different places, and a
    /// failure that names a bare array index is a failure nobody can act on. This is what
    /// turns an index back into a frame number.
    pub first_frame: i64,
}

impl FrameHashes {
    /// A window over `digests` starting at frame `first_frame`.
    #[must_use]
    pub fn new(digests: Vec<String>, first_frame: i64) -> Self {
        Self {
            digests,
            first_frame,
        }
    }

    /// How many frames the window holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.digests.len()
    }

    /// True when the window holds no frames at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.digests.is_empty()
    }

    /// The frame number digest `index` belongs to, when the window holds one.
    #[must_use]
    pub fn frame_of(&self, index: usize) -> Option<i64> {
        if index >= self.digests.len() {
            return None;
        }
        let offset = i64::try_from(index).ok()?;
        Some(self.first_frame + offset)
    }
}

/// A structural-similarity score between two stills, 0.0..=1.0, or `None` when unmeasurable.
///
/// Wrapped in a newtype rather than used as a bare `Option<f64>` so that "the comparison
/// could not be made" and "the comparison produced a score" cannot be confused with "the
/// score was zero" — which for a similarity check is the worst possible confusion, because
/// zero is a total mismatch and `None` is no answer at all.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Similarity(
    /// The score, when one could be taken.
    pub Option<f64>,
);

impl Similarity {
    /// A comparison that could not be made.
    pub const UNMEASURABLE: Self = Self(None);

    /// A measured score.
    #[must_use]
    pub const fn measured(score: f64) -> Self {
        Self(Some(score))
    }

    /// The score, when there is one.
    #[must_use]
    pub const fn score(self) -> Option<f64> {
        self.0
    }

    /// True when a score was taken.
    #[must_use]
    pub const fn is_measurable(self) -> bool {
        self.0.is_some()
    }
}

impl Default for Similarity {
    fn default() -> Self {
        Self::UNMEASURABLE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> CutFacts {
        CutFacts {
            path: MediaPath::new(r"H:\out\clip.mp4"),
            frame_count: 600,
            video_duration: 20.02,
            video_start_time: 0.0,
            audio_duration: Some(20.05),
            audio_start_time: Some(-0.03),
            rate: Some(FrameRate::FPS_29_97),
            codec: "h264".to_owned(),
            width: 1920,
            height: 1080,
            size_bytes: 1_048_576,
        }
    }

    #[test]
    fn a_file_reports_where_each_track_ends() {
        let measured = facts();
        assert!((measured.video_end() - 20.02).abs() < 1e-9);
        assert!((measured.audio_end().expect("has audio") - 20.02).abs() < 1e-9);
        assert!(measured.has_audio());
    }

    #[test]
    fn an_audio_track_with_no_start_time_is_read_as_starting_at_zero() {
        let mut measured = facts();
        measured.audio_start_time = None;
        assert!((measured.audio_end().expect("has audio") - 20.05).abs() < 1e-9);
    }

    #[test]
    fn a_file_with_no_audio_has_no_audio_end() {
        let mut measured = facts();
        measured.audio_duration = None;
        measured.audio_start_time = None;
        assert_eq!(measured.audio_end(), None);
        assert!(!measured.has_audio());
    }

    #[test]
    fn seconds_per_frame_comes_from_the_file_rate() {
        let measured = facts();
        let seconds = measured.seconds_per_frame().expect("has a rate");
        assert!((seconds - 1001.0 / 30_000.0).abs() < 1e-12);

        let mut unprobed = facts();
        unprobed.rate = None;
        assert_eq!(unprobed.seconds_per_frame(), None);
    }

    #[test]
    fn a_hash_window_knows_its_frames_and_its_length() {
        let hashes = FrameHashes::new(vec!["aa".to_owned(), "bb".to_owned()], 1_000);
        assert_eq!(hashes.len(), 2);
        assert!(!hashes.is_empty());
        assert_eq!(hashes.frame_of(0), Some(1_000));
        assert_eq!(hashes.frame_of(1), Some(1_001));
        assert_eq!(hashes.frame_of(2), None);
        assert!(FrameHashes::default().is_empty());
    }

    #[test]
    fn an_unmeasurable_similarity_is_not_a_score_of_zero() {
        assert_eq!(Similarity::UNMEASURABLE.score(), None);
        assert!(!Similarity::UNMEASURABLE.is_measurable());
        assert_eq!(Similarity::measured(0.0).score(), Some(0.0));
        assert!(Similarity::measured(0.0).is_measurable());
        assert_eq!(Similarity::default(), Similarity::UNMEASURABLE);
    }
}

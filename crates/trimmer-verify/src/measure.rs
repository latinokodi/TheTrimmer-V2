//! The measurement vocabulary: what a checking pass has to be able to do.
//!
//! [`MediaMeasurer`] is the seam between this crate and the tools that can see a file.
//! `trimmer-media` will implement it with ffprobe and ffmpeg; [`NoMeasurer`] implements it
//! with values a test hands in. That double is the whole reason the verdict logic in
//! [`crate::check`] can be exhaustive without any media at all: the four methods below are
//! the only place a media file could ever be needed, and none of them is called by a rule.

use trimmer_core::timecode::FrameRate;
use trimmer_core::{CoreResult, MediaPath};

use crate::facts::{CutFacts, FrameHashes, Similarity};

/// Everything a verification pass needs from the tools that can look at a file.
///
/// This crate never implements it. An implementation runs ffprobe and ffmpeg — which is
/// exactly what must not happen in the crate that decides whether a cut is good, because a
/// rule that can be reached only through a subprocess is a rule that can be tested only by
/// having the file. The rules live here; the measuring lives next door.
/// # Thread safety
///
/// The bound is `Send + Sync` because a measurer is held behind an `Arc` in shared application
/// state and called from whichever thread the caller is on. Requiring it here rather than at each
/// call site means an implementation that cannot be shared is refused when it is written.
pub trait MediaMeasurer: Send + Sync {
    /// The facts one probe pass reports about a finished file.
    ///
    /// # Errors
    ///
    /// Returns a [`trimmer_core::CoreError`] when the file cannot be read or probed.
    fn facts(&self, path: &MediaPath) -> CoreResult<CutFacts>;

    /// The MD5 of each decoded frame in a window, as `ffmpeg -f framemd5` reports it.
    ///
    /// The window starts at `start_frame` and runs for `count` frames on `rate`'s grid.
    ///
    /// # Errors
    ///
    /// Returns a [`trimmer_core::CoreError`] when the frames cannot be decoded.
    fn frame_hashes(
        &self,
        path: &MediaPath,
        start_frame: i64,
        count: i64,
        rate: FrameRate,
    ) -> CoreResult<FrameHashes>;

    /// Extract one frame as PNG bytes, for a pixel comparison.
    ///
    /// # Errors
    ///
    /// Returns a [`trimmer_core::CoreError`] when the frame cannot be extracted.
    fn extract_frame(&self, path: &MediaPath, frame: i64, rate: FrameRate) -> CoreResult<Vec<u8>>;

    /// The structural similarity between two stills, 0.0..=1.0, or [`Similarity::UNMEASURABLE`]
    /// when the comparison cannot be made.
    ///
    /// # Errors
    ///
    /// Returns a [`trimmer_core::CoreError`] when the images cannot be read as images.
    fn ssim(&self, a: &[u8], b: &[u8]) -> CoreResult<Similarity>;
}

/// A measurer that measures nothing: it hands back the values it was configured with.
///
/// This is the test double the crate's own rules are proved against, and it is public
/// because anyone verifying a cut on a machine with no media — a CI box, a review laptop —
/// wants the same thing. It touches no filesystem, starts no process and reads no clock, so
/// a test that uses it is as deterministic as the verdict logic it exercises.
///
/// The configured facts are returned for *any* path asked for, which is what makes it a
/// double rather than a model: the path is not what is being tested.
#[derive(Debug, Clone, PartialEq)]
pub struct NoMeasurer {
    facts: CutFacts,
    hashes: FrameHashes,
    frame: Vec<u8>,
    similarity: Similarity,
}

impl NoMeasurer {
    /// A measurer that reports `facts` and nothing else.
    #[must_use]
    pub fn new(facts: CutFacts) -> Self {
        Self {
            facts,
            hashes: FrameHashes::default(),
            frame: Vec::new(),
            similarity: Similarity::UNMEASURABLE,
        }
    }

    /// Report these frame hashes.
    #[must_use]
    pub fn with_hashes(mut self, hashes: FrameHashes) -> Self {
        self.hashes = hashes;
        self
    }

    /// Return these bytes from [`MediaMeasurer::extract_frame`].
    #[must_use]
    pub fn with_frame(mut self, frame: Vec<u8>) -> Self {
        self.frame = frame;
        self
    }

    /// Return this score from [`MediaMeasurer::ssim`].
    #[must_use]
    pub fn with_similarity(mut self, similarity: Similarity) -> Self {
        self.similarity = similarity;
        self
    }

    /// The facts this measurer was configured with.
    #[must_use]
    pub const fn configured_facts(&self) -> &CutFacts {
        &self.facts
    }
}

impl MediaMeasurer for NoMeasurer {
    fn facts(&self, _path: &MediaPath) -> CoreResult<CutFacts> {
        Ok(self.facts.clone())
    }

    fn frame_hashes(
        &self,
        _path: &MediaPath,
        start_frame: i64,
        count: i64,
        _rate: FrameRate,
    ) -> CoreResult<FrameHashes> {
        let mut digests = self.hashes.digests.clone();
        if count >= 0 {
            digests.truncate(count as usize);
        }
        Ok(FrameHashes::new(digests, start_frame))
    }

    fn extract_frame(
        &self,
        _path: &MediaPath,
        _frame: i64,
        _rate: FrameRate,
    ) -> CoreResult<Vec<u8>> {
        Ok(self.frame.clone())
    }

    fn ssim(&self, _a: &[u8], _b: &[u8]) -> CoreResult<Similarity> {
        Ok(self.similarity)
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
            audio_duration: None,
            audio_start_time: None,
            rate: Some(FrameRate::FPS_29_97),
            codec: "h264".to_owned(),
            width: 1920,
            height: 1080,
            size_bytes: 1_048_576,
        }
    }

    #[test]
    fn the_no_measurer_returns_the_facts_it_was_given() {
        let measurer = NoMeasurer::new(facts());
        let reported = measurer
            .facts(&MediaPath::new(r"H:\somewhere\else.mp4"))
            .expect("a double never fails");
        assert_eq!(reported, *measurer.configured_facts());
        assert_eq!(reported.frame_count, 600);
    }

    #[test]
    fn the_no_measurer_trims_hashes_to_the_window_it_was_asked_for() {
        let configured =
            FrameHashes::new(vec!["aa".to_owned(), "bb".to_owned(), "cc".to_owned()], 0);
        let measurer = NoMeasurer::new(facts()).with_hashes(configured);

        let window = measurer
            .frame_hashes(
                &MediaPath::new(r"H:\master.mp4"),
                1_000,
                2,
                FrameRate::FPS_29_97,
            )
            .expect("a double never fails");
        assert_eq!(window.len(), 2);
        assert_eq!(window.first_frame, 1_000);
        assert_eq!(window.digests, ["aa", "bb"]);

        // A count of zero is an empty window, not the whole thing.
        let empty = measurer
            .frame_hashes(&MediaPath::new(r"H:\master.mp4"), 0, 0, FrameRate::FPS_25)
            .expect("a double never fails");
        assert!(empty.is_empty());
    }

    #[test]
    fn the_no_measurer_returns_the_configured_frame_and_score() {
        let measurer = NoMeasurer::new(facts())
            .with_frame(vec![0x89, 0x50, 0x4e, 0x47])
            .with_similarity(Similarity::measured(0.991));

        assert_eq!(
            measurer
                .extract_frame(&MediaPath::new(r"H:\out\clip.mp4"), 0, FrameRate::FPS_25)
                .expect("a double never fails"),
            vec![0x89, 0x50, 0x4e, 0x47]
        );
        assert_eq!(
            measurer
                .ssim(b"left", b"right")
                .expect("a double never fails"),
            Similarity::measured(0.991)
        );
        assert_eq!(
            NoMeasurer::new(facts())
                .ssim(b"left", b"right")
                .expect("a double never fails"),
            Similarity::UNMEASURABLE
        );
    }
}

//! Transcript intelligence: search captions, and cut by text instead of by timecode.
//!
//! This is the feature that changes what the tool is *for*. A three-hour interview arrives
//! with a transcript and a hundred possible moments in it; asking an editor to find the
//! frame number of "the bit where he says the thing about custody" and then type two
//! timecodes is asking them to do the computer's job. With an index, the editor searches,
//! clicks a line, and the in and out points are already frame-exact.
//!
//! ## Why this module is pure
//!
//! V1 retimed captions with a set of string utilities. Here the transcript becomes a
//! *queryable index* with a stable shape, and everything the UI does with it — search,
//! sentence grouping, voice-activity bounds, turning a paragraph into a segment — is a pure
//! function of the cues. Loading from disk and writing back live in [`crate::caption`] and
//! in the application layer, so the search and sentence behaviour can be tested exhaustively
//! without a filesystem, and so it can be reused unchanged when a transcript arrives from a
//! service instead of a file.
//!
//! ## How a search becomes a cut
//!
//! [`TranscriptIndex::hits`] answers "where is this phrase", giving each match in frames at
//! the source's own rate. [`Sentence`] grouping is what makes a hit *usable*: cut points are
//! taken from the boundaries of the group a hit falls in, so a segment starts on the first
//! word of a sentence rather than in the middle of one.
//!
//! The two bounds are deliberately different operations:
//!
//! * [`snap_to_sentence`] grows a range outwards to the sentence it touches. This is the
//!   normal case — an editor clicked somewhere and wants whole sentences.
//! * [`snap_to_silence`] grows a range outwards to the nearest pauses. This is the case that
//!   matters when a cut must not clip a plosive: the gap between cues is where the scissors
//!   belong, and the transcript already knows where the gaps are.

use serde::{Deserialize, Serialize};

use crate::caption::Cue;
use crate::error::{CoreError, CoreResult};
use crate::timecode::FrameRate;

/// How cues are grouped into sentences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Grouping {
    /// Group on a pause at least this long, or a sentence-ending mark. The default, because
    /// it matches how an editor hears a paragraph.
    Sentence,
    /// Group on a pause only, so a long run of speech stays one block.
    Pause,
    /// One group per cue. The raw view, for a transcript whose cueing is already good.
    Cue,
}

/// A run of consecutive cues that belong together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sentence {
    /// Index of the first cue in [`TranscriptIndex::cues`].
    pub first_cue: usize,
    /// Index one past the last cue, so `[first_cue, last_cue)` is the run.
    pub last_cue: usize,
    /// Start in seconds from the beginning of the source.
    pub start_seconds: f64,
    /// End in seconds.
    pub end_seconds: f64,
    /// The text of the run, lines joined with single spaces.
    pub text: String,
}

impl Sentence {
    /// Start in frames, on the source's grid.
    #[must_use]
    pub fn start_frame(&self, rate: FrameRate) -> i64 {
        rate.frames_in(self.start_seconds)
    }

    /// End in frames, exclusive, on the source's grid.
    #[must_use]
    pub fn end_frame(&self, rate: FrameRate) -> i64 {
        rate.frames_in(self.end_seconds)
    }

    /// How long the run lasts.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.end_seconds - self.start_seconds
    }

    /// How many words the run holds.
    #[must_use]
    pub fn word_count(&self) -> usize {
        self.text.split_whitespace().count()
    }
}

/// One search result: where a phrase was found, and enough context to show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    /// Index of the cue the phrase was found in.
    pub cue: usize,
    /// Index of the group that cue belongs to, when grouping is available.
    pub sentence: Option<usize>,
    /// Byte offset of the match inside [`Cue::text`].
    pub byte_offset: usize,
    /// Length of the match in bytes.
    pub byte_len: usize,
    /// The cue's text with the match highlighted by `[[` and `]]`.
    pub highlighted: String,
    /// Where the match starts, in frames on the source's grid.
    pub start_frame: i64,
    /// Where the match starts, in seconds.
    pub start_seconds: f64,
}

/// A search index over a transcript's cues, with the sentences they form.
///
/// Built once and reused for every keystroke in the search box. The index holds a folded copy
/// of each cue so a case-insensitive search does not lower-case the whole transcript on each
/// query — at 3 000 cues and a keystroke every 50 ms, that difference is the difference
/// between an instant search box and a laggy one.
///
/// Deliberately *not* serialisable: the folded copies are a cache, and a cache that crosses a
/// process boundary is a cache that can disagree with the data it was derived from. A project
/// stores cues; the index is rebuilt from them, which takes microseconds.
#[derive(Debug, Clone)]
pub struct TranscriptIndex {
    /// The cues, in ascending order.
    pub cues: Vec<Cue>,
    /// The groups the cues form.
    pub sentences: Vec<Sentence>,
    /// Folded text of each cue with its provenance map, parallel to [`Self::cues`].
    folded: Vec<Folded>,
}

impl TranscriptIndex {
    /// Build an index over cues.
    #[must_use]
    pub fn new(cues: Vec<Cue>, grouping: Grouping) -> Self {
        let sentences = group_sentences(&cues, grouping);
        let folded = cues
            .iter()
            .map(|cue| Folded::new(&cue.one_line()))
            .collect();
        Self {
            cues,
            sentences,
            folded,
        }
    }

    /// How many cues the transcript holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cues.len()
    }

    /// True when the transcript holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    /// The index of the group a cue belongs to.
    #[must_use]
    pub fn sentence_of_cue(&self, cue: usize) -> Option<usize> {
        self.sentences
            .iter()
            .position(|sentence| cue >= sentence.first_cue && cue < sentence.last_cue)
    }

    /// Find a phrase, case-insensitively, across every cue.
    ///
    /// A phrase that spans two cues is *not* matched, and that is deliberate: the two cues
    /// are separated by a pause in the speech, so a match across them would be a match across
    /// a cut point, which is the one thing this feature exists to avoid.
    #[must_use]
    pub fn hits(&self, phrase: &str, rate: FrameRate, limit: usize) -> Vec<Hit> {
        let needle = fold(phrase);
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for (index, cue) in self.cues.iter().enumerate() {
            let haystack = &self.folded[index];
            let mut from = 0usize;
            while let Some(position) = haystack.text[from..].find(&needle) {
                let offset = from + position;
                // Offsets are into the folded text; `span` maps them back to the original so
                // the highlight lands on the right bytes even when the cue holds a double
                // space or a newline that folding collapsed.
                let line = cue.one_line();
                let (start, len) = haystack.span(offset, needle.len(), line.len());
                hits.push(Hit {
                    cue: index,
                    sentence: self.sentence_of_cue(index),
                    byte_offset: start,
                    byte_len: len,
                    highlighted: highlight(&line, start, len),
                    start_frame: rate.frames_in(cue.start),
                    start_seconds: cue.start,
                });
                if hits.len() >= limit {
                    return hits;
                }
                from = offset + needle.len().max(1);
            }
        }
        hits
    }

    /// The total number of matches, when a caller wants a count without the hits.
    #[must_use]
    pub fn count(&self, phrase: &str) -> usize {
        let needle = fold(phrase);
        if needle.is_empty() {
            return 0;
        }
        self.folded
            .iter()
            .map(|haystack| haystack.text.match_indices(&needle).count())
            .sum()
    }

    /// The group a frame falls inside, when one does.
    ///
    /// Used to snap a cut to a boundary: the UI shows the timecode the editor dragged to, and
    /// this answers "which sentence is that".
    #[must_use]
    pub fn sentence_at_frame(&self, frame: i64, rate: FrameRate) -> Option<usize> {
        let seconds = rate.seconds_of(frame);
        self.sentences.iter().position(|sentence| {
            seconds >= sentence.start_seconds && seconds < sentence.end_seconds
        })
    }

    /// Grow a frame range outwards until it covers the whole sentences it touches.
    #[must_use]
    pub fn snap_to_sentence(&self, start_frame: i64, end_frame: i64, rate: FrameRate) -> (i64, i64) {
        let (Some(first), Some(last)) = (
            self.sentence_at_frame(start_frame, rate),
            // The out point is exclusive, so the frame before it is the last kept frame.
            self.sentence_at_frame((end_frame - 1).max(start_frame), rate),
        ) else {
            return (start_frame, end_frame);
        };
        let first_sentence = &self.sentences[first];
        let last_sentence = &self.sentences[last];
        (
            first_sentence.start_frame(rate),
            last_sentence.end_frame(rate).max(first_sentence.end_frame(rate)),
        )
    }

    /// Grow a frame range outwards to the nearest pause, so a cut cannot clip a word.
    ///
    /// The search is bounded: a pause further away than [`max_pause_seconds`] is not a pause,
    /// it is a different part of the conversation, and snapping to it would silently deliver
    /// a segment many seconds longer than the one the editor marked.
    ///
    /// [`max_pause_seconds`]: Self::snap_to_silence
    #[must_use]
    pub fn snap_to_silence(
        &self,
        start_frame: i64,
        end_frame: i64,
        rate: FrameRate,
        max_pause_seconds: f64,
    ) -> (i64, i64) {
        let start_seconds = rate.seconds_of(start_frame);
        let end_seconds = rate.seconds_of(end_frame);

        // The in point moves BACK to the end of the last cue that finishes before it. That is
        // the cue with the largest end still below `start_seconds` — not simply the last cue in
        // the list and not the first one found scanning backwards, because a cue that ended
        // long ago would drag the cut far away from the mark the editor set.
        let mut previous_end: Option<f64> = None;
        for cue in &self.cues {
            if cue.end <= start_seconds {
                previous_end = Some(match previous_end {
                    Some(best) if best >= cue.end => best,
                    _ => cue.end,
                });
            }
        }

        // The out point moves FORWARD to the start of the first cue that begins after it.
        let next_start = self
            .cues
            .iter()
            .find(|cue| cue.start >= end_seconds)
            .map(|cue| cue.start);

        let mut snapped_start = start_seconds;
        if let Some(end) = previous_end {
            if start_seconds - end <= max_pause_seconds {
                snapped_start = end;
            }
        }

        let mut snapped_end = end_seconds;
        if let Some(start) = next_start {
            if start - end_seconds <= max_pause_seconds {
                snapped_end = start;
            }
        }

        // A snap moves the in point *earlier* and the out point *later*, and these two
        // operations are what enforce that. If a snap would move a boundary inward — which is
        // what happens when the requested frame already sits past the boundary it was measured
        // against — the requested frame wins and the range stays as the editor marked it. Without
        // this the "snap" could quietly shorten a cut, and a shortened cut clips a word.
        let start = rate.frames_in(snapped_start).min(start_frame);
        let end = rate.frames_in(snapped_end).max(end_frame);
        (start, end.max(start + 1))
    }

    /// Build the segment range for a search hit, snapped to whole sentences.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Transcript`] when the hit's cue index is out of range.
    pub fn range_for_hit(&self, hit: &Hit, rate: FrameRate) -> CoreResult<(i64, i64)> {
        if hit.cue >= self.cues.len() {
            return Err(CoreError::Transcript(format!(
                "hit refers to cue {}, but the transcript has {}",
                hit.cue,
                self.cues.len()
            )));
        }
        let Some(sentence_index) = hit.sentence.or_else(|| self.sentence_of_cue(hit.cue)) else {
            let cue = &self.cues[hit.cue];
            return Ok((rate.frames_in(cue.start), rate.frames_in(cue.end)));
        };
        let sentence = &self.sentences[sentence_index];
        Ok((sentence.start_frame(rate), sentence.end_frame(rate)))
    }

    /// The text of a whole group, for the reading pane.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Transcript`] when the index is out of range.
    pub fn sentence_text(&self, index: usize) -> CoreResult<&str> {
        self.sentences
            .get(index)
            .map(|sentence| sentence.text.as_str())
            .ok_or_else(|| {
                CoreError::Transcript(format!(
                    "there is no group {index}; the transcript has {}",
                    self.sentences.len()
                ))
            })
    }

    /// The full plain text, one group per line, for a word count or an export.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.sentences
            .iter()
            .map(|sentence| sentence.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// How many words the transcript holds.
    #[must_use]
    pub fn word_count(&self) -> usize {
        self.cues.iter().map(|cue| cue.one_line().split_whitespace().count()).sum()
    }
}

/// Folded text plus a map back to the original, so a match can be highlighted exactly.
///
/// Folding lower-cases and collapses whitespace runs, which means an offset into the folded
/// text is not an offset into the original. Building the map at fold time is both simpler and
/// more accurate than trying to reconstruct it later, and it costs one `usize` per folded
/// character — nothing at the scale of a transcript.
#[derive(Debug, Clone)]
struct Folded {
    /// Lower-cased text with every whitespace run collapsed to one space and trimmed.
    text: String,
    /// For each byte of [`Folded::text`], the byte it came from in the original. Whitespace
    /// runs all point at the first character of the run, and the end is `original.len()`.
    origin: Vec<usize>,
}

impl Folded {
    /// Fold a line.
    fn new(original: &str) -> Self {
        let mut text = String::with_capacity(original.len());
        let mut origin = Vec::with_capacity(original.len());
        let mut in_space = false;

        // Lower-casing can change a character's UTF-8 length, so the lower-cased form is
        // walked independently of the original; `last_origin` tracks where the character
        // being appended came from.
        for (byte_index, ch) in original.char_indices() {
            if ch.is_whitespace() {
                if !in_space && !text.is_empty() {
                    text.push(' ');
                    origin.push(byte_index);
                    in_space = true;
                }
                continue;
            }
            in_space = false;
            for lowered in ch.to_lowercase() {
                let before = text.len();
                text.push(lowered);
                for _ in before..text.len() {
                    origin.push(byte_index);
                }
            }
        }
        Self { text, origin }
    }

    /// The byte span in the original that a span in the folded text corresponds to.
    ///
    /// Both the start and the end of the match are looked up in the provenance map, so the span
    /// covers whole characters even where lower-casing changed a character's byte length. A
    /// match that ends exactly on a whitespace run resolves to the start of the run, which is
    /// the end of the last matched word — correct, because the run itself is not part of the
    /// match.
    fn span(&self, folded_offset: usize, folded_len: usize, original_len: usize) -> (usize, usize) {
        let start = self
            .origin
            .get(folded_offset)
            .copied()
            .unwrap_or(original_len);
        let end_folded = (folded_offset + folded_len).min(self.origin.len());
        let end = self.origin.get(end_folded).copied().unwrap_or(original_len);
        let end = end.max(start);
        (start, end - start)
    }
}

/// Fold text for searching, discarding the provenance map.
fn fold(text: &str) -> String {
    Folded::new(text).text
}

/// Wrap the matched span in `[[` and `]]`.
fn highlight(line: &str, offset: usize, len: usize) -> String {
    let end = (offset + len).min(line.len());
    if offset >= line.len() {
        return line.to_owned();
    }
    format!("{}[[{}]]{}", &line[..offset], &line[offset..end], &line[end..])
}

/// Group cues into sentences.
///
/// A group ends when the cue's text ends on a sentence mark, or when the pause before the
/// next cue is at least [`SENTENCE_PAUSE_SECONDS`]. Both conditions are needed: transcripts
/// from automatic tools often omit punctuation entirely, and transcripts from human captioners
/// often run several sentences together in one cue.
#[must_use]
pub fn group_sentences(cues: &[Cue], grouping: Grouping) -> Vec<Sentence> {
    if cues.is_empty() {
        return Vec::new();
    }
    match grouping {
        Grouping::Cue => cues
            .iter()
            .enumerate()
            .map(|(index, cue)| Sentence {
                first_cue: index,
                last_cue: index + 1,
                start_seconds: cue.start,
                end_seconds: cue.end,
                text: cue.one_line(),
            })
            .collect(),
        Grouping::Sentence | Grouping::Pause => {
            let break_on_punctuation = grouping == Grouping::Sentence;
            let mut sentences = Vec::new();
            let mut first = 0usize;
            for index in 0..cues.len() {
                let is_last = index + 1 == cues.len();
                let next_gap = if is_last {
                    0.0
                } else {
                    (cues[index + 1].start - cues[index].end).max(0.0)
                };
                let ends_sentence = break_on_punctuation && ends_on_mark(&cues[index].one_line());
                let pause = next_gap >= SENTENCE_PAUSE_SECONDS;
                if is_last || ends_sentence || pause {
                    let run = &cues[first..=index];
                    sentences.push(Sentence {
                        first_cue: first,
                        last_cue: index + 1,
                        start_seconds: run[0].start,
                        end_seconds: run[run.len() - 1].end,
                        text: run
                            .iter()
                            .map(crate::caption::Cue::one_line)
                            .collect::<Vec<_>>()
                            .join(" "),
                    });
                    first = index + 1;
                }
            }
            sentences
        }
    }
}

/// A pause at least this long ends a sentence when cues carry no punctuation.
pub const SENTENCE_PAUSE_SECONDS: f64 = 0.45;

/// True when a line ends on a mark that closes a sentence.
fn ends_on_mark(line: &str) -> bool {
    line.trim_end()
        .chars()
        .next_back()
        .is_some_and(|last| matches!(last, '.' | '!' | '?' | '…'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cues() -> Vec<Cue> {
        vec![
            Cue::new(0.0, 2.0, "So the custody question is"),
            Cue::new(2.1, 4.0, "the thing nobody wants to answer."),
            Cue::new(4.1, 5.0, "Right."),
            Cue::new(7.0, 9.5, "And that is where the market"),
            Cue::new(9.5, 11.0, "disagrees with the SEC."),
            Cue::new(11.0, 13.0, "Every single time."),
        ]
    }

    fn rate() -> FrameRate {
        FrameRate::FPS_29_97
    }

    #[test]
    fn a_phrase_is_found_case_insensitively_with_its_frame() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        let hits = index.hits("custody", rate(), 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].cue, 0);
        assert_eq!(hits[0].start_frame, 0);
        assert!(hits[0].highlighted.contains("[[custody]]"));

        let upper = index.hits("CUSTODY", rate(), 10);
        assert_eq!(upper.len(), 1);

        assert_eq!(index.count("the"), 4);
        assert!(index.hits("", rate(), 10).is_empty());
        assert!(index.hits("nothing at all here", rate(), 10).is_empty());
    }

    #[test]
    fn a_phrase_is_not_matched_across_a_pause_between_cues() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        // "answer Right" spans two cues, separated by a 100 ms gap. Not a match.
        assert!(index.hits("answer right", rate(), 10).is_empty());
        // Two cues whose texts touch across the boundary are still not a match: the cut point
        // between them is exactly what this feature exists to respect.
        assert!(index.hits("is the thing nobody", rate(), 10).is_empty());
        // "is the" sits inside cue 0 (text: "So the custody question is"), so it is not there;
        // "the thing" spans cue 0 and cue 1 and must not match either.
        assert_eq!(index.hits("custody question", rate(), 10).len(), 1);
        assert_eq!(index.hits("nobody wants to answer", rate(), 10).len(), 1);
    }

    #[test]
    fn every_occurrence_is_reported_not_just_the_first() {
        let index = TranscriptIndex::new(
            vec![
                Cue::new(0.0, 1.0, "time and time again"),
                Cue::new(1.0, 2.0, "time"),
            ],
            Grouping::Cue,
        );
        let hits = index.hits("time", rate(), 10);
        assert_eq!(hits.len(), 3);
        assert_eq!(index.count("time"), 3);
        // The limit is honoured.
        assert_eq!(index.hits("time", rate(), 2).len(), 2);
    }

    #[test]
    fn grouping_breaks_on_punctuation_and_on_a_long_pause() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        // Cue 1 ends with '.', cue 2 ends with '.', cue 4 ends with '.', cue 5 ends with '.'
        // and cue 3 is followed by a 2-second pause.
        let texts: Vec<&str> = index.sentences.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts[0], "So the custody question is the thing nobody wants to answer.");
        assert_eq!(texts[1], "Right.");
        assert_eq!(texts[2], "And that is where the market disagrees with the SEC.");
        assert_eq!(texts[3], "Every single time.");
    }

    #[test]
    fn pause_grouping_keeps_unpunctuated_speech_together() {
        let unpunctuated = vec![
            Cue::new(0.0, 2.0, "so there is no punctuation here"),
            Cue::new(2.05, 4.0, "and it just keeps going"),
            Cue::new(6.0, 8.0, "until eventually there is a gap"),
        ];
        let sentence = TranscriptIndex::new(unpunctuated.clone(), Grouping::Sentence);
        assert_eq!(sentence.sentences.len(), 2);
        assert_eq!(sentence.sentences[0].word_count(), 11);

        let pause = TranscriptIndex::new(unpunctuated, Grouping::Pause);
        assert_eq!(pause.sentences.len(), 2);
    }

    #[test]
    fn cue_grouping_gives_one_group_per_cue() {
        let index = TranscriptIndex::new(cues(), Grouping::Cue);
        assert_eq!(index.sentences.len(), 6);
        assert_eq!(index.sentences[0].text, "So the custody question is");
        assert_eq!(index.word_count(), 25);
    }

    #[test]
    fn a_hit_becomes_a_range_covering_its_whole_sentence() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        let hits = index.hits("SEC", rate(), 1);
        assert_eq!(hits.len(), 1);
        let (start, end) = index.range_for_hit(&hits[0], rate()).expect("range");
        // The third sentence runs 7.0 s to 11.0 s.
        assert_eq!(start, rate().frames_in(7.0));
        assert_eq!(end, rate().frames_in(11.0));
        assert!(end > start);
    }

    #[test]
    fn snapping_to_a_sentence_grows_a_range_outwards() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        // A range sitting in the middle of cue 0's sentence.
        let (start, end) = index.snap_to_sentence(15, 45, rate());
        assert_eq!(start, 0);
        assert_eq!(end, rate().frames_in(4.0));
    }

    #[test]
    fn snapping_to_silence_lands_in_the_gaps_and_respects_the_limit() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        // A range inside cue 0 (0.6 s to 1.0 s): the nearest pause either side is at 2.0/2.1.
        // The range sits inside cue 0 (0.0–2.0 s), so the only boundary in range is the pause
        // after cue 0, at 2.1 s: the out point snaps forward to it and the in point has no
        // earlier pause to reach, so it stays where the editor put it.
        // The range ends at frame 30, which is 1.001 s — 1.099 s short of the pause at 2.1 s.
        // A 1.2 s limit reaches it; a 1.0 s limit does not, and the range is left alone.
        let (start, end) = index.snap_to_silence(18, 30, rate(), 1.2);
        assert_eq!(start, 18);
        assert_eq!(end, rate().frames_in(2.1), "the out point should snap to the pause at 2.1 s");

        // With a limit too small to reach any gap, the range is left alone. Frames 18 and 30
        // are both inside cue 0 (0.0–2.0 s), so they are the requested range unchanged.
        let (start, end) = index.snap_to_silence(18, 30, rate(), 0.0001);
        assert_eq!((start, end), (18, 30));

        // With a limit large enough to reach the big gap, the out point lands on it.
        // 130 frames is 4.338 s, which sits inside cue 1 (2.1–4.0 s). The next cue starts at
        // 7.0 s, 2.66 s away and inside the 3 s limit, so the out point snaps to it.
        let (_, end) = index.snap_to_silence(18, 130, rate(), 3.0);
        assert_eq!(end, rate().frames_in(7.0));
    }

    #[test]
    fn a_snap_never_collapses_a_range_to_nothing() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        let (start, end) = index.snap_to_silence(60, 61, rate(), 10.0);
        assert!(end > start, "snap produced an empty range: {start}..{end}");
    }

    #[test]
    fn out_of_range_lookups_are_refused_with_the_numbers() {
        let index = TranscriptIndex::new(cues(), Grouping::Cue);
        let error = index.sentence_text(99).expect_err("refused");
        match error {
            CoreError::Transcript(message) => {
                assert!(message.contains("99"), "{message}");
                assert!(message.contains('6'), "{message}");
            }
            other => panic!("wrong error: {other:?}"),
        }

        let hit = Hit {
            cue: 99,
            sentence: None,
            byte_offset: 0,
            byte_len: 1,
            highlighted: String::new(),
            start_frame: 0,
            start_seconds: 0.0,
        };
        assert!(index.range_for_hit(&hit, rate()).is_err());
    }

    #[test]
    fn sentence_of_cue_and_sentence_at_frame_agree() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        assert_eq!(index.sentence_of_cue(0), Some(0));
        assert_eq!(index.sentence_of_cue(1), Some(0));
        assert_eq!(index.sentence_of_cue(2), Some(1));
        assert_eq!(index.sentence_of_cue(5), Some(3));
        // 0.5 s is inside cue 0, which is sentence 0.
        assert_eq!(index.sentence_at_frame(rate().frames_in(0.5), rate()), Some(0));
        // A frame far past the end belongs to no group.
        assert_eq!(index.sentence_at_frame(rate().frames_in(60.0), rate()), None);
    }

    #[test]
    fn an_empty_transcript_is_handled_without_panicking() {
        let index = TranscriptIndex::new(Vec::new(), Grouping::Sentence);
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        assert!(index.sentences.is_empty());
        assert_eq!(index.word_count(), 0);
        assert!(index.hits("anything", rate(), 5).is_empty());
        assert_eq!(index.count("anything"), 0);
        assert_eq!(index.plain_text(), "");
        assert_eq!(index.snap_to_sentence(10, 20, rate()), (10, 20));
        assert_eq!(index.sentence_at_frame(0, rate()), None);
    }

    #[test]
    fn plain_text_reads_as_the_transcript() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        let text = index.plain_text();
        assert!(text.starts_with("So the custody question is"));
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn folding_collapses_whitespace_so_a_double_space_still_matches() {
        let index = TranscriptIndex::new(
            vec![Cue::new(0.0, 1.0, "hello   there\nworld")],
            Grouping::Cue,
        );
        assert_eq!(index.hits("hello there world", rate(), 5).len(), 1);
        let hit = &index.hits("there", rate(), 5)[0];
        // The offsets index the *original* cue text, not the highlighted rendering, which has
        // had two characters of marker inserted. Asserting against the right string is the
        // point: slicing the highlighted text at an offset into the original proves nothing,
        // and an earlier version of this line did exactly that.
        let line = index.cues[hit.cue].one_line();
        assert_eq!(&line[hit.byte_offset..hit.byte_offset + hit.byte_len], "there");
        assert_eq!(hit.highlighted, "hello   [[there]] world");
        // The match starts after the collapsed whitespace run, not inside it.
        assert_eq!(hit.byte_offset, 8);
    }

    #[test]
    fn sentence_frames_land_on_the_source_grid() {
        let index = TranscriptIndex::new(cues(), Grouping::Sentence);
        let sentence = &index.sentences[2];
        assert_eq!(sentence.start_frame(rate()), rate().frames_in(7.0));
        assert_eq!(sentence.end_frame(rate()), rate().frames_in(11.0));
        assert!((sentence.duration() - 4.0).abs() < 1e-9);
    }
}

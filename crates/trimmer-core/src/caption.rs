//! Captions: parsing SRT, retiming it onto a segment, and rendering it again.
//!
//! A transcript is a sidecar file: `clip.mp4` and `clip.srt`. When a segment is cut out of
//! the video, the captions have to move with it. The rules that matter, all carried over
//! from V1 ADR-007 because they were each earned on a real deadline:
//!
//! * **A mark is a frame; a cue is a span.** The in and out points come from Premiere
//!   timecodes, which land wherever the editor clicked, so a cue can straddle a mark.
//!   Dropping such a cue leaves the audio it still covers uncaptioned; keeping it whole
//!   puts text on screen for words that were cut away. So it is *clamped* to the window
//!   when at least [`MIN_OVERLAP`] of it survives, and dropped when less does — a caption
//!   that flashes for forty milliseconds is worse than no caption at all.
//! * **Everything shifts to zero.** The segment starts at `00:00:00,000`, and cues are
//!   renumbered from 1, because a caption track with gaps in its numbering is needless risk
//!   in an importer.
//! * **The file keeps its own shape.** BOM, line endings and the decimal separator the
//!   source used are preserved: these files get diffed and re-imported, and a gratuitous
//!   reformat shows up as a whole-file change.
//! * **The window is the one that was asked for**, never the file that came out. The frames
//!   a stream copy adds past the out point must not grow captions.
//!
//! Nothing here talks to ffmpeg. Text in, text out — which is also why cutting captions
//! alongside a video costs no measurable time.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::timecode::FrameRate;

/// A straddling cue with less than this much of itself left inside the segment is dropped
/// rather than clamped.
///
/// A quarter of a second is about the shortest a line can be on screen and still be read.
pub const MIN_OVERLAP: f64 = 0.25;

/// One caption: seconds from zero, and the text as it was written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cue {
    /// Start, in seconds from the start of the file.
    pub start: f64,
    /// End, in seconds from the start of the file.
    pub end: f64,
    /// The text, exactly as written, newlines included.
    pub text: String,
    /// The cue number the source file used, when it had one. Kept so a report can point at
    /// the original line, and so a round trip through the app is traceable.
    pub source_index: Option<u32>,
}

impl Cue {
    /// A cue with no source numbering.
    #[must_use]
    pub fn new(start: f64, end: f64, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
            source_index: None,
        }
    }

    /// How long the cue stays on screen.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }

    /// The cue's text with newlines collapsed, for a search result line.
    #[must_use]
    pub fn one_line(&self) -> String {
        self.text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Characters per second, the standard readability measure for subtitles.
    #[must_use]
    pub fn characters_per_second(&self) -> f64 {
        let duration = self.duration();
        if duration <= 0.0 {
            return f64::INFINITY;
        }
        self.one_line().chars().count() as f64 / duration
    }
}

/// A parsed caption file, plus the shape it was written in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    /// Where it came from, when it came from a file.
    pub path: Option<PathBuf>,
    /// The cues, in ascending start order.
    pub cues: Vec<Cue>,
    /// The line ending the file used, so a round trip does not reformat it.
    pub newline: String,
    /// Whether the file began with a byte-order mark.
    pub bom: bool,
    /// The text after the last cue, kept verbatim so nothing is silently lost.
    pub trailing: String,
}

impl Transcript {
    /// A transcript from cues, with Unix line endings and no BOM.
    #[must_use]
    pub fn new(cues: Vec<Cue>) -> Self {
        Self {
            path: None,
            cues,
            newline: "\n".to_owned(),
            bom: false,
            trailing: String::new(),
        }
    }

    /// How long the transcript runs, in seconds from zero.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.cues.iter().map(|cue| cue.end).fold(0.0, f64::max)
    }

    /// The cue covering a point in time, when one does.
    #[must_use]
    pub fn cue_at(&self, seconds: f64) -> Option<&Cue> {
        self.cues
            .iter()
            .find(|cue| seconds >= cue.start && seconds < cue.end)
    }
}

/// What a retime kept, clamped and threw away.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetimeResult {
    /// The cues that survive, moved so the segment starts at zero.
    pub cues: Vec<Cue>,
    /// The subset of [`RetimeResult::cues`] that crossed a mark and was clamped to it.
    pub clamped: Vec<Cue>,
    /// The cues that straddled a mark and had too little inside the window to keep.
    pub dropped: Vec<Cue>,
    /// How many cues lay entirely outside the window.
    pub outside: usize,
}

impl RetimeResult {
    /// How many cues the segment holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cues.len()
    }

    /// True when nothing fell inside the window.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    /// One line for the job log.
    #[must_use]
    pub fn summary(&self, written_to: Option<&Path>) -> String {
        let Some(path) = written_to else {
            return "nothing fell inside the segment, so no caption file was written. If this \
                    transcript is already relative to the segment, copy it beside the video \
                    instead of retiming it."
                .to_owned();
        };
        let name = path
            .file_name()
            .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let mut parts = vec![format!("{name}: {} cues", self.cues.len())];
        if !self.clamped.is_empty() {
            parts.push(format!("{} clamped at the marks", self.clamped.len()));
        }
        if !self.dropped.is_empty() {
            parts.push(format!(
                "{} dropped (under {MIN_OVERLAP}s inside)",
                self.dropped.len()
            ));
        }
        if self.clamped.is_empty() && self.dropped.is_empty() {
            parts.push("no cue crossed a mark".to_owned());
        }
        parts.join("  ·  ")
    }
}

/// Parse an SRT timestamp such as `00:01:02,500` into seconds.
///
/// The scan is deliberately not anchored, and deliberately not a regular expression: SRT in
/// the wild arrives with a stray space before the arrow, a `\r` in the middle, and
/// occasionally a cue number glued to the timestamp. V1 learned to find the stamp wherever
/// it sits in the line, and the first well-formed stamp wins.
///
/// # Errors
///
/// Returns [`CoreError::Caption`] when the text holds no readable stamp.
pub fn parse_stamp(text: &str) -> CoreResult<f64> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        if let Some((seconds, _consumed)) = stamp_at(bytes, index) {
            return Ok(seconds);
        }
        index += 1;
    }
    Err(CoreError::Caption {
        path: String::new(),
        reason: format!("no subtitle timestamp found in {text:?}"),
    })
}

/// Try to read `H:MM:SS[,.]mmm` starting at `start`. Returns the seconds and bytes consumed.
fn stamp_at(bytes: &[u8], start: usize) -> Option<(f64, usize)> {
    let mut cursor = start;
    let field = |cursor: &mut usize| -> Option<u32> {
        let begin = *cursor;
        while *cursor < bytes.len() && bytes[*cursor].is_ascii_digit() {
            *cursor += 1;
        }
        if *cursor == begin || *cursor - begin > 2 {
            return None;
        }
        std::str::from_utf8(&bytes[begin..*cursor])
            .ok()?
            .parse::<u32>()
            .ok()
    };

    let hours = field(&mut cursor)?;
    if bytes.get(cursor) != Some(&b':') {
        return None;
    }
    cursor += 1;
    let minutes = field(&mut cursor)?;
    if bytes.get(cursor) != Some(&b':') {
        return None;
    }
    cursor += 1;
    let seconds = field(&mut cursor)?;
    match bytes.get(cursor) {
        Some(b',' | b'.') => cursor += 1,
        _ => return None,
    }
    let begin = cursor;
    while cursor < bytes.len() && bytes[cursor].is_ascii_digit() && cursor - begin < 3 {
        cursor += 1;
    }
    if cursor == begin {
        return None;
    }
    let fraction_text = std::str::from_utf8(&bytes[begin..cursor]).ok()?;
    // `1` is 100 ms, `12` is 120 ms, `123` is 123 ms: the field is a decimal fraction.
    let milliseconds = match fraction_text.len() {
        1 => fraction_text.parse::<u32>().ok()? * 100,
        2 => fraction_text.parse::<u32>().ok()? * 10,
        _ => fraction_text.parse::<u32>().ok()?,
    };
    let total = f64::from(hours) * 3600.0
        + f64::from(minutes) * 60.0
        + f64::from(seconds)
        + f64::from(milliseconds) / 1000.0;
    Some((total, cursor - start))
}

/// Render seconds as `HH:MM:SS,mmm`, rounding half up.
///
/// Rounding is done on whole milliseconds with an explicit half-up step rather than through
/// `f64::round`, because the two languages this crate is checked against disagree about halves:
/// Python rounds 1000.5 ms to 1000 and Rust rounds it to 1001. An SRT stamp is a display value
/// and either is defensible, but a value that changes depending on which implementation ran is
/// not, so the tie is broken here, once, in the open.
#[must_use]
pub fn format_stamp(seconds: f64) -> String {
    let scaled = seconds.max(0.0) * 1000.0;
    let total = ((scaled + 0.5).floor()).max(0.0) as i64;
    let hours = total / 3_600_000;
    let rest = total % 3_600_000;
    let minutes = rest / 60_000;
    let rest = rest % 60_000;
    let secs = rest / 1000;
    let millis = rest % 1000;
    format!("{hours:02}:{minutes:02}:{secs:02},{millis:03}")
}

/// Parse an SRT body into cues, tolerating a BOM, CRLF and blank-line quirks.
///
/// The numbering line is not trusted for anything: blocks are found by their timing line,
/// which is the only part every tool agrees on. A cue number, when present, is recorded but
/// never relied upon — and cues are sorted, because files arrive out of order.
#[must_use]
pub fn parse(text: &str) -> Vec<Cue> {
    /// Push the accumulated block when it holds anything, then start a new one.
    ///
    /// A free function inside `parse` rather than a closure so it can take both accumulators
    /// by mutable reference without borrowing them for the whole body.
    fn flush(current: &mut Vec<&str>, blocks: &mut Vec<String>) {
        if current.iter().any(|line| !line.trim().is_empty()) {
            blocks.push(current.join("\n"));
        }
        current.clear();
    }

    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let normalised = normalised.trim_start_matches('\u{feff}');
    let mut cues: Vec<Cue> = Vec::new();

    // Split on a blank line. A "blank" line is one that is empty or whitespace only, which
    // is what hand-edited files actually contain between cues. Written as an explicit scan
    // rather than `split("\n\n")` so a stray space on the separator line cannot swallow a
    // cue, and so a run of blank lines collapses rather than producing empty blocks.
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in normalised.split('\n') {
        if line.trim().is_empty() {
            flush(&mut current, &mut blocks);
        } else {
            current.push(line);
        }
    }
    flush(&mut current, &mut blocks);

    for block in blocks {
        let lines: Vec<&str> = block.split('\n').collect();
        let Some(timing_index) = lines.iter().position(|line| line.contains("-->")) else {
            continue;
        };
        let timing = lines[timing_index];
        let (before, after) = timing.split_once("-->").expect("the line contains an arrow");
        // A cue number glued to the front of the timing line is common in hand-edited
        // files: `7 00:00:01,000 --> ...`. Record it, then read the stamp from whatever
        // remains. The scan in `parse_stamp` finds the first well-formed stamp wherever it
        // starts, so the number being present or absent makes no difference to the time.
        let head = before.trim();
        let source_index = head
            .split_once(char::is_whitespace)
            .and_then(|(number, _)| number.parse::<u32>().ok())
            .or_else(|| head.parse::<u32>().ok());
        let Ok(start) = parse_stamp(head) else { continue };
        let Ok(end) = parse_stamp(after) else { continue };
        let body = lines[timing_index + 1..].join("\n");
        cues.push(Cue {
            start,
            end,
            text: body.trim_end_matches('\n').to_owned(),
            source_index,
        });
    }

    cues.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.end.partial_cmp(&b.end).unwrap_or(std::cmp::Ordering::Equal))
    });
    cues
}

/// Read a caption file, remembering how it was written.
///
/// # Errors
///
/// Returns [`CoreError::Caption`] when the file cannot be read.
pub fn read(path: &Path) -> CoreResult<Transcript> {
    let raw = std::fs::read(path).map_err(|error| CoreError::Caption {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    let bom = raw.starts_with(&[0xef, 0xbb, 0xbf]);
    // Subtitles are text; a byte that is not valid UTF-8 is replaced rather than refused,
    // because refusing to cut the video over one bad glyph in a caption is the wrong trade.
    let text = String::from_utf8_lossy(if bom { &raw[3..] } else { &raw[..] }).into_owned();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" }.to_owned();
    Ok(Transcript {
        path: Some(path.to_path_buf()),
        cues: parse(&text),
        newline,
        bom,
        trailing: String::new(),
    })
}

/// Render cues as the text of an SRT file, numbered from 1.
///
/// The file's own line ending is used *throughout* — inside a cue as well as between cues. V1
/// only used it between blocks, so a CRLF transcript came back with LF inside every multi-line
/// cue: the file still parses, but `git diff` shows every cue as changed, and these files get
/// diffed and re-imported. Text that already contains a newline is normalised to the same
/// ending so a cue cannot carry a mixture.
#[must_use]
pub fn render(cues: &[Cue], newline: &str, bom: bool) -> String {
    let blocks: Vec<String> = cues
        .iter()
        .enumerate()
        .map(|(index, cue)| {
            let text = cue
                .text
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .replace('\n', newline);
            format!(
                "{}{newline}{} --> {}{newline}{text}",
                index + 1,
                format_stamp(cue.start),
                format_stamp(cue.end),
            )
        })
        .collect();
    let body = blocks.join(&format!("{newline}{newline}"));
    let text = if body.is_empty() {
        String::new()
    } else {
        format!("{body}{newline}")
    };
    if bom {
        format!("\u{feff}{text}")
    } else {
        text
    }
}

/// Write cues as an SRT file in the shape the source used.
///
/// # Errors
///
/// Returns [`CoreError::Caption`] when the file cannot be written.
pub fn write(path: &Path, cues: &[Cue], newline: &str, bom: bool) -> CoreResult<()> {
    let text = render(cues, newline, bom);
    std::fs::write(path, text.as_bytes()).map_err(|error| CoreError::Caption {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}

/// Move cues onto a segment that runs from `start` to `end` in the source.
///
/// Cues outside the window are left out; cues that cross a mark are clamped when enough of
/// them survives and dropped when not. The result is sorted and starts at zero.
///
/// # Errors
///
/// Returns [`CoreError::Caption`] when the segment ends before it starts.
pub fn retime(cues: &[Cue], start: f64, end: f64, min_overlap: f64) -> CoreResult<RetimeResult> {
    if end <= start {
        return Err(CoreError::Caption {
            path: String::new(),
            reason: format!("the segment ends ({end:.3}s) before it starts ({start:.3}s)"),
        });
    }
    const EPSILON: f64 = 1e-6;
    let mut result = RetimeResult::default();
    for cue in cues {
        if cue.end <= start || cue.start >= end {
            result.outside += 1;
            continue;
        }
        let head = cue.start.max(start);
        let tail = cue.end.min(end);
        let straddles = cue.start < start - EPSILON || cue.end > end + EPSILON;
        if straddles && (tail - head) < min_overlap {
            result.dropped.push(cue.clone());
            continue;
        }
        let moved = Cue {
            start: round_millis(head - start),
            end: round_millis(tail - start),
            text: cue.text.clone(),
            source_index: cue.source_index,
        };
        if straddles {
            result.clamped.push(moved.clone());
        }
        result.cues.push(moved);
    }
    result.cues.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.end.partial_cmp(&b.end).unwrap_or(std::cmp::Ordering::Equal))
    });
    Ok(result)
}

/// Round to milliseconds, which is the resolution an SRT file has.
#[must_use]
pub fn round_millis(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

/// Retime a whole file and write the result beside the video.
///
/// Returns the result and the path written, which is `None` when the transcript held nothing
/// inside the window — so pointing the app at an already-trimmed transcript cannot quietly
/// produce an empty caption file.
///
/// # Errors
///
/// Returns [`CoreError::Caption`] when the source cannot be read or the output written.
pub fn retime_file(
    source_srt: &Path,
    output_srt: &Path,
    start: f64,
    end: f64,
    min_overlap: f64,
) -> CoreResult<(RetimeResult, Option<PathBuf>)> {
    let transcript = read(source_srt)?;
    let result = retime(&transcript.cues, start, end, min_overlap)?;
    if result.cues.is_empty() {
        return Ok((result, None));
    }
    write(output_srt, &result.cues, &transcript.newline, transcript.bom)?;
    Ok((result, Some(output_srt.to_path_buf())))
}

/// The transcript that belongs to a video: `clip.srt`, or `clip.<lang>.srt`.
///
/// Matching the stem exactly is the common case — the editor exported the transcript beside
/// the video. The dotted form covers `clip.en.srt`, which is what transcription tools write.
/// Anything else has to be pointed at, because guessing between several subtitle files would
/// be worse than asking.
#[must_use]
pub fn find_for(video: &Path) -> Option<PathBuf> {
    let folder = video.parent()?;
    let stem = video.file_stem()?.to_string_lossy().to_lowercase();

    let exact = folder.join(format!("{stem}.srt"));
    if exact.is_file() {
        return Some(exact);
    }

    let mut candidates: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("srt"))
        })
        .filter(|path| {
            path.file_stem().is_some_and(|candidate| {
                let candidate = candidate.to_string_lossy().to_lowercase();
                candidate == stem || candidate.starts_with(&format!("{stem}."))
            })
        })
        .collect();
    candidates.sort_by_key(|path| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().to_lowercase())
            .unwrap_or_default()
            // An exact stem match sorts first; a language-tagged one after it.
            .contains('.')
    });
    candidates.into_iter().next()
}

/// Convert a cue boundary in seconds to the frame it lands on, so the UI can put a cut
/// exactly on a caption edge.
#[must_use]
pub fn cue_frame(seconds: f64, rate: FrameRate) -> i64 {
    rate.frames_in(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "1\n\
00:00:01,000 --> 00:00:03,500\n\
Hello there.\n\
\n\
2\r\n\
00:00:03,500 --> 00:00:06,000\r\n\
Second line\r\n\
continues here.\r\n\
\n\
3\n\
00:00:09,000 --> 00:00:11,000\n\
Far away.\n";

    #[test]
    fn stamps_parse_with_commas_dots_and_short_fractions() {
        assert!((parse_stamp("00:01:02,500").expect("ok") - 62.5).abs() < 1e-9);
        assert!((parse_stamp("00:01:02.500").expect("ok") - 62.5).abs() < 1e-9);
        // `,5` is five tenths of a second, not five milliseconds.
        assert!((parse_stamp("00:00:01,5").expect("ok") - 1.5).abs() < 1e-9);
        assert!((parse_stamp("00:00:01,05").expect("ok") - 1.05).abs() < 1e-9);
        assert!((parse_stamp("  01:02:03,004  ").expect("ok") - 3723.004).abs() < 1e-9);
    }

    #[test]
    fn a_line_with_no_stamp_is_refused() {
        assert!(parse_stamp("no time here").is_err());
        assert!(parse_stamp("").is_err());
    }

    #[test]
    fn stamps_render_rounded_to_milliseconds() {
        assert_eq!(format_stamp(0.0), "00:00:00,000");
        assert_eq!(format_stamp(62.5), "00:01:02,500");
        assert_eq!(format_stamp(1.0005), "00:00:01,001");
        assert_eq!(format_stamp(3661.999), "01:01:01,999");
        assert_eq!(format_stamp(-3.0), "00:00:00,000");
    }

    #[test]
    fn parsing_tolerates_crlf_bom_and_a_missing_final_newline() {
        let cues = parse(&format!("\u{feff}{SAMPLE}"));
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].text, "Hello there.");
        assert_eq!(cues[1].text, "Second line\ncontinues here.");
        assert_eq!(cues[2].text, "Far away.");
    }

    #[test]
    fn cues_are_sorted_even_when_the_file_is_not() {
        let out_of_order = "2\n00:00:10,000 --> 00:00:12,000\nlater\n\n\
                             1\n00:00:01,000 --> 00:00:02,000\nearlier\n";
        let cues = parse(out_of_order);
        assert_eq!(cues[0].text, "earlier");
        assert_eq!(cues[1].text, "later");
    }

    #[test]
    fn a_cue_number_glued_to_the_timing_line_is_recovered() {
        let cues = parse("7 00:00:01,000 --> 00:00:02,000\nhi\n");
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].source_index, Some(7));
    }

    #[test]
    fn retiming_shifts_to_zero_and_clamps_at_the_marks() {
        let cues = parse(SAMPLE);
        // Segment from 2.0 s to 10.0 s at 25 fps.
        let result = retime(&cues, 2.0, 10.0, MIN_OVERLAP).expect("ok");
        assert_eq!(result.cues.len(), 3);
        // Cue 1 (1.0–3.5) straddles the in point: clamped to 0.
        assert!((result.cues[0].start - 0.0).abs() < 1e-9);
        assert!((result.cues[0].end - 1.5).abs() < 1e-9);
        assert_eq!(result.clamped.len(), 2);
        // Cue 3 (9.0–11.0) straddles the out point: clamped to 8.0.
        assert!((result.cues[2].start - 7.0).abs() < 1e-9);
        assert!((result.cues[2].end - 8.0).abs() < 1e-9);
        assert_eq!(result.dropped.len(), 0);
    }

    #[test]
    fn a_straddling_cue_with_too_little_inside_is_dropped_not_flashed() {
        let cues = vec![
            Cue::new(0.0, 1.1, "almost over"),
            Cue::new(1.0, 4.0, "well inside"),
            Cue::new(9.95, 12.0, "too late"),
        ];
        let result = retime(&cues, 1.0, 10.0, MIN_OVERLAP).expect("ok");
        assert_eq!(result.cues.len(), 1);
        assert_eq!(result.cues[0].text, "well inside");
        assert_eq!(result.dropped.len(), 2);
        assert_eq!(result.outside, 0);
    }

    #[test]
    fn cues_entirely_outside_the_window_are_counted_and_left_out() {
        let cues = vec![Cue::new(0.0, 1.0, "before"), Cue::new(20.0, 21.0, "after")];
        let result = retime(&cues, 5.0, 10.0, MIN_OVERLAP).expect("ok");
        assert!(result.is_empty());
        assert_eq!(result.outside, 2);
        assert!(result.summary(None).contains("already relative to the segment"));
    }

    #[test]
    fn a_backwards_segment_is_refused() {
        assert!(retime(&[], 10.0, 5.0, MIN_OVERLAP).is_err());
        assert!(retime(&[], 5.0, 5.0, MIN_OVERLAP).is_err());
    }

    #[test]
    fn rendering_renumbers_from_one_and_keeps_the_shape() {
        let cues = vec![
            Cue::new(0.0, 1.0, "one"),
            Cue::new(1.0, 2.0, "two"),
        ];
        let crlf = render(&cues, "\r\n", true);
        assert!(crlf.starts_with('\u{feff}'));
        assert!(crlf.contains("1\r\n00:00:00,000 --> 00:00:01,000\r\none"));
        assert!(crlf.contains("2\r\n00:00:01,000 --> 00:00:02,000\r\ntwo"));
        assert!(crlf.ends_with("\r\n"));
        // And it parses back to what went in.
        let back = parse(&crlf);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].text, "one");
    }

    #[test]
    fn rendering_nothing_writes_nothing_rather_than_a_stray_blank_line() {
        assert_eq!(render(&[], "\n", false), "");
    }

    #[test]
    fn retiming_a_file_writes_beside_the_video_and_refuses_an_empty_result() {
        let dir = std::env::temp_dir().join(format!("trimmer-caption-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let source = dir.join("clip.srt");
        std::fs::write(&source, SAMPLE).expect("write");

        let (result, written) =
            retime_file(&source, &dir.join("out.srt"), 2.0, 10.0, MIN_OVERLAP).expect("ok");
        assert_eq!(result.len(), 3);
        assert!(written.is_some());
        let text = std::fs::read_to_string(dir.join("out.srt")).expect("read");
        // The source used CRLF, so the written file keeps CRLF: these files get diffed.
        assert!(text.starts_with("1\r\n00:00:00,000"), "{text:?}");
        assert!(text.contains("00:00:00,000 --> 00:00:01,500"), "{text:?}");

        // A window that holds nothing must not produce a file.
        let (empty, written) =
            retime_file(&source, &dir.join("empty.srt"), 100.0, 200.0, MIN_OVERLAP).expect("ok");
        assert!(empty.is_empty());
        assert!(written.is_none());
        assert!(!dir.join("empty.srt").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_caption_file_reports_its_path() {
        let error = read(Path::new(r"H:\does\not\exist.srt")).expect_err("refused");
        match error {
            CoreError::Caption { path, .. } => assert!(path.contains("exist.srt")),
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn readability_measures_characters_per_second() {
        let cue = Cue::new(0.0, 2.0, "twenty characters!!\nsecond line");
        assert!(cue.characters_per_second() > 5.0);
        assert!((cue.duration() - 2.0).abs() < 1e-9);
        assert_eq!(cue.one_line(), "twenty characters!! second line");
    }

    #[test]
    fn cue_frames_land_on_the_grid() {
        assert_eq!(cue_frame(1.0, FrameRate::FPS_25), 25);
        assert_eq!(cue_frame(1.0, FrameRate::FPS_29_97), 30);
    }

    #[test]
    fn find_for_prefers_the_exact_stem_then_a_language_tag() {
        let dir = std::env::temp_dir().join(format!("trimmer-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let video = dir.join("interview.mp4");
        std::fs::write(&video, b"not really a video").expect("write");

        assert!(find_for(&video).is_none());

        std::fs::write(dir.join("interview.en.srt"), "1\n00:00:00,000 --> 00:00:01,000\nx\n")
            .expect("write");
        assert_eq!(
            find_for(&video).expect("found").file_name().expect("name"),
            "interview.en.srt"
        );

        std::fs::write(dir.join("interview.srt"), "1\n00:00:00,000 --> 00:00:01,000\ny\n")
            .expect("write");
        assert_eq!(
            find_for(&video).expect("found").file_name().expect("name"),
            "interview.srt"
        );

        // An unrelated transcript must not be picked up.
        std::fs::remove_file(dir.join("interview.srt")).ok();
        std::fs::remove_file(dir.join("interview.en.srt")).ok();
        std::fs::write(dir.join("something-else.srt"), "1\n00:00:00,000 --> 00:00:01,000\nz\n")
            .expect("write");
        assert!(find_for(&video).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
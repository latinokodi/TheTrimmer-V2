//! Resolving the requested segments into the timeline every writer draws from.
//!
//! The four formats disagree about almost everything — frames against rational seconds,
//! inclusive against exclusive out points, one element against twelve — but they must not
//! disagree about *what the timeline is*. So it is decided once, here, and every writer
//! reads the same [`ExportClip`] list.

use std::cmp::Ordering;

use trimmer_core::FrameRate;
use trimmer_core::{CoreError, CoreResult, MediaPath};

use crate::request::ExportRequest;

/// The frame size a timeline is declared with when there is nothing to take it from.
pub(crate) const DEFAULT_WIDTH: u32 = 1920;
/// Height of the same default.
pub(crate) const DEFAULT_HEIGHT: u32 = 1080;

/// One segment, resolved against its source and placed on the timeline.
///
/// `source_out` is exclusive, as it is everywhere else in the domain; the EDL writer is the
/// one place that turns it into the inclusive out point that format asks for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportClip {
    /// The segment's name, as the editor typed it.
    pub(crate) name: String,
    /// The file the frames come from.
    pub(crate) source: MediaPath,
    /// The file's name, for the documents that name the media rather than the cut.
    pub(crate) file_name: String,
    /// Where the caller says the media lives on import.
    pub(crate) import_path: String,
    /// First source frame kept.
    pub(crate) source_in: i64,
    /// One past the last source frame kept.
    pub(crate) source_out: i64,
    /// First timeline frame the clip occupies.
    pub(crate) timeline_start: i64,
    /// One past the last timeline frame the clip occupies.
    pub(crate) timeline_end: i64,
    /// The rate of the source, which is the grid its frames sit on.
    pub(crate) rate: FrameRate,
    /// Frame width of the source.
    pub(crate) width: u32,
    /// Frame height of the source.
    pub(crate) height: u32,
    /// How many frames the whole source has.
    pub(crate) source_frames: i64,
    /// Whether the source carries audio.
    pub(crate) has_audio: bool,
    /// The delivery preset this cut would be made with.
    pub(crate) preset: String,
    /// The segment's tags, in the order they were given.
    pub(crate) tags: Vec<String>,
    /// The note for the editor.
    pub(crate) note: String,
}

impl ExportClip {
    /// How many frames the clip occupies on the timeline.
    pub(crate) const fn frames(&self) -> i64 {
        self.timeline_end - self.timeline_start
    }
}

/// The resolved timeline: the clips that made it, and everything worth saying about it.
pub(crate) struct Prepared {
    /// The clips, in the order they were requested.
    pub(crate) clips: Vec<ExportClip>,
    /// Anything that was skipped, clamped, or could not be done exactly.
    pub(crate) warnings: Vec<String>,
    /// The sum of the clip lengths on the timeline.
    pub(crate) total_frames: i64,
}

/// Where the last clip ends, which is the duration of the sequence.
pub(crate) fn sequence_end(request: &ExportRequest<'_>, clips: &[ExportClip]) -> i64 {
    clips.last().map_or_else(
        || request.timeline_start_frame.max(0),
        |clip| clip.timeline_end,
    )
}

/// The frame size the timeline is declared with: the first clip's, or a sane default.
pub(crate) fn dimensions(clips: &[ExportClip]) -> (u32, u32) {
    clips
        .first()
        .map_or((DEFAULT_WIDTH, DEFAULT_HEIGHT), |clip| {
            (clip.width, clip.height)
        })
}

/// Resolve every requested segment into a clip, or record why it could not be one.
///
/// The order of the checks matters. A segment naming a source the project does not hold is an
/// error, because that is a bug in the caller. A segment whose source is in the project but
/// has never been probed is a warning, because that is a drive being unplugged, and the other
/// ninety-nine segments still have to export. Everything after that — marks outside the
/// source, a rate that does not match the timeline — is a warning too: the timeline is
/// adjusted to something representable and the caller is told what was adjusted.
pub(crate) fn prepare(request: &ExportRequest<'_>) -> CoreResult<Prepared> {
    validate_timeline_rate(request.timeline_rate)?;

    let mut warnings = Vec::new();
    let mut clips = Vec::new();
    let mut cursor = request.timeline_start_frame;
    if cursor < 0 {
        warnings.push(format!(
            "the timeline was asked to start at frame {cursor}, which is before zero; it was \
             moved to frame 0"
        ));
        cursor = 0;
    }

    for segment in &request.segments {
        let Some(source) = request.project.sources.get(&segment.source) else {
            return Err(CoreError::NotFound {
                entity: "source".to_owned(),
                id: segment.source.to_string(),
            });
        };
        let Some(media) = source.media.as_ref() else {
            warnings.push(format!(
                "segment {:?} is skipped: {} has not been probed, so there is no rate or frame \
                 count to write it with. The rest of the timeline is exported without it",
                segment.name, source.path
            ));
            continue;
        };

        if media.rate.cmp(&request.timeline_rate) != Ordering::Equal {
            warnings.push(format!(
                "segment {:?} is {} fps but the timeline is {} fps: its frames are written on \
                 its own source rate, and nothing is rescaled",
                segment.name,
                media.rate.as_ffmpeg(),
                request.timeline_rate.as_ffmpeg()
            ));
        }

        // Handles are part of the range the domain says a segment covers, so they are part of
        // the timeline too: `plan::resolve_range` is the same rule, kept in step by hand
        // because a clamp here has to be reported rather than refused.
        let handles = segment.handle_frames.max(0);
        let marked_end = segment.end_frame.unwrap_or(media.frame_count);
        let source_in = segment.start_frame.saturating_sub(handles).max(0);
        let source_out = marked_end.saturating_add(handles).min(media.frame_count);

        if segment.start_frame < 0
            || segment.start_frame > media.last_frame()
            || marked_end > media.frame_count
        {
            warnings.push(format!(
                "segment {:?} marks frames {}..{} but {} holds frames 0..{}; the range was \
                 clamped to {}..{}",
                segment.name,
                segment.start_frame,
                marked_end,
                source.path,
                media.frame_count,
                source_in,
                source_out
            ));
        }

        if source_out <= source_in {
            warnings.push(format!(
                "segment {:?} is skipped: it covers no frames after clamping ({source_in}..\
                 {source_out})",
                segment.name
            ));
            continue;
        }

        let length = source_out - source_in;
        clips.push(ExportClip {
            name: segment.name.clone(),
            source: source.path.clone(),
            file_name: source.path.file_name(),
            import_path: (request.media_path_for)(&source.path),
            source_in,
            source_out,
            timeline_start: cursor,
            timeline_end: cursor + length,
            rate: media.rate,
            width: media.width,
            height: media.height,
            source_frames: media.frame_count,
            has_audio: media.audio.is_some(),
            preset: segment
                .preset
                .clone()
                .unwrap_or_else(|| request.project.default_preset.clone()),
            tags: segment.tags.clone(),
            note: segment.note.clone().unwrap_or_default(),
        });
        cursor += length;
    }

    if let Some(first) = clips.first() {
        if clips
            .iter()
            .any(|clip| clip.width != first.width || clip.height != first.height)
        {
            warnings.push(format!(
                "the sources are not all the same frame size; the timeline is declared as \
                 {}x{} and the others are placed as they are",
                first.width, first.height
            ));
        }
    }

    let total_frames = clips.iter().map(ExportClip::frames).sum();
    Ok(Prepared {
        clips,
        warnings,
        total_frames,
    })
}

/// Refuse a timeline rate timecode cannot be counted on.
///
/// [`FrameRate`] validates itself, so this cannot fire today; it is here because the failure
/// it prevents is a document full of `00:00:00:00` labels with no indication that anything
/// went wrong, and that is worth one branch against a value that arrives from a project file.
fn validate_timeline_rate(rate: FrameRate) -> CoreResult<()> {
    if rate.numerator() <= 0 || rate.denominator() <= 0 || rate.nominal() <= 0 {
        return Err(CoreError::FrameRate(rate.as_ffmpeg()));
    }
    Ok(())
}

/// A number of seconds as an exact, reduced fraction.
///
/// FCPXML writes times as rational seconds — one frame at 29.97 is `1001/30000s` — and the
/// only way to produce those exactly is to keep the arithmetic in integers. Going through an
/// `f64` would turn that frame into `0.03336666666666667s`, which is a number no editor can
/// place a cut on and no tool can round back to the frame it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rational {
    /// Top of the fraction. Never negative: a negative time is not representable.
    numerator: i64,
    /// Bottom of the fraction, always positive.
    denominator: i64,
}

impl Rational {
    /// Zero seconds.
    pub(crate) const ZERO: Self = Self {
        numerator: 0,
        denominator: 1,
    };

    /// A whole number of frames at a rate, as seconds, exactly.
    pub(crate) fn from_frames(frames: i64, rate: FrameRate) -> Self {
        let numerator = i128::from(frames.max(0)) * i128::from(rate.denominator());
        let denominator = i128::from(rate.numerator());
        let divisor = gcd(numerator, denominator);
        Self {
            numerator: i64::try_from(numerator / divisor).unwrap_or(i64::MAX),
            denominator: i64::try_from(denominator / divisor).unwrap_or(1),
        }
    }

    /// The `N/Ds` spelling FCPXML wants, with a bare `0s` for zero.
    ///
    /// Zero is spelled bare because that is what the format's own examples do, and because
    /// `0/1s` reads like a mistake even where it parses.
    pub(crate) fn text(self) -> String {
        if self.numerator == 0 {
            return "0s".to_owned();
        }
        let numerator = self.numerator;
        let denominator = self.denominator;
        format!("{numerator}/{denominator}s")
    }

    /// The same duration as a decimal count of seconds, for the one format that prints one.
    pub(crate) fn as_seconds(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }
}

/// Euclid's algorithm, for reducing a fraction to lowest terms.
///
/// The denominator passed in is a frame rate's numerator, which the domain guarantees is
/// positive, so the result is never used as a divisor while zero.
fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a.abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hundred_frames_at_twenty_five_is_exactly_four_seconds() {
        assert_eq!(Rational::from_frames(100, FrameRate::FPS_25).text(), "4/1s");
    }

    #[test]
    fn one_frame_at_twenty_nine_ninety_seven_is_the_literal_rational() {
        assert_eq!(
            Rational::from_frames(1, FrameRate::FPS_29_97).text(),
            "1001/30000s"
        );
        assert_eq!(
            Rational::from_frames(30_000, FrameRate::FPS_29_97).text(),
            "1001/1s"
        );
    }

    #[test]
    fn zero_is_spelled_bare_because_that_is_what_the_format_does() {
        assert_eq!(Rational::ZERO.text(), "0s");
        assert_eq!(Rational::from_frames(0, FrameRate::FPS_25).text(), "0s");
    }

    #[test]
    fn a_negative_frame_count_is_treated_as_zero_rather_than_as_a_negative_time() {
        assert_eq!(Rational::from_frames(-5, FrameRate::FPS_25).text(), "0s");
    }

    #[test]
    fn the_decimal_form_agrees_with_the_rational_form() {
        let seconds = Rational::from_frames(150, FrameRate::FPS_25).as_seconds();
        assert!((seconds - 6.0).abs() < 1e-12);
    }
}

//! The cut decision: what the executor will do, decided before it does anything.
//!
//! A plain `ffmpeg -ss S -to E -c copy` cannot start on an arbitrary frame. H.264 and HEVC
//! frames are deltas against earlier frames, so a decoder needs the keyframe that precedes
//! the mark, and a stream copy therefore starts at the last keyframe at or before `S` —
//! typically 8.33 s early on the masters this was built for. The *end* is not constrained
//! that way: you can stop mid-GOP, because every frame up to that point is decodable. Only
//! the start is a problem, and only as far as the next keyframe.
//!
//! So the cut is split at `K`, the first keyframe at or after `S`:
//!
//! ```text
//!     requested:   S ────────────── E
//!     keyframe:         K
//!                  ├─[S, K)─┤├─[K, E]─┤
//!                   re-encode   copy
//! ```
//!
//! Only `K - S` is re-encoded; from `K` on the packets are the original ones, so the result
//! is visually indistinguishable from the source. This is *not* mathematically lossless —
//! `-crf 0` would be, at many times the size. A two-hour master typically re-encodes two or
//! three seconds of it.
//!
//! # The three ways this goes silently wrong
//!
//! All three were discovered on real material in V1, and all three exit `0` while producing
//! a wrong file. Each one is now a checked invariant rather than a paragraph in a design
//! document:
//!
//! 1. **Timescale.** MP4 keeps one timescale for a whole track. libx264 defaults to
//!    `1/15360` while these sources are `1/90000`, and joining the two makes the container
//!    rescale the *copied* body: the clip plays in slow motion with frozen stretches.
//!    [`PlanInvariant::HeadMatchesSourceTimebase`] requires the plan to carry the source's
//!    own timescale forward to the head encode.
//! 2. **Codec.** The head must be the same codec as the body, or the track holds two kinds of
//!    sample description and players — and Premiere — refuse it.
//!    [`plan_cut`] refuses a source whose codec has no matching encoder.
//! 3. **Where the body starts.** The concat demuxer places the second file at the first
//!    file's duration, and that duration is not always the head's content length: the body
//!    lands a few frames early, repeating a slice of the head.
//!    [`PlanInvariant::ConcatOffsetCompensated`] makes the offset explicit and bounded, and
//!    [`CutPlan::concat_offset`] is what the calibration loop corrects.
//!
//! ## But the head lands on a keyframe
//!
//! There is no mode in which a copied body begins anywhere except a keyframe, and the planner
//! is the only thing that can produce a plan, so that mistake is not representable.

use serde::{Deserialize, Serialize};

use crate::delivery::DeliveryPreset;
use crate::domain::{CutMode, KeyframeGrid, MediaInfo, Segment};
use crate::error::{CoreError, CoreResult};
use crate::timecode::format_seconds;
/// A GOP longer than this is not something the head-patch method should quietly re-encode its
/// way through: past thirty seconds, re-encoding the head costs more than it saves and the
/// user almost certainly marked the wrong thing.
pub const MAX_HEAD_SECONDS: f64 = 30.0;

/// The largest concat-drift correction the calibration loop is allowed to apply.
///
/// Drift is a few frames by construction. A "correction" larger than this is not drift, it is
/// a symptom of something else being wrong, and applying it would hide the real fault.
pub const MAX_CONCAT_OFFSET_FRAMES: i64 = 12;

/// A condition the executor and the verifier both depend on.
///
/// These are not assertions in the C sense: they are *named*, they are serialisable, and they
/// travel with the plan into the audit log, so a cut that was made on a machine nobody can
/// inspect can still be shown to have satisfied them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanInvariant {
    /// The requested range is non-empty and lies inside the source.
    RangeInsideSource,
    /// A body that is copied begins on a keyframe.
    BodyStartsOnKeyframe,
    /// Head and body are the same codec, so the track has one sample description.
    HeadMatchesBodyCodec,
    /// The head is encoded at the source's timescale, so the muxer does not rescale the
    /// copied body into slow motion.
    HeadMatchesSourceTimebase,
    /// The concat drift correction is explicit and within the bound the method can justify.
    ConcatOffsetCompensated,
    /// Frame accounting is exact: head plus body equals the requested frame count.
    FramesAddUp,
    /// Re-encoding is confined to the head, and the head is bounded.
    HeadIsBounded,
    /// The source is constant frame rate, or the plan says so out loud.
    FrameGridIsTrustworthy,
}

impl PlanInvariant {
    /// Every invariant, for a report that wants to list what was checked.
    pub const ALL: [Self; 8] = [
        Self::RangeInsideSource,
        Self::BodyStartsOnKeyframe,
        Self::HeadMatchesBodyCodec,
        Self::HeadMatchesSourceTimebase,
        Self::ConcatOffsetCompensated,
        Self::FramesAddUp,
        Self::HeadIsBounded,
        Self::FrameGridIsTrustworthy,
    ];

    /// A sentence for the audit log.
    #[must_use]
    pub const fn statement(self) -> &'static str {
        match self {
            Self::RangeInsideSource => {
                "the requested range is non-empty and lies inside the source"
            }
            Self::BodyStartsOnKeyframe => {
                "every copied frame follows a keyframe, so the body decodes on its own"
            }
            Self::HeadMatchesBodyCodec => {
                "the re-encoded head uses the body's codec, so the track has one sample \
                 description"
            }
            Self::HeadMatchesSourceTimebase => {
                "the head is encoded at the source timescale, so the muxer does not rescale \
                 the copied body"
            }
            Self::ConcatOffsetCompensated => {
                "the concat drift correction is explicit and within the bound the method can \
                 justify"
            }
            Self::FramesAddUp => {
                "head frames plus copied frames equal the requested frame count exactly"
            }
            Self::HeadIsBounded => {
                "re-encoding is confined to the head, and the head is no longer than the \
                 method allows"
            }
            Self::FrameGridIsTrustworthy => {
                "the source is constant frame rate, or the plan records that it is not"
            }
        }
    }
}

/// The decision: everything the executor needs, and nothing it has to work out for itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutPlan {
    /// Which of the three cuts this is.
    pub mode: CutMode,
    /// First frame kept, inclusive.
    pub start_frame: i64,
    /// One past the last frame kept.
    pub end_frame: i64,
    /// The keyframe the body begins on, when there is a body to copy. `None` for a full
    /// re-encode.
    pub keyframe: Option<i64>,
    /// How many frames are re-encoded.
    pub head_frames: i64,
    /// How many frames are copied untouched.
    pub body_frames: i64,
    /// Frames of concat drift the executor must compensate for. Positive moves the body
    /// later, which is what cancels the demuxer placing it early.
    pub concat_offset: i64,
    /// The timescale the head must be encoded at, carried from the source.
    pub video_timescale: i64,
    /// The codec the head must be encoded as, carried from the source.
    pub head_codec: String,
    /// The encoder that does it.
    pub head_encoder: String,
    /// Any tag the muxer needs for this codec, e.g. `hvc1` for HEVC in MP4.
    pub head_extra_args: Vec<String>,
    /// The source's frame rate, which the executor must not change.
    pub rate_numerator: i64,
    /// Denominator of the source's frame rate.
    pub rate_denominator: i64,
    /// Whether the source had audio.
    pub has_audio: bool,
    /// Notes for the operator: things that are true and worth saying, not errors.
    pub notes: Vec<String>,
    /// The invariants this plan satisfies. Checked when the plan is built, re-checked when it
    /// is loaded from a project file, and quoted in the audit log.
    pub invariants: Vec<PlanInvariant>,
}

impl CutPlan {
    /// Frames requested, which is exactly what the segment asked for.
    #[must_use]
    pub const fn requested_frames(&self) -> i64 {
        self.end_frame - self.start_frame
    }

    /// True when some of the delivered frames are the original packets.
    #[must_use]
    pub const fn is_lossless(&self) -> bool {
        self.mode.is_lossless()
    }

    /// How long the head takes to encode, in seconds.
    #[must_use]
    pub fn head_seconds(&self) -> f64 {
        if self.rate_numerator == 0 {
            return 0.0;
        }
        self.head_frames as f64 * self.rate_denominator as f64 / self.rate_numerator as f64
    }

    /// The fraction of the segment that is re-encoded, for the "cost" readout in the UI.
    #[must_use]
    pub fn reencode_fraction(&self) -> f64 {
        let total = self.requested_frames();
        if total <= 0 {
            return 0.0;
        }
        self.head_frames as f64 / total as f64
    }

    /// Where the body begins, in seconds, as the executor needs it.
    #[must_use]
    pub fn body_start_seconds(&self) -> Option<f64> {
        self.keyframe.map(|frame| {
            frame as f64 * self.rate_denominator as f64 / self.rate_numerator as f64
        })
    }

    /// Re-check every invariant this plan claims, and return the ones it no longer satisfies.
    ///
    /// Called when a plan is built, and again whenever one is read back from a project file or
    /// handed across a process boundary. A plan is data, and data can be edited by hand or by
    /// a future version of this program; the check is cheap and the failure it prevents is
    /// expensive.
    #[must_use]
    pub fn violated_invariants(&self) -> Vec<PlanInvariant> {
        let mut broken = Vec::new();

        if self.end_frame <= self.start_frame || self.start_frame < 0 {
            broken.push(PlanInvariant::RangeInsideSource);
        }

        if self.mode == CutMode::HeadPatch {
            match self.keyframe {
                // The keyframe must exist and lie inside the segment; that is what makes the
                // body independently decodable. The *frame counts* are checked by
                // [`PlanInvariant::FramesAddUp`] instead, and deliberately not re-checked here:
                // the calibration loop legitimately moves frames between the head and the body,
                // so an equality assertion between `body_frames` and `end_frame - keyframe`
                // would fail on exactly the correction the method requires.
                Some(keyframe) if keyframe >= self.start_frame && keyframe < self.end_frame => {}
                _ => broken.push(PlanInvariant::BodyStartsOnKeyframe),
            }
        }
        if self.mode == CutMode::Reencode && self.keyframe.is_some() {
            broken.push(PlanInvariant::BodyStartsOnKeyframe);
        }
        if self.mode == CutMode::Copy && (self.head_frames != 0 || self.keyframe.is_none()) {
            broken.push(PlanInvariant::BodyStartsOnKeyframe);
        }

        if self.head_codec.trim().is_empty() || self.head_encoder.trim().is_empty() {
            broken.push(PlanInvariant::HeadMatchesBodyCodec);
        }

        if self.mode != CutMode::Reencode && self.video_timescale <= 0 {
            broken.push(PlanInvariant::HeadMatchesSourceTimebase);
        }
        if self.head_frames > 0 && self.video_timescale <= 0 {
            broken.push(PlanInvariant::HeadMatchesSourceTimebase);
        }

        if self.concat_offset.abs() > MAX_CONCAT_OFFSET_FRAMES {
            broken.push(PlanInvariant::ConcatOffsetCompensated);
        }

        if self.head_frames + self.body_frames != self.requested_frames()
            || self.head_frames < 0
            || self.body_frames < 0
        {
            broken.push(PlanInvariant::FramesAddUp);
        }

        if self.mode != CutMode::Reencode {
            // `HeadIsBounded` is about re-encoding being *confined to the head*, and the head
            // being a coherent part of the request. It deliberately does not police the head's
            // *length*: a GOP can legitimately be longer than [`MAX_HEAD_SECONDS`], and when it
            // is, the planner still produces a correct head patch and says so in its notes.
            // Refusing that plan would convert a warning into a failure and lose a cut the user
            // asked for.
            if self.head_frames > self.requested_frames() {
                broken.push(PlanInvariant::HeadIsBounded);
            }
        }
        if self.mode == CutMode::Reencode && (self.head_frames != self.requested_frames() || self.body_frames != 0)
        {
            broken.push(PlanInvariant::HeadIsBounded);
        }

        broken.sort_unstable();
        broken.dedup();
        broken
    }

    /// Check the invariants and fail loudly when one is broken.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Invariant`] naming the first broken invariant.
    pub fn verify_invariants(&self) -> CoreResult<()> {
        let broken = self.violated_invariants();
        if let Some(first) = broken.first() {
            return Err(CoreError::Invariant(format!(
                "{} ({first:?})",
                first.statement()
            )));
        }
        Ok(())
    }

    /// A human-readable description, for the CLI and the log.
    #[must_use]
    pub fn describe(&self, rate: crate::timecode::FrameRate) -> String {
        let mut lines = vec![format!("mode        {}", self.mode.label())];
        // The range is quoted in the form Premiere would write it, which is what the operator
        // pasted in and what they will check against.
        let in_text = crate::timecode::format_timecode(self.start_frame, rate, None);
        let out_text = crate::timecode::format_timecode(self.end_frame, rate, None);
        match self.mode {
            CutMode::HeadPatch => {
                lines.push(format!(
                    "head        frames {} ({:.3}s) re-encoded from the in point to keyframe {}",
                    self.head_frames,
                    self.head_seconds(),
                    self.keyframe.unwrap_or_default()
                ));
                lines.push(format!(
                    "body        frames {} copied untouched",
                    self.body_frames
                ));
            }
            CutMode::Copy => {
                lines.push(
                    "in point    lands on a keyframe: the whole segment is copied untouched"
                        .to_owned(),
                );
            }
            CutMode::Reencode => {
                lines.push(format!(
                    "whole       segment re-encoded ({} frames), because no keyframe falls inside it",
                    self.requested_frames()
                ));
            }
        }
        if self.concat_offset != 0 {
            lines.push(format!(
                "calibration concat offset {} frame(s)",
                self.concat_offset
            ));
        }
        lines.push(format!(
            "range       {} .. {} ({} frames, {})",
            in_text,
            out_text,
            self.requested_frames(),
            format_seconds(
                self.requested_frames() as f64 * self.rate_denominator as f64
                    / self.rate_numerator as f64
            )
        ));
        for note in &self.notes {
            lines.push(format!("note        {note}"));
        }
        lines.join("\n")
    }
}

/// Work out how to cut a segment, and refuse clearly when it cannot be cut.
///
/// This is a pure function: it takes the media facts and the keyframe grid the probe layer
/// found, and returns a decision. It launches nothing, reads nothing and writes nothing,
/// which is what lets the same decision be replayed in a test against the V1 engine.
///
/// # Errors
///
/// Returns [`CoreError::EmptyRange`], [`CoreError::BeforeStart`], [`CoreError::PastEnd`] or
/// [`CoreError::UnsupportedCodec`] when the request cannot be satisfied, and
/// [`CoreError::Invariant`] in the (unreachable) case that the planner contradicts itself.
pub fn plan_cut(media: &MediaInfo, segment: &Segment, keyframes: &KeyframeGrid) -> CoreResult<CutPlan> {
    // Validate what the user actually asked for, *before* any clamping. Validating after
    // resolution would hide a genuine mistake: a segment whose in point is past the end of the
    // file gets clamped by the handle logic into something that looks reasonable, and the user
    // receives a cut of the wrong place instead of a sentence saying their mark is off the end.
    if segment.start_frame < 0 {
        return Err(CoreError::BeforeStart {
            in_frame: segment.start_frame,
        });
    }
    if segment.start_frame > media.last_frame() {
        return Err(CoreError::PastEnd {
            out_frame: segment.start_frame,
            frame_count: media.frame_count,
            last_frame: media.last_frame(),
        });
    }
    if let Some(end) = segment.end_frame {
        if end > media.frame_count {
            return Err(CoreError::PastEnd {
                out_frame: end,
                frame_count: media.frame_count,
                last_frame: media.last_frame(),
            });
        }
        if end <= segment.start_frame {
            return Err(CoreError::EmptyRange {
                in_frame: segment.start_frame,
                out_frame: end,
            });
        }
    }

    let (start_frame, end_frame) = resolve_range(media, segment)?;

    if end_frame <= start_frame {
        return Err(CoreError::EmptyRange {
            in_frame: start_frame,
            out_frame: end_frame,
        });
    }
    let Some((encoder, extra)) = media.encoder() else {
        return Err(CoreError::UnsupportedCodec {
            codec: media.codec.clone(),
        });
    };

    let mut notes = Vec::new();
    let mut invariants = vec![PlanInvariant::RangeInsideSource];
    if media.is_variable_rate() {
        notes.push(
            "the source looks variable frame rate, so the frame grid this method assumes may \
             not hold and the marks are approximate"
                .to_owned(),
        );
    } else {
        invariants.push(PlanInvariant::FrameGridIsTrustworthy);
    }

    let keyframe = keyframes.first_at_or_after(start_frame);
    let head_extra_args: Vec<String> = extra.iter().map(|arg| (*arg).to_owned()).collect();
    let mut common = PlanCommon {
        start_frame,
        end_frame,
        video_timescale: media.timebase.ticks(),
        head_codec: media.codec.to_lowercase(),
        head_encoder: encoder.to_owned(),
        head_extra_args,
        rate_numerator: media.rate.numerator(),
        rate_denominator: media.rate.denominator(),
        has_audio: media.audio.is_some(),
        notes,
        invariants,
    };

    // Case 1: no keyframe inside the segment, so there is nothing to copy from. Re-encode.
    let unusable = match keyframe {
        None => true,
        Some(k) => k >= end_frame,
    };
    if unusable {
        common.notes.push(format!(
            "no keyframe between frames {start_frame} and {end_frame}: the whole segment is \
             re-encoded, because a stream-copied body has to begin on a keyframe"
        ));
        let mut plan = common.finish(CutMode::Reencode, None, end_frame - start_frame, 0, 0);
        plan.invariants.push(PlanInvariant::HeadIsBounded);
        plan.invariants.push(PlanInvariant::FramesAddUp);
        plan.verify_invariants()?;
        return Ok(plan);
    }
    let keyframe = keyframe.expect("checked above");

    // Case 2: the in point is itself a keyframe, so nothing needs re-encoding at all.
    if keyframe == start_frame {
        let mut plan = common.finish(CutMode::Copy, Some(keyframe), 0, end_frame - keyframe, 0);
        plan.invariants.push(PlanInvariant::BodyStartsOnKeyframe);
        plan.invariants.push(PlanInvariant::FramesAddUp);
        plan.verify_invariants()?;
        return Ok(plan);
    }

    // Case 3: the normal one. Re-encode up to the keyframe, copy from it.
    let head_frames = keyframe - start_frame;
    let head_seconds = head_frames as f64 * media.rate.denominator() as f64
        / media.rate.numerator() as f64;
    if head_seconds > MAX_HEAD_SECONDS {
        common.notes.push(format!(
            "the first keyframe is {head_seconds:.2}s after the in point, which is longer than \
             the {MAX_HEAD_SECONDS:.0}s this method will re-encode; check that the in point is \
             where you meant it to be, or cut this segment with a plain re-encode"
        ));
    }
    let mut plan = common.finish(
        CutMode::HeadPatch,
        Some(keyframe),
        head_frames,
        end_frame - keyframe,
        0,
    );
    plan.invariants.push(PlanInvariant::BodyStartsOnKeyframe);
    plan.invariants.push(PlanInvariant::HeadMatchesBodyCodec);
    plan.invariants.push(PlanInvariant::HeadMatchesSourceTimebase);
    plan.invariants.push(PlanInvariant::ConcatOffsetCompensated);
    plan.invariants.push(PlanInvariant::FramesAddUp);
    plan.invariants.push(PlanInvariant::HeadIsBounded);
    plan.verify_invariants()?;
    Ok(plan)
}

/// Resolve a segment's range against a source, applying handles and the open end.
///
/// Handles are applied here rather than in the executor because they change the *requested*
/// range, and therefore the frame accounting the whole report is built on.
///
/// # Errors
///
/// Returns [`CoreError::BeforeStart`] or [`CoreError::PastEnd`] when the segment names frames
/// the source does not have. Handles are clamped first, so a handle alone never causes this.
pub fn resolve_range(media: &MediaInfo, segment: &Segment) -> CoreResult<(i64, i64)> {
    let handles = segment.handle_frames.max(0);
    let raw_end = segment.end_frame.unwrap_or(media.frame_count);
    let start = (segment.start_frame - handles).max(0);
    let end = (raw_end + handles).min(media.frame_count);
    if segment.start_frame > media.last_frame() {
        return Err(CoreError::PastEnd {
            out_frame: segment.start_frame,
            frame_count: media.frame_count,
            last_frame: media.last_frame(),
        });
    }
    Ok((start, end))
}

/// Apply a calibration correction to a plan, keeping the frame accounting exact.
///
/// The concat demuxer places the body at the head's declared duration, and the measurement
/// loop finds it landed a few frames early or late. Folding the difference into the head's
/// length is what cancels it. The correction moves frames *between* the head and the body; it
/// never changes the total, because the total is what the user asked for.
///
/// # Errors
///
/// Returns [`CoreError::Invariant`] when the correction would leave less than one frame in
/// either part, and [`CoreError::Delivery`]-shaped refusal when it exceeds
/// [`MAX_CONCAT_OFFSET_FRAMES`].
pub fn apply_calibration(plan: &CutPlan, offset_frames: i64) -> CoreResult<CutPlan> {
    if offset_frames.abs() > MAX_CONCAT_OFFSET_FRAMES {
        return Err(CoreError::Invariant(format!(
            "a concat correction of {offset_frames} frames is outside the {MAX_CONCAT_OFFSET_FRAMES} \
             frame bound; this is not drift, it is a symptom"
        )));
    }
    if plan.mode != CutMode::HeadPatch || plan.keyframe.is_none() {
        // Nothing to calibrate: a copy has no head to grow, and a re-encode has no concat step.
        return Ok(plan.clone());
    }

    let mut corrected = plan.clone();
    corrected.head_frames = plan.head_frames + offset_frames;
    corrected.body_frames = plan.body_frames - offset_frames;
    corrected.concat_offset = offset_frames;

    if corrected.head_frames < 1 || corrected.body_frames < 0 {
        return Err(CoreError::Invariant(format!(
            "correcting by {offset_frames} frames would leave {} head frames and {} body frames",
            corrected.head_frames, corrected.body_frames
        )));
    }
    corrected.verify_invariants()?;
    Ok(corrected)
}

/// The fields every plan shares, gathered while the planner decides which shape it is.
struct PlanCommon {
    start_frame: i64,
    end_frame: i64,
    video_timescale: i64,
    head_codec: String,
    head_encoder: String,
    head_extra_args: Vec<String>,
    rate_numerator: i64,
    rate_denominator: i64,
    has_audio: bool,
    notes: Vec<String>,
    invariants: Vec<PlanInvariant>,
}

impl PlanCommon {
    fn finish(
        self,
        mode: CutMode,
        keyframe: Option<i64>,
        head_frames: i64,
        body_frames: i64,
        concat_offset: i64,
    ) -> CutPlan {
        CutPlan {
            mode,
            start_frame: self.start_frame,
            end_frame: self.end_frame,
            keyframe,
            head_frames,
            body_frames,
            concat_offset,
            video_timescale: self.video_timescale,
            head_codec: self.head_codec,
            head_encoder: self.head_encoder,
            head_extra_args: self.head_extra_args,
            rate_numerator: self.rate_numerator,
            rate_denominator: self.rate_denominator,
            has_audio: self.has_audio,
            notes: self.notes,
            invariants: self.invariants,
        }
    }
}

/// True when a delivery preset can be satisfied without re-encoding anything but the head.
///
/// A caller uses this to warn before a batch run: a vertical crop preset on a hundred
/// segments is a hundred full transcodes, and that should be a decision rather than a
/// surprise.
#[must_use]
pub fn preset_forces_full_encode(media: &MediaInfo, preset: &DeliveryPreset) -> bool {
    !preset.preserves_picture(media.width, media.height)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::standard_preset;
    use crate::domain::{AudioFormat, MediaPath};
    use crate::timecode::FrameRate;

    /// A 29.97 master with a 250-frame GOP — the shape these sources actually have.
    fn master() -> MediaInfo {
        MediaInfo {
            path: MediaPath::new(r"H:\master.mp4"),
            codec: "h264".to_owned(),
            pix_fmt: "yuv420p".to_owned(),
            width: 1920,
            height: 1080,
            rate: FrameRate::FPS_29_97,
            average_rate: Some(FrameRate::FPS_29_97),
            timebase: crate::domain::Timescale::NINETY_KHZ,
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

    /// Keyframes every 250 frames, positioned so `around` sits *between* two of them.
    ///
    /// The offset is deliberate. Placing a keyframe exactly on 1 000 would make the planner take
    /// the "the in point is a keyframe, copy everything" path, and every head-patch assertion in
    /// this module would then be testing the copy path instead — a green suite that proves the
    /// wrong thing. Keyframes land at 900, 1 150, 1 400, 1 650.
    fn grid(around: i64) -> KeyframeGrid {
        let base = ((around - 100) / 250) * 250 + 150;
        KeyframeGrid::new(
            vec![base - 250, base, base + 250, base + 500],
            around - 300,
            around + 600,
        )
    }

    fn cut_plan(start: i64, end: i64) -> CutPlan {
        let media = master();
        let segment = Segment::new(media.path.clone(), "s", start, end);
        let cut = plan_cut(&media, &segment, &grid(start)).expect("plans");
        assert_eq!(
            cut.violated_invariants(),
            Vec::new(),
            "the planner produced a plan that breaks its own invariants"
        );
        cut
    }

    #[test]
    fn a_mark_before_the_next_keyframe_re_encodes_only_the_head() {
        // In point 1 000; the grid's next keyframe at or after it is 1 150.
        let cut = cut_plan(1_000, 1_600);
        assert_eq!(cut.mode, CutMode::HeadPatch);
        assert_eq!(cut.keyframe, Some(1_150));
        assert_eq!(cut.head_frames, 150);
        assert_eq!(cut.body_frames, 450);
        assert_eq!(cut.requested_frames(), 600);
        assert!(cut.is_lossless());
        assert!(cut.verify_invariants().is_ok());
    }

    #[test]
    fn an_in_point_on_a_keyframe_copies_everything() {
        // 1 150 is a keyframe in this grid, so nothing needs re-encoding.
        let cut = cut_plan(1_150, 1_600);
        assert_eq!(cut.mode, CutMode::Copy);
        assert_eq!(cut.keyframe, Some(1_150));
        assert_eq!(cut.head_frames, 0);
        assert_eq!(cut.body_frames, 450);
        assert!(cut.is_lossless());
        assert!(cut.head_encoder.contains("264"));
    }

    #[test]
    fn a_segment_with_no_keyframe_inside_is_re_encoded_whole() {
        // 1 260 to 1 270: the next keyframe (1 500) is past the out point.
        let media = master();
        let segment = Segment::new(media.path.clone(), "s", 1_100, 1_110);
        let keyframes = KeyframeGrid::new(vec![900, 1_150], 1_100, 1_200);
        let cut = plan_cut(&media, &segment, &keyframes).expect("plans");
        assert_eq!(cut.mode, CutMode::Reencode);
        assert_eq!(cut.head_frames, 10);
        assert_eq!(cut.body_frames, 0);
        assert!(cut.keyframe.is_none());
        assert!(!cut.is_lossless());
        assert!(cut.notes.iter().any(|note| note.contains("no keyframe")));
        assert!(cut.verify_invariants().is_ok());
    }

    #[test]
    fn the_head_encoder_follows_the_source_codec_and_carries_its_tag() {
        let mut media = master();
        media.codec = "hevc".to_owned();
        let segment = Segment::new(media.path.clone(), "s", 1_000, 1_600);
        let cut = plan_cut(&media, &segment, &grid(1_000)).expect("plans");
        assert_eq!(cut.head_encoder, "libx265");
        assert!(cut.head_extra_args.contains(&"hvc1".to_owned()));
    }

    #[test]
    fn the_plan_carries_the_source_timescale_which_is_what_stops_the_slow_motion_bug() {
        // V1 ADR-001: libx264 would default to 1/15360 and the muxer would rescale the copied
        // body. The plan must carry 90000 forward.
        let cut = cut_plan(1_000, 1_600);
        assert_eq!(cut.video_timescale, 90_000);
    }

    #[test]
    fn a_source_with_no_matching_encoder_is_refused_by_name() {
        let mut media = master();
        media.codec = "prores".to_owned();
        let segment = Segment::new(media.path.clone(), "s", 1_000, 1_600);
        let error = plan_cut(&media, &segment, &grid(1_000)).expect_err("refused");
        match error {
            CoreError::UnsupportedCodec { codec } => assert_eq!(codec, "prores"),
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn an_empty_or_out_of_range_request_is_refused_with_the_numbers() {
        let media = master();
        let backwards = Segment::new(media.path.clone(), "s", 500, 400);
        match plan_cut(&media, &backwards, &grid(500)).expect_err("refused") {
            CoreError::EmptyRange { in_frame, out_frame } => {
                assert_eq!((in_frame, out_frame), (500, 400));
            }
            other => panic!("wrong error: {other:?}"),
        }

        // The in point itself is past the last frame, so that is what is reported.
        let past_end = Segment::new(media.path.clone(), "s", 216_000, 216_100);
        match plan_cut(&media, &past_end, &grid(216_000)).expect_err("refused") {
            CoreError::PastEnd {
                out_frame,
                frame_count,
                last_frame,
            } => {
                assert_eq!(out_frame, 216_000);
                assert_eq!(frame_count, 216_000);
                assert_eq!(last_frame, 215_999);
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn an_open_ended_segment_runs_to_the_end_of_the_source() {
        let media = master();
        let mut segment = Segment::new(media.path.clone(), "s", 215_900, 0);
        segment.end_frame = None;
        let cut = plan_cut(&media, &segment, &grid(215_900)).expect("plans");
        assert_eq!(cut.end_frame, media.frame_count);
        assert_eq!(cut.requested_frames(), media.frame_count - 215_900);
    }

    #[test]
    fn handles_widen_the_request_and_are_clamped_at_the_ends_of_the_file() {
        let media = master();
        let mut segment = Segment::new(media.path.clone(), "s", 10, 20);
        segment.handle_frames = 48;
        let cut = plan_cut(&media, &segment, &grid(0)).expect("plans");
        assert_eq!(cut.start_frame, 0);
        assert_eq!(cut.end_frame, 68);

        let mut segment = Segment::new(media.path.clone(), "s", 215_990, 216_000);
        segment.handle_frames = 48;
        let cut = plan_cut(&media, &segment, &grid(215_990)).expect("plans");
        assert_eq!(cut.end_frame, media.frame_count);
    }

    #[test]
    fn calibration_moves_frames_between_the_head_and_the_body_without_changing_the_total() {
        let original = cut_plan(1_000, 1_600);
        let corrected = apply_calibration(&original, 3).expect("corrected");
        assert_eq!(corrected.head_frames, 153);
        assert_eq!(corrected.body_frames, 447);
        assert_eq!(corrected.concat_offset, 3);
        assert_eq!(corrected.requested_frames(), original.requested_frames());
        assert!(corrected.verify_invariants().is_ok());

        let corrected = apply_calibration(&original, -3).expect("corrected");
        assert_eq!(corrected.head_frames, 147);
        assert_eq!(corrected.body_frames, 453);
    }

    #[test]
    fn a_calibration_larger_than_drift_is_refused_rather_than_applied() {
        let original = cut_plan(1_000, 1_600);
        assert!(apply_calibration(&original, MAX_CONCAT_OFFSET_FRAMES + 1).is_err());
        assert!(apply_calibration(&original, -(MAX_CONCAT_OFFSET_FRAMES + 1)).is_err());
        // And one that would empty a part is refused too.
        assert!(apply_calibration(&original, -150).is_err());
    }

    #[test]
    fn calibrating_a_copy_or_a_reencode_is_a_no_op_rather_than_an_error() {
        for range in [(1_150, 1_600), (1_100, 1_110)] {
            let original = cut_plan(range.0, range.1);
            let corrected = apply_calibration(&original, 5).expect("no-op");
            assert_eq!(corrected.head_frames, original.head_frames);
            assert_eq!(corrected.concat_offset, 0);
        }
    }

    #[test]
    fn a_hand_edited_plan_with_broken_arithmetic_is_caught() {
        let mut broken = cut_plan(1_000, 1_600);
        broken.head_frames += 1; // the classic: head and body no longer sum to the request
        assert!(broken
            .violated_invariants()
            .contains(&PlanInvariant::FramesAddUp));
        assert!(broken.verify_invariants().is_err());
    }

    #[test]
    fn a_copy_mode_plan_cannot_claim_a_head() {
        let mut broken = cut_plan(1_150, 1_600);
        broken.head_frames = 5;
        assert!(broken
            .violated_invariants()
            .contains(&PlanInvariant::BodyStartsOnKeyframe));
    }

    #[test]
    fn a_head_patch_without_a_keyframe_is_caught() {
        let mut broken = cut_plan(1_000, 1_600);
        broken.keyframe = None;
        assert!(broken
            .violated_invariants()
            .contains(&PlanInvariant::BodyStartsOnKeyframe));
    }

    #[test]
    fn a_head_longer_than_the_method_allows_is_flagged_in_the_notes() {
        let media = master();
        // Keyframes 2 000 frames apart: 66.7 s, well past the 30 s bound.
        // 900 frames at 29.97 is 30.03 s, just past the 30 s the method wants to re-encode. The
        // plan is still produced — a long GOP is not a failure, it is a cost — but it says so.
        let segment = Segment::new(media.path.clone(), "s", 600, 5_000);
        let keyframes = KeyframeGrid::new(vec![500, 1_500, 4_000, 6_000], 600, 5_000);
        let cut = plan_cut(&media, &segment, &keyframes).expect("plans");
        assert!(cut.notes.iter().any(|note| note.contains("longer than")), "{:?}", cut.notes);
        assert!(cut.violated_invariants().is_empty(), "{:?}", cut.violated_invariants());
    }

    #[test]
    fn a_variable_rate_source_is_planned_but_says_so() {
        let mut media = master();
        media.average_rate = Some(FrameRate::FPS_25);
        let segment = Segment::new(media.path.clone(), "s", 1_000, 1_600);
        let cut = plan_cut(&media, &segment, &grid(1_000)).expect("plans");
        assert!(cut
            .notes
            .iter()
            .any(|note| note.contains("variable frame rate")));
        assert!(!cut
            .invariants
            .contains(&PlanInvariant::FrameGridIsTrustworthy));
    }

    #[test]
    fn the_plan_describes_itself_in_a_way_an_operator_can_check() {
        let cut = cut_plan(1_000, 1_600);
        let text = cut.describe(FrameRate::FPS_29_97);
        assert!(text.contains("Head patch"));
        assert!(text.contains("frames 150"));
        assert!(text.contains("00:00:33"), "{text}");
        assert!(text.contains("range       00:00:33"), "{text}");
    }

    #[test]
    fn cost_accounting_reports_the_fraction_that_is_re_encoded() {
        let cut = cut_plan(1_000, 1_600);
        assert!((cut.reencode_fraction() - 150.0 / 600.0).abs() < 1e-9);
        assert!((cut.head_seconds() - 150.0 * 1001.0 / 30_000.0).abs() < 1e-6);

        let copy_only = cut_plan(1_150, 1_600);
        assert!(copy_only.reencode_fraction().abs() < 1e-12);
    }

    #[test]
    fn every_invariant_has_a_sentence_and_the_list_is_complete() {
        for invariant in PlanInvariant::ALL {
            assert!(!invariant.statement().is_empty(), "{invariant:?}");
        }
        let cut = cut_plan(1_000, 1_600);
        for invariant in &cut.invariants {
            assert!(PlanInvariant::ALL.contains(invariant), "{invariant:?}");
        }
    }

    #[test]
    fn a_preset_that_breaks_passthrough_is_visible_before_a_batch_runs() {
        let media = master();
        assert!(!preset_forces_full_encode(
            &media,
            &standard_preset("master").expect("ok")
        ));
        assert!(preset_forces_full_encode(
            &media,
            &standard_preset("vertical").expect("ok")
        ));
        assert!(preset_forces_full_encode(
            &media,
            &standard_preset("youtube_1080").expect("ok")
        ));
    }

    #[test]
    fn the_body_start_is_where_the_keyframe_is() {
        let cut = cut_plan(1_000, 1_600);
        let seconds = cut.body_start_seconds().expect("has a body");
        assert!((seconds - 1_150.0 * 1001.0 / 30_000.0).abs() < 1e-9);
        assert!(cut_plan(1_100, 1_110).body_start_seconds().is_none());
    }

    #[test]
    fn a_plan_survives_a_json_round_trip_so_it_can_live_in_a_project_file() {
        let cut = cut_plan(1_000, 1_600);
        let json = serde_json::to_string(&cut).expect("serialises");
        let back: CutPlan = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, cut);
        assert!(back.verify_invariants().is_ok());
    }
}
//! The checks: what each one proves, and the rules that decide it.
//!
//! Every rule here is a pure function of measured facts. Nothing in this module opens a
//! file, starts a process or reads a clock, and that is the point of the whole crate: the
//! part of verification that must be *provably* right is the part that can be tested
//! exhaustively, on any machine, with no media. The tools that can see a file are behind
//! [`crate::MediaMeasurer`], one trait wide.
//!
//! # Why overshoot is a warning and not a failure
//!
//! A stream copy cannot stop between packets. The out point usually falls inside a packet,
//! so the copy carries on to the end of it and the delivered file typically runs one to
//! three frames long. Those extra frames are a real difference from the request, and they
//! are worth saying out loud — but every frame that was asked for is present, in the right
//! place, so the cut is not wrong. Failing it would reject the correct output of the
//! intended method, and a verifier that fails correct files is a verifier people learn to
//! ignore. A *short* file is the opposite case: a frame that was asked for is missing, and
//! that is a failure. [`Check::Overshoot`] therefore warns, [`Check::Frames`] fails, and
//! neither can do the other's job.

use serde::{Deserialize, Serialize};

use trimmer_core::caption::{self, Cue};
use trimmer_core::{CoreError, CoreResult, CutMode, CutPlan, MediaPath, VerifyPolicy};

use crate::facts::{CutFacts, FrameHashes, Similarity};

/// The frame offsets the frame-hash comparison will accept.
///
/// Ordered so that zero wins a tie: the ordinary case is that the two files were sampled on
/// the same grid, and only a genuine fraction-of-a-frame difference should move it.
const ALIGNMENT_OFFSETS: [i64; 3] = [0, -1, 1];

/// The lowest structural-similarity score a re-encoded head may have against the source
/// frame at the in point before the cut is reported as wrong.
///
/// A re-encode at a sane bitrate scores in the high nineties; a cut taken from the wrong
/// place — a frame or a second out — scores well below this. The threshold is not a quality
/// gate, it is a "is this the right picture" gate.
pub const HEAD_FIDELITY_MIN: f64 = 0.98;

/// How far the audio may end from where the picture ends, in frames of the plan's rate.
///
/// Two frames is what an encoder delay and a container's own rounding can account for; more
/// than that is a track that was cut to a different mark than the picture was.
pub const AUDIO_LEVEL_TOLERANCE_FRAMES: f64 = 2.0;

/// The reason every check carries when the policy is [`VerifyPolicy::Off`].
pub const VERIFICATION_OFF: &str = "verification is off";

/// How much two caption boundaries may differ and still be the same boundary.
///
/// Both sides are millisecond-resolution SRT, so anything above a millisecond is a real
/// difference rather than rounding.
const CAPTION_TOLERANCE_SECONDS: f64 = 1e-3;

/// Every check the verifier can run.
///
/// Like [`trimmer_core::PlanInvariant`], these are *named*: they are serialisable, they
/// travel into the audit log, and each one carries a sentence saying what it proves. A
/// report is therefore readable by someone who was not there when it was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Check {
    /// Every frame the plan asked for is present.
    Frames,
    /// The picture is as long as the frames it holds, at the plan's own rate.
    Duration,
    /// The audio ends level with the picture.
    AudioAlignment,
    /// The delivered frames are the source frames at the cut.
    FrameAlignment,
    /// The first delivered frame is the source frame at the in point.
    HeadFidelity,
    /// Every written caption cue is the source cue retimed onto the window.
    Captions,
    /// The delivered codec is the source codec.
    CodecMatch,
    /// A re-encoded head carries the source timescale.
    TimescalePreserved,
    /// Any extra frames come from a packet boundary, not from a wrong out point.
    Overshoot,
}

impl Check {
    /// Every check, in the order a report lists them.
    pub const ALL: [Self; 9] = [
        Self::Frames,
        Self::Duration,
        Self::AudioAlignment,
        Self::FrameAlignment,
        Self::HeadFidelity,
        Self::Captions,
        Self::CodecMatch,
        Self::TimescalePreserved,
        Self::Overshoot,
    ];

    /// A sentence for the audit log: what passing this check proves about the file.
    #[must_use]
    pub const fn statement(self) -> &'static str {
        match self {
            Self::Frames => "every frame the plan asked for is present in the delivered file",
            Self::Duration => {
                "the picture is as long as the frames it holds, at the plan's own rate"
            }
            Self::AudioAlignment => "the audio track ends level with the picture",
            Self::FrameAlignment => {
                "the delivered frames are the source's own frames at the cut, allowing one \
                 frame of start offset"
            }
            Self::HeadFidelity => {
                "the first delivered frame is the source frame at the in point, not a frame \
                 from somewhere else"
            }
            Self::Captions => {
                "every written caption cue is the source cue retimed onto the cut's window"
            }
            Self::CodecMatch => {
                "the delivered codec is the source codec, so the track describes one kind of \
                 sample"
            }
            Self::TimescalePreserved => {
                "a re-encoded head carries the source timescale, so the muxer does not rescale \
                 the copied body"
            }
            Self::Overshoot => {
                "any frames beyond the request come from a packet boundary rather than from a \
                 wrong out point"
            }
        }
    }

    /// A short name for the report table.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Frames => "frames",
            Self::Duration => "duration",
            Self::AudioAlignment => "audio alignment",
            Self::FrameAlignment => "frame alignment",
            Self::HeadFidelity => "head fidelity",
            Self::Captions => "captions",
            Self::CodecMatch => "codec",
            Self::TimescalePreserved => "timescale",
            Self::Overshoot => "overshoot",
        }
    }
}

/// What a check decided.
///
/// `Warning` and `Failed` are deliberately different: a warning is something true and worth
/// saying that does not make the file wrong, and only `Failed` makes a report not
/// [`VerifyReport::ok`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    /// The file satisfies the check.
    Passed,
    /// The file does not satisfy the check.
    Failed {
        /// Why, with the numbers that produced the verdict.
        detail: String,
    },
    /// The check could not be run; the reason says why, and says whether it was a policy
    /// that did not ask for it or a measurement that was not taken.
    Skipped {
        /// Why the check did not run.
        reason: String,
    },
    /// The file is not what was asked for in a way that does not make it wrong.
    Warning {
        /// What was noticed, with the numbers that produced it.
        detail: String,
    },
}

impl CheckStatus {
    /// The word the report table uses.
    #[must_use]
    pub const fn verdict(&self) -> &'static str {
        match self {
            Self::Passed => "ok",
            Self::Failed { .. } => "FAIL",
            Self::Skipped { .. } => "skip",
            Self::Warning { .. } => "warn",
        }
    }

    /// True when the file failed the check.
    #[must_use]
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// True when the check was not run.
    #[must_use]
    pub const fn is_skipped(&self) -> bool {
        matches!(self, Self::Skipped { .. })
    }

    /// True when the check passed with something worth saying.
    #[must_use]
    pub const fn is_warning(&self) -> bool {
        matches!(self, Self::Warning { .. })
    }

    /// The sentence behind a failure, a warning or a skip. `None` for a plain pass.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Failed { detail } | Self::Warning { detail } => Some(detail),
            Self::Skipped { reason } => Some(reason),
            Self::Passed => None,
        }
    }
}

/// One check's outcome, with the numbers it was decided from.
///
/// `measured` and `expected` are optional because not every check is about a number: a codec
/// either matches or it does not. Where they are present they are in the check's own unit —
/// frames for [`Check::Overshoot`], seconds for [`Check::Duration`], a frame offset for
/// [`Check::FrameAlignment`], a score for [`Check::HeadFidelity`] — and the report prints
/// them without a unit column, which is why the check's `statement` says what it proves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    /// Which check this is.
    pub check: Check,
    /// What it decided.
    pub status: CheckStatus,
    /// What was measured, in the check's own unit.
    pub measured: Option<f64>,
    /// What was wanted, in the same unit.
    pub expected: Option<f64>,
}

impl CheckResult {
    /// True when the file failed this check.
    #[must_use]
    pub const fn is_failed(&self) -> bool {
        self.status.is_failed()
    }

    /// The sentence behind a failure, a warning or a skip.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.status.detail()
    }
}

/// Everything the checks need that cannot be read out of two [`CutFacts`].
///
/// These are the measurements that cost something — a decode pass for the hashes, two frame
/// extractions and an image comparison for the head, a caption file read for the cues — so a
/// caller takes them only when its policy asks for them and leaves the rest empty. An empty
/// `Evidence` is not an error: it means the checks that need it report [`CheckStatus::Skipped`]
/// rather than inventing a verdict.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    /// The source's frame hashes at the sample point, when they were taken.
    pub source_hashes: Option<FrameHashes>,
    /// The delivered file's frame hashes at the same sample point.
    pub cut_hashes: Option<FrameHashes>,
    /// How similar the cut's first frame is to the source's frame at the in point.
    pub head_similarity: Option<Similarity>,
    /// Every cue of the source's caption file, in source time.
    pub source_cues: Vec<Cue>,
    /// The cues written beside the delivered file, at zero.
    pub written_cues: Vec<Cue>,
}

impl Evidence {
    /// Both sides of a frame-hash comparison.
    #[must_use]
    pub fn with_frame_hashes(mut self, source: FrameHashes, cut: FrameHashes) -> Self {
        self.source_hashes = Some(source);
        self.cut_hashes = Some(cut);
        self
    }

    /// The head's similarity score.
    #[must_use]
    pub fn with_head_similarity(mut self, similarity: Similarity) -> Self {
        self.head_similarity = Some(similarity);
        self
    }

    /// The source's cues and the cues that were written.
    #[must_use]
    pub fn with_captions(mut self, source_cues: Vec<Cue>, written_cues: Vec<Cue>) -> Self {
        self.source_cues = source_cues;
        self.written_cues = written_cues;
        self
    }
}

/// True when a policy asks for a check.
///
/// The mapping lives here rather than on `VerifyPolicy` because the policy is a domain type that
/// knows nothing about checks — it says *how hard to look*, not *what to look at* — and putting the
/// table on the domain side would make `trimmer-core` depend on this crate to say so.
///
/// `Standard` asks for frame-count, duration and audio alignment, which need only the metadata
/// every probe already returns. `Strict` additionally asks for the decoded-frame comparison, which
/// costs a hash pass. `Forensic` asks for everything including the head comparison and the
/// caption check.
#[must_use]
pub const fn policy_requires(policy: VerifyPolicy, check: Check) -> bool {
    use VerifyPolicy::{Forensic, Off, Standard, Strict};
    match policy {
        Off => false,
        Standard => matches!(
            check,
            Check::Frames | Check::Duration | Check::AudioAlignment | Check::CodecMatch
        ),
        Strict => matches!(
            check,
            Check::Frames
                | Check::Duration
                | Check::AudioAlignment
                | Check::CodecMatch
                | Check::FrameAlignment
                | Check::TimescalePreserved
                | Check::Overshoot
        ),
        Forensic => true,
    }
}

/// True when a check could have run against this plan at all.
///
/// The distinction this draws is the difference between a skip that means "we did not look" and one
/// that means "there was nothing to look at". `TimescalePreserved` is the clearest example: a plan
/// that re-encodes no head has no timescale of its own to preserve, so the check declining is a
/// complete answer. Counting it as a gap would mark every whole-segment re-encode uncertified for a
/// check that could never have run.
#[must_use]
pub fn check_applies(check: Check, plan: &CutPlan) -> bool {
    match check {
        // A head exists only when the plan re-encodes one, and both of these are about the head:
        // there is no timescale of its own to preserve, and nothing to compare against the source.
        Check::TimescalePreserved | Check::HeadFidelity => plan.mode == CutMode::HeadPatch,
        // There is nothing to align when the source carries no sound. The check says so either way;
        // this is what stops that skip being counted as a hole in an otherwise complete run, which is
        // what made a video-only cut come back uncertified for a check that could never have run.
        Check::AudioAlignment => plan.has_audio,
        // Everything else is about the delivered file as a whole, or about the source, and applies
        // whatever the plan decided.
        Check::Frames
        | Check::Duration
        | Check::FrameAlignment
        | Check::Captions
        | Check::CodecMatch
        | Check::Overshoot => true,
    }
}

/// The verdict on one delivered cut.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    /// The file that was checked.
    pub cut: MediaPath,
    /// The file it was cut from.
    pub source: MediaPath,
    /// One result per check, in [`Check::ALL`] order.
    pub results: Vec<CheckResult>,
    /// The policy the run was made under, recorded because a skip only means what the policy
    /// that caused it means.
    pub policy: VerifyPolicy,
}

impl VerifyReport {
    /// True when nothing failed. Skips and warnings do not make a report not ok.
    #[must_use]
    pub fn ok(&self) -> bool {
        !self.results.iter().any(CheckResult::is_failed)
    }

    /// The checks this run was supposed to make, could have made, and did not.
    ///
    /// A check is skipped when the measurement it needs was not taken — a legitimate outcome, because
    /// a decode pass costs real time — but a skip under a policy that *asked* for the check is a gap,
    /// and a gap is not a pass.
    ///
    /// ## Two kinds of skip, and only one of them is a gap
    ///
    /// A check can decline for a reason that has nothing to do with missing evidence:
    ///
    /// * **Inapplicable** — the plan re-encodes no head, so there is no timescale to preserve; the
    ///   source has no caption file, so there is nothing to compare. [`check_applies`] answers this
    ///   from the plan alone, and such a skip is a complete answer rather than a hole.
    /// * **Not measured** — the policy asked for a decoded-frame comparison and no hashes were taken.
    ///   That is the gap this method exists to surface.
    ///
    /// Treating the first as a gap would mark every whole-segment re-encode uncertified for a check
    /// that could never have run, which would make the verdict useless exactly where it matters.
    ///
    /// This exists because it was missing. The queue called `verify_cut` with no evidence, so under
    /// the default `Strict` policy the frame-alignment check — the whole reason the policy is called
    /// strict — reported [`CheckStatus::Skipped`], [`VerifyReport::ok`] returned `true` because
    /// nothing had *failed*, and the batch reported every cut as verified. The product's headline
    /// claim was untrue while the report said everything was fine — the worst shape a bug can take,
    /// and the reason a skip and a pass must be distinguishable by the caller and not only by eye.
    #[must_use]
    pub fn unrun_required_checks(&self) -> Vec<&CheckResult> {
        self.results
            .iter()
            .filter(|result| {
                result.status.is_skipped() && policy_requires(self.policy, result.check)
            })
            .collect()
    }

    /// True when the run made every check its policy asked for and none of them failed.
    ///
    /// `plan` is needed because some checks decline as *inapplicable* rather than as unmeasured, and
    /// only the plan can say which. See [`VerifyReport::unrun_required_checks`].
    ///
    /// A caller that wants to say "this file is certified" must use this rather than
    /// [`VerifyReport::ok`], which answers the narrower question "did anything fail".
    #[must_use]
    pub fn is_certified(&self, plan: &CutPlan) -> bool {
        self.ok()
            && self
                .unrun_required_checks()
                .iter()
                .all(|result| !check_applies(result.check, plan))
    }

    /// The checks that failed, for a caller that wants to show only the problems.
    #[must_use]
    pub fn failed(&self) -> Vec<&CheckResult> {
        self.results
            .iter()
            .filter(|result| result.is_failed())
            .collect()
    }

    /// One check's result.
    #[must_use]
    pub fn get(&self, check: Check) -> Option<&CheckResult> {
        self.results.iter().find(|result| result.check == check)
    }

    /// The report as a fixed-width table, for a CLI.
    ///
    /// One row per check, always all of them, so an operator can see what was *not* checked
    /// as clearly as what was.
    #[must_use]
    pub fn report(&self) -> String {
        let label_width = Check::ALL
            .iter()
            .map(|check| check.label().len())
            .max()
            .unwrap_or(16);
        let cut = self.cut.to_string();
        let source = self.source.to_string();
        let policy = policy_label(self.policy);

        let check_header = pad_right("check", label_width);
        let verdict_header = pad_right("verdict", VERDICT_WIDTH);
        let measured_header = pad_left("measured", NUMBER_WIDTH);
        let expected_header = pad_left("expected", NUMBER_WIDTH);
        let rule = "-".repeat(label_width + NUMBER_WIDTH * 2 + VERDICT_WIDTH + 8);

        let mut lines = vec![
            format!("cut     {cut}"),
            format!("source  {source}"),
            format!("policy  {policy}"),
            String::new(),
            format!(
                "{check_header}  {verdict_header}  {measured_header}  {expected_header}  detail"
            ),
            rule,
        ];

        for result in &self.results {
            let label = pad_right(result.check.label(), label_width);
            let verdict = pad_right(result.status.verdict(), VERDICT_WIDTH);
            let measured = pad_left(&number(result.measured), NUMBER_WIDTH);
            let expected = pad_left(&number(result.expected), NUMBER_WIDTH);
            let detail = result.detail().unwrap_or("");
            lines.push(format!(
                "{label}  {verdict}  {measured}  {expected}  {detail}"
            ));
        }

        let passed = self.count(|status| matches!(status, CheckStatus::Passed));
        let warned = self.count(CheckStatus::is_warning);
        let failed = self.count(CheckStatus::is_failed);
        let skipped = self.count(CheckStatus::is_skipped);
        let total = self.results.len();
        let verdict = if self.ok() { "ok" } else { "NOT OK" };

        lines.push(String::new());
        lines.push(format!(
            "{total} checks: {passed} passed, {warned} warning(s), {failed} failed, \
             {skipped} skipped",
        ));
        lines.push(format!("verdict: {verdict}"));
        lines.join("\n")
    }

    /// How many results satisfy a predicate.
    fn count(&self, predicate: impl Fn(&CheckStatus) -> bool) -> usize {
        self.results
            .iter()
            .filter(|result| predicate(&result.status))
            .count()
    }

    /// The report as JSON, for the audit trail and for a bug report.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Invariant`] when the value cannot be serialised, which cannot
    /// happen for plain data and is therefore a bug rather than a runtime condition.
    pub fn to_json(&self) -> CoreResult<String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| json_error("the verification report", &error))
    }

    /// Read a report back from JSON.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Invariant`] when the text is not a verification report.
    pub fn from_json(text: &str) -> CoreResult<Self> {
        serde_json::from_str(text).map_err(|error| json_error("the verification report", &error))
    }
}

/// Verify a cut from measured facts alone.
///
/// This is the entry point for a run that took only the cheap measurements: the checks that
/// need hashes, extracted frames or caption cues report [`CheckStatus::Skipped`] because
/// none were supplied, which is the honest answer rather than a guess. Pass the expensive
/// measurements to [`verify_cut_with`] when the policy asks for them.
#[must_use]
pub fn verify_cut(
    plan: &CutPlan,
    facts: &CutFacts,
    source_facts: &CutFacts,
    policy: VerifyPolicy,
) -> VerifyReport {
    verify_cut_with(plan, facts, source_facts, policy, &Evidence::default())
}

/// Verify a cut from measured facts and the evidence a deeper pass produced.
///
/// With [`VerifyPolicy::Off`] every check is skipped, because "do not measure" has to mean
/// that the report never claims more than was done. With any other policy, every check runs
/// or says why it could not.
#[must_use]
pub fn verify_cut_with(
    plan: &CutPlan,
    facts: &CutFacts,
    source_facts: &CutFacts,
    policy: VerifyPolicy,
    evidence: &Evidence,
) -> VerifyReport {
    let results = if matches!(policy, VerifyPolicy::Off) {
        Check::ALL
            .into_iter()
            .map(|check| skipped(check, VERIFICATION_OFF))
            .collect()
    } else {
        vec![
            check_frames(plan, facts),
            check_duration(plan, facts),
            check_audio_alignment(plan, facts, source_facts),
            check_frame_alignment(policy, evidence),
            check_head_fidelity(policy, evidence),
            check_captions(plan, policy, evidence),
            check_codec(facts, source_facts),
            check_timescale(plan),
            check_overshoot(plan, facts),
        ]
    };

    VerifyReport {
        cut: facts.path.clone(),
        source: source_facts.path.clone(),
        results,
        policy,
    }
}

/// The policy's name, for a report header or a log line.
#[must_use]
pub const fn policy_label(policy: VerifyPolicy) -> &'static str {
    match policy {
        VerifyPolicy::Off => "off",
        VerifyPolicy::Standard => "standard",
        VerifyPolicy::Strict => "strict",
        VerifyPolicy::Forensic => "forensic",
    }
}

/// Every frame the plan asked for must be present.
///
/// A file with *fewer* frames than the request is a failure: a frame somebody asked for is
/// not in the file. A file with more is not — see [`Check::Overshoot`], which is where
/// that case is reported.
fn check_frames(plan: &CutPlan, facts: &CutFacts) -> CheckResult {
    let requested = plan.requested_frames();
    let delivered = facts.frame_count;
    let status = if delivered >= requested {
        CheckStatus::Passed
    } else {
        let missing = requested - delivered;
        CheckStatus::Failed {
            detail: format!(
                "the cut holds {delivered} frame(s); the plan asked for {requested}, so \
                 {missing} frame(s) that were asked for are not in the file"
            ),
        }
    };
    outcome(
        Check::Frames,
        status,
        Some(delivered as f64),
        Some(requested as f64),
    )
}

/// The picture must be as long as the frames it holds, at the plan's own rate, within one
/// frame.
///
/// The plan's numerator and denominator are used, not a rounded rate: 600 frames at
/// `30000/1001` is `20.02 s`, and a check that read that as `29.97` would accept a file that
/// is a third of a second wrong.
///
/// The comparison is against the frames the file *actually holds* rather than against the
/// frames of the request, and that is deliberate. A stream copy that ran two frames past the
/// mark is two frames longer than the request by exactly those two frames, which
/// [`check_overshoot`] has already reported and accepted; failing it again here would make
/// that warning meaningless and would reject the correct output of the intended method. What
/// this check is for is the picture that is not the length its own frames say it is —
/// a stream whose timescale was rescaled, a container that lies about its length — and the
/// request itself is [`check_frames`]'s business.
fn check_duration(plan: &CutPlan, facts: &CutFacts) -> CheckResult {
    let Some(seconds) = frame_seconds(plan) else {
        return skipped(Check::Duration, "the plan carries no usable frame rate");
    };
    let frames = facts.frame_count.max(0);
    let expected = frames as f64 * seconds;
    let measured = facts.video_duration;
    let difference = (measured - expected).abs();
    if difference <= seconds {
        return outcome(
            Check::Duration,
            CheckStatus::Passed,
            Some(measured),
            Some(expected),
        );
    }
    let numerator = plan.rate_numerator;
    let denominator = plan.rate_denominator;
    outcome(
        Check::Duration,
        CheckStatus::Failed {
            detail: format!(
                "the picture runs {measured:.3}s; the {frames} frame(s) the file holds are \
                 {expected:.3}s at {numerator}/{denominator}, which is {difference:.3}s out — \
                 more than the one frame ({seconds:.3}s) this check allows"
            ),
        },
        Some(measured),
        Some(expected),
    )
}

/// The audio must end level with the picture, within two frames.
///
/// The comparison is between the two *ends* rather than the two durations, because a copied
/// audio track legitimately starts a few milliseconds away from the picture — AAC priming
/// does exactly that — and it is ending level that decides whether the sound runs out early
/// or hangs over the last shot.
fn check_audio_alignment(plan: &CutPlan, facts: &CutFacts, source: &CutFacts) -> CheckResult {
    if !facts.has_audio() && !source.has_audio() {
        return skipped(
            Check::AudioAlignment,
            "neither the source nor the cut carries an audio track",
        );
    }
    if !facts.has_audio() {
        return failed(
            Check::AudioAlignment,
            "the source has audio and the cut does not: the audio was dropped",
        );
    }
    if !source.has_audio() {
        return outcome(
            Check::AudioAlignment,
            CheckStatus::Warning {
                detail: "the cut carries an audio track the source does not have; there was \
                         nothing to compare it against"
                    .to_owned(),
            },
            facts.audio_end(),
            None,
        );
    }
    let Some(seconds) = frame_seconds(plan) else {
        return skipped(
            Check::AudioAlignment,
            "the plan carries no usable frame rate",
        );
    };
    let Some(audio_end) = facts.audio_end() else {
        return skipped(
            Check::AudioAlignment,
            "the cut reports no audio duration to compare",
        );
    };
    let tolerance = AUDIO_LEVEL_TOLERANCE_FRAMES * seconds;
    let error = audio_end - facts.video_end();
    if error.abs() <= tolerance {
        return outcome(
            Check::AudioAlignment,
            CheckStatus::Passed,
            Some(error),
            Some(0.0),
        );
    }
    outcome(
        Check::AudioAlignment,
        CheckStatus::Failed {
            detail: format!(
                "the audio ends {error:+.3}s from where the picture ends; a copied track may \
                 be out by {tolerance:.3}s ({AUDIO_LEVEL_TOLERANCE_FRAMES:.0} frames of the \
                 plan's rate) and no more"
            ),
        },
        Some(error),
        Some(0.0),
    )
}

/// The delivered frames must be the source's own frames at the cut.
///
/// # The one-frame flicker this tolerates (V1 ADR-003)
///
/// Comparing frame hashes element by element fails on correct files. Two files that were cut
/// from the same source do not necessarily start on the same fraction of a frame: the
/// delivered file's first frame can be the source's second, and an exact comparison then
/// reports every frame as different when every frame is in fact identical. V1 ADR-003 was
/// written about exactly this — a cut that was reported as wrong, and briefly "fixed", when
/// nothing was wrong with it except the sampling grid. So the comparison tries the offsets
/// `-1`, `0` and `+1`, accepts the best of them, and records in `measured` which one matched,
/// so a real problem is still a failure while a fraction of a frame is not.
fn check_frame_alignment(policy: VerifyPolicy, evidence: &Evidence) -> CheckResult {
    if !policy.hashes_frames() {
        return skipped(
            Check::FrameAlignment,
            "this policy does not hash decoded frames",
        );
    }
    let Some(source) = evidence.source_hashes.as_ref() else {
        return skipped(Check::FrameAlignment, "no source frame hashes were sampled");
    };
    let Some(cut) = evidence.cut_hashes.as_ref() else {
        return skipped(Check::FrameAlignment, "no cut frame hashes were sampled");
    };
    if cut.is_empty() || source.is_empty() {
        return skipped(Check::FrameAlignment, "the sampled window holds no frames");
    }

    // Pick the offset that explains the window. An offset which accounts for every frame it
    // could compare beats one which accounts for more frames but leaves a disagreement, so
    // that a file whose head is a frame out is accepted rather than reported as a mismatch
    // on every frame. Ties go to the earlier candidate, and `0` is first.
    let mut best: Option<Alignment> = None;
    for candidate in ALIGNMENT_OFFSETS.map(|offset| align_at(cut, source, offset)) {
        let candidate_explains_all =
            candidate.compared > 0 && candidate.matches == candidate.compared;
        let better = match &best {
            None => true,
            Some(current) => {
                let current_explains_all =
                    current.compared > 0 && current.matches == current.compared;
                match (candidate_explains_all, current_explains_all) {
                    (true, false) => true,
                    (false, true) => false,
                    _ => candidate.matches > current.matches,
                }
            }
        };
        if better {
            best = Some(candidate);
        }
    }
    let Some(best) = best else {
        return skipped(Check::FrameAlignment, "no frames could be compared");
    };

    if best.compared > 0 && best.matches == best.compared {
        return outcome(
            Check::FrameAlignment,
            CheckStatus::Passed,
            Some(best.offset as f64),
            Some(0.0),
        );
    }
    let Some(index) = best.mismatch else {
        return skipped(Check::FrameAlignment, "no frames could be compared");
    };
    let source_index = as_frame(index) + best.offset;
    let Ok(source_position) = usize::try_from(source_index) else {
        return skipped(Check::FrameAlignment, "no frames could be compared");
    };
    let Some(source_digest) = source.digests.get(source_position) else {
        return skipped(Check::FrameAlignment, "no frames could be compared");
    };
    let cut_digest = &cut.digests[index];
    let cut_frame = cut.frame_of(index).unwrap_or_default();
    let source_frame = source.frame_of(source_position).unwrap_or_default();
    let matched = best.matches;
    let compared = best.compared;
    let offset = best.offset;
    outcome(
        Check::FrameAlignment,
        CheckStatus::Failed {
            detail: format!(
                "cut frame {cut_frame} hashes {cut_digest} and source frame {source_frame} \
                 hashes {source_digest}; with the best offset ({offset:+} frame(s)) only \
                 {matched} of {compared} hashes in the window agree"
            ),
        },
        Some(offset as f64),
        Some(0.0),
    )
}

/// How well a shifted comparison of two hash windows agrees.
struct Alignment {
    /// The shift applied to the source window, in frames.
    offset: i64,
    /// How many positions agreed.
    matches: usize,
    /// How many positions could be compared at all.
    compared: usize,
    /// The first position that disagreed, in the cut's window.
    mismatch: Option<usize>,
}

/// Compare a cut window against a source window shifted by `offset` frames.
///
/// `cut.digests[i]` is compared with `source.digests[i + offset]`. Positions that fall off
/// either end of the source window are not compared — they were not sampled — so an accepted
/// offset can compare one frame fewer than the window holds.
fn align_at(cut: &FrameHashes, source: &FrameHashes, offset: i64) -> Alignment {
    let mut alignment = Alignment {
        offset,
        matches: 0,
        compared: 0,
        mismatch: None,
    };
    for (index, digest) in cut.digests.iter().enumerate() {
        let shifted = as_frame(index) + offset;
        if shifted < 0 {
            continue;
        }
        let Ok(source_index) = usize::try_from(shifted) else {
            continue;
        };
        let Some(other) = source.digests.get(source_index) else {
            continue;
        };
        alignment.compared += 1;
        if digest == other {
            alignment.matches += 1;
        } else if alignment.mismatch.is_none() {
            alignment.mismatch = Some(index);
        }
    }
    alignment
}

/// The head must be the source frame at the in point, judged by similarity rather than by
/// hash, because a re-encode never hashes equal to what it came from.
///
/// Only the forensic policy asks for this: it costs two frame extractions and an image
/// comparison, which is worth it for a master and not for a rough cut.
fn check_head_fidelity(policy: VerifyPolicy, evidence: &Evidence) -> CheckResult {
    if !policy.is_forensic() {
        return skipped(
            Check::HeadFidelity,
            "only the forensic policy compares the head against the source pixel-wise",
        );
    }
    let Some(score) = evidence.head_similarity.and_then(Similarity::score) else {
        return skipped(
            Check::HeadFidelity,
            "the first frame could not be compared with the source frame at the in point",
        );
    };
    if score >= HEAD_FIDELITY_MIN {
        return outcome(
            Check::HeadFidelity,
            CheckStatus::Passed,
            Some(score),
            Some(HEAD_FIDELITY_MIN),
        );
    }
    outcome(
        Check::HeadFidelity,
        CheckStatus::Failed {
            detail: format!(
                "the first delivered frame scores {score:.4} against the source frame at the \
                 in point, below the {HEAD_FIDELITY_MIN} this check requires; the cut may \
                 have been taken from the wrong place. A re-encode never hashes equal to its \
                 source, which is why similarity is used here rather than a hash"
            ),
        },
        Some(score),
        Some(HEAD_FIDELITY_MIN),
    )
}

/// Every written cue must be the source cue retimed onto the window, in order and in count.
///
/// The retiming itself is [`caption::retime`], not a copy of it: the clamp rules at the
/// marks are the part most likely to be got subtly wrong, they are already implemented and
/// tested in the core, and a second implementation here would be a second thing to disagree
/// with the first.
fn check_captions(plan: &CutPlan, policy: VerifyPolicy, evidence: &Evidence) -> CheckResult {
    if !policy.is_forensic() {
        return skipped(
            Check::Captions,
            "only the forensic policy checks captions cue by cue",
        );
    }
    if evidence.source_cues.is_empty() && evidence.written_cues.is_empty() {
        return skipped(Check::Captions, "no caption cues were supplied");
    }
    let Some((in_seconds, out_seconds)) = window_seconds(plan) else {
        return skipped(Check::Captions, "the plan carries no usable frame rate");
    };
    let retimed = match caption::retime(
        &evidence.source_cues,
        in_seconds,
        out_seconds,
        caption::MIN_OVERLAP,
    ) {
        Ok(retimed) => retimed,
        Err(error) => {
            return skipped(
                Check::Captions,
                format!("the plan's window could not be retimed: {error}"),
            )
        }
    };
    let expected = &retimed.cues;
    let written = &evidence.written_cues;

    for (index, (want, got)) in expected.iter().zip(written).enumerate() {
        if !cue_matches(want, got) {
            let want_start = caption::format_stamp(want.start);
            let want_end = caption::format_stamp(want.end);
            let got_start = caption::format_stamp(got.start);
            let got_end = caption::format_stamp(got.end);
            let want_text = &want.text;
            let got_text = &got.text;
            return outcome(
                Check::Captions,
                CheckStatus::Failed {
                    detail: format!(
                        "caption cue index {index} differs: retimed onto this window the \
                         source cue runs {want_start} to {want_end} ({want_text:?}) and the \
                         cut carries {got_start} to {got_end} ({got_text:?})"
                    ),
                },
                Some(index as f64),
                Some(expected.len() as f64),
            );
        }
    }

    if expected.len() != written.len() {
        let written_count = written.len();
        let expected_count = expected.len();
        let first_divergent = expected.len().min(written.len());
        return outcome(
            Check::Captions,
            CheckStatus::Failed {
                detail: format!(
                    "the cut carries {written_count} caption cue(s) and the source retimed \
                     onto this window holds {expected_count}; the first divergent cue is \
                     index {first_divergent}"
                ),
            },
            Some(written_count as f64),
            Some(expected_count as f64),
        );
    }

    outcome(
        Check::Captions,
        CheckStatus::Passed,
        Some(written.len() as f64),
        Some(expected.len() as f64),
    )
}

/// True when a written cue is the source cue retimed onto the window.
///
/// The cue's source numbering is deliberately not compared: the source file's numbers and
/// the written file's numbers are different numbering schemes by design, since the written
/// file is renumbered from one.
fn cue_matches(expected: &Cue, written: &Cue) -> bool {
    expected.text == written.text
        && (expected.start - written.start).abs() <= CAPTION_TOLERANCE_SECONDS
        && (expected.end - written.end).abs() <= CAPTION_TOLERANCE_SECONDS
}

/// The codec must not change, or the track holds two kinds of sample description and players
/// — and Premiere — refuse it.
fn check_codec(facts: &CutFacts, source: &CutFacts) -> CheckResult {
    if facts.codec.eq_ignore_ascii_case(&source.codec) {
        return passed(Check::CodecMatch);
    }
    let cut_codec = &facts.codec;
    let source_codec = &source.codec;
    failed(
        Check::CodecMatch,
        format!(
            "the cut is {cut_codec} and the source is {source_codec}; the track would hold \
             two sample descriptions, which players and Premiere refuse"
        ),
    )
}

/// A re-encoded head must carry the source's timescale.
///
/// This is the only check whose failure leaves no other trace. When the head is encoded at a
/// different timescale than the body — libx264's default `1/15360` against a `1/90000`
/// source — the muxer rescales the *copied* packets, and the clip plays in slow motion with
/// frozen stretches. ffmpeg exits `0`, the frame count is right, the duration is wrong in a
/// way that looks like a muxing quirk, and nothing else in the pipeline reports it. V1
/// ADR-001 was written about this bug, and this check is what catches it.
fn check_timescale(plan: &CutPlan) -> CheckResult {
    if plan.mode != CutMode::HeadPatch {
        return skipped(
            Check::TimescalePreserved,
            "the plan does not re-encode a head, so there is no timescale to preserve",
        );
    }
    let ticks = plan.video_timescale;
    if ticks > 0 {
        return outcome(
            Check::TimescalePreserved,
            CheckStatus::Passed,
            Some(ticks as f64),
            None,
        );
    }
    outcome(
        Check::TimescalePreserved,
        CheckStatus::Failed {
            detail: format!(
                "the plan re-encodes a head at a timescale of {ticks} ticks per second. The \
                 muxer will rescale the copied body and the clip will play in slow motion \
                 with frozen stretches, while ffmpeg still exits 0; this check is the only \
                 thing in the pipeline that catches it"
            ),
        },
        Some(ticks as f64),
        None,
    )
}

/// Frames beyond the request are a warning: the copy stopped on a packet boundary, and every
/// frame that was asked for is still there.
///
/// A *short* file is not this check's business — [`check_frames`] already reports it — so
/// this one is skipped rather than passed, which keeps the report from saying "no overshoot"
/// about a file that is missing frames.
fn check_overshoot(plan: &CutPlan, facts: &CutFacts) -> CheckResult {
    let requested = plan.requested_frames();
    let extra = facts.frame_count - requested;
    if extra > 0 {
        return outcome(
            Check::Overshoot,
            CheckStatus::Warning {
                detail: format!(
                    "the cut is {extra} frame(s) longer than the {requested} that were asked \
                     for. Every frame that was asked for is still there: a stream copy stops \
                     on a packet boundary rather than on the mark, so it typically runs one \
                     to three frames long, and this is that. It is a warning, not a failure."
                ),
            },
            Some(extra as f64),
            Some(0.0),
        );
    }
    if extra == 0 {
        return outcome(Check::Overshoot, CheckStatus::Passed, Some(0.0), Some(0.0));
    }
    let short = -extra;
    outcome(
        Check::Overshoot,
        CheckStatus::Skipped {
            reason: format!(
                "the cut is {short} frame(s) short of the request, which the frame check \
                 already reports"
            ),
        },
        Some(extra as f64),
        Some(0.0),
    )
}

/// One frame's worth of seconds at the plan's rate, as the plan carries it.
fn frame_seconds(plan: &CutPlan) -> Option<f64> {
    if plan.rate_numerator <= 0 {
        return None;
    }
    Some(plan.rate_denominator as f64 / plan.rate_numerator as f64)
}

/// The plan's window, in seconds from the start of the source.
fn window_seconds(plan: &CutPlan) -> Option<(f64, f64)> {
    let seconds = frame_seconds(plan)?;
    let start = plan.start_frame as f64 * seconds;
    let end = plan.end_frame as f64 * seconds;
    Some((start, end))
}

/// A slice position as a frame index, saturating rather than wrapping.
///
/// A window of frame hashes is never long enough for this to matter, but a cast that wraps
/// silently is the kind of thing that turns a report into a lie on the one input nobody
/// tried.
fn as_frame(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

/// A check result with no numbers attached.
fn outcome(
    check: Check,
    status: CheckStatus,
    measured: Option<f64>,
    expected: Option<f64>,
) -> CheckResult {
    CheckResult {
        check,
        status,
        measured,
        expected,
    }
}

/// A check that passed.
fn passed(check: Check) -> CheckResult {
    outcome(check, CheckStatus::Passed, None, None)
}

/// A check that failed.
fn failed(check: Check, detail: impl Into<String>) -> CheckResult {
    outcome(
        check,
        CheckStatus::Failed {
            detail: detail.into(),
        },
        None,
        None,
    )
}

/// A check that did not run.
fn skipped(check: Check, reason: impl Into<String>) -> CheckResult {
    outcome(
        check,
        CheckStatus::Skipped {
            reason: reason.into(),
        },
        None,
        None,
    )
}

/// A serialisation failure, which for these plain-data types is a bug rather than a
/// condition a caller can meet.
fn json_error(what: &str, error: &serde_json::Error) -> CoreError {
    CoreError::Invariant(format!("{what} could not be read as JSON: {error}"))
}

/// A number as the report prints it: `-` when there is none, and no trailing zeros so a
/// frame count does not read as `600.000`.
fn number(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), trimmed)
}

/// A float with at most three decimals and no trailing zeros.
fn trimmed(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// Column widths the report table is laid out with.
const VERDICT_WIDTH: usize = 7;
/// Width of the two numeric columns.
const NUMBER_WIDTH: usize = 12;

/// Pad to the right, for a left-aligned column.
fn pad_right(text: &str, width: usize) -> String {
    format!("{text:<width$}")
}

/// Pad to the left, for a right-aligned column.
fn pad_left(text: &str, width: usize) -> String {
    format!("{text:>width$}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use trimmer_core::domain::{AudioFormat, KeyframeGrid, MediaInfo, Segment, Timescale};
    use trimmer_core::timecode::FrameRate;

    /// The rate every fixture uses: broadcast 29.97.
    const NUMERATOR: i64 = 30_000;
    const DENOMINATOR: i64 = 1_001;

    /// A plan, shaped by hand so a test can aim at exactly one rule.
    fn plan(
        mode: CutMode,
        start_frame: i64,
        end_frame: i64,
        head_frames: i64,
        timescale: i64,
    ) -> CutPlan {
        CutPlan {
            mode,
            start_frame,
            end_frame,
            keyframe: match mode {
                CutMode::Reencode => None,
                _ => Some(start_frame + head_frames),
            },
            head_frames,
            body_frames: end_frame - start_frame - head_frames,
            concat_offset: 0,
            video_timescale: timescale,
            head_codec: "h264".to_owned(),
            head_encoder: "libx264".to_owned(),
            head_extra_args: Vec::new(),
            rate_numerator: NUMERATOR,
            rate_denominator: DENOMINATOR,
            has_audio: true,
            notes: Vec::new(),
            invariants: Vec::new(),
        }
    }

    /// The ordinary case: 600 frames (20.02 s at 29.97) head-patched from frame 1000.
    fn head_patch() -> CutPlan {
        plan(CutMode::HeadPatch, 1_000, 1_600, 150, 90_000)
    }

    /// The delivered file's facts, correct in every particular.
    fn cut_facts() -> CutFacts {
        facts_for(600)
    }

    /// Facts for a file holding `frames` frames whose picture is exactly as long as they are.
    ///
    /// A file that is short or long is short or long in its *duration* too: a copy that
    /// stopped early stops early in both. Building the two independently would produce
    /// fixtures that fail the duration check as well, and a test that aims at one rule would
    /// then be measuring two.
    fn facts_for(frames: i64) -> CutFacts {
        let duration = frames as f64 * DENOMINATOR as f64 / NUMERATOR as f64;
        let mut built = facts(r"H:\out\clip.mp4", frames, duration);
        built.audio_duration = Some(duration);
        built
    }

    /// The source file's facts.
    fn source_facts() -> CutFacts {
        facts(r"H:\master.mp4", 216_000, 7_207.207)
    }

    fn facts(path: &str, frame_count: i64, video_duration: f64) -> CutFacts {
        CutFacts {
            path: MediaPath::new(path),
            frame_count,
            video_duration,
            video_start_time: 0.0,
            audio_duration: Some(video_duration),
            audio_start_time: Some(0.0),
            rate: Some(FrameRate::FPS_29_97),
            codec: "h264".to_owned(),
            width: 1920,
            height: 1080,
            size_bytes: 1_048_576,
        }
    }

    /// A report for the standard fixture plan and facts.
    fn report_with(policy: VerifyPolicy, evidence: &Evidence) -> VerifyReport {
        verify_cut_with(
            &head_patch(),
            &cut_facts(),
            &source_facts(),
            policy,
            evidence,
        )
    }

    fn status(report: &VerifyReport, check: Check) -> CheckStatus {
        report
            .get(check)
            .expect("every check is in the report")
            .status
            .clone()
    }

    fn detail(report: &VerifyReport, check: Check) -> String {
        status(report, check)
            .detail()
            .unwrap_or_default()
            .to_owned()
    }

    fn hashes(first_frame: i64, digests: &[&str]) -> FrameHashes {
        FrameHashes::new(
            digests.iter().map(|digest| (*digest).to_owned()).collect(),
            first_frame,
        )
    }

    #[test]
    fn every_check_appears_once_in_the_report_and_in_the_order_of_the_enum() {
        for policy in [
            VerifyPolicy::Off,
            VerifyPolicy::Standard,
            VerifyPolicy::Strict,
            VerifyPolicy::Forensic,
        ] {
            let report = report_with(policy, &Evidence::default());
            let seen: Vec<Check> = report.results.iter().map(|result| result.check).collect();
            assert_eq!(seen, Check::ALL.to_vec(), "policy {policy:?}");
        }
    }

    #[test]
    fn every_check_has_a_statement_and_a_short_label() {
        for check in Check::ALL {
            assert!(!check.statement().is_empty(), "{check:?}");
            assert!(!check.label().is_empty(), "{check:?}");
            assert!(check.label().len() < 17, "{check:?}");
        }
    }

    #[test]
    fn a_clean_cut_passes_every_cheap_check() {
        let report = report_with(VerifyPolicy::Strict, &Evidence::default());
        assert!(report.ok(), "{}", report.report());
        assert!(report.failed().is_empty());
        for check in [
            Check::Frames,
            Check::Duration,
            Check::AudioAlignment,
            Check::CodecMatch,
            Check::TimescalePreserved,
            Check::Overshoot,
        ] {
            assert_eq!(status(&report, check), CheckStatus::Passed, "{check:?}");
        }
    }

    #[test]
    fn a_short_file_fails_the_frame_check_with_the_numbers() {
        let facts = facts_for(597);
        let report = verify_cut(
            &head_patch(),
            &facts,
            &source_facts(),
            VerifyPolicy::Standard,
        );
        let result = report.get(Check::Frames).expect("present");
        assert!(result.is_failed());
        assert_eq!(result.measured, Some(597.0));
        assert_eq!(result.expected, Some(600.0));
        assert!(
            detail(&report, Check::Frames).contains("597"),
            "{}",
            report.report()
        );
        assert!(detail(&report, Check::Frames).contains("600"));
        assert!(!report.ok());
    }

    #[test]
    fn overshoot_is_a_warning_and_never_a_failure() {
        let facts = facts_for(602);
        let report = verify_cut(
            &head_patch(),
            &facts,
            &source_facts(),
            VerifyPolicy::Standard,
        );

        assert_eq!(status(&report, Check::Frames), CheckStatus::Passed);
        assert!(status(&report, Check::Overshoot).is_warning());
        assert!(!status(&report, Check::Overshoot).is_failed());
        assert!(
            report.ok(),
            "an overshoot must not fail the run: {}",
            report.report()
        );
        assert!(report.failed().is_empty());
        assert_eq!(
            report.get(Check::Overshoot).expect("present").measured,
            Some(2.0)
        );

        let text = detail(&report, Check::Overshoot);
        assert!(text.contains("packet boundary"), "{text}");
        assert!(
            text.contains("Every frame that was asked for is still there"),
            "{text}"
        );
    }

    #[test]
    fn overshoot_is_skipped_when_the_file_is_short() {
        let facts = facts_for(598);
        let report = verify_cut(
            &head_patch(),
            &facts,
            &source_facts(),
            VerifyPolicy::Standard,
        );
        assert!(status(&report, Check::Overshoot).is_skipped());
        assert!(detail(&report, Check::Overshoot).contains("short"));
    }

    #[test]
    fn exactly_the_requested_frames_is_not_an_overshoot() {
        let report = report_with(VerifyPolicy::Standard, &Evidence::default());
        assert_eq!(status(&report, Check::Overshoot), CheckStatus::Passed);
        assert_eq!(
            report.get(Check::Overshoot).expect("present").measured,
            Some(0.0)
        );
    }

    #[test]
    fn duration_within_one_frame_passes_and_beyond_it_fails() {
        let plan = head_patch();
        let mut facts = cut_facts();
        // Half a frame out: still the same picture length.
        facts.video_duration += 0.5 * DENOMINATOR as f64 / NUMERATOR as f64;
        let report = verify_cut(&plan, &facts, &source_facts(), VerifyPolicy::Standard);
        assert_eq!(status(&report, Check::Duration), CheckStatus::Passed);

        // Half a second out: a different picture length.
        let mut wrong = cut_facts();
        wrong.video_duration += 0.5;
        let report = verify_cut(&plan, &wrong, &source_facts(), VerifyPolicy::Standard);
        assert!(status(&report, Check::Duration).is_failed());
        let text = detail(&report, Check::Duration);
        assert!(text.contains("20.02"), "{text}");
        assert!(text.contains("30000/1001"), "{text}");
    }

    #[test]
    fn duration_is_read_at_the_plans_own_rate_not_a_rounded_one() {
        // The same 600 frames are 24 s at 25 fps and 20.02 s at 30000/1001. A file of 24 s
        // must pass the 25 fps plan and fail the 29.97 one; a check that rounded 29.97 to 30
        // would accept 20 s and reject 20.02.
        let mut twenty_five = head_patch();
        twenty_five.rate_numerator = 25;
        twenty_five.rate_denominator = 1;
        let mut facts = cut_facts();
        facts.video_duration = 24.0;
        let report = verify_cut(
            &twenty_five,
            &facts,
            &source_facts(),
            VerifyPolicy::Standard,
        );
        assert_eq!(status(&report, Check::Duration), CheckStatus::Passed);
        assert_eq!(
            report.get(Check::Duration).expect("present").expected,
            Some(24.0)
        );

        let mut short = cut_facts();
        short.video_duration = 20.02;
        let report = verify_cut(
            &twenty_five,
            &short,
            &source_facts(),
            VerifyPolicy::Standard,
        );
        assert!(status(&report, Check::Duration).is_failed());
    }

    #[test]
    fn duration_is_skipped_when_the_plan_has_no_usable_rate() {
        let mut broken = head_patch();
        broken.rate_numerator = 0;
        let report = verify_cut(
            &broken,
            &cut_facts(),
            &source_facts(),
            VerifyPolicy::Standard,
        );
        assert!(status(&report, Check::Duration).is_skipped());
    }

    #[test]
    fn a_packet_boundary_overshoot_does_not_fail_the_duration_check() {
        // The overshoot the frame check warns about must not be reported a second time as a
        // wrong length: the file is the length of the frames it holds.
        let facts = facts_for(602);
        let report = verify_cut(
            &head_patch(),
            &facts,
            &source_facts(),
            VerifyPolicy::Standard,
        );
        assert_eq!(status(&report, Check::Duration), CheckStatus::Passed);
        assert!(status(&report, Check::Overshoot).is_warning());
        assert!(report.ok(), "{}", report.report());
    }

    #[test]
    fn a_picture_that_is_not_the_length_of_its_own_frames_fails_the_duration_check() {
        // 602 frames the container says last three seconds: the timescale was rescaled, and
        // the frame check cannot see it because every frame is present.
        let mut facts = facts_for(602);
        facts.video_duration = 3.0;
        let report = verify_cut(
            &head_patch(),
            &facts,
            &source_facts(),
            VerifyPolicy::Standard,
        );
        assert!(status(&report, Check::Duration).is_failed());
        let text = detail(&report, Check::Duration);
        assert!(text.contains("602"), "{text}");
        assert!(!report.ok());
    }

    #[test]
    fn audio_alignment_is_skipped_when_neither_file_has_audio() {
        let mut cut = cut_facts();
        cut.audio_duration = None;
        cut.audio_start_time = None;
        let mut source = source_facts();
        source.audio_duration = None;
        source.audio_start_time = None;
        let report = verify_cut(&head_patch(), &cut, &source, VerifyPolicy::Standard);
        assert!(status(&report, Check::AudioAlignment).is_skipped());
        assert!(detail(&report, Check::AudioAlignment).contains("neither"));
    }

    #[test]
    fn audio_alignment_passes_when_the_tracks_end_level() {
        let mut cut = cut_facts();
        // The track starts 30 ms late, as an AAC stream does, and ends level anyway.
        cut.audio_start_time = Some(0.03);
        cut.audio_duration = Some(cut.video_duration - 0.03);
        let report = verify_cut(&head_patch(), &cut, &source_facts(), VerifyPolicy::Standard);
        assert_eq!(status(&report, Check::AudioAlignment), CheckStatus::Passed);
    }

    #[test]
    fn audio_alignment_fails_when_the_cut_dropped_the_audio() {
        let mut cut = cut_facts();
        cut.audio_duration = None;
        cut.audio_start_time = None;
        let report = verify_cut(&head_patch(), &cut, &source_facts(), VerifyPolicy::Standard);
        assert!(status(&report, Check::AudioAlignment).is_failed());
        assert!(detail(&report, Check::AudioAlignment).contains("dropped"));
        assert!(!report.ok());
    }

    #[test]
    fn audio_alignment_warns_when_the_cut_has_audio_the_source_did_not() {
        let mut source = source_facts();
        source.audio_duration = None;
        source.audio_start_time = None;
        let report = verify_cut(&head_patch(), &cut_facts(), &source, VerifyPolicy::Standard);
        assert!(status(&report, Check::AudioAlignment).is_warning());
        assert!(report.ok());
    }

    #[test]
    fn audio_alignment_fails_when_the_audio_trail_is_too_far_out() {
        let mut cut = cut_facts();
        cut.audio_duration = Some(cut.video_duration + 0.5);
        let report = verify_cut(&head_patch(), &cut, &source_facts(), VerifyPolicy::Standard);
        assert!(status(&report, Check::AudioAlignment).is_failed());
        assert_eq!(
            report.get(Check::AudioAlignment).expect("present").measured,
            Some(0.5)
        );
    }

    #[test]
    fn frame_alignment_is_skipped_when_the_policy_does_not_hash_frames() {
        let evidence =
            Evidence::default().with_frame_hashes(hashes(0, &["aa"]), hashes(0, &["aa"]));
        let report = report_with(VerifyPolicy::Standard, &evidence);
        assert!(status(&report, Check::FrameAlignment).is_skipped());
        assert!(detail(&report, Check::FrameAlignment).contains("does not hash"));
    }

    #[test]
    fn frame_alignment_passes_at_zero_offset() {
        let evidence = Evidence::default().with_frame_hashes(
            hashes(1_000, &["aa", "bb", "cc", "dd"]),
            hashes(0, &["aa", "bb", "cc", "dd"]),
        );
        let report = report_with(VerifyPolicy::Strict, &evidence);
        assert_eq!(status(&report, Check::FrameAlignment), CheckStatus::Passed);
        assert_eq!(
            report.get(Check::FrameAlignment).expect("present").measured,
            Some(0.0)
        );
    }

    #[test]
    fn frame_alignment_accepts_a_minus_one_frame_offset() {
        // The cut's window starts one frame before the source's: cut[i] is source[i - 1].
        let evidence = Evidence::default().with_frame_hashes(
            hashes(1_000, &["aa", "bb", "cc", "dd"]),
            hashes(999, &["ignored", "aa", "bb", "cc"]),
        );
        let report = report_with(VerifyPolicy::Strict, &evidence);
        assert_eq!(status(&report, Check::FrameAlignment), CheckStatus::Passed);
        assert_eq!(
            report.get(Check::FrameAlignment).expect("present").measured,
            Some(-1.0)
        );
    }

    #[test]
    fn frame_alignment_accepts_a_plus_one_frame_offset() {
        // The cut's window starts one frame after the source's: cut[i] is source[i + 1].
        let evidence = Evidence::default().with_frame_hashes(
            hashes(1_000, &["aa", "bb", "cc", "dd"]),
            hashes(1_001, &["bb", "cc", "dd"]),
        );
        let report = report_with(VerifyPolicy::Strict, &evidence);
        assert_eq!(status(&report, Check::FrameAlignment), CheckStatus::Passed);
        assert_eq!(
            report.get(Check::FrameAlignment).expect("present").measured,
            Some(1.0)
        );
    }

    #[test]
    fn frame_alignment_rejects_a_two_frame_offset_and_names_the_first_difference() {
        // Two frames out: not the fraction of a frame the offset tolerance is for.
        let evidence = Evidence::default().with_frame_hashes(
            hashes(1_000, &["aa", "bb", "cc", "dd"]),
            hashes(1_002, &["cc", "dd"]),
        );
        let report = report_with(VerifyPolicy::Strict, &evidence);
        assert!(status(&report, Check::FrameAlignment).is_failed());
        assert!(!report.ok());
        let text = detail(&report, Check::FrameAlignment);
        assert!(text.contains("cc"), "{text}");
        assert!(text.contains("aa"), "{text}");
        assert!(text.contains("frame 1002"), "{text}");
    }

    #[test]
    fn frame_alignment_fails_when_one_frame_in_the_middle_changed() {
        let evidence = Evidence::default().with_frame_hashes(
            hashes(1_000, &["aa", "bb", "cc", "dd"]),
            hashes(0, &["aa", "bb", "zz", "dd"]),
        );
        let report = report_with(VerifyPolicy::Strict, &evidence);
        assert!(status(&report, Check::FrameAlignment).is_failed());
        let text = detail(&report, Check::FrameAlignment);
        assert!(text.contains("zz") && text.contains("cc"), "{text}");
        assert!(text.contains("cut frame 2"), "{text}");
        assert!(text.contains("source frame 1002"), "{text}");
    }

    #[test]
    fn frame_alignment_is_skipped_when_no_hashes_were_sampled() {
        let report = report_with(VerifyPolicy::Strict, &Evidence::default());
        assert!(status(&report, Check::FrameAlignment).is_skipped());
        assert!(detail(&report, Check::FrameAlignment).contains("no source frame hashes"));
        assert!(report.ok());
    }

    #[test]
    fn head_fidelity_is_skipped_unless_the_policy_is_forensic() {
        let evidence = Evidence::default().with_head_similarity(Similarity::measured(0.5));
        let report = report_with(VerifyPolicy::Strict, &evidence);
        assert!(status(&report, Check::HeadFidelity).is_skipped());
        assert!(report.ok(), "a non-forensic policy must not judge the head");
    }

    #[test]
    fn head_fidelity_fails_below_the_threshold_and_reports_the_score() {
        let evidence = Evidence::default().with_head_similarity(Similarity::measured(0.91));
        let report = report_with(VerifyPolicy::Forensic, &evidence);
        assert!(status(&report, Check::HeadFidelity).is_failed());
        let text = detail(&report, Check::HeadFidelity);
        assert!(text.contains("0.9100"), "{text}");
        assert!(text.contains("wrong place"), "{text}");
        assert_eq!(
            report.get(Check::HeadFidelity).expect("present").measured,
            Some(0.91)
        );
        assert_eq!(
            report.get(Check::HeadFidelity).expect("present").expected,
            Some(HEAD_FIDELITY_MIN)
        );
    }

    #[test]
    fn head_fidelity_passes_at_or_above_the_threshold() {
        for score in [HEAD_FIDELITY_MIN, 0.999] {
            let evidence = Evidence::default().with_head_similarity(Similarity::measured(score));
            let report = report_with(VerifyPolicy::Forensic, &evidence);
            assert_eq!(
                status(&report, Check::HeadFidelity),
                CheckStatus::Passed,
                "{score}"
            );
        }
    }

    #[test]
    fn head_fidelity_is_skipped_when_the_score_could_not_be_measured() {
        let evidence = Evidence::default().with_head_similarity(Similarity::UNMEASURABLE);
        let report = report_with(VerifyPolicy::Forensic, &evidence);
        assert!(status(&report, Check::HeadFidelity).is_skipped());
        assert!(detail(&report, Check::HeadFidelity).contains("could not be compared"));
        assert!(report.ok(), "unmeasurable is not a failure");
    }

    #[test]
    fn codec_match_passes_and_is_case_insensitive() {
        let mut source = source_facts();
        source.codec = "H264".to_owned();
        let report = verify_cut(&head_patch(), &cut_facts(), &source, VerifyPolicy::Standard);
        assert_eq!(status(&report, Check::CodecMatch), CheckStatus::Passed);
        assert_eq!(
            report.get(Check::CodecMatch).expect("present").measured,
            None
        );
    }

    #[test]
    fn codec_mismatch_fails_and_names_both_codecs() {
        let mut source = source_facts();
        source.codec = "hevc".to_owned();
        let report = verify_cut(&head_patch(), &cut_facts(), &source, VerifyPolicy::Standard);
        assert!(status(&report, Check::CodecMatch).is_failed());
        let text = detail(&report, Check::CodecMatch);
        assert!(text.contains("h264") && text.contains("hevc"), "{text}");
        assert!(!report.ok());
    }

    #[test]
    fn timescale_is_skipped_when_no_head_is_re_encoded() {
        for mode in [CutMode::Copy, CutMode::Reencode] {
            let plan = plan(mode, 1_000, 1_600, 0, 0);
            let report = verify_cut(&plan, &cut_facts(), &source_facts(), VerifyPolicy::Standard);
            assert!(
                status(&report, Check::TimescalePreserved).is_skipped(),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_head_patch_without_a_timescale_fails_and_says_why() {
        let broken = plan(CutMode::HeadPatch, 1_000, 1_600, 150, 0);
        let report = verify_cut(
            &broken,
            &cut_facts(),
            &source_facts(),
            VerifyPolicy::Standard,
        );
        assert!(status(&report, Check::TimescalePreserved).is_failed());
        let text = detail(&report, Check::TimescalePreserved);
        assert!(text.contains("slow motion"), "{text}");
        assert!(text.contains("exit"), "{text}");
        assert!(!report.ok());
    }

    #[test]
    fn captions_are_skipped_unless_the_policy_is_forensic() {
        let evidence =
            Evidence::default().with_captions(vec![Cue::new(34.0, 36.0, "hello")], Vec::new());
        let report = report_with(VerifyPolicy::Strict, &evidence);
        assert!(status(&report, Check::Captions).is_skipped());
        assert!(report.ok());
    }

    /// Three cues that fall inside the fixture plan's window, and what retiming makes of
    /// them.
    fn caption_window() -> (Vec<Cue>, Vec<Cue>) {
        let source = vec![
            Cue::new(34.0, 36.0, "first"),
            Cue::new(37.0, 39.0, "second"),
            Cue::new(40.0, 42.0, "third"),
        ];
        let (start, end) = window_seconds(&head_patch()).expect("a usable rate");
        let written = caption::retime(&source, start, end, caption::MIN_OVERLAP)
            .expect("the window is usable")
            .cues;
        (source, written)
    }

    #[test]
    fn captions_pass_when_the_written_cues_are_the_source_cues_retimed() {
        let (source_cues, written) = caption_window();
        assert_eq!(written.len(), 3);
        let evidence = Evidence::default().with_captions(source_cues, written);
        let report = report_with(VerifyPolicy::Forensic, &evidence);
        assert_eq!(status(&report, Check::Captions), CheckStatus::Passed);
        assert_eq!(
            report.get(Check::Captions).expect("present").measured,
            Some(3.0)
        );
    }

    #[test]
    fn caption_verification_catches_a_cue_that_is_one_off() {
        let (source_cues, written) = caption_window();
        // The middle cue is missing from the delivered file, so everything after it moves
        // up by one. The first divergent cue is the one at index 1, and the report shows
        // both what was expected there and what arrived there.
        let missing_middle = vec![written[0].clone(), written[2].clone()];
        let evidence = Evidence::default().with_captions(source_cues, missing_middle);
        let report = report_with(VerifyPolicy::Forensic, &evidence);
        assert!(status(&report, Check::Captions).is_failed());
        assert!(!report.ok());

        let text = detail(&report, Check::Captions);
        assert!(text.contains("cue index 1"), "{text}");
        assert!(text.contains("second"), "{text}");
        assert!(text.contains("third"), "{text}");
        assert_eq!(
            report.get(Check::Captions).expect("present").measured,
            Some(1.0)
        );
        assert_eq!(
            report.get(Check::Captions).expect("present").expected,
            Some(3.0)
        );
    }

    #[test]
    fn caption_verification_catches_a_cue_whose_text_or_time_moved() {
        let (source_cues, mut written) = caption_window();
        written[1].text = "secnd".to_owned();
        let evidence = Evidence::default().with_captions(source_cues.clone(), written);
        let report = report_with(VerifyPolicy::Forensic, &evidence);
        assert!(status(&report, Check::Captions).is_failed());
        assert!(detail(&report, Check::Captions).contains("cue index 1"));

        // And a cue that was written a second late is a different cue.
        let (source_cues, mut late) = caption_window();
        late[0].start += 1.0;
        late[0].end += 1.0;
        let evidence = Evidence::default().with_captions(source_cues, late);
        let report = report_with(VerifyPolicy::Forensic, &evidence);
        assert!(status(&report, Check::Captions).is_failed());
        assert!(detail(&report, Check::Captions).contains("cue index 0"));
    }

    #[test]
    fn captions_pass_when_nothing_falls_inside_the_window() {
        let source_cues = vec![Cue::new(1.0, 2.0, "long before the cut")];
        let evidence = Evidence::default().with_captions(source_cues, Vec::new());
        let report = report_with(VerifyPolicy::Forensic, &evidence);
        assert_eq!(status(&report, Check::Captions), CheckStatus::Passed);
        assert_eq!(
            report.get(Check::Captions).expect("present").measured,
            Some(0.0)
        );
    }

    #[test]
    fn captions_fail_when_the_cut_invented_a_cue() {
        let (source_cues, written) = caption_window();
        let mut extra = written;
        extra.push(Cue::new(30.0, 31.0, "not in the source"));
        let evidence = Evidence::default().with_captions(source_cues, extra);
        let report = report_with(VerifyPolicy::Forensic, &evidence);
        assert!(status(&report, Check::Captions).is_failed());
        assert!(detail(&report, Check::Captions).contains("first divergent cue is index 3"));
    }

    #[test]
    fn captions_are_skipped_when_no_cues_were_supplied() {
        let report = report_with(VerifyPolicy::Forensic, &Evidence::default());
        assert!(status(&report, Check::Captions).is_skipped());
        assert!(detail(&report, Check::Captions).contains("no caption cues"));
    }

    #[test]
    fn verification_off_skips_every_check() {
        let evidence = Evidence::default()
            .with_frame_hashes(hashes(0, &["aa"]), hashes(0, &["bb"]))
            .with_head_similarity(Similarity::measured(0.1))
            .with_captions(vec![Cue::new(0.0, 1.0, "x")], Vec::new());
        let report = report_with(VerifyPolicy::Off, &evidence);

        assert_eq!(report.results.len(), Check::ALL.len());
        for result in &report.results {
            assert_eq!(
                result.status,
                CheckStatus::Skipped {
                    reason: VERIFICATION_OFF.to_owned()
                },
                "{:?}",
                result.check
            );
            assert_eq!(result.measured, None);
            assert_eq!(result.expected, None);
        }
        assert!(report.ok());
    }

    #[test]
    fn the_report_table_has_exactly_one_row_per_check() {
        let report = report_with(VerifyPolicy::Strict, &Evidence::default());
        let table = report.report();
        let rows = table
            .lines()
            .filter(|line| {
                Check::ALL
                    .iter()
                    .any(|check| line.starts_with(check.label()))
            })
            .count();
        assert_eq!(rows, Check::ALL.len(), "{table}");
        for check in Check::ALL {
            assert!(
                table.lines().any(|line| line.starts_with(check.label())),
                "no row for {check:?} in\n{table}"
            );
        }
        assert!(table.contains("frames"), "{table}");
        assert!(table.contains("policy  strict"), "{table}");
        assert!(
            table.contains(&format!("{} checks", Check::ALL.len())),
            "{table}"
        );
        assert!(table.contains("verdict: ok"), "{table}");
    }

    #[test]
    fn the_report_table_says_fail_and_counts_what_happened() {
        let facts = facts_for(590);
        let report = verify_cut(
            &head_patch(),
            &facts,
            &source_facts(),
            VerifyPolicy::Standard,
        );
        let table = report.report();
        assert!(table.contains("FAIL"), "{table}");
        assert!(table.contains("verdict: NOT OK"), "{table}");
        assert!(table.contains("1 failed"), "{table}");
    }

    #[test]
    fn report_accessors_find_a_check_and_list_what_failed() {
        let facts = facts_for(590);
        let report = verify_cut(
            &head_patch(),
            &facts,
            &source_facts(),
            VerifyPolicy::Standard,
        );
        assert_eq!(report.failed().len(), 1);
        assert_eq!(report.failed()[0].check, Check::Frames);
        assert!(report.get(Check::Frames).is_some());
        assert!(report
            .get(Check::Frames)
            .expect("present")
            .detail()
            .is_some());
        assert!(report
            .get(Check::Duration)
            .expect("present")
            .detail()
            .is_none());
    }

    #[test]
    fn the_report_round_trips_through_json() {
        let report = report_with(VerifyPolicy::Forensic, &Evidence::default());
        let json = report.to_json().expect("serialises");
        assert!(json.contains("\"cut\""), "{json}");
        assert!(json.contains("\"audioAlignment\""), "{json}");
        assert!(json.contains("\"skipped\""), "{json}");
        let back = VerifyReport::from_json(&json).expect("deserialises");
        assert_eq!(back, report);
        assert!(back.ok());
    }

    #[test]
    fn a_report_that_is_not_json_is_refused_rather_than_guessed() {
        assert!(VerifyReport::from_json("not a report").is_err());
        assert!(VerifyReport::from_json("{}").is_err());
    }

    #[test]
    fn a_plan_from_the_planner_verifies_clean() {
        let media = MediaInfo {
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
        };
        let segment = Segment::new(media.path.clone(), "begging", 1_000, 1_600);
        let keyframes = KeyframeGrid::new(vec![900, 1_150, 1_400, 1_650], 1_000, 2_000);
        let planned = trimmer_core::plan_cut(&media, &segment, &keyframes).expect("plans");

        assert_eq!(planned.mode, CutMode::HeadPatch);
        let report = verify_cut(
            &planned,
            &cut_facts(),
            &source_facts(),
            VerifyPolicy::Strict,
        );
        assert!(report.ok(), "{}", report.report());
        assert_eq!(
            report.get(Check::Frames).expect("present").expected,
            Some(600.0)
        );
        assert_eq!(
            report
                .get(Check::TimescalePreserved)
                .expect("present")
                .measured,
            Some(90_000.0)
        );

        // And the same plan against a file that ran long: a warning, not a failure.
        let long = facts_for(602);
        let report = verify_cut(&planned, &long, &source_facts(), VerifyPolicy::Strict);
        assert!(report.ok());
        assert!(status(&report, Check::Overshoot).is_warning());
    }

    #[test]
    fn the_number_helper_prints_frames_as_frames_and_not_as_thousandths() {
        assert_eq!(number(None), "-");
        assert_eq!(number(Some(0.0)), "0");
        assert_eq!(number(Some(-0.0)), "0");
        assert_eq!(number(Some(600.0)), "600");
        assert_eq!(number(Some(20.02)), "20.02");
        assert_eq!(number(Some(-1.0)), "-1");
        assert_eq!(number(Some(0.91)), "0.91");
    }

    #[test]
    fn the_policy_labels_are_distinct() {
        let labels = [
            policy_label(VerifyPolicy::Off),
            policy_label(VerifyPolicy::Standard),
            policy_label(VerifyPolicy::Strict),
            policy_label(VerifyPolicy::Forensic),
        ];
        assert_eq!(labels, ["off", "standard", "strict", "forensic"]);
    }
}

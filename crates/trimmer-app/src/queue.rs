//! The cut queue: a batch of segments, run one at a time, with progress and per-item outcomes.
//!
//! ## Why one at a time
//!
//! A cut is not CPU-bound, it is **disk-bound**. A head patch reads a multi-gigabyte master and
//! writes a segment; four of those on one spindle do not finish four times sooner, they make each
//! other slower and make the progress display meaningless. Sequential also gives `Cancel` an
//! unambiguous meaning: finish the segment in flight, then stop. A half-written segment is worse
//! than a segment that was never started.
//!
//! ## What a studio actually needs from a batch
//!
//! Not "it ran". They need to know, afterwards, exactly what happened to every segment, because
//! the deliverable goes to a client. So every item ends in a [`JobStatus`] that carries the
//! numbers: the plan that was used, how many frames came out, the overshoot, the verification
//! verdict, and — when something failed — a sentence saying what. [`BatchOutcome`] is that record,
//! and it is what the interface's proof panel renders and what an audit log stores.
//!
//! ## Failure policy
//!
//! One bad segment does not abandon the other ninety-nine. [`QueueOptions::stop_on_error`]
//! defaults to `false` for exactly that reason: a batch that stops at the first problem costs a
//! studio the whole run, whereas a batch that finishes and reports "one of these failed, here is
//! which" costs them one segment. Serious failures — a cancelled run, a disk that has filled —
//! stop the batch regardless, because those affect every remaining item.

use std::sync::Arc;

use trimmer_core::VerifyPolicy;
use trimmer_core::{format_seconds, CutMode, CutPlan, MediaPath, SegmentId};
use trimmer_media::{CancelFlag, Progress, ProgressSink, RunOptions};
use trimmer_verify::{AuditManifest, CutFacts, VerifyReport};

use crate::ports::{cut_request_with, Clock, Measurer, MediaEngine, SegmentCutRequest};
use crate::workspace::Workspace;
use crate::AppResult;

/// A job's identity within one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct JobId(pub usize);

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Where a job is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    /// Not started.
    Pending,
    /// Being planned.
    Planning,
    /// Being cut.
    Cutting,
    /// Written, and now being measured.
    Verifying,
    /// Finished, successfully or not.
    Done,
}

impl JobState {
    /// A short label for a progress display.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Pending => "waiting",
            Self::Planning => "planning",
            Self::Cutting => "cutting",
            Self::Verifying => "checking",
            Self::Done => "done",
        }
    }
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JobStatus {
    /// Cut, and the checks passed or were switched off.
    Succeeded {
        /// What was written.
        output: MediaPath,
        /// The plan that was carried out.
        plan: Box<CutPlan>,
        /// Frames in the file.
        frames: i64,
        /// Frames the copy ran past the out point.
        overshoot: i64,
        /// The verification verdict.
        verification: Box<VerifyReport>,
        /// How many ffmpeg invocations it took.
        steps: usize,
        /// How long it took, in seconds.
        seconds: f64,
    },
    /// Cut, but a check failed. The file is on disk; it is not certified.
    Unverified {
        /// What was written.
        output: MediaPath,
        /// The plan.
        plan: Box<CutPlan>,
        /// Frames in the file.
        frames: i64,
        /// The failing verdict, with the failing checks named.
        verification: Box<VerifyReport>,
        /// How long it took.
        seconds: f64,
    },
    /// Not cut.
    Failed {
        /// A sentence saying what went wrong.
        reason: String,
        /// True when the reason was a cancellation rather than a defect.
        cancelled: bool,
    },
    /// Not attempted, because the segment cannot be cut or is switched off.
    Skipped {
        /// Why.
        reason: String,
    },
}

impl JobStatus {
    /// True when the job produced a certified file.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded { .. })
    }

    /// True when the job wrote a file at all.
    #[must_use]
    pub const fn wrote_a_file(&self) -> bool {
        matches!(self, Self::Succeeded { .. } | Self::Unverified { .. })
    }

    /// The output path, when there is one.
    #[must_use]
    pub fn output(&self) -> Option<&MediaPath> {
        match self {
            Self::Succeeded { output, .. } | Self::Unverified { output, .. } => Some(output),
            _ => None,
        }
    }

    /// A one-line verdict for a list.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Succeeded {
                frames,
                overshoot,
                verification,
                seconds,
                ..
            } => {
                let tail = if *overshoot > 0 {
                    format!(" ({overshoot} frame(s) past the out point, as a copy does)")
                } else {
                    String::new()
                };
                format!(
                    "{frames} frames in {seconds:.1}s, checks {} {tail}",
                    // This line is for a *certified* file, which the caller has already established,
                    // so it reports the narrower question: did anything fail.
                    if verification.ok() {
                        "passed"
                    } else {
                        "FAILED"
                    }
                )
            }
            Self::Unverified {
                verification,
                seconds,
                ..
            } => {
                // A file is uncertified for one of two different reasons, and the sentence must say
                // which: a check that FAILED means the file is suspect, whereas a check that never
                // RAN means this run cannot vouch for it. Collapsing them would either libel a good
                // file or excuse a bad one.
                let failed: Vec<&str> = verification
                    .failed()
                    .iter()
                    .map(|result| result.check.label())
                    .collect();
                let gaps: Vec<&str> = verification
                    .unrun_required_checks()
                    .iter()
                    .map(|result| result.check.label())
                    .collect();
                let why = match (failed.is_empty(), gaps.is_empty()) {
                    (false, false) => format!(
                        "{} failed ({}) and {} did not run ({})",
                        failed.len(),
                        failed.join(", "),
                        gaps.len(),
                        gaps.join(", ")
                    ),
                    (false, true) => format!("{} failed: {}", failed.len(), failed.join(", ")),
                    (true, false) => format!(
                        "{} check(s) this policy asks for did not run: {}",
                        gaps.len(),
                        gaps.join(", ")
                    ),
                    (true, true) => "a check failed without saying which".to_owned(),
                };
                format!("uncertified — written in {seconds:.1}s but {why}")
            }
            Self::Failed { reason, cancelled } => {
                if *cancelled {
                    format!("cancelled: {reason}")
                } else {
                    format!("failed: {reason}")
                }
            }
            Self::Skipped { reason } => format!("skipped: {reason}"),
        }
    }
}

/// Something that happened during a batch.
///
/// `Serialize` because the daemon and the desktop shell both forward these to a client.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QueueEvent {
    /// The batch began.
    Started {
        /// How many jobs it will run.
        total: usize,
    },
    /// A job changed state.
    State {
        /// Which job.
        job: JobId,
        /// The segment's name, for a display.
        name: String,
        /// The new state.
        state: JobState,
    },
    /// A step inside a job reported progress.
    JobProgress {
        /// Which job.
        job: JobId,
        /// What it said.
        progress: Progress,
    },
    /// A job ended.
    Finished {
        /// Which job.
        job: JobId,
        /// The segment's name.
        name: String,
        /// How it ended.
        status: JobStatus,
    },
    /// The batch ended.
    Completed {
        /// How many succeeded.
        succeeded: usize,
        /// How many wrote a file that failed a check.
        unverified: usize,
        /// How many failed.
        failed: usize,
        /// How many were skipped.
        skipped: usize,
    },
}

/// Which kind of event something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueEventKind {
    /// The batch began.
    Started,
    /// A job changed state.
    State,
    /// A step inside a job reported progress.
    JobProgress,
    /// A job ended.
    Finished,
    /// The batch ended.
    Completed,
}

impl QueueEvent {
    /// Which kind of event this is, for a caller that routes on it rather than matching.
    #[must_use]
    pub const fn kind(&self) -> QueueEventKind {
        match self {
            Self::Started { .. } => QueueEventKind::Started,
            Self::State { .. } => QueueEventKind::State,
            Self::JobProgress { .. } => QueueEventKind::JobProgress,
            Self::Finished { .. } => QueueEventKind::Finished,
            Self::Completed { .. } => QueueEventKind::Completed,
        }
    }
}

/// Receives queue events.
///
/// A trait rather than a channel because the three consumers want different things from the same
/// stream: the interface repaints, the CLI prints, and the daemon forwards to a server-sent-events
/// client. None of them should have to know about the others' buffering.
pub trait QueueSink: Send + Sync {
    /// Record one event.
    fn event(&self, event: QueueEvent);
}

/// A sink that discards everything, for a caller that only wants the outcome.
#[derive(Debug, Default, Clone, Copy)]
pub struct QuietQueueSink;

impl QueueSink for QuietQueueSink {
    fn event(&self, _event: QueueEvent) {}
}

/// A sink that collects events, for assertions in tests and for a report built after the fact.
#[derive(Debug, Default)]
pub struct CollectingQueueSink {
    events: std::sync::Mutex<Vec<QueueEvent>>,
}

impl CollectingQueueSink {
    /// A new, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The events received so far.
    #[must_use]
    pub fn events(&self) -> Vec<QueueEvent> {
        self.events
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl QueueSink for CollectingQueueSink {
    fn event(&self, event: QueueEvent) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(event);
        }
    }
}

/// Everything a batch produced.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchOutcome {
    /// One entry per segment, in order.
    pub jobs: Vec<(JobId, SegmentId, String, JobStatus)>,
    /// Frames delivered across the batch.
    pub delivered_frames: i64,
    /// Seconds delivered across the batch.
    pub delivered_seconds: f64,
    /// Seconds the batch took.
    pub elapsed_seconds: f64,
    /// True when the run was cancelled.
    pub cancelled: bool,
    /// The audit manifest, with one entry per job.
    pub audit: Box<AuditManifest>,
}

impl BatchOutcome {
    /// How many jobs there were.
    #[must_use]
    pub fn total(&self) -> usize {
        self.jobs.len()
    }

    /// How many succeeded.
    #[must_use]
    pub fn succeeded(&self) -> usize {
        self.jobs
            .iter()
            .filter(|(_, _, _, s)| s.is_success())
            .count()
    }

    /// How many wrote a file that failed a check.
    #[must_use]
    pub fn unverified(&self) -> usize {
        self.jobs
            .iter()
            .filter(|(_, _, _, s)| matches!(s, JobStatus::Unverified { .. }))
            .count()
    }

    /// How many failed.
    #[must_use]
    pub fn failed(&self) -> usize {
        self.jobs
            .iter()
            .filter(|(_, _, _, s)| matches!(s, JobStatus::Failed { .. }))
            .count()
    }

    /// How many were skipped.
    #[must_use]
    pub fn skipped(&self) -> usize {
        self.jobs
            .iter()
            .filter(|(_, _, _, s)| matches!(s, JobStatus::Skipped { .. }))
            .count()
    }

    /// True when every job either succeeded or was deliberately skipped.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failed() == 0 && self.unverified() == 0 && !self.cancelled
    }

    /// The failed jobs, for a "these need attention" list.
    #[must_use]
    pub fn failures(&self) -> Vec<&(JobId, SegmentId, String, JobStatus)> {
        self.jobs
            .iter()
            .filter(|(_, _, _, status)| {
                matches!(
                    status,
                    JobStatus::Failed { .. } | JobStatus::Unverified { .. }
                )
            })
            .collect()
    }

    /// A report an operator can read and a log can keep.
    #[must_use]
    pub fn report(&self) -> String {
        let mut lines = vec![format!(
            "batch       {} jobs in {}: {} written, {} failed a check, {} failed, {} skipped",
            self.total(),
            format_seconds(self.elapsed_seconds),
            self.succeeded(),
            self.unverified(),
            self.failed(),
            self.skipped()
        )];
        if self.cancelled {
            lines.push(
                "            the run was cancelled; the segments below were not attempted"
                    .to_owned(),
            );
        }
        lines.push(format!(
            "delivered   {} frames, {}",
            self.delivered_frames,
            format_seconds(self.delivered_seconds)
        ));
        for (job, _, name, status) in &self.jobs {
            lines.push(format!("{job:>4} {name:<32} {}", status.summary()));
        }
        lines.join("\n")
    }
}

/// How a batch should behave.
#[derive(Debug, Clone)]
pub struct QueueOptions {
    /// Stop at the first failure rather than finishing the batch.
    pub stop_on_error: bool,
    /// Skip the verification step even when the project asks for it.
    ///
    /// Only useful for a rough cut where the operator has already decided to trust the engine.
    pub skip_verification: bool,
    /// Record one audit entry per job, for a manifest that can be signed.
    pub audit: bool,
    /// A label for the run, used in the audit manifest.
    pub label: String,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            // One bad segment must not cost a studio the other ninety-nine.
            stop_on_error: false,
            skip_verification: false,
            audit: true,
            label: "batch".to_owned(),
        }
    }
}

/// The batch runner.
pub struct Queue {
    engine: Arc<dyn MediaEngine>,
    measurer: Arc<dyn Measurer>,
    clock: Arc<dyn Clock>,
    cancel: CancelFlag,
    app_version: String,
    machine: String,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("app_version", &self.app_version)
            .field("machine", &self.machine)
            .field("cancelled", &self.cancel.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl Queue {
    /// A queue over an engine, a measurer and a clock.
    #[must_use]
    pub fn new(
        engine: Arc<dyn MediaEngine>,
        measurer: Arc<dyn Measurer>,
        clock: Arc<dyn Clock>,
        app_version: impl Into<String>,
        machine: impl Into<String>,
    ) -> Self {
        Self {
            engine,
            measurer,
            clock,
            cancel: CancelFlag::new(),
            app_version: app_version.into(),
            machine: machine.into(),
        }
    }

    /// A flag a user interface can set to stop the run.
    #[must_use]
    pub fn cancel_flag(&self) -> CancelFlag {
        self.cancel.clone()
    }

    /// Run every enabled segment in a workspace, in order.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::BatchInProgress`] never — the queue is not reentrant, and a caller that
    /// wants one run at a time holds one queue. Returns [`AppError`] only for a failure that
    /// affects the whole batch; a segment's own failure is recorded in its [`JobStatus`].
    pub async fn run(
        &self,
        workspace: &mut Workspace,
        options: &QueueOptions,
        sink: Arc<dyn QueueSink>,
    ) -> AppResult<BatchOutcome> {
        let started = self.clock.now_millis();
        let order: Vec<SegmentId> = workspace
            .project()
            .segments
            .iter()
            .filter(|segment| segment.enabled)
            .map(|segment| segment.id)
            .collect();

        let mut audit = AuditManifest::new(
            workspace.project().id,
            &workspace.project().created_by,
            self.app_version.clone(),
            self.machine.clone(),
            self.clock.now_unix(),
        );
        let mut jobs: Vec<(JobId, SegmentId, String, JobStatus)> = Vec::new();
        let mut delivered_frames = 0i64;
        let mut delivered_seconds = 0.0f64;
        let mut cancelled = false;

        sink.event(QueueEvent::Started { total: order.len() });

        for (index, id) in order.iter().enumerate() {
            let job = JobId(index);
            let Some(segment) = workspace.project().segment(*id).cloned() else {
                continue;
            };
            let name = segment.name.clone();

            if self.cancel.is_cancelled() {
                cancelled = true;
                let status = JobStatus::Failed {
                    reason: "the run was cancelled before this segment started".to_owned(),
                    cancelled: true,
                };
                sink.event(QueueEvent::Finished {
                    job,
                    name: name.clone(),
                    status: status.clone(),
                });
                jobs.push((job, *id, name, status));
                continue;
            }

            sink.event(QueueEvent::State {
                job,
                name: name.clone(),
                state: JobState::Planning,
            });

            let status = self
                .run_one(workspace, *id, job, &name, options, &sink)
                .await;
            if let Some((frames, seconds)) = delivered_of(&status) {
                delivered_frames += frames;
                delivered_seconds += seconds;
            }
            if let JobStatus::Failed {
                cancelled: was_cancelled,
                ..
            } = &status
            {
                if *was_cancelled {
                    cancelled = true;
                }
            }

            if options.audit {
                audit.record(
                    &workspace.project().created_by,
                    "cut",
                    serde_json::json!({
                        "segment": id.to_string(),
                        "name": name,
                        "status": status.summary(),
                        "output": status.output().map(ToString::to_string),
                    }),
                    self.clock.now_unix(),
                );
            }

            sink.event(QueueEvent::Finished {
                job,
                name: name.clone(),
                status: status.clone(),
            });
            let stop = options.stop_on_error
                && matches!(
                    status,
                    JobStatus::Failed {
                        cancelled: false,
                        ..
                    }
                );
            jobs.push((job, *id, name, status));
            if stop || cancelled {
                cancelled = cancelled || stop;
                break;
            }
        }

        let outcome = BatchOutcome {
            jobs,
            delivered_frames,
            delivered_seconds,
            elapsed_seconds: (self.clock.now_millis().saturating_sub(started)) as f64 / 1000.0,
            cancelled,
            audit: Box::new(audit),
        };

        sink.event(QueueEvent::Completed {
            succeeded: outcome.succeeded(),
            unverified: outcome.unverified(),
            failed: outcome.failed(),
            skipped: outcome.skipped(),
        });
        Ok(outcome)
    }

    /// Run one job. Never returns an error: a segment's failure is its status.
    async fn run_one(
        &self,
        workspace: &mut Workspace,
        id: SegmentId,
        job: JobId,
        name: &str,
        options: &QueueOptions,
        sink: &Arc<dyn QueueSink>,
    ) -> JobStatus {
        // A preview first, so a segment that cannot be cut is skipped with a reason rather than
        // failing three processes in. This is also what makes the batch's dry-run honest: the
        // same planning code answers both questions.
        let preview = match workspace.preview(id).await {
            Ok(preview) => preview,
            Err(error) => {
                return JobStatus::Skipped {
                    reason: error.to_string(),
                }
            }
        };
        if !preview.problems.is_empty() {
            return JobStatus::Skipped {
                reason: preview.problems.join("; "),
            };
        }

        let request = match cut_request_with(workspace, id, preview.plan.clone()) {
            Ok(request) => request,
            Err(error) => {
                return JobStatus::Skipped {
                    reason: error.to_string(),
                }
            }
        };

        sink.event(QueueEvent::State {
            job,
            name: name.to_owned(),
            state: JobState::Cutting,
        });
        let step_sink = Arc::new(JobSink {
            job,
            inner: Arc::clone(sink),
        });
        let run_options = RunOptions {
            policy: trimmer_media::PollPolicy::long(),
            cancel: self.cancel.clone(),
            sink: step_sink,
            label: format!("cut {name}"),
            // The passes inside a cut ask for themselves: each one knows what it is expected to
            // produce, and a run whose length is not known yet cannot be given a fraction here.
            watch: None,
        };

        let cut = match self.engine.cut(&request, &run_options).await {
            Ok(cut) => cut,
            Err(error) if error.is_cancelled() => {
                return JobStatus::Failed {
                    reason: "the run was cancelled".to_owned(),
                    cancelled: true,
                }
            }
            Err(error) => {
                return JobStatus::Failed {
                    reason: error.to_string(),
                    cancelled: false,
                }
            }
        };

        // Verify. The policy decides how hard; a project with verification off records a verdict
        // that says so rather than pretending to have checked.
        let policy = if options.skip_verification {
            VerifyPolicy::Off
        } else {
            workspace.verify_policy()
        };
        sink.event(QueueEvent::State {
            job,
            name: name.to_owned(),
            state: JobState::Verifying,
        });
        let verification = self.verify(&request, &cut.plan, policy);

        // The wall-clock cost of the cut is the sum of its steps, not the difference between two
        // reads of the clock: a batch reports per-segment timings, and the steps are what the
        // engine actually measured.
        let elapsed = cut
            .steps
            .iter()
            .filter_map(|step| step.seconds)
            .sum::<f64>();

        // `ok()` answers "did anything fail"; `is_certified()` answers "did every check the policy
        // asked for actually run, and pass". Only the second may be reported as verified, because a
        // skip under a policy that requested the check is a gap rather than a pass.
        if verification.is_certified(&cut.plan) {
            JobStatus::Succeeded {
                output: cut.output.clone(),
                plan: Box::new(cut.plan.clone()),
                frames: cut.frame_count,
                overshoot: cut.overshoot,
                verification: Box::new(verification),
                steps: cut.steps.len(),
                seconds: elapsed,
            }
        } else {
            JobStatus::Unverified {
                output: cut.output.clone(),
                plan: Box::new(cut.plan.clone()),
                frames: cut.frame_count,
                verification: Box::new(verification),
                seconds: elapsed,
            }
        }
    }

    /// Measure a finished cut against its source.
    ///
    /// Not `async`: the measurement seam is synchronous by design, so the verdict logic never has
    /// to be. The real measurer blocks on the async prober internally, which is acceptable here
    /// because a batch is sequential and there is nothing else for this thread to do.
    ///
    /// ## Why the evidence is gathered here
    ///
    /// An audit found that this method called `verify_cut` with no evidence at all, so under the
    /// default `Strict` policy the frame-alignment check — the whole reason the policy is called
    /// strict — reported itself skipped, `ok()` returned true because nothing had *failed*, and the
    /// batch reported every cut as verified. The verdict logic was right; the wiring was absent.
    ///
    /// The frames are therefore hashed here, at the point both files certainly have frames: the
    /// keyframe the body begins on. For a copy that is frame zero of the output; for a head patch it
    /// is `keyframe - start_frame` into the delivered file. Anchoring on the *last* twelve frames of
    /// the request would be the stronger check for a dropped tail — `trimmer-verify` says so in its
    /// own documentation — but the body start is the one position guaranteed to exist in both files
    /// whatever the overshoot, and a comparison that cannot be made is worth less than one that can.
    ///
    /// A hash pass that fails is not fatal. The check reports itself skipped and the file comes back
    /// uncertified, which is the honest outcome and still better than claiming a pass.
    fn verify(
        &self,
        request: &SegmentCutRequest,
        plan: &CutPlan,
        policy: VerifyPolicy,
    ) -> VerifyReport {
        if policy == VerifyPolicy::Off {
            let facts = Self::empty_facts(&request.output);
            return trimmer_verify::verify_cut(plan, &facts, &facts, policy);
        }

        // The facts. A probe that fails leaves its check reporting a skip rather than inventing one.
        let cut_facts = self
            .measurer
            .facts(&request.output)
            .unwrap_or_else(|_| Self::empty_facts(&request.output));
        let source_facts = self
            .measurer
            .facts(&request.media.path)
            .unwrap_or_else(|_| Self::empty_facts(&request.output));

        if !policy.hashes_frames() {
            return trimmer_verify::verify_cut(plan, &cut_facts, &source_facts, policy);
        }

        // The sample window, anchored on the keyframe the body begins on.
        const SAMPLE_USIZE: usize = 24;
        // A frame count fits an i64 by construction — a master with more than 2^63 frames does not
        // exist — so the conversion is stated rather than performed with a lossy cast.
        let sample = i64::try_from(SAMPLE_USIZE).unwrap_or(i64::MAX);
        let rate = request.media.rate;
        let anchor = plan.keyframe.unwrap_or(plan.start_frame);
        let cut_anchor = anchor - plan.start_frame;

        let mut evidence = trimmer_verify::Evidence::default();
        if let (Ok(source), Ok(delivered)) = (
            self.measurer
                .frame_hashes(&request.media.path, anchor, sample, rate),
            self.measurer
                .frame_hashes(&request.output, cut_anchor, sample, rate),
        ) {
            // `first_frame` records which frame each window starts on, so the comparison knows what
            // it is aligning and can report which offset matched.
            evidence = evidence.with_frame_hashes(
                trimmer_verify::FrameHashes::new(source.digests, anchor),
                trimmer_verify::FrameHashes::new(delivered.digests, cut_anchor),
            );
        }

        if policy.is_forensic() {
            evidence = with_captions(evidence, request, rate);
        }

        trimmer_verify::verify_cut_with(plan, &cut_facts, &source_facts, policy, &evidence)
    }

    /// Facts that say "nothing is known", so a check that cannot measure reports rather than
    /// lies instead of silently passing.
    fn empty_facts(path: &MediaPath) -> CutFacts {
        CutFacts {
            path: path.clone(),
            frame_count: -1,
            video_duration: -1.0,
            video_start_time: 0.0,
            audio_duration: None,
            audio_start_time: None,
            rate: None,
            codec: String::new(),
            width: 0,
            height: 0,
            size_bytes: 0,
        }
    }
}

/// A progress sink that wraps each step event in a queue event tagged with its job.
///
/// This is the seam between the two vocabularies: `trimmer-media` speaks [`Progress`] about one
/// process, the queue speaks [`QueueEvent`] about a batch, and neither should have to know about
/// the other.
struct JobSink {
    job: JobId,
    inner: Arc<dyn QueueSink>,
}

impl ProgressSink for JobSink {
    fn report(&self, progress: Progress) {
        self.inner.event(QueueEvent::JobProgress {
            job: self.job,
            progress,
        });
    }
}

/// Read the source caption cues and the written sidecar, when the delivered cut has one.
///
/// The written sidecar is named after the output, which is where `caption::retime_file` puts it so a
/// player loads it with no further setup. When there is no sidecar — a cut made without a transcript
/// — the source cues are still read, because the caption check then reports the *mismatch* of "the
/// source had captions and none were written" rather than a skip, which is the more useful verdict.
fn with_captions(
    evidence: trimmer_verify::Evidence,
    request: &SegmentCutRequest,
    rate: trimmer_core::FrameRate,
) -> trimmer_verify::Evidence {
    let _ = rate;
    let Some(source_path) = trimmer_core::caption::find_for(request.media.path.as_path()) else {
        return evidence;
    };
    let Ok(source) = trimmer_core::caption::read(&source_path) else {
        return evidence;
    };
    let sidecar = request.output.with_extension("srt");
    let written = trimmer_core::caption::read(sidecar.as_path())
        .map(|transcript| transcript.cues)
        .unwrap_or_default();
    evidence.with_captions(source.cues, written)
}

/// The frames and seconds a status delivered, for the batch totals.
fn delivered_of(status: &JobStatus) -> Option<(i64, f64)> {
    match status {
        JobStatus::Succeeded {
            frames,
            verification,
            ..
        }
        | JobStatus::Unverified {
            frames,
            verification,
            ..
        } => {
            let _ = verification;
            Some((*frames, 0.0))
        }
        _ => None,
    }
}

/// A sink that does nothing, for a caller that only wants the outcome.
#[must_use]
pub fn quiet_sink() -> Arc<dyn QueueSink> {
    Arc::new(QuietQueueSink)
}

/// A cut mode as a word, for a report.
///
/// A free function rather than a method on `CutMode` because the domain should not grow a
/// presentation concern; the interface asks this crate for the word instead.
#[must_use]
pub const fn mode_word(mode: CutMode) -> &'static str {
    match mode {
        CutMode::Copy => "copied",
        CutMode::HeadPatch => "head-patched",
        CutMode::Reencode => "re-encoded",
    }
}

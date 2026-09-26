//! Performing the cut: the head-patch method, step by step.
//!
//! Each ffmpeg invocation is built by its own pure function returning an argument vector. That
//! is not decoration: the argument vectors are where the three silent-failure modes live, so
//! they are separated from the running of them and covered by tests that assert the exact
//! flags. A future change that drops `-video_track_timescale` fails a test in this file rather
//! than producing a slow-motion deliverable three months later.
//!
//! # The steps of a head patch
//!
//! ```text
//!   source ────────────────────────────────────────────────────────────►
//!            S              K                              E
//!            ├── head ──────┤
//!            │ re-encoded   │
//!                           ├──────── body ───────────────┤
//!                           │ picture copied, sound copied in a separate pass
//!                                                          │
//!   head.mp4 + body.mp4 ──concat──► joined.mp4 ──move──► output
//! ```
//!
//! The split at `K` — the first keyframe at or after `S` — comes from
//! [`trimmer_core::plan_cut`]. This module only carries it out.

use std::path::{Path, PathBuf};

use trimmer_core::{apply_calibration, plan_cut, KeyframeGrid};
use trimmer_core::{
    CutMode, CutPlan, DeliveryPreset, Geometry, MediaInfo, MediaPath, Segment, VideoTreatment,
};

use crate::probe::Prober;
use crate::process::{Progress, RunOptions};
use crate::tool::{ToolPaths, ToolSet};
use crate::{MediaError, MediaResult, ProcessRunner};

/// Pixel formats libx264 and libx265 accept straight through.
///
/// Anything else — a 12-bit source, an exotic 4:2:2 — is converted to the 8-bit 4:2:0 its codec
/// family expects, because handing libx264 a format it cannot take makes it fail rather than
/// silently convert, which is the better outcome but not a useful one at trim time.
pub const PASSTHROUGH_PIX_FMTS: [&str; 6] = [
    "yuv420p",
    "yuv422p",
    "yuv444p",
    "yuv420p10le",
    "yuv422p10le",
    "yuv444p10le",
];

/// How a cut is encoded. These are the quality and speed decisions, kept apart from the
/// *structural* decisions that [`CutPlan`] already made.
#[derive(Debug, Clone, PartialEq)]
pub struct CutConfig {
    /// Constant rate factor for the re-encoded head. 0 is mathematically lossless and enormous.
    pub crf: u8,
    /// x264/x265 speed preset.
    pub preset: String,
    /// Audio bitrate for re-encoded audio.
    pub audio_bitrate: String,
    /// Stream-copy the head's audio instead of re-encoding it.
    ///
    /// Faster, but only safe when the head begins on an audio packet boundary; otherwise the
    /// first fraction of a second of sound is misaligned against the picture. Off by default
    /// because the failure is subtle and the re-encode is cheap.
    pub head_audio_copy: bool,
    /// Write the MP4 index at the front so the file streams and scrubs instantly.
    pub faststart: bool,
    /// Ask the encoder for two passes of lookahead over a *long* head. Ignored for a short one,
    /// where there is nothing to look ahead of.
    pub long_head_seconds: f64,
}

impl Default for CutConfig {
    fn default() -> Self {
        Self {
            crf: 18,
            preset: "veryfast".to_owned(),
            audio_bitrate: "192k".to_owned(),
            head_audio_copy: false,
            faststart: true,
            long_head_seconds: 10.0,
        }
    }
}

impl CutConfig {
    /// The pixel format to write, given the source's.
    #[must_use]
    pub fn pixel_format(source_pix_fmt: &str) -> String {
        if PASSTHROUGH_PIX_FMTS.contains(&source_pix_fmt) {
            source_pix_fmt.to_owned()
        } else {
            "yuv420p".to_owned()
        }
    }

    /// True when the head is long enough that encoder lookahead is worth asking for.
    #[must_use]
    pub fn wants_lookahead(&self, head_seconds: f64) -> bool {
        head_seconds >= self.long_head_seconds
    }
}

/// One ffmpeg invocation, recorded so a report can show exactly what ran.
///
/// `Eq` is not derived because [`ExecutionStep::seconds`] is a float; see [`Progress`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionStep {
    /// A short label, e.g. `head encode`.
    pub label: String,
    /// The program.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// How long it took, once it has run.
    pub seconds: Option<f64>,
    /// Whether it exited zero.
    pub ok: Option<bool>,
}

impl ExecutionStep {
    /// The command line, for a log or an audit record.
    #[must_use]
    pub fn command_line(&self) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }
}

/// What a completed cut produced.
#[derive(Debug, Clone, PartialEq)]
pub struct CutOutcome {
    /// The plan that was carried out, after any calibration correction.
    pub plan: CutPlan,
    /// The file that was written.
    pub output: MediaPath,
    /// Every ffmpeg invocation, in order.
    pub steps: Vec<ExecutionStep>,
    /// The output's frame count, as measured after the fact.
    pub frame_count: i64,
    /// Frames the stream copy ran past the out point. Never a defect: see the module docs of
    /// `trimmer-verify`.
    pub overshoot: i64,
    /// Notes worth showing the operator.
    pub notes: Vec<String>,
}

impl CutOutcome {
    /// True when some of the delivered frames are the original packets.
    #[must_use]
    pub const fn is_lossless(&self) -> bool {
        self.plan.is_lossless()
    }

    /// The fraction of the segment that had to be re-encoded, which is the cost of the cut.
    #[must_use]
    pub fn reencode_fraction(&self) -> f64 {
        self.plan.reencode_fraction()
    }
}

/// A prepared command: the builder's output, ready to run.
///
/// Kept as its own type so that every argument builder can be *tested as data* without a
/// process, and so that a caller who wants to show a user what will happen can do so. `Serialize`
/// because a dry run sends these to the interface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    /// A short label.
    pub label: String,
    /// The arguments to pass to ffmpeg.
    pub args: Vec<String>,
}

impl Prepared {
    /// The command line, with the program name.
    #[must_use]
    pub fn command_line(&self, ffmpeg: &Path) -> String {
        let mut parts = vec![ffmpeg.display().to_string()];
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }

    /// The arguments as `OsString`, for the runner.
    fn os_args(&self) -> Vec<std::ffi::OsString> {
        self.args.iter().map(std::ffi::OsString::from).collect()
    }
}

/// Runs the cut. Holds the resolved tools and the process runner.
#[derive(Debug, Clone)]
pub struct CutExecutor {
    tools: ToolPaths,
    runner: ProcessRunner,
    prober: Prober,
}

impl CutExecutor {
    /// An executor using resolved tool paths.
    #[must_use]
    pub fn new(tools: ToolPaths) -> Self {
        let prober = Prober::new(tools.clone());
        Self {
            tools,
            runner: ProcessRunner::new(),
            prober,
        }
    }

    /// The prober this executor uses, so a caller does not resolve the tools twice.
    #[must_use]
    pub const fn prober(&self) -> &Prober {
        &self.prober
    }

    /// Cut a segment, using the head-patch method when the plan allows it.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::Cancelled`] when the caller cancels, and propagates process
    /// failures. The domain's refusals surface as [`MediaError::Core`].
    pub async fn cut(
        &self,
        media: &MediaInfo,
        segment: &Segment,
        preset: &DeliveryPreset,
        config: &CutConfig,
        output: &Path,
        options: &RunOptions,
    ) -> MediaResult<CutOutcome> {
        let plan = plan_cut(
            media,
            segment,
            &self.cached_keyframes(media, segment, options).await?,
        )?;
        self.cut_with_plan(media, &plan, preset, config, output, options)
            .await
    }

    /// Fetch the keyframes a plan needs.
    ///
    /// The window is deliberately *not* the whole file: a keyframe listing over two hours costs
    /// a seek per GOP. Only the window the planner looks at is read, which is the segment plus
    /// [`trimmer_core::plan::MAX_HEAD_SECONDS`].
    async fn cached_keyframes(
        &self,
        media: &MediaInfo,
        segment: &Segment,
        _options: &RunOptions,
    ) -> MediaResult<KeyframeGrid> {
        let end = segment.end_frame.unwrap_or(media.frame_count);
        let window = media.rate.frames_in(trimmer_core::plan::MAX_HEAD_SECONDS);
        let to = (segment.start_frame + window)
            .min(end)
            .max(segment.start_frame + 1);
        self.prober.keyframes(media, segment.start_frame, to).await
    }

    /// Carry out a plan that has already been made.
    ///
    /// # Errors
    ///
    /// As [`CutExecutor::cut`].
    pub async fn cut_with_plan(
        &self,
        media: &MediaInfo,
        plan: &CutPlan,
        preset: &DeliveryPreset,
        config: &CutConfig,
        output: &Path,
        options: &RunOptions,
    ) -> MediaResult<CutOutcome> {
        plan.verify_invariants()?;
        let forces_encode = !preset.preserves_picture(media.width, media.height);

        // A preset that reshapes the picture cannot be satisfied by copying packets, so the
        // whole segment is re-encoded. This is a deliberate, visible decision rather than a
        // silent fallback: `plan_cut` reported a lossless plan, and the delivery preset is what
        // overrides it.
        if forces_encode {
            options.sink.report(Progress::Message {
                text: format!(
                    "the {} preset changes the picture, so the whole segment is re-encoded \
                     rather than copied",
                    preset.name
                ),
            });
            return self
                .reencode_segment(media, plan, preset, config, output, options)
                .await;
        }

        match plan.mode {
            CutMode::Copy => {
                self.copy_segment(media, plan, config, output, options)
                    .await
            }
            CutMode::Reencode => {
                self.reencode_segment(media, plan, preset, config, output, options)
                    .await
            }
            CutMode::HeadPatch => {
                self.head_patch(media, plan, preset, config, output, options)
                    .await
            }
        }
    }

    /// Re-cut with a measured correction folded into the head's length.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::Cancelled`] when the caller cancels, and
    /// [`MediaError::Core`] when the correction is not one the method can justify.
    pub async fn cut_calibrated(
        &self,
        media: &MediaInfo,
        segment: &Segment,
        preset: &DeliveryPreset,
        config: &CutConfig,
        output: &Path,
        measured_offset: i64,
        options: &RunOptions,
    ) -> MediaResult<CutOutcome> {
        let keyframes = self.cached_keyframes(media, segment, options).await?;
        let plan = plan_cut(media, segment, &keyframes)?;
        let corrected = apply_calibration(&plan, measured_offset)?;
        self.cut_with_plan(media, &corrected, preset, config, output, options)
            .await
    }

    /// The in-point-is-a-keyframe path: nothing is re-encoded at all.
    async fn copy_segment(
        &self,
        media: &MediaInfo,
        plan: &CutPlan,
        config: &CutConfig,
        output: &Path,
        options: &RunOptions,
    ) -> MediaResult<CutOutcome> {
        let prepared = prepare_copy(media, plan, config);
        let mut steps = Vec::new();
        self.run_step(&prepared, output, options, &mut steps)
            .await?;

        let facts = self.prober.probe(output).await?;
        Ok(CutOutcome {
            plan: plan.clone(),
            output: MediaPath::new(output.to_path_buf()),
            steps,
            frame_count: facts.frame_count,
            overshoot: (facts.frame_count - plan.requested_frames()).max(0),
            notes: vec!["the in point lands on a keyframe, so no frame was re-encoded".to_owned()],
        })
    }

    /// The fallback: no usable keyframe inside the segment, so all of it is re-encoded.
    async fn reencode_segment(
        &self,
        media: &MediaInfo,
        plan: &CutPlan,
        preset: &DeliveryPreset,
        config: &CutConfig,
        output: &Path,
        options: &RunOptions,
    ) -> MediaResult<CutOutcome> {
        let prepared = prepare_reencode(media, plan, preset, config);
        let mut steps = Vec::new();
        self.run_step(&prepared, output, options, &mut steps)
            .await?;

        let facts = self.prober.probe(output).await?;
        Ok(CutOutcome {
            plan: plan.clone(),
            output: MediaPath::new(output.to_path_buf()),
            steps,
            frame_count: facts.frame_count,
            overshoot: (facts.frame_count - plan.requested_frames()).max(0),
            notes: vec!["the whole segment was re-encoded".to_owned()],
        })
    }

    /// The normal path: re-encode the head, copy the body, join them.
    ///
    /// `preset` is not consulted here, and that is deliberate: a head patch *copies* the body,
    /// so it cannot honour a geometry or a loudness target. [`CutExecutor::cut_with_plan`]
    /// checks [`DeliveryPreset::preserves_picture`] first and routes a reshaping preset to
    /// [`CutExecutor::reencode_segment`] instead, so reaching this function means the preset was
    /// a passthrough. If that routing is ever changed, this comment is the thing that should be
    /// read first.
    #[allow(clippy::unused_self)]
    async fn head_patch(
        &self,
        media: &MediaInfo,
        plan: &CutPlan,
        _preset: &DeliveryPreset,
        config: &CutConfig,
        output: &Path,
        options: &RunOptions,
    ) -> MediaResult<CutOutcome> {
        let work = WorkDir::create(output)?;
        let mut steps = Vec::new();
        let mut notes = plan.notes.clone();

        let head = work.path().join("head.mp4");
        let body = work.path().join("body.mp4");
        let joined = work.path().join("joined.mp4");

        // 1. The head.
        let prepared = prepare_head(media, plan, config);
        self.run_step(&prepared, &head, options, &mut steps).await?;

        // 2. The body: picture and sound as two separate passes, then muxed.
        //
        //    One pass would make ffmpeg seek the audio to its own sync point — up to four AAC
        //    frames before the keyframe — and `-avoid_negative_ts make_zero` would then rebase
        //    the body on that earlier audio packet, so the picture would begin 80 ms into its own
        //    file and the joined segment would sit about three frames early. This is V1 ADR-002
        //    and it is the single most valuable lesson in the codebase.
        let body_steps = prepare_body(media, plan, config);
        if body_steps.len() == 1 {
            // No audio: the picture copy is the body.
            self.run_step(&body_steps[0], &body, options, &mut steps)
                .await?;
        } else {
            let picture = work.path().join("body-picture.mp4");
            let sound = work.path().join("body-sound.m4a");
            self.run_step(&body_steps[0], &picture, options, &mut steps)
                .await?;
            self.run_step(&body_steps[1], &sound, options, &mut steps)
                .await?;
            self.run_step(&body_steps[2], &body, options, &mut steps)
                .await?;
        }

        // 3. The join, with the head's duration stated rather than inferred.
        let head_seconds =
            plan.head_frames as f64 * plan.rate_denominator as f64 / plan.rate_numerator as f64;
        let listing = work.path().join("concat.txt");
        std::fs::write(&listing, concat_list(&head, &body, head_seconds)).map_err(|error| {
            MediaError::WorkingFile {
                path: listing.display().to_string(),
                reason: error.to_string(),
            }
        })?;
        let prepared = prepare_join(&listing);
        self.run_step(&prepared, &joined, options, &mut steps)
            .await?;

        // 4. Publish.
        if output.exists() {
            std::fs::remove_file(output).map_err(|error| MediaError::WorkingFile {
                path: output.display().to_string(),
                reason: error.to_string(),
            })?;
        }
        std::fs::rename(&joined, output)
            .or_else(|_| {
                // A rename across volumes fails on Windows; copy then.
                std::fs::copy(&joined, output)
                    .map(|_| ())
                    .and_then(|()| std::fs::remove_file(&joined))
            })
            .map_err(|error| MediaError::WorkingFile {
                path: output.display().to_string(),
                reason: error.to_string(),
            })?;

        let facts = self.prober.probe(output).await?;
        let overshoot = (facts.frame_count - plan.requested_frames()).max(0);
        if overshoot > 0 {
            notes.push(format!(
                "the copy stopped {overshoot} frame(s) past the out point, which is how a stream \
                 copy behaves; every frame you asked for is in the file"
            ));
        }
        Ok(CutOutcome {
            plan: plan.clone(),
            output: MediaPath::new(output.to_path_buf()),
            steps,
            frame_count: facts.frame_count,
            overshoot,
            notes,
        })
    }

    /// The MD5 of each decoded frame in a window, as `ffmpeg -f framemd5` reports it.
    ///
    /// This is the product's ground truth for "is this the same picture". Two files holding the
    /// same packets decode to identical frames, so their digests match exactly, and a re-encoded
    /// frame never does. That is what makes the alignment check a proof rather than an estimate.
    ///
    /// `-map 0:v:0 -an` is not optional: without it the muxer hashes audio frames too, and one
    /// audio frame per 21 ms quietly pads every list.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::ProcessFailed`] when ffmpeg cannot read the file, and
    /// [`MediaError::Cancelled`] when the caller cancels.
    pub async fn frame_hashes(
        &self,
        path: &Path,
        start_seconds: f64,
        count: usize,
        options: &RunOptions,
    ) -> MediaResult<Vec<String>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let args = crate::process::argv(&[
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-ss",
            &format!("{start_seconds:.6}"),
            "-i",
            path.to_str().unwrap_or_default(),
            "-map",
            "0:v:0",
            "-an",
            "-frames:v",
            &count.to_string(),
            "-f",
            "framemd5",
            "-",
        ]);
        let step = RunOptions {
            label: "hash frames".to_owned(),
            ..options.clone()
        };
        let output = self
            .runner
            .run(self.tools.path(ToolSet::Ffmpeg), &args, &step)
            .await?;
        Ok(output
            .stdout
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .filter_map(|line| {
                line.split(',')
                    .next_back()
                    .map(|field| field.trim().to_owned())
                    .filter(|field| !field.is_empty())
            })
            .collect())
    }

    /// How similar two decoded frames are, as ffmpeg's `ssim` filter reports it, 0..=1.
    ///
    /// Frame hashes cannot compare a re-encoded frame with its original — every pixel moves a little
    /// by construction — so the *head* of a cut is checked this way instead: the score peaks on the
    /// right frame and falls away on its neighbours, which is what makes "did the head land on the
    /// mark" answerable at all.
    ///
    /// Returns `None` when the comparison could not be measured, which the check reports as
    /// unmeasurable rather than as a pass.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::Cancelled`] when the caller cancels, and a process failure when ffmpeg
    /// cannot run.
    pub async fn ssim(&self, a: &Path, b: &Path, options: &RunOptions) -> MediaResult<Option<f64>> {
        let args = crate::process::argv(&[
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-i",
            a.to_str().unwrap_or_default(),
            "-i",
            b.to_str().unwrap_or_default(),
            "-lavfi",
            "ssim",
            "-f",
            "null",
            "-",
        ]);
        let step = RunOptions {
            label: "compare frames".to_owned(),
            ..options.clone()
        };
        let output = self
            .runner
            .run(self.tools.path(ToolSet::Ffmpeg), &args, &step)
            .await?;
        // ffmpeg writes the score to stderr as `... All:0.998 (18.5)`.
        Ok(output
            .stderr
            .split("All:")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<f64>().ok()))
    }

    /// Write one decoded frame as a PNG, so two frames can be compared pixel-wise.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::ProcessFailed`] when ffmpeg cannot read the frame.
    pub async fn frame_png(
        &self,
        path: &Path,
        at_seconds: f64,
        out: &Path,
        options: &RunOptions,
    ) -> MediaResult<bool> {
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let args = crate::process::argv(&[
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-ss",
            &format!("{at_seconds:.6}"),
            "-i",
            path.to_str().unwrap_or_default(),
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            out.to_str().unwrap_or_default(),
        ]);
        let step = RunOptions {
            label: "extract a frame".to_owned(),
            ..options.clone()
        };
        self.runner
            .run(self.tools.path(ToolSet::Ffmpeg), &args, &step)
            .await?;
        Ok(out.is_file() && out.metadata().is_ok_and(|meta| meta.len() > 0))
    }

    /// Run one prepared step, recording it.
    async fn run_step(
        &self,
        prepared: &Prepared,
        output: &Path,
        options: &RunOptions,
        steps: &mut Vec<ExecutionStep>,
    ) -> MediaResult<()> {
        let step_options = RunOptions {
            label: prepared.label.clone(),
            ..options.clone()
        };
        // The output path is appended here rather than by each builder. A builder is a pure function
        // of the plan and knows nothing about where a file is going, which is what lets it be tested
        // as data; the caller that knows the destination is the one that names it.
        //
        // An earlier version had the builders omit the path and the caller forget it, so every step
        // failed with `At least one output file must be specified`. No test of the argument *vectors*
        // could have caught that — which is precisely what the end-to-end test is for.
        let mut args: Vec<std::ffi::OsString> = prepared.os_args();
        args.push(output.as_os_str().to_os_string());

        let started = std::time::Instant::now();
        let result = self
            .runner
            .run(self.tools.path(ToolSet::Ffmpeg), &args, &step_options)
            .await;

        let mut recorded = prepared.args.clone();
        recorded.push(output.display().to_string());
        steps.push(ExecutionStep {
            label: prepared.label.clone(),
            program: self.tools.path(ToolSet::Ffmpeg).display().to_string(),
            args: recorded,
            seconds: Some(started.elapsed().as_secs_f64()),
            ok: Some(result.is_ok()),
        });
        result.map(|_| ())
    }
}

/// A temporary directory that cleans itself up.
///
/// The intermediates of a head patch are the head, the body picture, the body sound and the
/// joined file. Leaving them behind would silently double the disk a trim consumes; deleting
/// them in a `Drop` means a cancelled or failed trim cleans up too, which a `finally` around
/// only the success path would not.
struct WorkDir {
    path: PathBuf,
    keep: bool,
}

impl WorkDir {
    fn create(output: &Path) -> MediaResult<Self> {
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        let name = format!(
            ".trimmer-{}-{}",
            std::process::id(),
            output.file_stem().map_or_else(
                || "work".to_owned(),
                |stem| stem.to_string_lossy().into_owned()
            )
        );
        let path = parent.join(name);
        std::fs::create_dir_all(&path).map_err(|error| MediaError::WorkingFile {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        Ok(Self { path, keep: false })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Argument builders. Pure functions of the plan, testable without a process.
// ---------------------------------------------------------------------------------------

/// The ffmpeg arguments every call starts with.
///
/// `-v error` rather than `-v warning`: the log is built from the steps this crate records, and
/// ffmpeg's informational chatter adds nothing but noise. A failure still prints its reason.
#[must_use]
pub fn common_head_args() -> Vec<String> {
    ["-hide_banner", "-nostdin", "-v", "error", "-y"]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect()
}

/// Map the picture, and the sound too when there is any.
fn map_streams(has_audio: bool) -> Vec<String> {
    let mut args = vec!["-map".to_owned(), "0:v:0".to_owned()];
    if has_audio {
        args.push("-map".to_owned());
        args.push("0:a:0".to_owned());
    }
    args
}

/// Build the head encode.
///
/// Three flags here are the whole point and each one guards a defect:
///
/// * `-video_track_timescale` set to the source's own timescale. Without it libx264 chooses
///   `1/15360`, the muxer rescales the copied body to match, and the deliverable plays in slow
///   motion with frozen stretches while ffmpeg exits 0.
/// * the encoder chosen from the source codec, so the track holds one sample description.
/// * `-r` pinned to the source rate, so the head cannot introduce a rate change.
#[must_use]
pub fn prepare_head(media: &MediaInfo, plan: &CutPlan, config: &CutConfig) -> Prepared {
    let mut args = common_head_args();
    args.extend([
        "-ss".to_owned(),
        format!("{:.6}", media.seconds_of(plan.start_frame)),
        "-i".to_owned(),
        media.path.to_string(),
        "-t".to_owned(),
        format!("{:.6}", plan.head_seconds()),
    ]);
    args.extend(map_streams(plan.has_audio));
    args.extend(["-vf".to_owned(), "setpts=PTS-STARTPTS".to_owned()]);
    args.extend([
        "-c:v".to_owned(),
        plan.head_encoder.clone(),
        "-preset".to_owned(),
        config.preset.clone(),
        "-crf".to_owned(),
        config.crf.to_string(),
        "-pix_fmt".to_owned(),
        CutConfig::pixel_format(&media.pix_fmt),
        "-r".to_owned(),
        media.rate.as_ffmpeg(),
        "-video_track_timescale".to_owned(),
        plan.video_timescale.to_string(),
    ]);
    args.extend(plan.head_extra_args.iter().cloned());
    if plan.has_audio {
        if config.head_audio_copy {
            args.extend(["-c:a".to_owned(), "copy".to_owned()]);
        } else if let Some(audio) = &media.audio {
            args.extend([
                "-af".to_owned(),
                "asetpts=PTS-STARTPTS".to_owned(),
                "-c:a".to_owned(),
                "aac".to_owned(),
                "-b:a".to_owned(),
                config.audio_bitrate.clone(),
                "-ar".to_owned(),
                audio.sample_rate.to_string(),
                "-ac".to_owned(),
                audio.channels.to_string(),
            ]);
        }
    }
    if config.faststart {
        args.extend(["-movflags".to_owned(), "+faststart".to_owned()]);
    }
    // The output path is appended by the caller, which is the only thing that knows it.
    Prepared {
        label: format!(
            "head encode, frames {}..{}",
            plan.start_frame,
            plan.keyframe.unwrap_or(plan.start_frame)
        ),
        args,
    }
}

/// Build the body copy. One, three or three steps depending on the streams.
///
/// With audio, this returns **three** prepared commands: picture copied with an *input* seek on
/// the keyframe, sound copied with an *output* seek so the packets before the mark are dropped
/// rather than hunted for, and a mux of the two. See the module documentation for why one pass
/// is not acceptable.
#[must_use]
pub fn prepare_body(media: &MediaInfo, plan: &CutPlan, _config: &CutConfig) -> Vec<Prepared> {
    let Some(keyframe) = plan.keyframe else {
        return Vec::new();
    };
    let body_frames = plan.body_frames.max(0);
    let duration = body_frames as f64 * plan.rate_denominator as f64 / plan.rate_numerator as f64;

    let picture_args = {
        let mut args = common_head_args();
        args.extend([
            "-ss".to_owned(),
            format!("{:.6}", media.seconds_of(keyframe)),
            "-i".to_owned(),
            media.path.to_string(),
            "-t".to_owned(),
            format!("{duration:.6}"),
            "-map".to_owned(),
            "0:v:0".to_owned(),
            "-c".to_owned(),
            "copy".to_owned(),
            "-avoid_negative_ts".to_owned(),
            "make_zero".to_owned(),
        ]);
        args
    };

    if !plan.has_audio {
        return vec![Prepared {
            label: format!("body copy, frames {keyframe}.. from the original packets"),
            args: picture_args,
        }];
    }

    let sound_args = {
        let mut args = common_head_args();
        args.extend([
            // No `-ss` before `-i`: this is an *output* seek, which decodes from the start and
            // discards, so the sound lands on the mark instead of on an earlier sync point.
            "-i".to_owned(),
            media.path.to_string(),
            "-ss".to_owned(),
            format!("{:.6}", media.seconds_of(keyframe)),
            "-t".to_owned(),
            format!("{duration:.6}"),
            "-map".to_owned(),
            "0:a:0".to_owned(),
            "-c".to_owned(),
            "copy".to_owned(),
        ]);
        args
    };

    let mux_args = {
        let mut args = common_head_args();
        args.extend([
            "-i".to_owned(),
            "PICTURE".to_owned(),
            "-i".to_owned(),
            "SOUND".to_owned(),
            "-map".to_owned(),
            "0:v:0".to_owned(),
            "-map".to_owned(),
            "1:a:0".to_owned(),
            "-c".to_owned(),
            "copy".to_owned(),
            "-avoid_negative_ts".to_owned(),
            "make_zero".to_owned(),
        ]);
        args
    };

    vec![
        Prepared {
            label: format!("body picture copy, frames {keyframe}.."),
            args: picture_args,
        },
        Prepared {
            label: "body sound copy, cut on an output seek".to_owned(),
            args: sound_args,
        },
        Prepared {
            label: "body mux, picture and sound".to_owned(),
            args: mux_args,
        },
    ]
}

/// Build the concat join.
#[must_use]
pub fn prepare_join(listing: &Path) -> Prepared {
    let mut args = common_head_args();
    args.extend([
        "-f".to_owned(),
        "concat".to_owned(),
        "-safe".to_owned(),
        "0".to_owned(),
        "-i".to_owned(),
        listing.display().to_string(),
        "-c".to_owned(),
        "copy".to_owned(),
        "-movflags".to_owned(),
        "+faststart".to_owned(),
    ]);
    Prepared {
        label: "join head and body".to_owned(),
        args,
    }
}

/// Build the whole-segment copy, for an in point that lands on a keyframe.
///
/// The cut is bounded by **time**, one frame past the out point. Pinning the count with
/// `-frames:v` looks tempting — an MP4 holds one packet per video frame — but with B-frames
/// `-frames:v` counts packets in *decode* order, which is not presentation order. Measured on a
/// 91-frame cut in V1: the last frame kept was the source's 151st, and the 150th that had been
/// asked for was gone, while the file still reported "91 frames, exactly as asked". Time-based
/// stops cannot do that.
#[must_use]
pub fn prepare_copy(media: &MediaInfo, plan: &CutPlan, config: &CutConfig) -> Prepared {
    let mut args = common_head_args();
    args.extend([
        "-ss".to_owned(),
        format!("{:.6}", media.seconds_of(plan.start_frame)),
        "-i".to_owned(),
        media.path.to_string(),
    ]);
    args.extend(map_streams(plan.has_audio));
    args.extend([
        "-c".to_owned(),
        "copy".to_owned(),
        "-t".to_owned(),
        format!(
            "{:.6}",
            (plan.requested_frames() + 1) as f64 * plan.rate_denominator as f64
                / plan.rate_numerator as f64
        ),
    ]);
    if config.faststart {
        args.extend(["-movflags".to_owned(), "+faststart".to_owned()]);
    }
    Prepared {
        label: "lossless copy, no re-encode".to_owned(),
        args,
    }
}

/// Build the whole-segment re-encode, including any geometry the preset asks for.
#[must_use]
pub fn prepare_reencode(
    media: &MediaInfo,
    plan: &CutPlan,
    preset: &DeliveryPreset,
    config: &CutConfig,
) -> Prepared {
    // The preset says what to encode with and how good; the plan says which codec family the
    // body uses. A passthrough preset names no encoder, so the plan's own head encoder is used
    // and the config supplies the quality.
    let (encoder, crf, speed) = match &preset.video {
        VideoTreatment::Encode {
            encoder,
            quality,
            speed,
            ..
        } => (encoder.clone(), *quality, speed.clone()),
        _ => (plan.head_encoder.clone(), config.crf, config.preset.clone()),
    };

    let mut args = common_head_args();
    args.extend([
        "-ss".to_owned(),
        format!("{:.6}", media.seconds_of(plan.start_frame)),
        "-i".to_owned(),
        media.path.to_string(),
        "-t".to_owned(),
        format!(
            "{:.6}",
            plan.requested_frames() as f64 * plan.rate_denominator as f64
                / plan.rate_numerator as f64
        ),
    ]);
    let mut filters = vec!["setpts=PTS-STARTPTS".to_owned()];
    filters.extend(preset.geometry.filters());
    args.extend(map_streams(plan.has_audio));
    args.extend(["-vf".to_owned(), filters.join(",")]);
    args.extend([
        "-c:v".to_owned(),
        encoder,
        "-preset".to_owned(),
        speed,
        "-pix_fmt".to_owned(),
        CutConfig::pixel_format(&media.pix_fmt),
        "-video_track_timescale".to_owned(),
        plan.video_timescale.to_string(),
    ]);
    if crf > 0 {
        args.extend(["-crf".to_owned(), crf.to_string()]);
    }
    args.extend(plan.head_extra_args.iter().cloned());
    if plan.has_audio {
        if let Some(audio) = &media.audio {
            let mut audio_filter = vec!["asetpts=PTS-STARTPTS".to_owned()];
            if let Some(loudness) = preset.loudness {
                audio_filter.push(loudness.filter_args());
            }
            args.extend([
                "-af".to_owned(),
                audio_filter.join(","),
                "-c:a".to_owned(),
                "aac".to_owned(),
                "-b:a".to_owned(),
                config.audio_bitrate.clone(),
                "-ar".to_owned(),
                audio.sample_rate.to_string(),
                "-ac".to_owned(),
                audio.channels.to_string(),
            ]);
        }
    }
    if config.faststart && preset.container.supports_faststart() {
        args.extend(["-movflags".to_owned(), "+faststart".to_owned()]);
    }
    Prepared {
        label: "encode the whole segment".to_owned(),
        args,
    }
}

/// The concat list, with the head's duration **stated** rather than inferred.
///
/// Without the `duration` line the demuxer uses whatever duration the head's container reports,
/// and on these sources that is a few frames short of the head's actual content — which is
/// exactly how the body ends up repeating the end of the head. Stating it removes the guess.
#[must_use]
pub fn concat_list(head: &Path, body: &Path, head_seconds: f64) -> String {
    format!(
        "ffconcat version 1.0\nfile '{head}'\nduration {head_seconds:.6}\nfile '{body}'\n",
        head = escape_concat_path(head),
        body = escape_concat_path(body),
        head_seconds = head_seconds,
    )
}

/// The concat demuxer's own quoting: single quotes, with an embedded single quote written as
/// `'\''`. A path containing an apostrophe would otherwise terminate the quoted string and the
/// demuxer would read the rest of the path as directives.
fn escape_concat_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', r"'\''")
}

/// True when a geometry would force the picture to be re-encoded.
#[must_use]
pub fn geometry_forces_encode(media: &MediaInfo, geometry: Geometry) -> bool {
    geometry.forces_encode(media.width, media.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trimmer_core::{AudioFormat, FrameRate, Segment, Timescale};

    fn media() -> MediaInfo {
        MediaInfo {
            path: MediaPath::new(r"H:\masters\Andy Ross.mp4"),
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

    /// A plan for the default source, with keyframes at 900 and 1 400 so an in point of 1 000
    /// gives a 400-frame head.
    fn plan_for(start: i64, end: i64) -> CutPlan {
        plan_for_media(&media(), start, end)
    }

    /// A plan for a *specific* source. Tests that vary the source must build their plan from the
    /// same value the command builder is given: a plan carries the encoder and the timescale, so
    /// pairing a plan made for one source with a different source is a test that proves nothing.
    fn plan_for_media(media: &MediaInfo, start: i64, end: i64) -> CutPlan {
        let segment = Segment::new(media.path.clone(), "s", start, end);
        let keyframes = KeyframeGrid::new(vec![400, 900, 1_400, 1_900], start - 200, end);
        plan_cut(media, &segment, &keyframes).expect("plans")
    }

    fn find(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    }

    #[test]
    fn the_head_encode_pins_the_source_timescale_which_is_what_stops_the_slow_motion_bug() {
        let media = media();
        let plan = plan_for(1_000, 1_600);
        let prepared = prepare_head(&media, &plan, &CutConfig::default());
        assert_eq!(
            find(&prepared.args, "-video_track_timescale").as_deref(),
            Some("90000"),
            "without the source timescale the muxer rescales the copied body"
        );
    }

    #[test]
    fn the_head_encode_pins_the_source_rate_and_encoder() {
        let media = media();
        let plan = plan_for(1_000, 1_600);
        let prepared = prepare_head(&media, &plan, &CutConfig::default());
        assert_eq!(find(&prepared.args, "-r").as_deref(), Some("30000/1001"));
        assert_eq!(find(&prepared.args, "-c:v").as_deref(), Some("libx264"));
        assert_eq!(find(&prepared.args, "-crf").as_deref(), Some("18"));
        assert_eq!(find(&prepared.args, "-pix_fmt").as_deref(), Some("yuv420p"));
    }

    #[test]
    fn the_head_encode_uses_the_encoder_that_matches_an_hevc_body() {
        let mut media = media();
        media.codec = "hevc".to_owned();
        let plan = plan_for_media(&media, 1_000, 1_600);
        let prepared = prepare_head(&media, &plan, &CutConfig::default());
        assert_eq!(find(&prepared.args, "-c:v").as_deref(), Some("libx265"));
        // The hvc1 tag comes from the plan, so the muxer writes a track players accept.
        assert!(prepared.args.contains(&"hvc1".to_owned()));
    }

    #[test]
    fn the_head_encode_asks_for_the_clip_length_and_resets_the_timestamps() {
        let media = media();
        let plan = plan_for(1_000, 1_600);
        let prepared = prepare_head(&media, &plan, &CutConfig::default());
        // 400 frames at 29.97 is 13.346 667 s.
        let requested: f64 = find(&prepared.args, "-t")
            .expect("has -t")
            .parse()
            .expect("num");
        assert!(
            (requested - 400.0 * 1001.0 / 30_000.0).abs() < 1e-6,
            "{requested}"
        );
        assert!(prepared.args.contains(&"setpts=PTS-STARTPTS".to_owned()));
        // The seek is an input seek, before -i, so the decoder starts on the mark.
        let ss_index = prepared
            .args
            .iter()
            .position(|arg| arg == "-ss")
            .expect("-ss");
        let i_index = prepared
            .args
            .iter()
            .position(|arg| arg == "-i")
            .expect("-i");
        assert!(ss_index < i_index, "the head seek must be an input seek");
    }

    #[test]
    fn an_unsupported_pixel_format_is_converted_rather_than_passed_through() {
        assert_eq!(CutConfig::pixel_format("yuv420p"), "yuv420p");
        assert_eq!(CutConfig::pixel_format("yuv420p10le"), "yuv420p10le");
        assert_eq!(CutConfig::pixel_format("yuv444p12le"), "yuv420p");
        assert_eq!(CutConfig::pixel_format("rgb24"), "yuv420p");
    }

    #[test]
    fn the_body_is_copied_as_three_separate_steps_when_there_is_sound() {
        let media = media();
        let plan = plan_for(1_000, 1_600);
        let steps = prepare_body(&media, &plan, &CutConfig::default());
        assert_eq!(steps.len(), 3, "picture, sound and mux");
        assert!(steps[0].label.contains("picture"));
        assert!(steps[1].label.contains("sound"));
        assert!(steps[2].label.contains("mux"));
    }

    #[test]
    fn the_body_picture_is_seeked_as_an_input_and_the_sound_as_an_output() {
        let media = media();
        let plan = plan_for(1_000, 1_600);
        let steps = prepare_body(&media, &plan, &CutConfig::default());

        // Picture: `-ss` before `-i`.
        let picture = &steps[0].args;
        let picture_ss = picture.iter().position(|arg| arg == "-ss").expect("-ss");
        let picture_i = picture.iter().position(|arg| arg == "-i").expect("-i");
        assert!(picture_ss < picture_i, "the picture must use an input seek");

        // Sound: `-ss` AFTER `-i`, which is the whole point.
        let sound = &steps[1].args;
        let sound_i = sound.iter().position(|arg| arg == "-i").expect("-i");
        let sound_ss = sound.iter().position(|arg| arg == "-ss").expect("-ss");
        assert!(
            sound_i < sound_ss,
            "the sound must use an output seek so it drops the packets before the mark instead \
             of hunting for an audio sync point"
        );
    }

    #[test]
    fn a_source_with_no_audio_produces_one_body_step_not_three() {
        let mut media = media();
        media.audio = None;
        let plan = plan_for_media(&media, 1_000, 1_600);
        let steps = prepare_body(&media, &plan, &CutConfig::default());
        assert_eq!(steps.len(), 1);
        assert!(steps[0].args.contains(&"0:v:0".to_owned()));
        assert!(!steps[0].args.contains(&"0:a:0".to_owned()));
    }

    #[test]
    fn the_copy_path_is_bounded_by_time_and_never_by_a_frame_count() {
        let media = media();
        let plan = plan_for(900, 1_400);
        assert_eq!(plan.mode, CutMode::Copy);
        let prepared = prepare_copy(&media, &plan, &CutConfig::default());
        assert!(
            !prepared.args.contains(&"-frames:v".to_owned()),
            "`-frames:v` counts packets in decode order with B-frames, and drops a wanted frame"
        );
        let requested: f64 = find(&prepared.args, "-t")
            .expect("has -t")
            .parse()
            .expect("num");
        // 500 frames requested, bounded at 501 frames of time.
        assert!(
            (requested - 501.0 * 1001.0 / 30_000.0).abs() < 1e-6,
            "{requested}"
        );
    }

    #[test]
    fn the_concat_list_states_the_head_duration() {
        let text = concat_list(
            Path::new(r"H:\work\head.mp4"),
            Path::new(r"H:\work\body.mp4"),
            1.5,
        );
        assert!(text.starts_with("ffconcat version 1.0\n"));
        assert!(text.contains("file 'H:\\work\\head.mp4'\n"));
        assert!(text.contains("duration 1.500000\n"), "{text}");
        assert!(text.contains("file 'H:\\work\\body.mp4'\n"));
        // The duration line must come between the two files, not after both.
        let head = text.find("head.mp4").expect("head");
        let duration = text.find("duration").expect("duration");
        let body = text.find("body.mp4").expect("body");
        assert!(head < duration && duration < body);
    }

    #[test]
    fn an_apostrophe_in_a_path_cannot_escape_the_concat_quoting() {
        // A directory called `Andy's masters` is not exotic, and an unescaped quote would make
        // the demuxer read the rest of the path as directives.
        let text = concat_list(
            Path::new(r"H:\Andy's masters\head.mp4"),
            Path::new(r"H:\Andy's masters\body.mp4"),
            1.0,
        );
        assert!(text.contains(r"'\''"), "{text}");
        assert!(
            !text.contains("file 'H:\\Andy's"),
            "the quote was not escaped: {text}"
        );
    }

    #[test]
    fn the_join_copies_rather_than_re_encodes() {
        let prepared = prepare_join(Path::new(r"H:\work\concat.txt"));
        assert_eq!(find(&prepared.args, "-c").as_deref(), Some("copy"));
        assert_eq!(find(&prepared.args, "-f").as_deref(), Some("concat"));
        assert_eq!(find(&prepared.args, "-safe").as_deref(), Some("0"));
        assert!(prepared.args.contains(&"+faststart".to_owned()));
    }

    #[test]
    fn a_geometry_change_is_applied_as_filters_and_re_encodes() {
        let media = media();
        let plan = plan_for(1_000, 1_600);
        let preset = trimmer_core::delivery::standard_preset("vertical").expect("preset");
        let prepared = prepare_reencode(&media, &plan, &preset, &CutConfig::default());
        let filters = find(&prepared.args, "-vf").expect("has filters");
        assert!(filters.contains("crop=1080:1920"), "{filters}");
        // The preset's quality and speed win over the config's.
        assert_eq!(find(&prepared.args, "-crf").as_deref(), Some("20"));
        assert_eq!(find(&prepared.args, "-preset").as_deref(), Some("medium"));
    }

    #[test]
    fn a_loudness_target_is_applied_to_the_re_encoded_audio() {
        let media = media();
        let plan = plan_for(1_000, 1_600);
        let preset = trimmer_core::delivery::standard_preset("youtube_1080").expect("preset");
        let prepared = prepare_reencode(&media, &plan, &preset, &CutConfig::default());
        let filter = find(&prepared.args, "-af").expect("has an audio filter");
        assert!(filter.contains("loudnorm=I=-14.0"), "{filter}");
        assert!(filter.contains("asetpts=PTS-STARTPTS"), "{filter}");
    }

    #[test]
    fn the_master_preset_leaves_the_picture_and_sound_untouched_when_copied() {
        let media = media();
        let plan = plan_for(900, 1_400);
        let prepared = prepare_copy(&media, &plan, &CutConfig::default());
        assert_eq!(find(&prepared.args, "-c").as_deref(), Some("copy"));
        assert!(find(&prepared.args, "-af").is_none());
        assert!(find(&prepared.args, "-vf").is_none());
    }

    #[test]
    fn every_prepared_command_starts_with_the_quiet_flags() {
        let media = media();
        let plan = plan_for(1_000, 1_600);
        let prepared = [
            prepare_head(&media, &plan, &CutConfig::default()),
            prepare_join(Path::new("x")),
            prepare_copy(&media, &plan, &CutConfig::default()),
            prepare_reencode(
                &media,
                &plan,
                &trimmer_core::delivery::standard_preset("master").expect("preset"),
                &CutConfig::default(),
            ),
        ];
        for command in prepared {
            assert!(command.args.contains(&"-nostdin".to_owned()), "{command:?}");
            assert!(command.args.contains(&"-y".to_owned()), "{command:?}");
            // Nothing may be left to a shell, so no argument may be a shell metacharacter.
            for arg in &command.args {
                assert!(!arg.contains("&&"), "{arg}");
                assert!(!arg.contains('|'), "{arg}");
            }
        }
    }

    #[test]
    fn a_preset_that_reshapes_the_frame_is_detected_before_the_cut_starts() {
        let media = media();
        let vertical = trimmer_core::delivery::standard_preset("vertical").expect("preset");
        assert!(geometry_forces_encode(&media, vertical.geometry));
        assert!(!geometry_forces_encode(
            &media,
            trimmer_core::delivery::standard_preset("master")
                .expect("preset")
                .geometry
        ));
    }

    #[test]
    fn a_long_head_is_recognised_as_worth_lookahead() {
        let config = CutConfig::default();
        assert!(!config.wants_lookahead(2.0));
        assert!(config.wants_lookahead(12.0));
    }

    #[test]
    fn the_command_line_of_a_step_is_reconstructible_for_the_audit_log() {
        let step = ExecutionStep {
            label: "head encode".to_owned(),
            program: "ffmpeg".to_owned(),
            args: vec!["-i".to_owned(), r"H:\a b.mp4".to_owned()],
            seconds: Some(1.25),
            ok: Some(true),
        };
        assert_eq!(step.command_line(), r"ffmpeg -i H:\a b.mp4");
    }
}

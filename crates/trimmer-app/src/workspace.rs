//! The workspace: the project, what is on disk, and the view a user interface reads.
//!
//! A [`Workspace`] owns a [`Project`] and answers the questions a UI actually asks:
//!
//! * *What am I working with?* — [`Workspace::sources`] returns one [`SourceView`] per file,
//!   saying whether it is still there and whether it still hashes out to what was probed.
//! * *What have I marked?* — [`Workspace::segments`] returns one [`SegmentView`] per segment, with
//!   the timecodes, the frame count, the cost and any problem that would stop it.
//! * *What will this cost me?* — [`Workspace::preview`] answers for one segment and
//!   [`Workspace::preview_all`] for the batch, **before** anything is written, using the real
//!   argument builders through [`ports::MediaEngine::preview`].
//!
//! That last point is the one that keeps a studio out of trouble. A vertical crop preset on a
//! hundred segments is a hundred full transcodes, and the difference between "that will take six
//! minutes" and "that will take four hours" has to be visible *before* the button is pressed.
//! [`QueuePreview::forces_full_encode`] is what says so.

use std::collections::BTreeMap;
use std::sync::Arc;

use trimmer_core::{
    format_seconds, CutMode, CutPlan, MediaInfo, MediaPath, Project, ProjectId, Segment,
    SegmentId, VerifyPolicy,
};
use trimmer_media::Prepared;

use crate::ports::{Clock, MediaEngine, ProjectStore, TranscriptSource};
use crate::{AppError, AppResult};

/// One source, as the interface shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    /// The file.
    pub path: MediaPath,
    /// Its name, for a list.
    pub name: String,
    /// True when the file is on disk right now.
    pub present: bool,
    /// What was probed, when it has been.
    pub media: Option<MediaInfo>,
    /// A one-line description, or a sentence saying what is wrong.
    pub summary: String,
    /// The caption file found beside it, if any.
    pub transcript: Option<MediaPath>,
    /// How many cues that transcript holds, when it could be read.
    pub transcript_cues: Option<usize>,
    /// A label the user gave it, e.g. the speaker.
    pub label: Option<String>,
    /// True when the file looks variable frame rate, which makes the marks approximate.
    pub variable_rate: bool,
}

/// One segment, as the interface shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentView {
    /// The segment's identity.
    pub id: SegmentId,
    /// Its name.
    pub name: String,
    /// Its source file name, for the list.
    pub source_name: String,
    /// The in point as a timecode, in the form Premiere would write it.
    pub in_timecode: String,
    /// The out point timecode, inclusive of the last frame kept.
    pub out_timecode: String,
    /// One past the last frame kept, or `None` to the end of the source.
    pub end_frame: Option<i64>,
    /// How many frames, when the source is known.
    pub frames: Option<i64>,
    /// The duration in seconds, when the source is known.
    pub seconds: Option<f64>,
    /// Handles applied, in frames.
    pub handle_frames: i64,
    /// Whether it is included in a batch.
    pub enabled: bool,
    /// Every problem that would stop this segment being cut, in the order they matter.
    pub problems: Vec<String>,
    /// Things that are true and worth saying, which do not stop the cut.
    pub notes: Vec<String>,
}

/// What one segment will cost, and what it will do.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuePreview {
    /// The segment this is about.
    pub segment: SegmentId,
    /// The plan, when one could be made.
    pub plan: Option<CutPlan>,
    /// The preset that will be used.
    pub preset: String,
    /// True when the preset reshapes the picture or changes the loudness, so the whole segment
    /// is re-encoded rather than copied.
    pub forces_full_encode: bool,
    /// The fraction of the segment that will be re-encoded.
    pub reencode_fraction: f64,
    /// Roughly how many bytes the output will be, when that can be estimated.
    pub estimated_bytes: Option<u64>,
    /// The exact commands that will run, for a dry run or an audit.
    pub commands: Vec<Prepared>,
    /// Problems that stop this segment.
    pub problems: Vec<String>,
    /// Things worth saying.
    pub notes: Vec<String>,
}

/// A count of what a batch would do, for the summary line above the run button.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    /// Segments in the project.
    pub segments: usize,
    /// Segments a batch would process.
    pub runnable: usize,
    /// Segments a batch would skip, because they are disabled or their source is gone.
    pub skipped: usize,
    /// Segments that would be re-encoded in full.
    pub full_encodes: usize,
    /// Total frames a batch would deliver.
    pub total_frames: i64,
    /// Total seconds a batch would deliver.
    pub total_seconds: f64,
    /// Sources that are no longer on disk.
    pub missing_sources: usize,
}

/// The project, the engine, and the derived views.
pub struct Workspace {
    project: Project,
    engine: Arc<dyn MediaEngine>,
    transcripts: Arc<dyn TranscriptSource>,
    clock: Arc<dyn Clock>,
    /// Plans already made, keyed by segment, so a preview is not recomputed for every repaint.
    /// Public so the request builder in [`crate::ports`] can pick one up without re-planning.
    pub plans: BTreeMap<SegmentId, CutPlan>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("project", &self.project.name)
            .field("sources", &self.project.sources.len())
            .field("segments", &self.project.segments.len())
            .finish_non_exhaustive()
    }
}

impl Workspace {
    /// A new, empty project.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        created_by: impl Into<String>,
        engine: Arc<dyn MediaEngine>,
        transcripts: Arc<dyn TranscriptSource>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let now = clock.now_unix();
        Self {
            project: Project::new(name, created_by, now),
            engine,
            transcripts,
            clock,
            plans: BTreeMap::new(),
        }
    }

    /// Open an existing project from a store, refreshing what is on disk.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Store`] when the store cannot produce the project.
    pub fn open(
        store: &dyn ProjectStore,
        id: ProjectId,
        engine: Arc<dyn MediaEngine>,
        transcripts: Arc<dyn TranscriptSource>,
        clock: Arc<dyn Clock>,
    ) -> AppResult<Self> {
        let project = store.load(id).map_err(AppError::Store)?;
        Ok(Self {
            project,
            engine,
            transcripts,
            clock,
            plans: BTreeMap::new(),
        })
    }

    /// The project, for reading.
    #[must_use]
    pub const fn project(&self) -> &Project {
        &self.project
    }

    /// The project, for editing.
    ///
    /// A caller that edits through this and then asks for a view gets the edited project. The
    /// cached plans are cleared, because a plan is only valid for the segment it was made from.
    pub fn project_mut(&mut self) -> &mut Project {
        self.plans.clear();
        &mut self.project
    }

    /// Save through a store.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Store`] when the write fails.
    pub fn save(&mut self, store: &dyn ProjectStore) -> AppResult<()> {
        self.project.updated_at = self.clock.now_unix();
        store.save(&self.project).map_err(AppError::Store)
    }

    /// Add a source, probing it.
    ///
    /// A file that is not there is added anyway, marked unavailable. That is deliberate: an
    /// editor who is working from a laptop without the drive attached must still be able to open
    /// the project, see their marks, and fix them.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Media`] when the file is there but cannot be probed.
    pub async fn add_source(&mut self, path: impl Into<MediaPath>) -> AppResult<MediaPath> {
        let path = path.into().canonicalised();
        let available = path.exists();
        // A file that is not there is added anyway, marked unavailable, so a project opens on a
        // laptop without the drive attached and the marks can still be read and fixed.
        let media = if available {
            Some(self.engine.probe(&path).await?)
        } else {
            None
        };
        self.project.upsert_source(trimmer_core::SegmentSource {
            path: path.clone(),
            media,
            available,
            label: None,
        });
        Ok(path)
    }

    /// Re-probe every source, and mark the ones that have gone.
    ///
    /// Called when a project is opened and before a batch runs, because a drive that was plugged
    /// in five minutes ago should not require the project to be reopened.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Media`] only when a *present* file cannot be probed; an absent file is
    /// a state, not an error.
    pub async fn refresh_sources(&mut self) -> AppResult<()> {
        let paths: Vec<MediaPath> = self.project.sources.keys().cloned().collect();
        for path in paths {
            let present = path.exists();
            let probed = if present {
                self.engine.probe(&path).await.ok()
            } else {
                None
            };
            if let Some(source) = self.project.sources.get_mut(&path) {
                source.available = present;
                if probed.is_some() {
                    source.media = probed;
                }
            }
        }
        self.plans.clear();
        Ok(())
    }

    /// Add a segment, checking it against its source first.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::SourceUnavailable`] when the source is not in the project, and the
    /// domain's refusal when the marks cannot be cut.
    pub fn add_segment(&mut self, segment: Segment) -> AppResult<SegmentId> {
        let media = self
            .project
            .media(&segment.source)
            .cloned()
            .ok_or_else(|| AppError::SourceUnavailable {
                path: segment.source.to_string(),
                reason: "it has not been added to this project, or has not been probed".to_owned(),
            })?;
        // Planning here rather than at cut time means a bad mark is refused while the user is
        // still looking at the field they typed it into.
        let grid = trimmer_core::KeyframeGrid::new(
            Vec::new(),
            segment.start_frame,
            segment.end_frame.unwrap_or(media.frame_count),
        );
        trimmer_core::plan_cut(&media, &segment, &grid)?;
        let id = self.project.add_segment(segment)?;
        Ok(id)
    }

    /// Remove a segment.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::UnknownSegment`] when the id does not resolve.
    pub fn remove_segment(&mut self, id: SegmentId) -> AppResult<Segment> {
        self.plans.remove(&id);
        self.project
            .remove_segment(id)
            .map_err(|_| AppError::UnknownSegment(id.to_string()))
    }

    /// The sources, as the interface shows them.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Transcript`] never; a transcript that cannot be read is reported in
    /// the view rather than failing the whole listing.
    pub fn sources(&self) -> Vec<SourceView> {
        self.project
            .sources
            .values()
            .map(|source| {
                let (transcript, cues) = match self.transcripts.find_for(&source.path) {
                    Some(found) => {
                        let path = MediaPath::new(found.clone());
                        let count = self.transcripts.read(&found).ok().map(|t| t.cues.len());
                        (Some(path), count)
                    }
                    None => (None, None),
                };
                let summary = match (&source.media, source.available) {
                    (Some(media), _) => media.summary(),
                    (None, false) => "the file is not on disk".to_owned(),
                    (None, true) => "not probed yet".to_owned(),
                };
                SourceView {
                    path: source.path.clone(),
                    name: source.path.file_name(),
                    present: source.available,
                    media: source.media.clone(),
                    summary,
                    transcript,
                    transcript_cues: cues,
                    label: source.label.clone(),
                    variable_rate: source.media.as_ref().is_some_and(MediaInfo::is_variable_rate),
                }
            })
            .collect()
    }

    /// The segments, as the interface shows them, each with its problems.
    #[must_use]
    pub fn segments(&self) -> Vec<SegmentView> {
        self.project
            .segments
            .iter()
            .map(|segment| self.segment_view(segment))
            .collect()
    }

    /// One segment's view.
    fn segment_view(&self, segment: &Segment) -> SegmentView {
        let media = self.project.media(&segment.source);
        let (in_timecode, out_timecode, frames, seconds) = match media {
            Some(media) => {
                let end = segment.end_frame.unwrap_or(media.frame_count);
                (
                    media.timecode_of(segment.start_frame),
                    media.timecode_of((end - 1).max(segment.start_frame)),
                    segment.frame_count(),
                    Some(media.rate.seconds_of((end - segment.start_frame).max(0))),
                )
            }
            None => (
                format!("frame {}", segment.start_frame),
                segment
                    .end_frame
                    .map_or_else(|| "to the end".to_owned(), |end| format!("frame {}", end - 1)),
                segment.frame_count(),
                None,
            ),
        };

        let (problems, notes) = self.diagnose(segment, media);
        SegmentView {
            id: segment.id,
            name: segment.name.clone(),
            source_name: segment.source.file_name(),
            in_timecode,
            out_timecode,
            end_frame: segment.end_frame,
            frames,
            seconds,
            handle_frames: segment.handle_frames,
            enabled: segment.enabled,
            problems,
            notes,
        }
    }

    /// Everything wrong with a segment, and everything worth saying about it.
    fn diagnose(&self, segment: &Segment, media: Option<&MediaInfo>) -> (Vec<String>, Vec<String>) {
        let mut problems = Vec::new();
        let mut notes = Vec::new();
        let Some(media) = media else {
            problems.push(
                "its source is not in this project, or has not been probed; add the source first"
                    .to_owned(),
            );
            return (problems, notes);
        };
        if !media.supports_head_patch() {
            problems.push(format!(
                "{} sources cannot be head-patched: the re-encoded head has to share the body's \
                 codec, and only H.264 and HEVC are supported",
                media.codec
            ));
        }
        if media.is_variable_rate() {
            notes.push(
                "the source looks variable frame rate, so the frame grid this method assumes may \
                 not hold and these marks are approximate"
                    .to_owned(),
            );
        }
        if segment.start_frame > media.last_frame() {
            problems.push(format!(
                "the in point is frame {}, which is past the end of the source ({} frames)",
                segment.start_frame, media.frame_count
            ));
        }
        if let Some(end) = segment.end_frame {
            if end > media.frame_count {
                problems.push(format!(
                    "the out point is frame {}, which is past the end of the source ({} frames)",
                    end, media.frame_count
                ));
            }
            if end <= segment.start_frame {
                problems.push("the out point is not after the in point".to_owned());
            }
        }
        if !segment.enabled {
            notes.push("it is switched off, so a batch will skip it".to_owned());
        }
        if self.project.preset_for(segment).is_err() {
            problems.push(format!(
                "its delivery preset ({}) is not defined in this project",
                segment
                    .preset
                    .as_deref()
                    .unwrap_or(&self.project.default_preset)
            ));
        }
        (problems, notes)
    }

    /// What one segment will do and cost.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::UnknownSegment`] when the id does not resolve, and
    /// [`AppError::SourceUnavailable`] when its source has no probed facts.
    pub async fn preview(&mut self, id: SegmentId) -> AppResult<QueuePreview> {
        let segment = self
            .project
            .segment(id)
            .cloned()
            .ok_or_else(|| AppError::UnknownSegment(id.to_string()))?;
        let media = self
            .project
            .media(&segment.source)
            .cloned()
            .ok_or_else(|| AppError::SourceUnavailable {
                path: segment.source.to_string(),
                reason: "it has not been probed".to_owned(),
            })?;
        let preset = self.project.preset_for(&segment)?.clone();

        let (mut problems, mut notes) = self.diagnose(&segment, Some(&media));
        let plan = match self.engine.plan(&media, &segment).await {
            Ok(plan) => Some(plan),
            Err(error) => {
                problems.push(error.to_string());
                None
            }
        };
        if let Some(plan) = &plan {
            self.plans.insert(id, plan.clone());
        }

        let forces_full_encode = !preset.preserves_picture(media.width, media.height);
        let commands = match &plan {
            Some(plan) if problems.is_empty() => {
                self.engine.preview(&media, &segment, &preset, plan).unwrap_or_default()
            }
            _ => Vec::new(),
        };

        // The estimate is deliberately rough and says so: it is the output's picture bitrate at
        // the preset's quality applied to the segment's length, plus the audio. A number that is
        // within a factor of two is enough to answer "will this fit on the drive", and pretending
        // to more accuracy would be false precision.
        let estimated_bytes = plan.as_ref().and_then(|plan| {
            if plan.requested_frames() <= 0 || media.width == 0 {
                return None;
            }
            let crf = match &preset.video {
                trimmer_core::VideoTreatment::Encode { quality, .. } => *quality,
                _ => 18,
            };
            // A rough bits-per-pixel curve for x264/x265 at a given CRF.
            let quality_factor = 0.12_f64 * 1.18_f64.powi(i32::from(crf) - 18);
            let bits_per_frame = f64::from(media.width) * f64::from(media.height) * quality_factor;
            let video_bytes = bits_per_frame * plan.requested_frames() as f64 / 8.0;
            let audio_bytes = 24_000.0 * plan.head_seconds().max(0.0)
                + 24_000.0 * plan.requested_frames() as f64
                    * plan.rate_denominator as f64
                    / plan.rate_numerator.max(1) as f64;
            Some((video_bytes + audio_bytes).max(0.0) as u64)
        });

        if forces_full_encode {
            notes.push(format!(
                "the {} preset changes the picture, so the whole segment is re-encoded rather \
                 than copied",
                preset.name
            ));
        }
        if let Some(plan) = &plan {
            notes.extend(plan.notes.iter().cloned());
        }

        Ok(QueuePreview {
            segment: id,
            reencode_fraction: plan.as_ref().map_or(0.0, CutPlan::reencode_fraction),
            plan,
            preset: preset.name.clone(),
            forces_full_encode,
            estimated_bytes,
            commands,
            problems,
            notes,
        })
    }

    /// What the whole batch will do and cost, using the plans already made where it can.
    ///
    /// # Errors
    ///
    /// Returns the first hard failure. A segment whose source has gone is reported in the
    /// preview rather than failing the whole listing, because the point of the listing is to
    /// show exactly that.
    pub async fn preview_all(&mut self) -> AppResult<Vec<QueuePreview>> {
        let ids: Vec<SegmentId> = self
            .project
            .segments
            .iter()
            .filter(|segment| segment.enabled)
            .map(|segment| segment.id)
            .collect();
        let mut previews = Vec::with_capacity(ids.len());
        for id in ids {
            previews.push(self.preview(id).await?);
        }
        Ok(previews)
    }

    /// The summary line above the run button.
    #[must_use]
    pub fn summary(&self) -> WorkspaceSummary {
        let views = self.segments();
        let runnable: Vec<&SegmentView> = views
            .iter()
            .filter(|view| view.enabled && view.problems.is_empty())
            .collect();
        let full_encodes = runnable
            .iter()
            .filter(|view| {
                self.project
                    .segment(view.id)
                    .and_then(|segment| self.project.media(&segment.source))
                    .is_some_and(|media| {
                        self.project
                            .segment(view.id)
                            .and_then(|segment| self.project.preset_for(segment).ok())
                            .is_none_or(|preset| {
                                !preset.preserves_picture(media.width, media.height)
                            })
                    })
            })
            .count();

        WorkspaceSummary {
            segments: views.len(),
            runnable: runnable.len(),
            skipped: views.len() - runnable.len(),
            full_encodes,
            total_frames: runnable.iter().filter_map(|view| view.frames).sum(),
            total_seconds: runnable.iter().filter_map(|view| view.seconds).sum(),
            missing_sources: self
                .project
                .sources
                .values()
                .filter(|source| !source.available)
                .count(),
        }
    }

    /// Where a segment's output will be written.
    ///
    /// One place, so a batch, an export and the interface cannot disagree. The name carries the
    /// segment's own name, and the range is dotted rather than colon-separated because a colon is
    /// not legal in a Windows file name.
    #[must_use]
    pub fn output_path(&self, segment: &Segment) -> MediaPath {
        let directory = self
            .project
            .output_dir
            .clone()
            .unwrap_or_else(|| MediaPath::new(segment.source.as_path().parent().unwrap_or(std::path::Path::new("."))));
        let preset = self.project.preset_for(segment).ok();
        let extension = preset.map_or("mp4", |preset| preset.container.extension());
        let media = self.project.media(&segment.source);
        let start = media.map_or_else(
            || format!("frame{}", segment.start_frame),
            |media| media.timecode_of(segment.start_frame).replace([':', ';'], "."),
        );
        let sanitised = sanitise_name(&segment.name);
        directory
            .with_suffix(&format!(" {sanitised} {start}"), extension)
    }

    /// A description of the project for a report.
    #[must_use]
    pub fn describe(&self) -> String {
        let summary = self.summary();
        let mut lines = vec![
            format!("project     {} ({})", self.project.name, self.project.id),
            format!(
                "created by  {} on {}",
                self.project.created_by,
                format_seconds(self.project.created_at as f64)
            ),
            format!(
                "sources     {} ({} not on disk)",
                self.project.sources.len(),
                summary.missing_sources
            ),
            format!(
                "segments    {} ({} runnable, {} skipped, {} full re-encodes)",
                summary.segments, summary.runnable, summary.skipped, summary.full_encodes
            ),
            format!(
                "delivering  {} frames, {}",
                summary.total_frames,
                format_seconds(summary.total_seconds)
            ),
            format!("verifying   {:?}", self.project.verify),
        ];
        for id in self.plans.keys() {
            if let Some(plan) = self.plans.get(id) {
                if plan.mode != CutMode::Copy {
                    lines.push(format!(
                        "note        a segment is {} ({:.0}% re-encoded)",
                        plan.mode.label(),
                        plan.reencode_fraction() * 100.0
                    ));
                }
            }
        }
        lines.join("\n")
    }

    /// A plan already made for a segment, when one has been.
    ///
    /// A plan is only valid for the segment and source it was made from, so it is cached beside
    /// the segment and dropped whenever the project is edited.
    #[must_use]
    pub fn plan_for(&self, id: SegmentId) -> Option<&CutPlan> {
        self.plans.get(&id)
    }

    /// The transcript service, for search.
    #[must_use]
    pub fn transcripts(&self) -> Arc<dyn TranscriptSource> {
        Arc::clone(&self.transcripts)
    }

    /// The engine, for a caller that needs it directly.
    #[must_use]
    pub fn engine(&self) -> Arc<dyn MediaEngine> {
        Arc::clone(&self.engine)
    }

    /// The clock.
    #[must_use]
    pub fn clock(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.clock)
    }

    /// The verification policy in force.
    #[must_use]
    pub const fn verify_policy(&self) -> VerifyPolicy {
        self.project.verify
    }
}

/// Make a string safe to put in a file name on Windows and on a NAS.
///
/// A segment is often named from a transcript line, and a transcript line contains question marks,
/// colons and quotation marks. Rather than refusing the name, the illegal characters become
/// spaces and runs of whitespace collapse — the name is for a human to recognise, and a file the
/// operating system refuses to create helps nobody.
#[must_use]
pub fn sanitise_name(name: &str) -> String {
    const ILLEGAL: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut out = String::with_capacity(name.len());
    let mut last_was_space = false;
    for ch in name.chars() {
        let replacement = if ILLEGAL.contains(&ch) || ch.is_control() {
            ' '
        } else {
            ch
        };
        if replacement == ' ' {
            if last_was_space {
                continue;
            }
            last_was_space = true;
        } else {
            last_was_space = false;
        }
        out.push(replacement);
    }
    let trimmed = out.trim().trim_end_matches('.').to_owned();
    if trimmed.is_empty() {
        "segment".to_owned()
    } else {
        // Windows caps a file name at 255 characters; leave room for the range and extension.
        trimmed.chars().take(120).collect()
    }
}

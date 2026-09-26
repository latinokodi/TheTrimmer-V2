//! Watch folders: what to do when a file appears.
//!
//! This is how the product stops being an application and becomes infrastructure. A studio's
//! capture machine drops a master into `\\nas\ingest`; an assistant drops a marker list beside it;
//! the cuts are simply *there* ten minutes later. Nobody opens anything.
//!
//! ## Rules, not watchers
//!
//! The filesystem watching itself is `notify`'s job and lives in the daemon. What lives here is
//! the part that has to be *right*: given a folder's contents, which files are a job, what should
//! happen to them, and — the question that actually matters — **how do we avoid doing it twice?**
//!
//! A naive watch folder re-cuts the same master every time a file is touched, and a master being
//! copied in is touched many times while it is still arriving. So a job is only recognised when
//! all of these hold:
//!
//! 1. The master has a known video extension.
//! 2. Its size has stopped changing between two polls ([`WatchPolicy::settle_seconds`]) — a file
//!    that is still growing is still being copied, and cutting it produces a truncated segment.
//! 3. Its marker list exists, when one is required.
//! 4. It has not already been processed, judged by the **content fingerprint** rather than the
//!    timestamp. A timestamp changes when a file is copied again; the content does not.
//!
//! ## Marker lists
//!
//! A job needs marks. The studio supplies them as a sidecar whose name matches the master and
//! whose extension says what kind it is:
//!
//! * `<master>.marks.txt` — the simplest form, one mark per line.
//! * `<master>.csv` — a spreadsheet export.
//! * `<master>.edl` — an edit decision list from another tool.
//! * `<master>.trimmerproj` — a project exported from this application.
//!
//! A line in a text marker list is deliberately forgiving, because an assistant typing marks into
//! Notepad should not have to match a grammar:
//!
//! ```text
//! # a comment
//! cold open, 00:00:10:00, 00:02:31:12
//! 00:14:02;00 - 00:18:20;00
//! 00:31:00:00            # to the end of the master
//! ```
//!
//! The name is optional and everything after `#` is a comment. The two timecodes can be separated
//! by a comma, a dash or whitespace, because all three are what a person types.

use std::path::{Path, PathBuf};

use trimmer_core::timecode::split_timecodes;
use trimmer_core::{format_seconds, FrameRate, MediaPath};

/// What triggers a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchTrigger {
    /// A master appeared, and a marker list is beside it.
    MarkerList,
    /// A master appeared with no marks: cut it whole into a preset's format.
    WholeFile,
    /// A project file appeared.
    ProjectFile,
}

/// What a job does with a segment list once the marks are read.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchAction {
    /// Load the marks as segments and queue them.
    CutMarkedSegments,
    /// Queue the whole file as one segment.
    CutWholeFile,
    /// Open the project and run its batch.
    RunProject,
}

/// How a watch folder behaves.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchPolicy {
    /// Video extensions that are treated as a master.
    pub video_extensions: Vec<String>,
    /// Marker-list extensions, in the order they are preferred.
    pub marker_extensions: Vec<String>,
    /// Cut the master whole when no marker list is present.
    pub cut_whole_when_unmarked: bool,
    /// A file must have stopped growing for this long before it is considered arrived.
    pub settle_seconds: u64,
    /// Delivery preset to use for `WholeFile` jobs.
    pub default_preset: String,
    /// Where outputs go. `None` means beside the master.
    pub output_dir: Option<MediaPath>,
    /// Delete the marker list after a successful run, so a re-drop is visibly a new job.
    pub consume_marker_list: bool,
    /// Move processed masters to a subfolder, so the folder does not accumulate.
    pub archive_into: Option<String>,
}

impl Default for WatchPolicy {
    fn default() -> Self {
        Self {
            video_extensions: [
                "mp4", "mov", "mkv", "m4v", "mxf", "avi", "webm", "mts", "m2ts",
            ]
            .iter()
            .map(|extension| (*extension).to_owned())
            .collect(),
            // Most specific first: a project beats an EDL beats a CSV beats a text file.
            marker_extensions: ["trimmerproj", "edl", "csv", "marks.txt", "txt"]
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect(),
            cut_whole_when_unmarked: false,
            // Thirty seconds is long enough that a copy over a gigabit link has finished, and
            // short enough that an operator does not wonder whether the folder is working.
            settle_seconds: 30,
            default_preset: trimmer_core::delivery::DEFAULT_PRESET.to_owned(),
            output_dir: None,
            consume_marker_list: false,
            archive_into: None,
        }
    }
}

impl WatchPolicy {
    /// True when an extension names a master.
    #[must_use]
    pub fn is_video(&self, path: &Path) -> bool {
        extension_of(path).is_some_and(|extension| {
            self.video_extensions
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&extension))
        })
    }

    /// The marker list for a master, if one is beside it.
    ///
    /// The extension is matched against the *whole* tail of the file name, so `marks.txt` matches
    /// `master.marks.txt` while `txt` matches `master.txt`, and the order in
    /// [`WatchPolicy::marker_extensions`] decides which wins when both are present.
    #[must_use]
    pub fn marker_for(&self, master: &Path) -> Option<PathBuf> {
        let folder = master.parent()?;
        let stem = master.file_stem()?.to_string_lossy().to_lowercase();
        for extension in &self.marker_extensions {
            let candidate = folder.join(format!("{stem}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    /// True when a marker extension names a project file, which is run rather than parsed.
    #[must_use]
    pub fn is_project_marker(extension: &str) -> bool {
        extension.eq_ignore_ascii_case("trimmerproj")
    }
}

/// A file that is in the folder, with the two facts the rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedFile {
    /// The file.
    pub path: PathBuf,
    /// Its size at the last poll.
    pub size_bytes: u64,
    /// Seconds since its size last changed.
    pub quiet_seconds: u64,
}

/// One job the folder implies.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchPlan {
    /// The master to cut.
    pub master: MediaPath,
    /// The marker list, when the job has one.
    pub markers: Option<MediaPath>,
    /// What triggered it.
    pub trigger: WatchTrigger,
    /// What to do.
    pub action: WatchAction,
    /// The preset to use.
    pub preset: String,
    /// Where outputs go.
    pub output_dir: Option<MediaPath>,
    /// The marks, when they were read from a simple text or CSV list.
    pub marks: Vec<WatchMark>,
    /// A sentence saying why this file is *not* a job yet, when it is not.
    pub blocked_by: Option<String>,
}

/// One mark from a marker list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchMark {
    /// A name, when the line had one.
    pub name: Option<String>,
    /// The in point as written.
    pub in_text: String,
    /// The out point as written, if there was one.
    pub out_text: Option<String>,
    /// The in point in frames, when it could be read.
    pub in_frame: Option<i64>,
    /// The out point in frames, when it could be read.
    pub out_frame: Option<i64>,
}

impl WatchMark {
    /// True when both marks were readable.
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        self.in_frame.is_some()
    }

    /// A description for a log.
    #[must_use]
    pub fn describe(&self) -> String {
        let name = self.name.as_deref().unwrap_or("(unnamed)");
        match (self.in_frame, self.out_frame) {
            (Some(start), Some(end)) => format!("{name}: frames {start}..{end}"),
            (Some(start), None) => format!("{name}: frames {start}.. to the end"),
            _ => format!("{name}: unreadable marks"),
        }
    }
}

/// A folder being watched.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchFolder {
    /// The folder.
    pub path: MediaPath,
    /// The rules.
    pub policy: WatchPolicy,
    /// The content fingerprints already processed, so a file is never cut twice.
    pub processed: std::collections::BTreeSet<String>,
}

impl WatchFolder {
    /// A folder with the default policy.
    #[must_use]
    pub fn new(path: impl Into<MediaPath>) -> Self {
        Self {
            path: path.into(),
            policy: WatchPolicy::default(),
            processed: std::collections::BTreeSet::new(),
        }
    }

    /// A fingerprint for a file: its name and its size.
    ///
    /// The timestamp is deliberately *not* part of this. A file copied to a share gets a new
    /// modification time, and a watch folder that re-cut every master each time it was re-dropped
    /// would be worse than useless. The name and size together are stable across a copy and change
    /// when the content does, which is the property that matters.
    #[must_use]
    pub fn fingerprint(path: &Path, size: u64) -> String {
        format!("{}:{size}", path.to_string_lossy().to_lowercase())
    }

    /// Decide what a set of observed files implies.
    ///
    /// Pure: it takes the folder's contents as data and returns the jobs, so the rules that decide
    /// whether a studio's automation behaves can be tested without touching a filesystem or
    /// waiting for a file to finish copying.
    #[must_use]
    pub fn plan(&self, observed: &[ObservedFile]) -> Vec<WatchPlan> {
        let mut plans = Vec::new();
        for file in observed {
            if !self.policy.is_video(&file.path) {
                continue;
            }
            let master = MediaPath::new(file.path.clone());
            let fingerprint = Self::fingerprint(&file.path, file.size_bytes);
            if self.processed.contains(&fingerprint) {
                continue;
            }

            // Still arriving. Cutting a file that is being copied produces a truncated segment,
            // and the failure is silent: ffmpeg reads what is there and exits zero.
            if file.quiet_seconds < self.policy.settle_seconds {
                plans.push(self.blocked_plan(
                    master,
                    WatchTrigger::MarkerList,
                    format!(
                        "the file changed {}s ago and the policy waits {}s for a copy to finish",
                        file.quiet_seconds, self.policy.settle_seconds
                    ),
                ));
                continue;
            }

            let markers = self.policy.marker_for(&file.path);
            let plan = match markers {
                Some(markers) => {
                    let extension = extension_of(&markers).unwrap_or_default();
                    let (action, trigger, marks) = if WatchPolicy::is_project_marker(&extension) {
                        (
                            WatchAction::RunProject,
                            WatchTrigger::ProjectFile,
                            Vec::new(),
                        )
                    } else {
                        (
                            WatchAction::CutMarkedSegments,
                            WatchTrigger::MarkerList,
                            Vec::new(),
                        )
                    };
                    WatchPlan {
                        master,
                        markers: Some(MediaPath::new(markers)),
                        trigger,
                        action,
                        preset: self.policy.default_preset.clone(),
                        output_dir: self.policy.output_dir.clone(),
                        marks,
                        blocked_by: None,
                    }
                }
                None if self.policy.cut_whole_when_unmarked => WatchPlan {
                    master,
                    markers: None,
                    trigger: WatchTrigger::WholeFile,
                    action: WatchAction::CutWholeFile,
                    preset: self.policy.default_preset.clone(),
                    output_dir: self.policy.output_dir.clone(),
                    marks: Vec::new(),
                    blocked_by: None,
                },
                None => self.blocked_plan(
                    master,
                    WatchTrigger::MarkerList,
                    "no marker list beside it, and the policy does not cut unmarked files"
                        .to_owned(),
                ),
            };
            plans.push(plan);
        }
        plans.sort_by(|left, right| left.master.to_string().cmp(&right.master.to_string()));
        plans
    }

    /// The jobs that are ready to run.
    #[must_use]
    pub fn ready(&self, observed: &[ObservedFile]) -> Vec<WatchPlan> {
        self.plan(observed)
            .into_iter()
            .filter(|plan| plan.blocked_by.is_none())
            .collect()
    }

    /// A plan that says why it is not a job yet.
    fn blocked_plan(&self, master: MediaPath, trigger: WatchTrigger, reason: String) -> WatchPlan {
        WatchPlan {
            master,
            markers: None,
            trigger,
            action: WatchAction::CutMarkedSegments,
            preset: self.policy.default_preset.clone(),
            output_dir: self.policy.output_dir.clone(),
            marks: Vec::new(),
            blocked_by: Some(reason),
        }
    }

    /// Record a job as done, so it is never planned again.
    pub fn mark_processed(&mut self, plan: &WatchPlan, size_bytes: u64) {
        self.processed
            .insert(Self::fingerprint(plan.master.as_path(), size_bytes));
    }

    /// Read the marks out of a marker list.
    ///
    /// # Errors
    ///
    /// Returns a sentence when the file cannot be read. A malformed *line* is skipped rather than
    /// failing the file, because an assistant's spreadsheet will always have one line with a
    /// comment in the marks column, and refusing the whole job over it helps nobody.
    pub fn read_marks(&self, plan: &WatchPlan, rate: FrameRate) -> Result<Vec<WatchMark>, String> {
        let Some(markers) = &plan.markers else {
            return Ok(Vec::new());
        };
        let text = std::fs::read_to_string(markers.as_path())
            .map_err(|error| format!("could not read {markers}: {error}"))?;
        Ok(parse_marks(&text, rate))
    }

    /// What a ready plan will deliver, in a sentence, for a log.
    #[must_use]
    pub fn describe(&self, plan: &WatchPlan, rate: Option<FrameRate>) -> String {
        let master = plan.master.file_name();
        match &plan.blocked_by {
            Some(reason) => format!("{master}: waiting — {reason}"),
            None => match plan.trigger {
                WatchTrigger::WholeFile => {
                    format!("{master}: cut whole into the {} preset", plan.preset)
                }
                WatchTrigger::ProjectFile => {
                    format!("{master}: run the project's batch")
                }
                WatchTrigger::MarkerList => match rate {
                    Some(rate) => {
                        let marks = self.read_marks(plan, rate).unwrap_or_default();
                        let usable = marks.iter().filter(|mark| mark.is_usable()).count();
                        let total: f64 = marks
                            .iter()
                            .filter_map(|mark| match (mark.in_frame, mark.out_frame) {
                                (Some(start), Some(end)) => {
                                    Some(rate.seconds_of((end - start).max(0)))
                                }
                                _ => None,
                            })
                            .sum();
                        format!(
                            "{master}: {usable} of {} marks usable, {} to deliver",
                            marks.len(),
                            format_seconds(total)
                        )
                    }
                    None => {
                        format!("{master}: marks beside it, waiting for the source to be probed")
                    }
                },
            },
        }
    }
}

/// Read marks out of a marker list.
///
/// See the module documentation for the accepted forms. Anything that cannot be read as a pair of
/// timecodes is skipped, and a line that has only an in point means "to the end of the master".
#[must_use]
pub fn parse_marks(text: &str, rate: FrameRate) -> Vec<WatchMark> {
    let mut marks = Vec::new();
    for raw in text.lines() {
        // Everything after a `#` is a comment, which is how an assistant annotates a line.
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // A leading index column, as a spreadsheet export produces: `3, cold open, 0:10, 2:31`.
        let line = strip_leading_index(line);
        let (name, marks_part) = split_name_and_marks(line);
        // Both marks are found by scanning for timecode-shaped runs and then disambiguating a
        // comma the way `split_timecodes` documents: two full timecodes joined by a comma split,
        // while a comma two digits from the end is part of a drop-frame timecode.
        let mut found: Vec<i64> = split_timecodes(marks_part, rate);
        // Whatever `split_timecodes` could not reach — because it was not comma-joined — is
        // picked up by the looser scan and parsed on its own.
        if found.is_empty() {
            found = find_timecodes(marks_part)
                .iter()
                .filter_map(|text| trimmer_core::timecode::parse_timecode(text, rate).ok())
                .collect();
        }
        let in_frame = found.first().copied();
        let out_frame = found.get(1).copied();
        // The text is kept as the operator wrote it, for a log that can be matched against their
        // own notes, rather than re-rendered from the frame numbers.
        let written = find_timecodes(marks_part);
        let in_text = written.first().cloned().unwrap_or_default();
        let out_text = written.get(1).cloned();
        marks.push(WatchMark {
            name,
            in_text,
            out_text,
            in_frame,
            out_frame,
        });
    }
    marks
}

/// Drop a leading spreadsheet row number, with its separator.
///
/// A leading number is only an index when what follows is *marks* rather than a bare seconds
/// count. `3, cold open, 0:10, 2:31` has an index; `3, 0:10, 2:31` starts at three seconds, not at
/// row three, because the rest of the line carries real timecodes. Getting this wrong is silent:
/// the line still parses, it just parses ten seconds early.
fn strip_leading_index(line: &str) -> &str {
    let trimmed = line.trim_start();
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return trimmed;
    }
    let rest = &trimmed[digits.len()..];
    let Some(separator) = rest.chars().next() else {
        return trimmed;
    };
    if !matches!(separator, ',' | '\t' | ';') {
        return trimmed;
    }
    let after = rest[1..].trim_start();
    if after.contains(':') {
        after
    } else {
        trimmed
    }
}

/// Split a line into its name and the part that holds the timecodes.
///
/// A comma or a tab is the separator, because those are what a spreadsheet writes. With neither,
/// there is no name and the whole line is marks.
fn split_name_and_marks(line: &str) -> (Option<String>, &str) {
    let separator = line.find([',', '\t']);
    match separator {
        Some(index) => {
            let name = line[..index].trim();
            let rest = line[index + 1..].trim();
            // A line that begins with a timecode has no name even though it has a comma.
            if find_timecodes(name).is_empty() && !name.is_empty() {
                (Some(name.to_owned()), rest)
            } else {
                (None, line)
            }
        }
        None => (None, line),
    }
}

/// Find every timecode-looking token in a string, in order.
///
/// Not a parser: it collects the tokens and lets `parse_timecode` decide. That way the accepted
/// spelling lives in one place — the same function the rest of the product uses — instead of being
/// duplicated in a watch-folder grammar that would drift from it.
fn find_timecodes(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut separators = 0usize;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if matches!(ch, ':' | ';' | '.' | ',') && !current.is_empty() {
            // A comma is ambiguous: it is both a field separator in a drop-frame timecode and the
            // separator between a name and the marks. Inside a run of digits it is part of the
            // timecode, so it is accepted here and `parse_timecode` rejects it if it is not —
            // which keeps the accepted spelling in one place instead of duplicating it in a
            // watch-folder grammar that would drift from the real parser.
            current.push(ch);
            separators += 1;
        } else {
            // Whitespace, a hyphen and anything else all end a run.
            flush_timecode(&mut current, separators, &mut found);
            separators = 0;
        }
    }
    flush_timecode(&mut current, separators, &mut found);
    found
}

/// Accept the accumulated run if it looks like a timecode, and clear it.
fn flush_timecode(current: &mut String, separators: usize, found: &mut Vec<String>) {
    let text = current.trim_matches([',', '.', ':', ';']).to_owned();
    current.clear();
    // Two or more separators means `HH:MM:SS:FF` or `MM:SS:FF`; one means `SS;FF` or a bare
    // number of seconds, and a bare number is legal. Zero means a number that is not a timecode
    // at all, which is how a stray year or an index is skipped.
    if separators >= 1 && !text.is_empty() {
        found.push(text);
    }
}

/// The extension of a path, lower-cased, allowing a two-part extension such as `marks.txt`.
fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
}

/// The full tail of a file name after its first dot, lower-cased: `master.marks.txt` ->
/// `marks.txt`. Used for the two-part marker extensions.
#[must_use]
pub fn compound_extension(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let (_, rest) = name.split_once('.')?;
    Some(rest.to_owned())
}

//! The command line's vocabulary.
//!
//! Every subcommand carries both `about` and `long_about`, and so does every flag that is not
//! self-evident. That is not decoration: `--help` is the only documentation a person has while
//! they are standing in front of the tool, and a flag whose meaning has to be guessed is a flag
//! that will be used wrongly. Where a default is a decision rather than a convenience — the
//! verification policy, the delivery preset, whether an out point is inclusive — the long help
//! says *why*.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use trimmer_core::VerifyPolicy;
use trimmer_export::ExportFormat;

/// The one-line description of the program.
pub const ABOUT: &str = "Frame-exact, verified segment cutting for professional post-production.";

/// The longer description, shown by `thetrimmer --help`.
pub const LONG_ABOUT: &str = "\
TheTrimmer cuts segments out of a master without re-encoding what it does not have to, and
checks that what came out is what was asked for.

The method is a *head patch*: the frames before the first keyframe at or after the in point are
re-encoded, everything after it is the original packets, and the two are joined at the source's
own timescale. That is what makes a cut frame-exact rather than approximately right.

Every cut is verified by default, and a file that fails a check is reported rather than
quietly delivered.

Exit codes:
    0    it worked
    1    a check failed: `doctor` found something missing, or `verify` failed a cut
    2    a usage error, or the domain refused the request
   130    interrupted";

/// The whole command line.
#[derive(Debug, Parser)]
#[command(
    name = "thetrimmer",
    bin_name = "thetrimmer",
    version,
    about = ABOUT,
    long_about = LONG_ABOUT,
    after_help = "Run `thetrimmer <command> --help` for what a command does and why.",
    propagate_version = true
)]
pub struct Cli {
    /// Where the project database lives.
    ///
    /// Defaults to this machine's data directory for TheTrimmer. `THE_TRIMMER_STORE` sets it
    /// too, and the flag wins over the variable — which is what makes `thetrimmer --store
    /// ./test.sqlite project new ...` usable in a test without touching a user's real projects.
    #[arg(long, global = true, env = "THE_TRIMMER_STORE", value_name = "PATH")]
    pub store: Option<PathBuf>,

    /// Say more about what is happening, on stderr.
    ///
    /// Adds the exact ffmpeg command lines as they are run. stdout stays reserved for the
    /// command's own output, so a pipeline is never polluted by progress.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Every subcommand.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check that this machine can cut, and say what it found.
    #[command(
        long_about = "Resolves ffmpeg and ffprobe, prints the build's version and the capability \
                      report, says where the project store lives and whether a licence was \
                      found.\n\n\
                      Exits 0 when ffmpeg and ffprobe both resolve and the build has libx264, \
                      and 1 otherwise. That is what makes it usable as a preflight check in a \
                      script."
    )]
    Doctor,

    /// Read a file and print every fact the cutter depends on.
    #[command(
        long_about = "Probes one file and prints its codec, geometry, rate, timescale, frame \
                      count, audio layout, timecode range and size — the same facts the \
                      workspace shows in its source list.\n\n\
                      It does not need a project and does not write anything."
    )]
    Probe(ProbeArgs),

    /// Cut one segment out of one file.
    #[command(
        long_about = "The single-segment path: probe, plan, and cut one range out of one source \
                      with the head-patch method.\n\n\
                      By default the out point is *inclusive* — `--out 00:00:10:00` keeps the \
                      frame at that timecode as the last frame. `--out-exclusive` makes it the \
                      usual half-open bound instead. Editors say the first and Premiere writes \
                      the second, which is exactly why it is a flag and not a guess.\n\n\
                      Nothing is written until the plan is shown and confirmed, unless `-y` is \
                      given."
    )]
    Cut(CutArgs),

    /// Run a saved project through the batch queue.
    #[command(
        long_about = "Loads a project from the store and cuts its enabled segments in order, \
                      reporting one line per segment.\n\n\
                      Named segments limit the run to those segments; everything else is \
                      skipped for this run and left alone in the project."
    )]
    Batch(BatchArgs),

    /// Inspect and edit a project in the store.
    #[command(
        long_about = "A project inspector and editor over the same SQLite store the rest of the \
                      program uses. Every subcommand takes a project id, which `project list` \
                      prints."
    )]
    Project(ProjectArgs),

    /// Write a timeline document an editor can import.
    #[command(
        long_about = "Builds a Premiere `xmeml` sequence, an FCPXML library, a CMX3600 edit \
                      decision list or a CSV, from a project. Media paths in the document are \
                      resolved to this machine's absolute paths."
    )]
    Export(ExportArgs),

    /// Check a finished cut against its plan.
    #[command(
        long_about = "Measures the source and the delivered file through ffprobe and runs every \
                      check in the plan's policy, printing one row per check.\n\n\
                      Exits 1 when any check failed, so it can gate a delivery."
    )]
    Verify(VerifyArgs),

    /// Read, install and identify licences.
    #[command(
        long_about = "An offline licence is a signed document and a machine identity. `status` \
                      verifies the installed one, `install` copies a key file into the config \
                      directory, and `machine` prints this machine's identity — the value a \
                      licence has to name to be bound to this machine."
    )]
    Licence(LicenceArgs),

    /// Serve the local HTTP/JSON API.
    #[command(
        long_about = "Starts the headless daemon in the foreground. It binds to 127.0.0.1 only: \
                      the API can cut files, delete projects and read a licence, so it is a \
                      local control surface and not a service.\n\n\
                      The token must be at least 16 characters."
    )]
    Daemon(DaemonArgs),

    /// Watch a folder and cut what arrives.
    #[command(
        long_about = "A foreground loop over the watch-folder rules. It uses the filesystem \
                      notification service, and re-scans the folder every two seconds as well — \
                      the second scan is what lets the settle rule work, since a file that is \
                      still being copied generates events the whole time it is arriving.\n\n\
                      Without `--apply` it prints what it would do and cuts nothing."
    )]
    Watch(WatchArgs),
}

/// `probe`
#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// The file to read.
    #[arg(value_name = "VIDEO")]
    pub video: PathBuf,

    /// Print the machine-readable form instead of one line per fact.
    #[arg(long, long_help = "Emits a JSON object on stdout with every field the probe found. \
                             Intended for a script; the human form is the default.")]
    pub json: bool,
}

/// `cut`
#[derive(Debug, Args)]
pub struct CutArgs {
    /// The source to cut from.
    #[arg(value_name = "VIDEO")]
    pub video: PathBuf,

    /// The first frame to keep, inclusive, as `HH:MM:SS:FF`.
    #[arg(long = "in", value_name = "TC", long_help = "Accepts `HH:MM:SS:FF`, `MM:SS:FF` or \
                                                       a bare frame count, on the source's own \
                                                       frame rate.")]
    pub in_point: String,

    /// The last frame to keep, as `HH:MM:SS:FF`.
    #[arg(long = "out", value_name = "TC", long_help = "Inclusive by default: the frame at this \
                                                        timecode is the last one kept. Add \
                                                        `--out-exclusive` for the half-open \
                                                        reading.")]
    pub out_point: String,

    /// Treat the out point as one past the last frame kept.
    #[arg(long)]
    pub out_exclusive: bool,

    /// Where to write the segment.
    ///
    /// Defaults beside the source, named after the segment and its in point.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// The delivery preset to satisfy.
    #[arg(
        long,
        default_value = "master",
        value_name = "NAME",
        long_help = "`master` copies the picture and rewrites only what it must. \
                     `thetrimmer project show` lists the presets a project carries; the \
                     standard library includes `master`, `prores_master`, `youtube_1080`, \
                     `vertical`, `square_social`, `podcast_audio`, `broadcast_r128` and \
                     `mxf_op1a`."
    )]
    pub preset: String,

    /// Override the encoder's quality for a re-encoded head.
    #[arg(long, value_name = "N", long_help = "Constant rate factor. Lower is better and \
                                               bigger; 18 is visually lossless for most \
                                               material.")]
    pub crf: Option<u8>,

    /// Frames of handle to add either side, for a crossfade.
    #[arg(long, default_value_t = 0, value_name = "FRAMES")]
    pub handles: i64,

    /// How hard to check the finished cut.
    #[arg(long, value_enum, default_value_t = VerifyArg::Strict, value_name = "POLICY")]
    pub verify: VerifyArg,

    /// Retime this caption file onto the segment.
    #[arg(long, value_name = "PATH", conflicts_with = "no_srt")]
    pub srt: Option<PathBuf>,

    /// Do not write a retimed caption file beside the segment.
    #[arg(long)]
    pub no_srt: bool,

    /// Print the exact commands that would run, and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Do not ask before writing.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// `batch`
#[derive(Debug, Args)]
pub struct BatchArgs {
    /// The project id, as `project list` prints it.
    #[arg(value_name = "PROJECT")]
    pub project: String,

    /// Only these segments, by name or id.
    ///
    /// Everything else in the project is left alone.
    #[arg(value_name = "SEGMENTS")]
    pub segments: Vec<String>,

    /// Stop at the first failure instead of finishing the batch.
    #[arg(long, long_help = "Off by default, because one bad segment should not cost a studio \
                             the other ninety-nine.")]
    pub stop_on_error: bool,

    /// Skip verification even when the project asks for it.
    #[arg(long)]
    pub no_verify: bool,

    /// A label for this run, recorded in the audit manifest.
    #[arg(long, default_value = "cli batch", value_name = "TEXT")]
    pub label: String,

    /// Print the outcome as JSON.
    #[arg(long)]
    pub json: bool,
}

/// `project`
#[derive(Debug, Args)]
pub struct ProjectArgs {
    /// What to do.
    #[command(subcommand)]
    pub command: ProjectCommand,
}

/// Every `project` subcommand.
#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Create a project.
    New(ProjectNewArgs),
    /// List the projects in the store.
    List,
    /// Show one project and its segments.
    Show(ProjectIdArg),
    /// Add a source, probing it when the file is there.
    AddSource(ProjectAddSourceArgs),
    /// Mark a cut.
    AddSegment(ProjectAddSegmentArgs),
    /// Remove a mark.
    RemoveSegment(ProjectRemoveSegmentArgs),
    /// Write the project out as a readable JSON document.
    Export(ProjectExportArgs),
}

/// `project new`
#[derive(Debug, Args)]
pub struct ProjectNewArgs {
    /// What to call it.
    #[arg(long, value_name = "TEXT")]
    pub name: String,

    /// Who is making it, recorded in the audit trail.
    #[arg(long, default_value = "thetrimmer", value_name = "TEXT")]
    pub by: String,
}

/// A command that takes only a project id.
#[derive(Debug, Args)]
pub struct ProjectIdArg {
    /// The project id.
    #[arg(value_name = "PROJECT")]
    pub project: String,
}

/// `project add-source`
#[derive(Debug, Args)]
pub struct ProjectAddSourceArgs {
    /// The project id.
    #[arg(value_name = "PROJECT")]
    pub project: String,

    /// The file to add.
    ///
    /// A file that is not there is still recorded, marked unavailable: a project has to survive
    /// a drive being unplugged.
    #[arg(value_name = "PATH")]
    pub path: PathBuf,
}

/// `project add-segment`
#[derive(Debug, Args)]
pub struct ProjectAddSegmentArgs {
    /// The project id.
    #[arg(value_name = "PROJECT")]
    pub project: String,

    /// The source to cut, as it was added.
    #[arg(long, value_name = "PATH")]
    pub source: PathBuf,

    /// What to call the segment.
    #[arg(long, value_name = "TEXT")]
    pub name: String,

    /// The first frame to keep, inclusive.
    #[arg(long = "in", value_name = "TC")]
    pub in_point: String,

    /// The last frame to keep.
    #[arg(long = "out", value_name = "TC")]
    pub out_point: Option<String>,

    /// Treat the out point as one past the last frame kept.
    #[arg(long)]
    pub out_exclusive: bool,

    /// The delivery preset for this segment.
    #[arg(long, value_name = "NAME")]
    pub preset: Option<String>,

    /// Frames of handle either side.
    #[arg(long, default_value_t = 0, value_name = "FRAMES")]
    pub handles: i64,
}

/// `project remove-segment`
#[derive(Debug, Args)]
pub struct ProjectRemoveSegmentArgs {
    /// The project id.
    #[arg(value_name = "PROJECT")]
    pub project: String,

    /// The segment id.
    #[arg(value_name = "SEGMENT")]
    pub segment: String,
}

/// `project export`
#[derive(Debug, Args)]
pub struct ProjectExportArgs {
    /// The project id.
    #[arg(value_name = "PROJECT")]
    pub project: String,

    /// Where to write the document.
    #[arg(short, long, value_name = "PATH")]
    pub output: PathBuf,
}

/// `export`
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// The project id.
    #[arg(value_name = "PROJECT")]
    pub project: String,

    /// Which document to write.
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub format: FormatArg,

    /// Where to write it.
    #[arg(short, long, value_name = "PATH")]
    pub output: PathBuf,
}

/// `verify`
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// The file that was cut from.
    #[arg(value_name = "SOURCE")]
    pub source: PathBuf,

    /// The file that was cut.
    #[arg(value_name = "OUTPUT")]
    pub output: PathBuf,

    /// The plan the cut was made from, as JSON.
    #[arg(long, value_name = "JSON", long_help = "A `CutPlan` as `project show --plan` writes \
                                                  it, or as a run's audit record holds it.")]
    pub plan: PathBuf,

    /// How hard to check.
    #[arg(long, value_enum, default_value_t = VerifyArg::Strict, value_name = "POLICY")]
    pub policy: VerifyArg,
}

/// `licence`
#[derive(Debug, Args)]
pub struct LicenceArgs {
    /// What to do.
    #[command(subcommand)]
    pub command: LicenceCommand,
}

/// Every `licence` subcommand.
#[derive(Debug, Subcommand)]
pub enum LicenceCommand {
    /// Print the installed licence and whether it is good right now.
    Status,
    /// Copy a key file into the configuration directory.
    Install(LicenceInstallArgs),
    /// Print this machine's identity.
    Machine,
}

/// `licence install`
#[derive(Debug, Args)]
pub struct LicenceInstallArgs {
    /// The key file, as it arrived.
    #[arg(value_name = "KEY-FILE")]
    pub key_file: PathBuf,
}

/// `daemon`
#[derive(Debug, Args)]
pub struct DaemonArgs {
    /// The port to listen on.
    #[arg(long, default_value_t = 8787, value_name = "N")]
    pub port: u16,

    /// The bearer token every request must carry.
    #[arg(
        long,
        env = "THE_TRIMMER_TOKEN",
        hide_env_values = true,
        value_name = "TEXT",
        long_help = "At least 16 characters. There is no default: a daemon that can delete a \
                     project will not start without one."
    )]
    pub token: String,
}

/// `watch`
#[derive(Debug, Args)]
pub struct WatchArgs {
    /// The folder to watch.
    #[arg(value_name = "FOLDER")]
    pub folder: PathBuf,

    /// The delivery preset to cut with.
    #[arg(long, default_value = "master", value_name = "NAME")]
    pub preset: String,

    /// Actually cut, rather than saying what would be cut.
    #[arg(long)]
    pub apply: bool,

    /// A file must have stopped growing for this many seconds.
    #[arg(
        long,
        default_value_t = 30,
        value_name = "SECONDS",
        long_help = "Cutting a file that is still being copied produces a truncated segment, \
                     and ffmpeg exits 0 while doing it."
    )]
    pub settle: u64,
}

/// Which verification policy a flag asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum VerifyArg {
    /// Do not measure. Fast, and for a rough cut.
    Off,
    /// Frame count, duration and audio alignment.
    Standard,
    /// Also hash decoded frames at sample points. The default.
    Strict,
    /// Also compare the head by structural similarity and check every caption cue.
    Forensic,
}

impl From<VerifyArg> for VerifyPolicy {
    fn from(value: VerifyArg) -> Self {
        match value {
            VerifyArg::Off => Self::Off,
            VerifyArg::Standard => Self::Standard,
            VerifyArg::Strict => Self::Strict,
            VerifyArg::Forensic => Self::Forensic,
        }
    }
}

/// Which document an export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    /// Final Cut Pro 7 `xmeml` version 4, which Premiere Pro and Resolve import.
    Premiere,
    /// Final Cut Pro X `fcpxml` 1.11.
    Fcpxml,
    /// CMX3600 edit decision list.
    Edl,
    /// One RFC 4180 row per segment.
    Csv,
}

impl From<FormatArg> for ExportFormat {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Premiere => Self::PremiereXml,
            FormatArg::Fcpxml => Self::Fcpxml,
            FormatArg::Edl => Self::Edl,
            FormatArg::Csv => Self::Csv,
        }
    }
}

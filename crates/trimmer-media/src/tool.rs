//! Finding ffmpeg and ffprobe, and reporting what the build can actually do.
//!
//! A studio machine may have three ffmpeg builds on it: one on `PATH`, one bundled inside a
//! capture application, and one installed by a codec pack that is three years old and has no
//! libx265. Resolving the tool is therefore a decision the user must be able to override, and
//! what the resolved build *supports* is a fact the application has to check before it promises
//! a cut.

use std::path::{Path, PathBuf};

use crate::MediaError;

/// The environment variable prefix that overrides a tool's location.
const ENV_PREFIX: &str = "THE_TRIMMER_";

/// The two external programs this crate needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolSet {
    /// The encoder and muxer.
    Ffmpeg,
    /// The inspector.
    Ffprobe,
}

impl ToolSet {
    /// Every tool, for a doctor report.
    pub const ALL: [Self; 2] = [Self::Ffmpeg, Self::Ffprobe];

    /// The program name, as it appears on `PATH`.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    /// The environment variable that overrides it, without the prefix.
    #[must_use]
    pub const fn env_suffix(self) -> &'static str {
        match self {
            Self::Ffmpeg => "FFMPEG",
            Self::Ffprobe => "FFPROBE",
        }
    }

    /// The full variable name, e.g. `THE_TRIMMER_FFMPEG`.
    #[must_use]
    pub fn env_var(self) -> String {
        format!("{ENV_PREFIX}{}", self.env_suffix())
    }
}

/// Where each tool was found, and which build it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPaths {
    /// The resolved ffmpeg.
    pub ffmpeg: PathBuf,
    /// The resolved ffprobe.
    pub ffprobe: PathBuf,
    /// Whether each path came from an environment override rather than `PATH`.
    pub overridden: Vec<ToolSet>,
}

impl ToolPaths {
    /// Resolve both tools, honouring `THE_TRIMMER_FFMPEG` and `THE_TRIMMER_FFPROBE`.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::ToolOverrideMissing`] when an override names a path that is not
    /// there, and [`MediaError::ToolNotFound`] when a tool is neither overridden nor on `PATH`.
    /// An override that does not resolve is an error rather than a fallback: a user who has
    /// pointed the app at a specific build needs to know that build has gone.
    pub fn resolve() -> Result<Self, MediaError> {
        Self::resolve_with(&|name| std::env::var(name).ok(), &which)
    }

    /// Resolve both tools against an injected environment and an injected `PATH` search.
    ///
    /// Both are parameters rather than ambient state so the resolution rules can be tested
    /// exactly — including the case where nothing is found, which on a machine that *has*
    /// ffmpeg installed is otherwise impossible to exercise.
    ///
    /// # Errors
    ///
    /// As [`ToolPaths::resolve`].
    pub fn resolve_with(
        env: &dyn Fn(&str) -> Option<String>,
        find: &dyn Fn(&str) -> Option<PathBuf>,
    ) -> Result<Self, MediaError> {
        let mut overridden = Vec::new();
        let mut resolve_one = |tool: ToolSet| -> Result<PathBuf, MediaError> {
            let variable = tool.env_var();
            if let Some(value) = env(&variable).filter(|value| !value.trim().is_empty()) {
                let path = PathBuf::from(value.trim());
                if !path.is_file() {
                    return Err(MediaError::ToolOverrideMissing {
                        env: tool.env_suffix().to_owned(),
                        path: path.display().to_string(),
                    });
                }
                overridden.push(tool);
                return Ok(path);
            }
            find(tool.program()).ok_or_else(|| MediaError::ToolNotFound {
                tool: tool.program().to_owned(),
                env: tool.env_suffix().to_owned(),
            })
        };

        let ffmpeg = resolve_one(ToolSet::Ffmpeg)?;
        let ffprobe = resolve_one(ToolSet::Ffprobe)?;
        Ok(Self {
            ffmpeg,
            ffprobe,
            overridden,
        })
    }

    /// A pair of paths for tests and for a caller that already knows where the tools are.
    #[must_use]
    pub fn new(ffmpeg: impl Into<PathBuf>, ffprobe: impl Into<PathBuf>) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
            overridden: Vec::new(),
        }
    }

    /// The path for a tool.
    #[must_use]
    pub fn path(&self, tool: ToolSet) -> &Path {
        match tool {
            ToolSet::Ffmpeg => &self.ffmpeg,
            ToolSet::Ffprobe => &self.ffprobe,
        }
    }
}

/// Find a program on `PATH`, adding the Windows extensions `Command` would try.
///
/// `std::process::Command` does this search itself, but doing it here means the resolved path
/// can be shown to the user, logged, and stored in a project — which is what a studio needs
/// when it asks "which ffmpeg did this render actually use".
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .map(str::to_lowercase)
            .collect()
    } else {
        vec![String::new()]
    };
    for folder in std::env::split_paths(&path) {
        let candidate = folder.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
        for extension in &extensions {
            let with_extension = folder.join(format!("{program}{extension}"));
            if with_extension.is_file() {
                return Some(with_extension);
            }
        }
    }
    None
}

/// What an ffmpeg build can actually do.
///
/// Checked before a cut is promised, not after it fails. A source in HEVC on a machine whose
/// ffmpeg has no libx265 is a refusal with a sentence, not a stack trace twelve seconds in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// The first line of `ffmpeg -version`.
    pub version_line: String,
    /// Encoder names the build reports, e.g. `libx264`.
    pub encoders: Vec<String>,
    /// Muxer names the build reports, e.g. `mxf`.
    pub muxers: Vec<String>,
    /// Filter names the build reports, e.g. `loudnorm`.
    pub filters: Vec<String>,
}

impl Capabilities {
    /// True when the build can encode with a named encoder.
    #[must_use]
    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders.iter().any(|encoder| encoder == name)
    }

    /// True when the build can write a named container.
    #[must_use]
    pub fn has_muxer(&self, name: &str) -> bool {
        self.muxers.iter().any(|muxer| muxer == name)
    }

    /// True when the build has a named filter, which is how loudness normalisation is checked
    /// before a delivery preset that needs it is accepted.
    #[must_use]
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.iter().any(|filter| filter == name)
    }

    /// A doctor report: one line per capability the application depends on.
    #[must_use]
    pub fn doctor_report(&self) -> String {
        let mut lines = vec![format!("ffmpeg      {}", self.version_line)];
        for (name, why) in [
            ("libx264", "H.264 sources"),
            ("libx265", "HEVC sources"),
            ("aac", "audio re-encoding"),
            ("pcm_s24le", "broadcast audio"),
            ("prores_ks", "ProRes delivery"),
            ("mpeg2video", "MXF delivery"),
        ] {
            lines.push(format!(
                "{name:<11} {:<4} ({why})",
                if self.has_encoder(name) { "yes" } else { "NO" }
            ));
        }
        for (name, why) in [("loudnorm", "loudness targeting"), ("ssim", "head fidelity")] {
            lines.push(format!(
                "{name:<11} {:<4} ({why})",
                if self.has_filter(name) { "yes" } else { "NO" }
            ));
        }
        for (name, why) in [("mp4", "MP4 delivery"), ("mxf", "MXF delivery")] {
            lines.push(format!(
                "{name:<11} {:<4} ({why})",
                if self.has_muxer(name) { "yes" } else { "NO" }
            ));
        }
        lines.join("\n")
    }
}

/// Parse the output of `ffmpeg -encoders` and friends.
///
/// The listings are human-readable tables, not a machine format:
///
/// ```text
///  V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC (codec h264)
///  ....... loudnorm            Apply ISO 226 loudness normalization
/// ```
///
/// The name is the second whitespace-separated field on a line whose first field is flags. The
/// header rows and the `------` rules are skipped by requiring the first field to look like
/// flags: six characters, all from the flag alphabet.
#[must_use]
pub fn parse_listing(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(flags) = fields.next() else { continue };
        let Some(name) = fields.next() else { continue };
        if flags.len() != 6
            || flags == "------"
            || !flags
                .chars()
                .all(|ch| matches!(ch, 'V' | 'A' | 'S' | 'D' | 'E' | 'F' | 'X' | 'B' | 'T' | '.'))
        {
            continue;
        }
        // A leading `=` marks an encoder whose output is not a standalone stream.
        let name = name.trim_start_matches('=');
        if !name.is_empty() && !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_variables_are_named_the_way_v1_named_them() {
        // Carried over verbatim so an existing deployment's configuration keeps working.
        assert_eq!(ToolSet::Ffmpeg.env_var(), "THE_TRIMMER_FFMPEG");
        assert_eq!(ToolSet::Ffprobe.env_var(), "THE_TRIMMER_FFPROBE");
        assert_eq!(ToolSet::Ffmpeg.program(), "ffmpeg");
    }

    #[test]
    fn an_override_that_resolves_wins_and_is_recorded() {
        let exe = std::env::current_exe().expect("the test binary exists");
        let text = exe.display().to_string();
        let paths = ToolPaths::resolve_with(
            &|name| {
                if name.ends_with("FFMPEG") {
                    Some(text.clone())
                } else {
                    None
                }
            },
            &|_| Some(PathBuf::from("C:\\fake\\ffprobe.exe")),
        )
        .expect("the override plus the fake PATH is enough");
        assert_eq!(paths.ffmpeg, exe);
        assert_eq!(paths.overridden, vec![ToolSet::Ffmpeg]);
    }

    #[test]
    fn an_override_that_does_not_resolve_is_an_error_not_a_fallback() {
        let error = ToolPaths::resolve_with(
            &|name| {
                if name.ends_with("FFMPEG") {
                    Some(r"H:\nope\ffmpeg.exe".to_owned())
                } else {
                    None
                }
            },
            // Even with a perfectly good ffmpeg on PATH, a broken override is an error: a user
            // who pointed the app at a specific build needs to know that build has gone.
            &|_| Some(PathBuf::from("C:\\fake\\ffmpeg.exe")),
        )
        .expect_err("refused");
        match error {
            MediaError::ToolOverrideMissing { env, path } => {
                assert_eq!(env, "FFMPEG");
                assert!(path.contains("ffmpeg.exe"));
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn an_empty_override_is_ignored_rather_than_treated_as_a_path() {
        // `set THE_TRIMMER_FFMPEG=` is how a Windows user unsets a variable in a shell, so an
        // empty value must fall through to the PATH search rather than fail.
        let result = ToolPaths::resolve_with(
            &|name| {
                if name.ends_with("FFMPEG") {
                    Some(String::new())
                } else {
                    None
                }
            },
            &|program| Some(PathBuf::from(format!("C:\\fake\\{program}.exe"))),
        );
        let paths = result.expect("the empty override falls through");
        assert_eq!(paths.ffmpeg, PathBuf::from("C:\\fake\\ffmpeg.exe"));
        assert!(paths.overridden.is_empty());
    }

    #[test]
    fn a_missing_tool_names_the_variable_that_would_fix_it() {
        let error = ToolPaths::resolve_with(&|_| None, &|_| None)
            .expect_err("nothing is on this fake PATH");
        match error {
            MediaError::ToolNotFound { tool, env } => {
                assert_eq!(tool, "ffmpeg");
                assert_eq!(env, "FFMPEG");
                // The message a user reads must name both the program and the variable that
                // would fix it, because "ffmpeg not found" with no next step is a dead end.
                let message = MediaError::ToolNotFound {
                    tool: tool.clone(),
                    env: env.clone(),
                }
                .to_string();
                assert!(message.contains("ffmpeg"), "{message}");
                assert!(message.contains("THE_TRIMMER_FFMPEG"), "{message}");
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn listing_parsing_reads_encoder_names_and_skips_the_table_furniture() {
        let text = "\
Encoders:
 V..... = Video
 ------
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC (codec h264)
 V....D libx265              libx265 H.265 / HEVC (codec hevc)
 V..... =h264_nvenc          NVIDIA NVENC H.264 encoder
 A....D aac                  AAC (Advanced Audio Coding)
 S..... ssim                 Calculate the SSIM between two video streams
";
        let names = parse_listing(text);
        assert!(names.contains(&"libx264".to_owned()), "{names:?}");
        assert!(names.contains(&"libx265".to_owned()));
        assert!(names.contains(&"aac".to_owned()));
        assert!(names.contains(&"ssim".to_owned()));
        // The `=` is a marker, not part of the name.
        assert!(names.contains(&"h264_nvenc".to_owned()));
        // Furniture is not a name.
        assert!(!names.contains(&"Encoders:".to_owned()));
        assert!(!names.contains(&"------".to_owned()));
        assert!(!names.contains(&"Video".to_owned()));
    }

    #[test]
    fn listing_parsing_deduplicates() {
        let text = " V....D libx264  first\n A....D libx264  again\n";
        assert_eq!(parse_listing(text), vec!["libx264".to_owned()]);
    }

    #[test]
    fn capabilities_answer_the_questions_the_cut_asks() {
        let capabilities = Capabilities {
            version_line: "ffmpeg version 8.0.1".to_owned(),
            encoders: parse_listing(" V....D libx264  x\n A....D aac  y\n"),
            muxers: parse_listing(" E....D mp4  x\n"),
            filters: parse_listing(" S..... loudnorm  x\n"),
        };
        assert!(capabilities.has_encoder("libx264"));
        assert!(!capabilities.has_encoder("libx265"));
        assert!(capabilities.has_muxer("mp4"));
        assert!(capabilities.has_filter("loudnorm"));
        assert!(!capabilities.has_filter("ssim"));

        let report = capabilities.doctor_report();
        assert!(report.contains("8.0.1"));
        assert!(report.contains("libx264"));
        // The report must be honest about what is missing, and say what it is for.
        assert!(report.contains("libx265     NO"), "{report}");
        assert!(report.contains("HEVC sources"));
    }
}

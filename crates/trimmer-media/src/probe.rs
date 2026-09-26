//! Asking ffprobe what a file is.
//!
//! One call answers everything the domain needs about a source: rate, frame count, timebase,
//! audio layout, size and start time. That is a deliberate choice rather than an incidental
//! one — each of those fields costs a process launch if asked for separately, roughly 100 ms
//! each on a multi-gigabyte master, and the application asks for all of them after every cut
//! and again for every check.
//!
//! Keyframes are the one thing kept separate, because they cannot come from the same call: the
//! demuxer has to seek and the decoder has to look at frames. `-skip_frame nokey` makes that
//! cheap, since nothing that is not a keyframe is decoded.

use std::path::Path;

use serde::Deserialize;
use trimmer_core::{FrameRate, KeyframeGrid, MediaInfo, MediaPath, Timescale};

use crate::process::{PollPolicy, RunOptions};
use crate::tool::ToolPaths;
use crate::{MediaError, MediaResult, ProcessRunner};

/// The subset of ffprobe's JSON this crate reads.
///
/// Only the fields that are actually used. ffprobe's output shape varies between versions and
/// between containers, so every field is optional and every absence has a defined fallback —
/// a probe that insisted on a field a given muxer does not write would fail on real files.
///
/// Public because [`Prober::probe_json`] returns it, for a caller that needs a field this crate
/// does not model. Every field is `pub` for the same reason.
#[derive(Debug, Deserialize)]
pub struct ProbeJson {
    /// One entry per elementary stream.
    #[serde(default)]
    pub streams: Vec<ProbeStream>,
    /// Container-level facts.
    #[serde(default)]
    pub format: ProbeFormat,
}

/// Container-level facts.
#[derive(Debug, Default, Deserialize)]
pub struct ProbeFormat {
    /// Duration in seconds, as a string because ffprobe writes it as one.
    #[serde(default)]
    pub duration: Option<String>,
    /// Size in bytes.
    #[serde(default)]
    pub size: Option<String>,
}

/// One elementary stream.
#[derive(Debug, Default, Deserialize)]
pub struct ProbeStream {
    /// `video`, `audio`, `subtitle`, `data`.
    #[serde(default)]
    pub codec_type: Option<String>,
    /// Codec name, e.g. `h264`.
    #[serde(default)]
    pub codec_name: Option<String>,
    /// Frame width.
    #[serde(default)]
    pub width: Option<u32>,
    /// Frame height.
    #[serde(default)]
    pub height: Option<u32>,
    /// Pixel format.
    #[serde(default)]
    pub pix_fmt: Option<String>,
    /// The rate the container claims.
    #[serde(default)]
    pub r_frame_rate: Option<String>,
    /// The rate actually measured.
    #[serde(default)]
    pub avg_frame_rate: Option<String>,
    /// The stream's time base, as `1/den`.
    #[serde(default)]
    pub time_base: Option<String>,
    /// Frame count, when the container records one.
    #[serde(default)]
    pub nb_frames: Option<String>,
    /// Stream duration in seconds.
    #[serde(default)]
    pub duration: Option<String>,
    /// First presentation timestamp.
    #[serde(default)]
    pub start_time: Option<String>,
    /// Audio sample rate.
    #[serde(default)]
    pub sample_rate: Option<String>,
    /// Audio channel count.
    #[serde(default)]
    pub channels: Option<u16>,
}

/// Reads media facts by running ffprobe.
#[derive(Debug, Clone)]
pub struct Prober {
    tools: ToolPaths,
    runner: ProcessRunner,
}

impl Prober {
    /// A prober using resolved tool paths.
    #[must_use]
    pub fn new(tools: ToolPaths) -> Self {
        Self {
            tools,
            runner: ProcessRunner::new(),
        }
    }

    /// The resolved tools.
    ///
    /// Exposed so a caller that needs a *different* ffmpeg call — a frame-hash pass, say — can reuse
    /// the resolution rather than doing it again and possibly picking a different build from `PATH`.
    #[must_use]
    pub const fn tools(&self) -> &ToolPaths {
        &self.tools
    }

    /// Probe a file into the domain's shape.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::NoVideoStream`] when the file has no picture, and
    /// [`MediaError::BadProbe`] when ffprobe's output cannot be read.
    pub async fn probe(&self, path: &Path) -> MediaResult<MediaInfo> {
        let json = self.probe_json(path).await?;
        let video = json
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("video"))
            .ok_or_else(|| MediaError::NoVideoStream {
                path: path.display().to_string(),
            })?;
        let audio = json
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("audio"));

        // `r_frame_rate` is the base rate the container claims; it is what timecode is read
        // against. It is preferred over `avg_frame_rate` because on a constant-rate file they
        // agree, and on a variable-rate file the claimed rate is the grid the *marks* were made
        // on, which is the one the user typed against.
        let rate = video
            .r_frame_rate
            .as_deref()
            .and_then(|text| FrameRate::parse(text).ok())
            .or_else(|| {
                video
                    .avg_frame_rate
                    .as_deref()
                    .and_then(|text| FrameRate::parse(text).ok())
            })
            .unwrap_or(FrameRate::FPS_30);
        let average_rate = video
            .avg_frame_rate
            .as_deref()
            .and_then(|text| FrameRate::parse(text).ok());

        let timebase = video
            .time_base
            .as_deref()
            .and_then(|text| Timescale::from_ffprobe_time_base(text).ok())
            .unwrap_or(Timescale::NINETY_KHZ);

        let duration = video
            .duration
            .as_deref()
            .and_then(|text| text.parse::<f64>().ok())
            .or_else(|| {
                json.format
                    .duration
                    .as_deref()
                    .and_then(|text| text.parse::<f64>().ok())
            })
            .unwrap_or(0.0);

        let frame_count = video
            .nb_frames
            .as_deref()
            .and_then(|text| text.parse::<i64>().ok())
            .filter(|count| *count > 0)
            .unwrap_or_else(|| (duration * rate.as_f64()).round() as i64);

        let audio_format = audio.map(|stream| trimmer_core::AudioFormat {
            codec: stream.codec_name.clone().unwrap_or_else(|| "?".to_owned()),
            sample_rate: stream
                .sample_rate
                .as_deref()
                .and_then(|text| text.parse::<u32>().ok())
                .unwrap_or(48_000),
            channels: stream.channels.unwrap_or(2),
        });

        Ok(MediaInfo {
            path: MediaPath::new(path.to_path_buf()),
            codec: video.codec_name.clone().unwrap_or_else(|| "?".to_owned()),
            pix_fmt: video
                .pix_fmt
                .clone()
                .unwrap_or_else(|| "yuv420p".to_owned()),
            width: video.width.unwrap_or(0),
            height: video.height.unwrap_or(0),
            rate,
            average_rate,
            timebase,
            frame_count,
            audio: audio_format,
            size_bytes: json
                .format
                .size
                .as_deref()
                .and_then(|text| text.parse::<u64>().ok())
                .unwrap_or(0),
            start_time: video
                .start_time
                .as_deref()
                .and_then(|text| text.parse::<f64>().ok())
                .unwrap_or(0.0),
        })
    }

    /// The keyframes inside a window, as frame numbers on the source's grid.
    ///
    /// The window is expressed in frames and converted here, so the caller never has to think
    /// in seconds. A little slack is taken either side of the window so a keyframe sitting
    /// exactly on a mark is not missed to a rounding difference.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::ProcessFailed`] when ffprobe cannot read the file.
    pub async fn keyframes(
        &self,
        media: &MediaInfo,
        from_frame: i64,
        to_frame: i64,
    ) -> MediaResult<KeyframeGrid> {
        let rate = media.rate;
        // A second of slack either side, so a keyframe sitting exactly on a mark is not missed to a
        // rounding difference. The window is bounded by the file's own start.
        let start = (rate.seconds_of(from_frame.max(0)) - 1.0).max(0.0);
        let end = rate.seconds_of(to_frame.max(1)) + 1.0;

        // `-read_intervals` bounds the demuxer's seek, and `-skip_frame nokey` means nothing that
        // is not a keyframe is decoded. Together they make this cheap on a two-hour master: a seek
        // per GOP, not a decode of the file.
        let args = crate::process::argv(&[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-skip_frame",
            "nokey",
            "-show_entries",
            "frame=pts_time",
            "-of",
            "csv=p=0",
            "-read_intervals",
            &format!("{start:.6}%{end:.6}"),
            media.path.as_path().to_str().unwrap_or_default(),
        ]);

        let options = RunOptions {
            policy: PollPolicy::quick(),
            label: "list keyframes".to_owned(),
            ..RunOptions::default()
        };
        let output = self
            .runner
            .run(
                self.tools.path(crate::tool::ToolSet::Ffprobe),
                &args,
                &options,
            )
            .await?;

        let mut frames: Vec<i64> = Vec::new();
        for token in output.stdout.split_whitespace() {
            if let Ok(seconds) = token.parse::<f64>() {
                frames.push(rate.frames_in(seconds));
            }
        }
        Ok(KeyframeGrid::new(frames, from_frame, to_frame))
    }

    /// The raw JSON, for a caller that needs a field this crate does not model.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::BadProbe`] when ffprobe's output is not the expected JSON.
    pub async fn probe_json(&self, path: &Path) -> MediaResult<ProbeJson> {
        let args = crate::process::argv(&[
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ]);
        let mut args = args;
        args.push(path.as_os_str().to_os_string());

        let options = RunOptions {
            policy: PollPolicy::quick(),
            label: "probe".to_owned(),
            ..RunOptions::default()
        };
        let output = self
            .runner
            .run(
                self.tools.path(crate::tool::ToolSet::Ffprobe),
                &args,
                &options,
            )
            .await?;
        serde_json::from_str(&output.stdout)
            .map_err(|error| MediaError::BadProbe(format!("{error}: {}", output.stdout.trim())))
    }

    /// The first line of `ffmpeg -version`, for a report.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::ProcessFailed`] when ffmpeg cannot be run.
    pub async fn ffmpeg_version(&self) -> MediaResult<String> {
        let options = RunOptions {
            policy: PollPolicy::quick(),
            label: "version".to_owned(),
            ..RunOptions::default()
        };
        let output = self
            .runner
            .run(
                self.tools.path(crate::tool::ToolSet::Ffmpeg),
                &crate::process::argv(&["-version"]),
                &options,
            )
            .await?;
        Ok(output.stdout.lines().next().unwrap_or("unknown").to_owned())
    }

    /// What the resolved ffmpeg build can do.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::ProcessFailed`] when the listings cannot be obtained.
    pub async fn capabilities(&self) -> MediaResult<crate::tool::Capabilities> {
        let ffmpeg = self.tools.path(crate::tool::ToolSet::Ffmpeg);
        let listing = |flag: &str| {
            let runner = self.runner;
            let program = ffmpeg.to_path_buf();
            let flag = flag.to_owned();
            async move {
                let options = RunOptions {
                    policy: PollPolicy::quick(),
                    label: format!("list {flag}"),
                    ..RunOptions::default()
                };
                runner
                    .run(&program, &crate::process::argv(&[&flag]), &options)
                    .await
            }
        };
        let version = self.ffmpeg_version().await?;
        let encoders = crate::tool::parse_listing(&listing("-encoders").await?.stdout);
        let muxers = crate::tool::parse_listing(&listing("-muxers").await?.stdout);
        let filters = crate::tool::parse_listing(&listing("-filters").await?.stdout);
        Ok(crate::tool::Capabilities {
            version_line: version,
            encoders,
            muxers,
            filters,
        })
    }
}

/// Adapts [`Prober`] to the measurement trait `trimmer-verify` defines.
///
/// The trait lives in `trimmer-verify` so that the *verdict* logic can be tested with no media
/// at all. This type is the adapter that gives it real answers, and it is the only place the
/// two meet.
#[derive(Debug, Clone)]
pub struct FactsMeasurer {
    prober: Prober,
}

impl FactsMeasurer {
    /// Wrap a prober.
    #[must_use]
    pub const fn new(prober: Prober) -> Self {
        Self { prober }
    }

    /// The facts for a finished or source file.
    ///
    /// # Errors
    ///
    /// Propagates a probe failure.
    pub async fn facts(&self, path: &MediaPath) -> MediaResult<trimmer_core::MediaInfo> {
        self.prober.probe(path.as_path()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_json_tolerates_the_variations_real_files_have() {
        // No `nb_frames`, no stream `duration`, no `sample_rate`: all of these are absent on
        // some real files, and a probe that insisted on them would fail on those files.
        let text = r#"{
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080,
                 "pix_fmt": "yuv420p", "r_frame_rate": "30000/1001",
                 "avg_frame_rate": "30000/1001", "time_base": "1/90000"},
                {"codec_type": "audio", "codec_name": "aac", "channels": 2}
            ],
            "format": {"duration": "3600.5", "size": "6000000000"}
        }"#;
        let parsed: ProbeJson = serde_json::from_str(text).expect("parses");
        let video = parsed
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("video"))
            .expect("video");
        assert_eq!(video.width, Some(1920));
        assert_eq!(video.nb_frames, None);
        assert_eq!(
            FrameRate::parse(video.r_frame_rate.as_deref().expect("rate")).expect("ok"),
            FrameRate::FPS_29_97
        );
        assert_eq!(
            Timescale::from_ffprobe_time_base(video.time_base.as_deref().expect("tb")).expect("ok"),
            Timescale::NINETY_KHZ
        );
        // The frame count falls back to duration x rate when the container does not say.
        let duration: f64 = parsed
            .format
            .duration
            .as_deref()
            .expect("dur")
            .parse()
            .expect("num");
        let rate = FrameRate::FPS_29_97;
        assert_eq!((duration * rate.as_f64()).round() as i64, 107_907);
    }

    #[test]
    fn a_timebase_that_is_not_one_over_n_is_refused() {
        assert!(Timescale::from_ffprobe_time_base("1001/30000").is_err());
        assert!(Timescale::from_ffprobe_time_base("0/0").is_err());
        assert_eq!(
            Timescale::from_ffprobe_time_base("1/48000")
                .expect("ok")
                .ticks(),
            48_000
        );
    }

    #[test]
    fn an_unreadable_probe_is_a_named_error_not_a_panic() {
        let error = serde_json::from_str::<ProbeJson>("not json at all")
            .map_err(|error| MediaError::BadProbe(error.to_string()))
            .expect_err("refused");
        assert!(matches!(error, MediaError::BadProbe(_)));
        assert!(error
            .to_string()
            .starts_with("ffprobe returned unreadable output"));
    }
}

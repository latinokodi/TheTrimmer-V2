//! Delivery presets: what a cut is delivered *as*, separate from how it is cut.
//!
//! V1 had two knobs (`--crf`, `--preset`) and wrote an MP4 beside the source. That is the
//! right shape for a utility and the wrong shape for a product, because the question an
//! editor actually asks is "give me the vertical cut and the YouTube cut from the same
//! segment", not "give me a CRF".
//!
//! A preset therefore bundles the container, the video treatment, the audio treatment, the
//! frame geometry and the loudness target — and, critically, it is *a value*, so a project
//! can name one, a segment can override it, and an export can carry it. Presets are data;
//! adding a format is adding a row, not a code path.
//!
//! ## The loudness point
//!
//! Loudness targeting is not a garnish. A cut taken out of the middle of a podcast has the
//! level of the room it was recorded in, and a social platform will not normalise it up for
//! you — it will only turn you *down*. So a delivery preset states the target and the
//! measurement basis, and the executor hands those to ffmpeg's `loudnorm` in two-pass mode.
//! [`LoudnessTarget::measurement_basis`] is where the spec's numbers live.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// The preset a project uses when a segment does not name one.
pub const DEFAULT_PRESET: &str = "master";

/// The container a cut is written into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    /// MP4. The default: plays everywhere, and Premiere imports it without a word.
    Mp4,
    /// QuickTime MOV. Required by some broadcast and ProRes deliveries.
    Mov,
    /// Matroska. Tolerant, good for archival when the codec is exotic.
    Mkv,
    /// MXF OP1a. Broadcast house format.
    Mxf,
    /// WAV, for an audio-only deliverable.
    Wav,
}

impl Container {
    /// The file extension, without the dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Mkv => "mkv",
            Self::Mxf => "mxf",
            Self::Wav => "wav",
        }
    }

    /// The muxer ffmpeg should be told to use. `None` lets ffmpeg infer it from the
    /// extension, which is correct for every container here and safer than guessing a
    /// muxer name that may not be compiled in.
    #[must_use]
    pub const fn muxer(self) -> Option<&'static str> {
        match self {
            Self::Mxf => Some("mxf"),
            _ => None,
        }
    }

    /// True when the container carries video.
    #[must_use]
    pub const fn carries_video(self) -> bool {
        !matches!(self, Self::Wav)
    }

    /// True when `+faststart` is meaningful for this container.
    #[must_use]
    pub const fn supports_faststart(self) -> bool {
        matches!(self, Self::Mp4 | Self::Mov)
    }
}

/// What happens to the picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VideoTreatment {
    /// There is no picture in this deliverable. Only legal in a container that carries no
    /// video; the validator refuses it anywhere else.
    None,
    /// Leave the original packets alone. This is the whole point of the product, so it is
    /// the default and the presets that cannot honour it say [`VideoTreatment::Encode`].
    Copy,
    /// Re-encode at a quality target. Used for the re-encoded head, and for a full pass
    /// when geometry or codec has to change.
    Encode {
        /// Encoder name as ffmpeg knows it, e.g. `libx264`.
        encoder: String,
        /// Constant rate factor, or another encoder-specific quality number.
        quality: u8,
        /// Speed preset.
        speed: String,
        /// Target codec name, for the record and for the export metadata.
        codec: String,
        /// Also write a hardware encoder variant when one is available.
        prefer_hardware: bool,
    },
}

impl VideoTreatment {
    /// The x264/x265 encoder for a source codec, so a head patch keeps the codec family.
    #[must_use]
    pub fn for_source_codec(codec: &str) -> Option<Self> {
        match codec.to_lowercase().as_str() {
            "h264" => Some(Self::Encode {
                encoder: "libx264".to_owned(),
                quality: 18,
                speed: "veryfast".to_owned(),
                codec: "h264".to_owned(),
                prefer_hardware: false,
            }),
            "hevc" | "h265" => Some(Self::Encode {
                encoder: "libx265".to_owned(),
                quality: 18,
                speed: "veryfast".to_owned(),
                codec: "hevc".to_owned(),
                prefer_hardware: false,
            }),
            _ => None,
        }
    }

    /// True when the picture is left untouched.
    #[must_use]
    pub const fn is_copy(&self) -> bool {
        matches!(self, Self::Copy)
    }

    /// True when there is no picture at all in the deliverable.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// What happens to the sound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AudioTreatment {
    /// Drop the audio entirely.
    None,
    /// Leave the original packets alone.
    Copy,
    /// Re-encode.
    Encode {
        /// Encoder name as ffmpeg knows it, e.g. `aac`.
        encoder: String,
        /// Target bitrate, e.g. `192k`.
        bitrate: String,
        /// Target sample rate in Hz.
        sample_rate: u32,
        /// Target channel count.
        channels: u16,
    },
}

impl AudioTreatment {
    /// AAC at 48 kHz stereo, the safe default for web delivery.
    #[must_use]
    pub fn aac_stereo(bitrate: &str) -> Self {
        Self::Encode {
            encoder: "aac".to_owned(),
            bitrate: bitrate.to_owned(),
            sample_rate: 48_000,
            channels: 2,
        }
    }
}

/// A loudness target, in the terms the standard states it.
///
/// The numbers here are the published ones: −14 LUFS integrated for the streaming
/// platforms, −16 for podcast delivery, −23 for EBU R128 broadcast and −24 for ATSC A/85.
/// They are stated as data so the report can quote the spec rather than a magic number.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessTarget {
    /// Integrated loudness target in LUFS.
    pub integrated_lufs: f64,
    /// True loudness peak ceiling in dBTP.
    pub true_peak_dbtp: f64,
    /// Loudness range target in LU. `None` leaves the range alone.
    pub range_lu: Option<f64>,
}

impl LoudnessTarget {
    /// −14 LUFS, −1 dBTP: what the streaming platforms normalise to.
    pub const STREAMING: Self = Self {
        integrated_lufs: -14.0,
        true_peak_dbtp: -1.0,
        range_lu: Some(11.0),
    };
    /// −16 LUFS, −1 dBTP: stereo podcast delivery.
    pub const PODCAST: Self = Self {
        integrated_lufs: -16.0,
        true_peak_dbtp: -1.0,
        range_lu: Some(11.0),
    };
    /// −23 LUFS, −1 dBTP: EBU R128, European broadcast.
    pub const EBU_R128: Self = Self {
        integrated_lufs: -23.0,
        true_peak_dbtp: -1.0,
        range_lu: Some(15.0),
    };
    /// −24 LUFS, −2 dBTP: ATSC A/85, North American broadcast.
    pub const ATSC_A85: Self = Self {
        integrated_lufs: -24.0,
        true_peak_dbtp: -2.0,
        range_lu: Some(15.0),
    };

    /// The name of the standard this target implements, for the report.
    #[must_use]
    pub const fn measurement_basis(&self) -> &'static str {
        if (self.integrated_lufs + 14.0).abs() < 0.01 {
            "streaming normalisation (-14 LUFS)"
        } else if (self.integrated_lufs + 16.0).abs() < 0.01 {
            "stereo podcast delivery (-16 LUFS)"
        } else if (self.integrated_lufs + 23.0).abs() < 0.01 {
            "EBU R128 (-23 LUFS)"
        } else if (self.integrated_lufs + 24.0).abs() < 0.01 {
            "ATSC A/85 (-24 LUFS)"
        } else {
            "custom target"
        }
    }

    /// The arguments for a `loudnorm` filter, in single-pass mode.
    ///
    /// The executor prefers a measured two-pass run and uses this for the measurement pass
    /// and for preview; see `trimmer-media::loudnorm`.
    #[must_use]
    pub fn filter_args(&self) -> String {
        let range = self
            .range_lu
            .map_or_else(String::new, |range| format!(":lra={range:.1}"));
        format!(
            "loudnorm=I={:.1}:TP={:.1}{range}",
            self.integrated_lufs, self.true_peak_dbtp
        )
    }
}

/// How the frame is re-shaped for a different aspect ratio.
///
/// Blurred-background and crop are both needed: an interview cut for vertical wants the
/// subject centred and *kept*, whereas a wide screen recording wants the middle band with
/// the sides filled. Guessing one loses the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AspectFit {
    /// Do not change the geometry.
    Native,
    /// Scale to fit inside the frame, filling the remainder with a blurred copy of itself.
    BlurredBackground,
    /// Scale to fill and crop the overflow. Loses the edges; keeps the subject.
    Crop,
    /// Scale to fit and add solid bars.
    Pillarbox,
}

/// Target frame geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Geometry {
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// How to reconcile the source's shape with this one.
    pub fit: AspectFit,
}

impl Geometry {
    /// Leave the source's own geometry alone.
    pub const NATIVE: Self = Self {
        width: 0,
        height: 0,
        fit: AspectFit::Native,
    };
    /// 1080p landscape.
    pub const HD_1080: Self = Self {
        width: 1920,
        height: 1080,
        fit: AspectFit::Native,
    };
    /// 1080×1920 vertical, filling the frame.
    pub const VERTICAL_1080: Self = Self {
        width: 1080,
        height: 1920,
        fit: AspectFit::Crop,
    };
    /// 1080×1920 vertical on a blurred bed, which keeps the whole wide frame readable.
    pub const VERTICAL_BLUR: Self = Self {
        width: 1080,
        height: 1920,
        fit: AspectFit::BlurredBackground,
    };
    /// 1080×1080 square.
    pub const SQUARE_1080: Self = Self {
        width: 1080,
        height: 1080,
        fit: AspectFit::Crop,
    };

    /// True when the geometry is the source's own.
    #[must_use]
    pub const fn is_native(self) -> bool {
        self.width == 0 || self.height == 0 || matches!(self.fit, AspectFit::Native)
    }

    /// True when the picture must be re-encoded to satisfy this geometry.
    #[must_use]
    pub fn forces_encode(self, source_width: u32, source_height: u32) -> bool {
        if self.is_native() {
            return false;
        }
        self.width != source_width
            || self.height != source_height
            || !matches!(self.fit, AspectFit::Native)
    }

    /// The `scale`/`crop`/`pad` chain for ffmpeg, as filter strings.
    ///
    /// Returned as separate filters rather than one comma-joined string so the executor can
    /// log and test each step, and so a preset that only needs one of them does not carry
    /// the others.
    #[must_use]
    pub fn filters(self) -> Vec<String> {
        if self.is_native() {
            return Vec::new();
        }
        let (width, height) = (self.width, self.height);
        match self.fit {
            AspectFit::Native => Vec::new(),
            AspectFit::BlurredBackground => vec![format!(
                "split=2[bg][fg];\
                 [bg]scale={width}:{height}:force_original_aspect_ratio=increase,\
                 crop={width}:{height},gblur=sigma=28[bgb];\
                 [fg]scale={width}:{height}:force_original_aspect_ratio=decrease[fgs];\
                 [bgb][fgs]overlay=(W-w)/2:(H-h)/2"
            )],
            AspectFit::Crop => vec![format!(
                "scale={width}:{height}:force_original_aspect_ratio=increase,\
                 crop={width}:{height}"
            )],
            AspectFit::Pillarbox => vec![format!(
                "scale={width}:{height}:force_original_aspect_ratio=decrease,\
                 pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"
            )],
        }
    }
}

/// One delivery preset: a complete answer to "what should come out".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryPreset {
    /// Stable name, e.g. `master` or `vertical`.
    pub name: String,
    /// What the preset is for, shown in the UI.
    pub description: String,
    /// The container.
    pub container: Container,
    /// What happens to the picture.
    pub video: VideoTreatment,
    /// What happens to the sound.
    pub audio: AudioTreatment,
    /// Target geometry.
    pub geometry: Geometry,
    /// Loudness target, when the audio is to be normalised.
    pub loudness: Option<LoudnessTarget>,
    /// Frames of handle to use when the segment does not specify its own.
    pub default_handle_frames: i64,
    /// Whether this preset is safe to run as a batch on many segments unattended.
    pub batch_safe: bool,
}

impl DeliveryPreset {
    /// Validate the preset before it is used to build a command line.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Delivery`] for a combination that cannot be produced: video
    /// treatment in a container that carries no video, a re-encode with no encoder, a video
    /// container with no video stream, and an audio encode at an impossible rate.
    pub fn validate(&self) -> CoreResult<()> {
        let fail = |reason: &str| CoreError::Delivery {
            preset: self.name.clone(),
            reason: reason.to_owned(),
        };

        let carries_video = self.container.carries_video();
        let treatment_is_none = self.video.is_none();
        if carries_video && treatment_is_none {
            return Err(fail(
                "the preset removes the picture from a video container",
            ));
        }
        if !carries_video && !treatment_is_none {
            return Err(fail(
                "the container carries no picture, but the preset sets a video treatment",
            ));
        }
        if let VideoTreatment::Encode {
            encoder, quality, ..
        } = &self.video
        {
            if encoder.trim().is_empty() {
                return Err(fail("the video treatment names no encoder"));
            }
            if *quality > 51 {
                return Err(fail("video quality above 51 is not a valid CRF"));
            }
        }
        if let AudioTreatment::Encode {
            sample_rate,
            channels,
            encoder,
            ..
        } = &self.audio
        {
            if encoder.trim().is_empty() {
                return Err(fail("the audio treatment names no encoder"));
            }
            if *sample_rate < 8_000 || *sample_rate > 384_000 {
                return Err(fail(
                    "the audio sample rate is outside anything a codec accepts",
                ));
            }
            if *channels == 0 || *channels > 16 {
                return Err(fail(
                    "the audio channel count is outside anything a codec accepts",
                ));
            }
        }
        if let Some(loudness) = self.loudness {
            if !loudness.integrated_lufs.is_finite() || loudness.integrated_lufs > 0.0 {
                return Err(fail("the loudness target is not a negative LUFS value"));
            }
            if !loudness.true_peak_dbtp.is_finite() || loudness.true_peak_dbtp > 0.0 {
                return Err(fail("the true-peak ceiling is not a negative dBTP value"));
            }
        }
        if self.default_handle_frames < 0 {
            return Err(fail("negative handle frames would cut into the segment"));
        }
        Ok(())
    }

    /// True when this preset leaves both picture and sound untouched.
    #[must_use]
    pub fn is_passthrough(&self) -> bool {
        self.video.is_copy() && self.audio == AudioTreatment::Copy
    }

    /// True when the preset can be satisfied without re-encoding the picture, which is what
    /// makes a cut cheap. Geometry and loudness both break it.
    #[must_use]
    pub fn preserves_picture(&self, source_width: u32, source_height: u32) -> bool {
        self.video.is_copy() && !self.geometry.forces_encode(source_width, source_height)
    }
}

/// The preset library every project starts with.
///
/// Three of these are passthrough by design — `master`, `prores_master` and `wav_split` —
/// because a studio that buys this tool is buying the cut, not a transcode. The rest exist
/// so that the same segment can go to a platform without a second pass in another app.
#[must_use]
pub fn standard_presets() -> Vec<DeliveryPreset> {
    vec![
        DeliveryPreset {
            name: "master".to_owned(),
            description: "Original packets, MP4, audio copied. Nothing is re-encoded except \
                          the head frames a cut cannot avoid."
                .to_owned(),
            container: Container::Mp4,
            video: VideoTreatment::Copy,
            audio: AudioTreatment::Copy,
            geometry: Geometry::NATIVE,
            loudness: None,
            default_handle_frames: 0,
            batch_safe: true,
        },
        DeliveryPreset {
            name: "master_faststart".to_owned(),
            description: "As master, with the index at the front so it streams and scrubs \
                          instantly."
                .to_owned(),
            container: Container::Mp4,
            video: VideoTreatment::Copy,
            audio: AudioTreatment::Copy,
            geometry: Geometry::NATIVE,
            loudness: None,
            default_handle_frames: 0,
            batch_safe: true,
        },
        DeliveryPreset {
            name: "prores_master".to_owned(),
            description: "Apple ProRes 422 HQ in MOV at source geometry. For a finishing \
                          house that will not touch a long-GOP file."
                .to_owned(),
            container: Container::Mov,
            video: VideoTreatment::Encode {
                encoder: "prores_ks".to_owned(),
                quality: 12,
                speed: "medium".to_owned(),
                codec: "prores".to_owned(),
                prefer_hardware: false,
            },
            audio: AudioTreatment::Encode {
                encoder: "pcm_s24le".to_owned(),
                bitrate: "unused".to_owned(),
                sample_rate: 48_000,
                channels: 2,
            },
            geometry: Geometry::NATIVE,
            loudness: None,
            default_handle_frames: 0,
            batch_safe: false,
        },
        DeliveryPreset {
            name: "youtube_1080".to_owned(),
            description: "1920x1080 H.264 at CRF 18, AAC 320k, normalised to -14 LUFS.".to_owned(),
            container: Container::Mp4,
            video: VideoTreatment::Encode {
                encoder: "libx264".to_owned(),
                quality: 18,
                speed: "medium".to_owned(),
                codec: "h264".to_owned(),
                prefer_hardware: true,
            },
            audio: AudioTreatment::aac_stereo("320k"),
            geometry: Geometry::HD_1080,
            loudness: Some(LoudnessTarget::STREAMING),
            default_handle_frames: 0,
            batch_safe: true,
        },
        DeliveryPreset {
            name: "vertical".to_owned(),
            description: "1080x1920 H.264, centre-cropped from the middle of frame, \
                          normalised to -14 LUFS. For Shorts, Reels and TikTok."
                .to_owned(),
            container: Container::Mp4,
            video: VideoTreatment::Encode {
                encoder: "libx264".to_owned(),
                quality: 20,
                speed: "medium".to_owned(),
                codec: "h264".to_owned(),
                prefer_hardware: true,
            },
            audio: AudioTreatment::aac_stereo("256k"),
            geometry: Geometry::VERTICAL_1080,
            loudness: Some(LoudnessTarget::STREAMING),
            default_handle_frames: 0,
            batch_safe: true,
        },
        DeliveryPreset {
            name: "vertical_blur".to_owned(),
            description: "1080x1920 with the whole wide frame on a blurred bed. Keeps both \
                          speakers when a crop would cut one out."
                .to_owned(),
            container: Container::Mp4,
            video: VideoTreatment::Encode {
                encoder: "libx264".to_owned(),
                quality: 20,
                speed: "medium".to_owned(),
                codec: "h264".to_owned(),
                prefer_hardware: true,
            },
            audio: AudioTreatment::aac_stereo("256k"),
            geometry: Geometry::VERTICAL_BLUR,
            loudness: Some(LoudnessTarget::STREAMING),
            default_handle_frames: 0,
            batch_safe: true,
        },
        DeliveryPreset {
            name: "square_social".to_owned(),
            description: "1080x1080 H.264, cropped, normalised to -14 LUFS.".to_owned(),
            container: Container::Mp4,
            video: VideoTreatment::Encode {
                encoder: "libx264".to_owned(),
                quality: 20,
                speed: "medium".to_owned(),
                codec: "h264".to_owned(),
                prefer_hardware: true,
            },
            audio: AudioTreatment::aac_stereo("256k"),
            geometry: Geometry::SQUARE_1080,
            loudness: Some(LoudnessTarget::STREAMING),
            default_handle_frames: 0,
            batch_safe: true,
        },
        DeliveryPreset {
            name: "podcast_audio".to_owned(),
            description: "Audio only, AAC 192k in WAV, normalised to -16 LUFS. For an \
                          audio-first feed."
                .to_owned(),
            container: Container::Wav,
            video: VideoTreatment::None,
            audio: AudioTreatment::aac_stereo("192k"),
            geometry: Geometry::NATIVE,
            loudness: Some(LoudnessTarget::PODCAST),
            default_handle_frames: 0,
            batch_safe: true,
        },
        DeliveryPreset {
            name: "wav_split".to_owned(),
            description: "48 kHz 24-bit stereo WAV, untouched. For a sound editor.".to_owned(),
            container: Container::Wav,
            video: VideoTreatment::None,
            audio: AudioTreatment::Encode {
                encoder: "pcm_s24le".to_owned(),
                bitrate: "unused".to_owned(),
                sample_rate: 48_000,
                channels: 2,
            },
            geometry: Geometry::NATIVE,
            loudness: None,
            default_handle_frames: 0,
            batch_safe: true,
        },
        DeliveryPreset {
            name: "broadcast_r128".to_owned(),
            description: "1920x1080 H.264 high profile, PCM audio, EBU R128 -23 LUFS. \
                          European broadcast delivery."
                .to_owned(),
            container: Container::Mov,
            video: VideoTreatment::Encode {
                encoder: "libx264".to_owned(),
                quality: 16,
                speed: "slow".to_owned(),
                codec: "h264".to_owned(),
                prefer_hardware: false,
            },
            audio: AudioTreatment::Encode {
                encoder: "pcm_s24le".to_owned(),
                bitrate: "unused".to_owned(),
                sample_rate: 48_000,
                channels: 2,
            },
            geometry: Geometry::HD_1080,
            loudness: Some(LoudnessTarget::EBU_R128),
            default_handle_frames: 0,
            batch_safe: false,
        },
        DeliveryPreset {
            name: "mxf_op1a".to_owned(),
            description: "MXF OP1a, XDCAM HD422 50 Mbit at 1920x1080, PCM audio. For a \
                          broadcast server that will not take an MP4."
                .to_owned(),
            container: Container::Mxf,
            video: VideoTreatment::Encode {
                encoder: "mpeg2video".to_owned(),
                quality: 0,
                speed: "medium".to_owned(),
                codec: "mpeg2video".to_owned(),
                prefer_hardware: false,
            },
            audio: AudioTreatment::Encode {
                encoder: "pcm_s24le".to_owned(),
                bitrate: "unused".to_owned(),
                sample_rate: 48_000,
                channels: 2,
            },
            geometry: Geometry::HD_1080,
            loudness: None,
            default_handle_frames: 0,
            batch_safe: false,
        },
    ]
}

/// Look a preset up in the standard library.
///
/// # Errors
///
/// Returns [`CoreError::Delivery`] when no standard preset has that name.
pub fn standard_preset(name: &str) -> CoreResult<DeliveryPreset> {
    standard_presets()
        .into_iter()
        .find(|preset| preset.name == name)
        .ok_or_else(|| CoreError::Delivery {
            preset: name.to_owned(),
            reason: "no such standard preset".to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_preset_is_valid() {
        for preset in standard_presets() {
            preset
                .validate()
                .unwrap_or_else(|error| panic!("preset {} is not valid: {error}", preset.name));
        }
    }

    #[test]
    fn the_default_preset_exists_and_is_passthrough() {
        let preset = standard_preset(DEFAULT_PRESET).expect("the default exists");
        assert!(preset.is_passthrough());
        assert!(preset.batch_safe);
        assert!(preset.preserves_picture(3840, 2160));
    }

    #[test]
    fn preset_names_are_unique() {
        let mut names: Vec<String> = standard_presets().into_iter().map(|p| p.name).collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count, "a preset name is used twice");
    }

    #[test]
    fn an_unknown_preset_is_refused_by_name() {
        let error = standard_preset("nope").expect_err("refused");
        match error {
            CoreError::Delivery { preset, .. } => assert_eq!(preset, "nope"),
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn geometry_reports_when_it_forces_an_encode() {
        let vertical = Geometry::VERTICAL_1080;
        assert!(vertical.forces_encode(1920, 1080));
        assert!(!Geometry::NATIVE.forces_encode(1920, 1080));
        assert!(!Geometry::HD_1080.forces_encode(1920, 1080));
    }

    #[test]
    fn crop_geometry_builds_a_scale_then_crop_chain() {
        let filters = Geometry::VERTICAL_1080.filters();
        assert_eq!(filters.len(), 1);
        assert!(filters[0].contains("force_original_aspect_ratio=increase"));
        assert!(filters[0].contains("crop=1080:1920"));
    }

    #[test]
    fn blurred_background_keeps_the_whole_frame() {
        let filters = Geometry::VERTICAL_BLUR.filters();
        assert_eq!(filters.len(), 1);
        assert!(filters[0].contains("gblur"));
        assert!(filters[0].contains("overlay"));
        // The foreground is *decreased* to fit, which is what keeps the sides visible.
        assert!(filters[0].contains("force_original_aspect_ratio=decrease"));
    }

    #[test]
    fn native_geometry_produces_no_filters() {
        assert!(Geometry::NATIVE.filters().is_empty());
    }

    #[test]
    fn loudness_targets_name_the_standard_they_implement() {
        assert!(LoudnessTarget::STREAMING
            .measurement_basis()
            .contains("streaming"));
        assert!(LoudnessTarget::EBU_R128
            .measurement_basis()
            .contains("R128"));
        assert!(LoudnessTarget::ATSC_A85
            .measurement_basis()
            .contains("A/85"));
        assert_eq!(
            LoudnessTarget::PODCAST.filter_args(),
            "loudnorm=I=-16.0:TP=-1.0:lra=11.0"
        );
    }

    #[test]
    fn an_impossible_preset_is_refused_with_a_reason() {
        let mut preset = standard_preset("master").expect("ok");
        preset.loudness = Some(LoudnessTarget {
            integrated_lufs: 4.0,
            true_peak_dbtp: -1.0,
            range_lu: None,
        });
        let error = preset.validate().expect_err("refused");
        match error {
            CoreError::Delivery { reason, .. } => assert!(reason.contains("LUFS"), "{reason}"),
            other => panic!("wrong error: {other:?}"),
        }

        let mut audio_in_a_video_container = standard_preset("podcast_audio").expect("ok");
        audio_in_a_video_container.audio = AudioTreatment::Encode {
            encoder: "aac".to_owned(),
            bitrate: "192k".to_owned(),
            sample_rate: 4_000,
            channels: 2,
        };
        assert!(audio_in_a_video_container.validate().is_err());
    }

    #[test]
    fn containers_report_their_own_capabilities() {
        assert_eq!(Container::Mp4.extension(), "mp4");
        assert!(Container::Mp4.supports_faststart());
        assert_eq!(Container::Mxf.muxer(), Some("mxf"));
        assert!(!Container::Wav.carries_video());
    }

    #[test]
    fn video_treatment_for_a_source_codec_preserves_the_family() {
        match VideoTreatment::for_source_codec("hevc").expect("known") {
            VideoTreatment::Encode { encoder, codec, .. } => {
                assert_eq!(encoder, "libx265");
                assert_eq!(codec, "hevc");
            }
            other => panic!("wrong treatment: {other:?}"),
        }
        assert!(VideoTreatment::for_source_codec("prores").is_none());
    }
}

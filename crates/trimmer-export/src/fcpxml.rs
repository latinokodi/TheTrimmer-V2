//! FCPXML version 1.11, where every time is a rational number of seconds.
//!
//! The format is unforgiving about time. `offset`, `duration`, `start` and `frameDuration`
//! are all `N/Ds`, and a float that is a ten-millionth of a second out is a clip that lands
//! on the wrong frame. So every time in this module is produced by
//! [`Rational::from_frames`](crate::clips::Rational::from_frames), which is integer
//! arithmetic from the frame count and the rate's own numerator and denominator.

use trimmer_core::CoreResult;
use trimmer_core::FrameRate;

use crate::clips::{dimensions, prepare, sequence_end, ExportClip, Rational};
use crate::request::{ExportFormat, ExportProduct, ExportRequest};
use crate::xml;

/// Build an FCPXML 1.11 library document.
///
/// # Errors
///
/// Returns `CoreError::NotFound` when a segment names a source the project does not hold.
pub fn export_fcpxml(request: &ExportRequest<'_>) -> CoreResult<ExportProduct> {
    let prepared = prepare(request)?;
    let mut warnings = prepared.warnings;
    let body = document(request, &prepared.clips, &mut warnings);
    Ok(ExportProduct {
        format: ExportFormat::Fcpxml,
        suggested_extension: ExportFormat::Fcpxml.extension(),
        body,
        warnings,
        clip_count: prepared.clips.len(),
        total_frames: prepared.total_frames,
    })
}

/// One `<asset>`: a source file, and the first clip that named it.
struct Asset<'a> {
    /// The resource id, from `r2` up; `r1` is the sequence format.
    id: usize,
    /// The clip whose source this is.
    clip: &'a ExportClip,
}

/// The distinct sources behind the clips, in the order they first appear.
///
/// FCPXML gives every source one asset, so two cuts into the same master reference the same
/// resource instead of duplicating the file — which is also what keeps a relinked library
/// pointing at one file rather than at one per cut.
fn assets(clips: &[ExportClip]) -> Vec<Asset<'_>> {
    let mut collected: Vec<Asset<'_>> = Vec::new();
    for clip in clips {
        if collected
            .iter()
            .any(|asset| asset.clip.source == clip.source)
        {
            continue;
        }
        let id = collected.len() + 2;
        collected.push(Asset { id, clip });
    }
    collected
}

/// The resource id for a clip's source.
fn asset_id(assets: &[Asset<'_>], clip: &ExportClip) -> usize {
    assets
        .iter()
        .find(|asset| asset.clip.source == clip.source)
        .map_or(2, |asset| asset.id)
}

/// The whole document, as text.
fn document(
    request: &ExportRequest<'_>,
    clips: &[ExportClip],
    warnings: &mut Vec<String>,
) -> String {
    let rate = request.timeline_rate;
    let (width, height) = dimensions(clips);
    let assets = assets(clips);
    let sequence_name = xml::clean(&request.sequence_name, "the sequence name", warnings);
    let format_name = format_name(height, rate);
    let frame_duration = Rational::from_frames(1, rate).text();
    let duration = Rational::from_frames(sequence_end(request, clips), rate).text();
    let tc_start = Rational::ZERO.text();
    let tc_format = if rate.supports_drop_frame() {
        "DF"
    } else {
        "NDF"
    };

    let mut out = String::with_capacity(2048 + clips.len() * 512);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<!DOCTYPE fcpxml>\n");
    out.push_str("<fcpxml version=\"1.11\">\n");
    xml::line(&mut out, 1, "<resources>");
    xml::line(
        &mut out,
        2,
        &format!(
            "<format id=\"r1\" name=\"{format_name}\" frameDuration=\"{frame_duration}\" \
             width=\"{width}\" height=\"{height}\"/>"
        ),
    );
    for asset in &assets {
        asset_element(&mut out, asset, warnings);
    }
    xml::line(&mut out, 1, "</resources>");
    xml::line(&mut out, 1, "<library>");
    xml::line(&mut out, 2, &format!("<event name=\"{sequence_name}\">"));
    xml::line(&mut out, 3, &format!("<project name=\"{sequence_name}\">"));
    xml::line(
        &mut out,
        4,
        &format!(
            "<sequence format=\"r1\" duration=\"{duration}\" tcStart=\"{tc_start}\" \
             tcFormat=\"{tc_format}\">"
        ),
    );
    xml::line(&mut out, 5, "<spine>");
    for clip in clips {
        asset_clip(&mut out, clip, asset_id(&assets, clip), rate, warnings);
    }
    xml::line(&mut out, 5, "</spine>");
    xml::line(&mut out, 4, "</sequence>");
    xml::line(&mut out, 3, "</project>");
    xml::line(&mut out, 2, "</event>");
    xml::line(&mut out, 1, "</library>");
    out.push_str("</fcpxml>\n");
    out
}

/// One `<asset>` element.
fn asset_element(out: &mut String, asset: &Asset<'_>, warnings: &mut Vec<String>) {
    let clip = asset.clip;
    let id = asset.id;
    let name = xml::clean(
        &clip.file_name,
        &format!("the file name behind resource r{id}"),
        warnings,
    );
    let src = xml::fcpxml_url(&clip.import_path);
    let duration = Rational::from_frames(clip.source_frames, clip.rate).text();
    let has_audio = u8::from(clip.has_audio);

    xml::line(
        out,
        2,
        &format!(
            "<asset id=\"r{id}\" name=\"{name}\" src=\"{src}\" start=\"0s\" \
             duration=\"{duration}\" hasVideo=\"1\" hasAudio=\"{has_audio}\" format=\"r1\"/>"
        ),
    );
}

/// One `<asset-clip>`: a cut placed on the spine.
fn asset_clip(
    out: &mut String,
    clip: &ExportClip,
    asset: usize,
    timeline_rate: FrameRate,
    warnings: &mut Vec<String>,
) {
    let name = xml::clean(&clip.name, &format!("the name of clip r{asset}"), warnings);
    // `offset` and `duration` are positions and lengths on the *timeline*, so they are
    // measured at the timeline's rate; `start` is a position in the *source*, so it is
    // measured at the source's own rate. On a rate mismatch those two grids differ, which is
    // exactly why the mismatch is reported rather than papered over.
    let offset = Rational::from_frames(clip.timeline_start, timeline_rate).text();
    let duration = Rational::from_frames(clip.frames(), timeline_rate).text();
    let start = Rational::from_frames(clip.source_in, clip.rate).text();

    xml::line(
        out,
        6,
        &format!(
            "<asset-clip ref=\"r{asset}\" offset=\"{offset}\" duration=\"{duration}\" \
             start=\"{start}\" name=\"{name}\"/>"
        ),
    );
}

/// The name FCP gives a video format, such as `FFVideoFormat1080p30`.
///
/// The timebase in the name is nominal, so a 29.97 sequence is `…p30`, which is what every
/// FCPXML file in the wild says.
fn format_name(height: u32, rate: FrameRate) -> String {
    let timebase = rate.nominal();
    format!("FFVideoFormat{height}p{timebase}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_name_uses_the_nominal_timebase() {
        assert_eq!(
            format_name(1080, FrameRate::FPS_29_97),
            "FFVideoFormat1080p30"
        );
        assert_eq!(format_name(720, FrameRate::FPS_25), "FFVideoFormat720p25");
    }

    #[test]
    fn the_frame_duration_of_a_single_frame_is_the_rates_denominator_over_its_numerator() {
        assert_eq!(
            Rational::from_frames(1, FrameRate::FPS_29_97).text(),
            "1001/30000s"
        );
        assert_eq!(Rational::from_frames(1, FrameRate::FPS_25).text(), "1/25s");
        assert_eq!(
            Rational::from_frames(1, FrameRate::FPS_23_976).text(),
            "1001/24000s"
        );
    }
}

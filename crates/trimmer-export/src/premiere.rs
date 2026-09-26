//! The Final Cut Pro 7 `xmeml` document, which Premiere Pro imports as a sequence.
//!
//! `xmeml` version 4 is the interchange format the post houses this tool was built for
//! actually round-trip through: Premiere Pro reads it, Resolve reads it, and it carries the
//! three things an edit needs — where a clip sits on the timeline, which frames of the source
//! it shows, and where the source file is. Positions are frame numbers, not seconds, which is
//! why this format and the domain agree so easily.

use trimmer_core::FrameRate;
use trimmer_core::CoreResult;

use crate::clips::{dimensions, prepare, sequence_end, ExportClip};
use crate::request::{ExportFormat, ExportProduct, ExportRequest};
use crate::xml;

/// Build a Premiere Pro sequence document.
///
/// # Errors
///
/// Returns `CoreError::NotFound` when a segment names a source the project does not hold.
pub fn export_premiere_xml(request: &ExportRequest<'_>) -> CoreResult<ExportProduct> {
    let prepared = prepare(request)?;
    let mut warnings = prepared.warnings;
    let body = document(request, &prepared.clips, &mut warnings);
    Ok(ExportProduct {
        format: ExportFormat::PremiereXml,
        suggested_extension: ExportFormat::PremiereXml.extension(),
        body,
        warnings,
        clip_count: prepared.clips.len(),
        total_frames: prepared.total_frames,
    })
}

/// The whole document, as text.
fn document(
    request: &ExportRequest<'_>,
    clips: &[ExportClip],
    warnings: &mut Vec<String>,
) -> String {
    let rate = request.timeline_rate;
    let (width, height) = dimensions(clips);
    let sequence_name = xml::clean(&request.sequence_name, "the sequence name", warnings);

    let mut out = String::with_capacity(2048 + clips.len() * 1024);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<!DOCTYPE xmeml>\n");
    out.push_str("<xmeml version=\"4\">\n");
    xml::line(&mut out, 1, "<sequence>");
    xml::line(&mut out, 2, &format!("<name>{sequence_name}</name>"));
    xml::line(
        &mut out,
        2,
        &format!("<duration>{}</duration>", sequence_end(request, clips)),
    );
    xml::line(&mut out, 2, &rate_element(rate));
    xml::line(&mut out, 2, "<media>");
    xml::line(&mut out, 3, "<video>");
    xml::line(&mut out, 4, "<format>");
    xml::line(&mut out, 5, "<samplecharacteristics>");
    xml::line(&mut out, 6, &rate_element(rate));
    xml::line(&mut out, 6, &format!("<width>{width}</width>"));
    xml::line(&mut out, 6, &format!("<height>{height}</height>"));
    xml::line(&mut out, 5, "</samplecharacteristics>");
    xml::line(&mut out, 4, "</format>");
    xml::line(&mut out, 4, "<track>");
    for (index, clip) in clips.iter().enumerate() {
        clip_item(&mut out, index + 1, clip, warnings);
    }
    xml::line(&mut out, 4, "</track>");
    xml::line(&mut out, 3, "</video>");
    xml::line(&mut out, 2, "</media>");
    xml::line(&mut out, 1, "</sequence>");
    out.push_str("</xmeml>\n");
    out
}

/// One `<clipitem>`, with its file element.
///
/// Every clipitem carries its own full `<file>` element. FCP7 permits a bare
/// `<file id="file-1"/>` reference after the first, but a document that defines its files
/// inline is trivially readable by every importer, including the ones that ignore the
/// reference form.
fn clip_item(out: &mut String, index: usize, clip: &ExportClip, warnings: &mut Vec<String>) {
    let name = xml::clean(&clip.name, &format!("the name of clip {index}"), warnings);
    let file_name = xml::clean(
        &clip.file_name,
        &format!("the file name of clip {index}"),
        warnings,
    );
    let url = xml::premiere_url(&clip.import_path);
    let source_frames = clip.source_frames;

    xml::line(out, 5, &format!("<clipitem id=\"clipitem-{index}\">"));
    xml::line(out, 6, &format!("<name>{name}</name>"));
    // The clipitem duration is the master clip's length, not the length of this cut: this is
    // how Premiere knows how much material there is to trim into, and it is what makes the
    // handles in an edit work.
    xml::line(out, 6, &format!("<duration>{source_frames}</duration>"));
    xml::line(out, 6, &rate_element(clip.rate));
    xml::line(out, 6, &format!("<start>{}</start>", clip.timeline_start));
    xml::line(out, 6, &format!("<end>{}</end>", clip.timeline_end));
    xml::line(out, 6, &format!("<in>{}</in>", clip.source_in));
    xml::line(out, 6, &format!("<out>{}</out>", clip.source_out));
    xml::line(out, 6, &format!("<file id=\"file-{index}\">"));
    xml::line(out, 7, &format!("<name>{file_name}</name>"));
    xml::line(out, 7, &format!("<pathurl>{url}</pathurl>"));
    xml::line(out, 7, &rate_element(clip.rate));
    xml::line(out, 7, &format!("<duration>{source_frames}</duration>"));
    xml::line(out, 7, "<media>");
    xml::line(out, 8, "<video>");
    xml::line(out, 9, "<samplecharacteristics>");
    xml::line(out, 10, &format!("<width>{}</width>", clip.width));
    xml::line(out, 10, &format!("<height>{}</height>", clip.height));
    xml::line(out, 9, "</samplecharacteristics>");
    xml::line(out, 8, "</video>");
    xml::line(out, 7, "</media>");
    xml::line(out, 6, "</file>");
    xml::line(out, 5, "</clipitem>");
}

/// The `<rate>` element as FCP7 writes it: a nominal timebase and an NTSC flag.
///
/// The timebase is the *nominal* integer count — 30 for 29.97 — while the flag is what tells
/// an importer that the nominal count is a lie by a thousandth. Without the flag, a 29.97
/// sequence imports as a 30 fps one and every cut drifts.
fn rate_element(rate: FrameRate) -> String {
    let timebase = rate.nominal();
    let ntsc = if is_ntsc(rate) { "TRUE" } else { "FALSE" };
    format!("<rate><timebase>{timebase}</timebase><ntsc>{ntsc}</ntsc></rate>")
}

/// True for the 1000/1001 family, which is the whole of what NTSC means here.
fn is_ntsc(rate: FrameRate) -> bool {
    rate.as_ffmpeg().contains("1001")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ntsc_flag_follows_the_thousand_over_one_thousand_and_one_family() {
        for rate in [
            FrameRate::FPS_23_976,
            FrameRate::FPS_29_97,
            FrameRate::FPS_59_94,
        ] {
            assert!(is_ntsc(rate), "{rate} should be NTSC");
        }
        for rate in [FrameRate::FPS_24, FrameRate::FPS_25, FrameRate::FPS_30] {
            assert!(!is_ntsc(rate), "{rate} should not be NTSC");
        }
    }

    #[test]
    fn the_timebase_is_nominal_not_fractional() {
        assert_eq!(
            rate_element(FrameRate::FPS_29_97),
            "<rate><timebase>30</timebase><ntsc>TRUE</ntsc></rate>"
        );
        assert_eq!(
            rate_element(FrameRate::FPS_25),
            "<rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>"
        );
    }
}

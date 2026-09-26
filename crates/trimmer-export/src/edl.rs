//! CMX3600 edit decision lists.
//!
//! An EDL is the oldest of the four formats and the one with the most ways to be quietly
//! wrong. Two matter here.
//!
//! **Out points are inclusive.** Everywhere else in this program a range is `[start, end)`,
//! so a fifty-frame cut ends at frame 50. An EDL writes `49` — the last frame the event
//! actually contains. Writing the exclusive end would add one frame to every event in the
//! list, which is the kind of error nobody notices until the conform is a second long.
//!
//! **Drop-frame is a property of the whole list, not of an event.** The `FCM` line says which
//! counting the timecodes use, and the timecode separator agrees with it: `:` for non-drop,
//! `;` for drop. A list whose header and whose stamps disagree is read differently by
//! different tools.

use trimmer_core::timecode::format_timecode;
use trimmer_core::CoreResult;

use crate::clips::{prepare, ExportClip};
use crate::request::{ExportFormat, ExportProduct, ExportRequest};
use crate::xml;

/// The reel name for material that has no tape behind it.
///
/// CMX3600 reserves eight characters for the reel, and `AX` is the conventional spelling of
/// "auxiliary source" — a file, a graphics card, anything that was never on a reel.
const REEL: &str = "AX";

/// The largest event number a three-digit field can hold.
const MAX_EVENTS: usize = 999;

/// Build a CMX3600 edit decision list.
///
/// # Errors
///
/// Returns `CoreError::NotFound` when a segment names a source the project does not hold.
pub fn export_edl(request: &ExportRequest<'_>) -> CoreResult<ExportProduct> {
    let prepared = prepare(request)?;
    let mut warnings = prepared.warnings;
    let body = document(request, &prepared.clips, &mut warnings);
    Ok(ExportProduct {
        format: ExportFormat::Edl,
        suggested_extension: ExportFormat::Edl.extension(),
        body,
        warnings,
        clip_count: prepared.clips.len(),
        total_frames: prepared.total_frames,
    })
}

/// The whole list, as text.
fn document(
    request: &ExportRequest<'_>,
    clips: &[ExportClip],
    warnings: &mut Vec<String>,
) -> String {
    let rate = request.timeline_rate;
    let drop = rate.supports_drop_frame();
    let title = line_text(&request.sequence_name, "the sequence name", warnings);

    let mut out = String::with_capacity(256 + clips.len() * 128);
    xml::line(&mut out, 0, &format!("TITLE: {title}"));
    out.push_str(if drop {
        "FCM: DROP FRAME\n"
    } else {
        "FCM: NON-DROP FRAME\n"
    });
    out.push('\n');

    if clips.len() > MAX_EVENTS {
        warnings.push(format!(
            "the timeline has {} events; a CMX3600 list numbers events in three digits, so \
             events past {MAX_EVENTS} are written with four and some readers stop at the \
             three-digit limit",
            clips.len()
        ));
    }

    for (index, clip) in clips.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let event = index + 1;
        let source_in = format_timecode(clip.source_in, clip.rate, None);
        let source_out = format_timecode(clip.source_out - 1, clip.rate, None);
        let record_in = format_timecode(clip.timeline_start, rate, Some(drop));
        let record_out = format_timecode(clip.timeline_end - 1, rate, Some(drop));
        xml::line(
            &mut out,
            0,
            &format!(
                "{event:03}  {REEL:<8} V     C        {source_in} {source_out} {record_in} \
                 {record_out}"
            ),
        );
        // An EDL has no field for the file: the reel column holds eight characters and the
        // event describes a tape that does not exist. The comment line is the only place a
        // conforming tool can be told which file the event actually came from, so it is
        // written for every event rather than only when there is more than one source.
        let file_name = line_text(
            &clip.file_name,
            &format!("the file name of event {event}"),
            warnings,
        );
        xml::line(&mut out, 0, &format!("* FROM CLIP NAME: {file_name}"));
    }
    out
}

/// Text with the characters a line-oriented list cannot carry removed.
///
/// A newline in a title does not produce a bad EDL, it produces two EDLs, and the second one
/// is gibberish. The forbidden set is the same one XML uses, for the same reason.
fn line_text(raw: &str, what: &str, warnings: &mut Vec<String>) -> String {
    if xml::has_forbidden(raw) {
        warnings.push(format!(
            "{what} contained control characters; they were removed, because an edit decision \
             list is read one line at a time"
        ));
    }
    xml::strip_forbidden(raw).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use trimmer_core::FrameRate;

    #[test]
    fn the_reel_stays_inside_the_eight_character_field() {
        assert!(REEL.len() <= 8, "the reel field is eight characters wide");
    }

    #[test]
    fn drop_frame_is_offered_only_where_the_rate_has_a_drop_frame_form() {
        assert!(FrameRate::FPS_29_97.supports_drop_frame());
        assert!(FrameRate::FPS_59_94.supports_drop_frame());
        assert!(!FrameRate::FPS_25.supports_drop_frame());
        assert!(!FrameRate::FPS_30.supports_drop_frame());
    }
}

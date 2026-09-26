//! End-to-end tests for the export layer, written against the public API only.
//!
//! Every assertion here is about a *document* — the text an editor would import — rather than
//! about a file on disk. That is the whole point of the crate being a pure string builder:
//! a test can read the third clip's `<in>` without writing anything anywhere.

use trimmer_core::domain::{AudioFormat, SegmentSource, Timescale};
use trimmer_core::FrameRate;
use trimmer_core::{MediaInfo, MediaPath, Project, Segment};
use trimmer_export::{
    export, export_csv, export_edl, export_fcpxml, export_premiere_xml, write_csv, ExportFormat,
    ExportRequest,
};

/// The master most fixtures cut from.
const MASTER: &str = r"H:\THEROLLUPFILES\master.mp4";
/// A second source, for the tests about more than one file.
const SECOND: &str = r"H:\THEROLLUPFILES\Andy Ross.mp4";
/// A source that is in the project but has never been probed.
const OFFLINE: &str = r"H:\THEROLLUPFILES\offline.mov";

/// The resolver the tests use by default: the source path, unchanged.
fn identity(path: &MediaPath) -> String {
    path.to_string()
}

/// A `'static` reference to [`identity`], because an [`ExportRequest`] borrows its resolver.
static IDENTITY: fn(&MediaPath) -> String = identity;

/// A resolver that pretends the masters live on a Linux NAS instead.
fn remapped(path: &MediaPath) -> String {
    path.to_string().replace(r"H:\THEROLLUPFILES", "/mnt/masters")
}

/// A `'static` reference to [`remapped`].
static REMAPPED: fn(&MediaPath) -> String = remapped;

/// Media facts for a source, with everything a writer reads filled in.
fn media(path: &str, rate: FrameRate, frames: i64) -> MediaInfo {
    MediaInfo {
        path: MediaPath::new(path),
        codec: "h264".to_owned(),
        pix_fmt: "yuv420p".to_owned(),
        width: 1920,
        height: 1080,
        rate,
        average_rate: Some(rate),
        timebase: Timescale::NINETY_KHZ,
        frame_count: frames,
        audio: Some(AudioFormat {
            codec: "aac".to_owned(),
            sample_rate: 48_000,
            channels: 2,
        }),
        size_bytes: 6_000_000_000,
        start_time: 0.0,
    }
}

/// A project holding one probed master and one unprobed, offline source.
fn project() -> Project {
    let mut project = Project::new("Rollup", "Fernando", 1_700_000_000);
    project.upsert_source(SegmentSource {
        path: MediaPath::new(MASTER),
        media: Some(media(MASTER, FrameRate::FPS_25, 10_000)),
        available: true,
        label: Some("Andy".to_owned()),
    });
    project.upsert_source(SegmentSource::unprobed(OFFLINE));
    project
}

/// A project whose master runs at 29.97, for the NTSC and drop-frame tests.
fn ntsc_project() -> Project {
    let mut project = Project::new("Rollup", "Fernando", 1_700_000_000);
    project.upsert_source(SegmentSource {
        path: MediaPath::new(MASTER),
        media: Some(media(MASTER, FrameRate::FPS_29_97, 10_000)),
        available: true,
        label: None,
    });
    project
}

/// A request at 25 fps, starting at frame 0, with the identity resolver.
fn request<'a>(project: &'a Project, segments: &'a [Segment]) -> ExportRequest<'a> {
    ExportRequest {
        project,
        segments: segments.iter().collect(),
        timeline_start_frame: 0,
        sequence_name: "Rollup".to_owned(),
        timeline_rate: FrameRate::FPS_25,
        media_path_for: &IDENTITY,
    }
}

/// Three cuts, of 50, 30 and 20 frames, from the probed master.
fn three_cuts() -> Vec<Segment> {
    vec![
        Segment::new(MASTER, "cold open", 100, 150),
        Segment::new(MASTER, "begging", 200, 230),
        Segment::new(MASTER, "the button", 300, 320),
    ]
}

// --- the dispatcher -------------------------------------------------------------------

/// Read a document back with a real XML parser, so "well formed" is not an opinion.
fn assert_xml_parses(body: &str) {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(body);
    let mut depth: i64 = 0;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => depth -= 1,
            Ok(_) => {}
            Err(error) => panic!("{error} at {:?}\n{body}", reader.buffer_position()),
        }
    }
    assert_eq!(depth, 0, "unbalanced tags in\n{body}");
}

#[test]
fn both_xml_documents_are_well_formed_even_when_a_name_is_full_of_markup() {
    let project = project();
    let segments = three_cuts();
    let plain = request(&project, &segments);
    assert_xml_parses(&export_premiere_xml(&plain).expect("exports").body);
    assert_xml_parses(&export_fcpxml(&plain).expect("exports").body);

    let awkward = [Segment::new(MASTER, "a & b < c > d \" e ' f", 0, 50)];
    let markup = request(&project, &awkward);
    assert_xml_parses(&export_premiere_xml(&markup).expect("exports").body);
    assert_xml_parses(&export_fcpxml(&markup).expect("exports").body);

    let empty = Project::new("Empty", "Fernando", 0);
    let nothing = request(&empty, &[]);
    assert_xml_parses(&export_premiere_xml(&nothing).expect("exports").body);
    assert_xml_parses(&export_fcpxml(&nothing).expect("exports").body);
}

#[test]
fn export_dispatches_to_the_right_format_and_extension_every_time() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    for (format, extension) in [
        (ExportFormat::PremiereXml, "xml"),
        (ExportFormat::Fcpxml, "fcpxml"),
        (ExportFormat::Edl, "edl"),
        (ExportFormat::Csv, "csv"),
    ] {
        let product = export(&request, format).expect("exports");
        assert_eq!(product.format, format);
        assert_eq!(product.suggested_extension, extension);
        assert_eq!(product.suggested_extension, format.extension());
        assert_eq!(product.clip_count, 3);
        assert!(!product.body.is_empty());
    }
}

#[test]
fn the_dispatcher_and_the_format_function_produce_the_same_document() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    assert_eq!(
        export(&request, ExportFormat::PremiereXml).expect("exports"),
        export_premiere_xml(&request).expect("exports")
    );
    assert_eq!(
        export(&request, ExportFormat::Fcpxml).expect("exports"),
        export_fcpxml(&request).expect("exports")
    );
    assert_eq!(
        export(&request, ExportFormat::Edl).expect("exports"),
        export_edl(&request).expect("exports")
    );
    assert_eq!(
        export(&request, ExportFormat::Csv).expect("exports"),
        export_csv(&request).expect("exports")
    );
}

// --- an empty project -----------------------------------------------------------------

#[test]
fn an_empty_project_produces_a_valid_empty_document_in_all_four_formats() {
    let project = Project::new("Nothing yet", "Fernando", 0);
    let request = request(&project, &[]);

    let premiere = export_premiere_xml(&request).expect("exports");
    assert_eq!(premiere.clip_count, 0);
    assert_eq!(premiere.total_frames, 0);
    assert!(premiere.warnings.is_empty(), "{:?}", premiere.warnings);
    assert!(premiere.body.contains("<xmeml version=\"4\">"));
    assert!(premiere.body.contains("</xmeml>"));
    assert!(premiere.body.contains("<track>\n        </track>"));
    assert!(!premiere.body.contains("<clipitem"));

    let fcpxml = export_fcpxml(&request).expect("exports");
    assert_eq!(fcpxml.clip_count, 0);
    assert!(fcpxml.body.contains("<fcpxml version=\"1.11\">"));
    assert!(fcpxml.body.contains("<spine>\n          </spine>"));
    assert!(!fcpxml.body.contains("<asset-clip"));

    let edl = export_edl(&request).expect("exports");
    assert_eq!(edl.clip_count, 0);
    assert!(edl.body.starts_with("TITLE: Rollup\nFCM: NON-DROP FRAME\n"));
    assert!(!edl.body.contains("001"));

    let csv = export_csv(&request).expect("exports");
    assert_eq!(csv.clip_count, 0);
    assert_eq!(csv.body.lines().count(), 1);
    assert!(csv.body.ends_with("\r\n"));
}

// --- Premiere XML ---------------------------------------------------------------------

#[test]
fn premiere_xml_escapes_markup_characters_in_names() {
    let project = project();
    let segments = [Segment::new(MASTER, "cold & \"warm\" <open>", 0, 50)];
    let request = request(&project, &segments);

    let body = export_premiere_xml(&request).expect("exports").body;
    assert!(
        body.contains("<name>cold &amp; &quot;warm&quot; &lt;open&gt;</name>"),
        "{body}"
    );
}

#[test]
fn premiere_xml_escapes_an_apostrophe_too_so_the_escaping_does_not_depend_on_where_text_lands() {
    let project = project();
    let segments = [Segment::new(MASTER, "Andy's take", 0, 50)];
    let request = request(&project, &segments);

    let body = export_premiere_xml(&request).expect("exports").body;
    assert!(body.contains("<name>Andy&apos;s take</name>"), "{body}");
}

#[test]
fn premiere_xml_writes_a_file_url_with_spaces_encoded_and_backslashes_turned_into_slashes() {
    let project = project();
    let segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let plain_request = request(&project, &segments);

    let body = export_premiere_xml(&plain_request).expect("exports").body;
    assert!(
        body.contains("<pathurl>file://localhost/H:/THEROLLUPFILES/master.mp4</pathurl>"),
        "{body}"
    );

    let mut spaced = Project::new("Rollup", "Fernando", 0);
    spaced.upsert_source(SegmentSource {
        path: MediaPath::new(SECOND),
        media: Some(media(SECOND, FrameRate::FPS_25, 500)),
        available: true,
        label: None,
    });
    let spaced_segments = [Segment::new(SECOND, "cold open", 0, 50)];
    let spaced_request = request(&spaced, &spaced_segments);
    let body = export_premiere_xml(&spaced_request).expect("exports").body;
    assert!(
        body.contains("<pathurl>file://localhost/H:/THEROLLUPFILES/Andy%20Ross.mp4</pathurl>"),
        "{body}"
    );
}

#[test]
fn premiere_xml_accumulates_start_and_end_so_three_clips_butt_up_against_each_other() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    let body = export_premiere_xml(&request).expect("exports").body;

    // 50, 30 and 20 frames long, so the timeline reads 0..50, 50..80, 80..100.
    for fragment in [
        "<start>0</start>",
        "<end>50</end>",
        "<start>50</start>",
        "<end>80</end>",
        "<start>80</start>",
        "<end>100</end>",
    ] {
        assert!(body.contains(fragment), "missing {fragment} in\n{body}");
    }

    // And the source ranges are the marks the user made, untouched.
    for fragment in [
        "<in>100</in>",
        "<out>150</out>",
        "<in>200</in>",
        "<out>230</out>",
        "<in>300</in>",
        "<out>320</out>",
    ] {
        assert!(body.contains(fragment), "missing {fragment} in\n{body}");
    }
}

#[test]
fn premiere_xml_reports_the_sequence_duration_as_the_end_of_the_last_clip() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    let body = export_premiere_xml(&request).expect("exports").body;
    assert!(body.contains("<duration>100</duration>"), "{body}");
    assert!(body.contains("<name>Rollup</name>"), "{body}");
    assert!(body.contains("<width>1920</width>"));
    assert!(body.contains("<height>1080</height>"));
}

#[test]
fn premiere_xml_marks_ntsc_rates_and_only_ntsc_rates() {
    let ntsc = ntsc_project();
    let segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let ntsc_request = ExportRequest {
        project: &ntsc,
        segments: segments.iter().collect(),
        timeline_start_frame: 0,
        sequence_name: "Rollup".to_owned(),
        timeline_rate: FrameRate::FPS_29_97,
        media_path_for: &IDENTITY,
    };
    let body = export_premiere_xml(&ntsc_request).expect("exports").body;
    assert!(body.contains("<rate><timebase>30</timebase><ntsc>TRUE</ntsc></rate>"), "{body}");
    assert!(!body.contains("<ntsc>FALSE</ntsc>"), "{body}");

    let pal = project();
    let pal_segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let pal_request = request(&pal, &pal_segments);
    let body = export_premiere_xml(&pal_request).expect("exports").body;
    assert!(body.contains("<rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>"), "{body}");
    assert!(!body.contains("<ntsc>TRUE</ntsc>"), "{body}");
}

#[test]
fn premiere_xml_names_every_clipitem_and_file_element() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    let body = export_premiere_xml(&request).expect("exports").body;
    for index in 1..=3 {
        assert!(body.contains(&format!("<clipitem id=\"clipitem-{index}\">")), "{body}");
        assert!(body.contains(&format!("<file id=\"file-{index}\">")), "{body}");
    }
    // The clipitem duration is the master clip's length, which is what Premiere trims into.
    assert!(body.contains("<duration>10000</duration>"), "{body}");
}

#[test]
fn a_control_character_is_stripped_out_of_xml_and_reported_rather_than_written() {
    let project = project();
    let segments = [Segment::new(MASTER, "cold\u{7} open", 0, 50)];
    let request = request(&project, &segments);

    let product = export_premiere_xml(&request).expect("exports");
    assert!(product.body.contains("<name>cold open</name>"), "{}", product.body);
    assert!(!product.body.contains('\u{7}'));
    assert_eq!(product.warnings.len(), 1);
    assert!(product.warnings[0].contains("control"), "{:?}", product.warnings);
}

#[test]
fn a_negative_timeline_start_is_clamped_to_zero_and_reported() {
    let project = project();
    let segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let request = ExportRequest {
        project: &project,
        segments: segments.iter().collect(),
        timeline_start_frame: -25,
        sequence_name: "Rollup".to_owned(),
        timeline_rate: FrameRate::FPS_25,
        media_path_for: &IDENTITY,
    };

    let product = export_premiere_xml(&request).expect("exports");
    assert!(product.body.contains("<start>0</start>"), "{}", product.body);
    assert_eq!(product.warnings.len(), 1);
    assert!(product.warnings[0].contains("-25"), "{:?}", product.warnings);
}

// --- FCPXML ---------------------------------------------------------------------------

#[test]
fn fcpxml_writes_the_frame_duration_of_a_thirty_fps_timeline_as_an_exact_rational() {
    let project = ntsc_project();
    let segments = [Segment::new(MASTER, "cold open", 0, 1)];
    let request = ExportRequest {
        project: &project,
        segments: segments.iter().collect(),
        timeline_start_frame: 0,
        sequence_name: "Rollup".to_owned(),
        timeline_rate: FrameRate::FPS_29_97,
        media_path_for: &IDENTITY,
    };

    let body = export_fcpxml(&request).expect("exports").body;
    assert!(body.contains("frameDuration=\"1001/30000s\""), "{body}");
    // One frame of timeline is one frame of source here, so the clip is the same duration.
    assert!(body.contains("duration=\"1001/30000s\""), "{body}");
    assert!(body.contains("tcFormat=\"DF\""), "{body}");
}

#[test]
fn fcpxml_writes_a_hundred_frames_at_twenty_five_as_exactly_four_over_one() {
    let project = project();
    let segments = [Segment::new(MASTER, "cold open", 0, 100)];
    let request = request(&project, &segments);

    let body = export_fcpxml(&request).expect("exports").body;
    assert!(body.contains("offset=\"0s\""), "{body}");
    assert!(body.contains("duration=\"4/1s\""), "{body}");
    assert!(body.contains("tcFormat=\"NDF\""), "{body}");
    assert!(body.contains("frameDuration=\"1/25s\""), "{body}");
    assert!(!body.contains("0.033"), "no floating point time reached the document:\n{body}");
}

#[test]
fn fcpxml_gives_every_distinct_source_one_asset_and_reuses_it() {
    let mut project = project();
    project.upsert_source(SegmentSource {
        path: MediaPath::new(SECOND),
        media: Some(media(SECOND, FrameRate::FPS_25, 500)),
        available: true,
        label: None,
    });
    let segments = [
        Segment::new(MASTER, "one", 0, 50),
        Segment::new(SECOND, "two", 0, 50),
        Segment::new(MASTER, "three", 50, 100),
    ];
    let request = request(&project, &segments);

    let body = export_fcpxml(&request).expect("exports").body;
    assert_eq!(body.matches("<asset ").count(), 2, "{body}");
    assert!(body.contains("<asset id=\"r2\""), "{body}");
    assert!(body.contains("<asset id=\"r3\""), "{body}");
    // The third cut comes from the same file as the first, so it points back at r2.
    assert_eq!(body.matches("ref=\"r2\"").count(), 2, "{body}");
    assert_eq!(body.matches("ref=\"r3\"").count(), 1, "{body}");
}

#[test]
fn fcpxml_offsets_and_starts_are_rational_seconds_on_the_right_grid() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    let body = export_fcpxml(&request).expect("exports").body;
    // Second clip: 50 frames in, so it starts 2 s into the timeline, and its source in point
    // is frame 200, which is 8 s into the file.
    assert!(body.contains("offset=\"2/1s\""), "{body}");
    assert!(body.contains("start=\"8/1s\""), "{body}");
    // Third clip: 80 frames in and 20 long.
    assert!(body.contains("offset=\"16/5s\""), "{body}");
    assert!(body.contains("duration=\"4/5s\""), "{body}");
    // The sequence is 100 frames, which is 4 s.
    assert!(body.contains("<sequence format=\"r1\" duration=\"4/1s\""), "{body}");
}

#[test]
fn fcpxml_declares_the_sequence_format_from_the_first_clip() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    let body = export_fcpxml(&request).expect("exports").body;
    assert!(
        body.contains("<format id=\"r1\" name=\"FFVideoFormat1080p25\""),
        "{body}"
    );
    assert!(body.contains("<library>"), "{body}");
    assert!(body.contains("<event name=\"Rollup\">"), "{body}");
    assert!(body.contains("<project name=\"Rollup\">"), "{body}");
}

// --- EDL ------------------------------------------------------------------------------

#[test]
fn edl_numbers_events_in_three_digits_from_one_and_lays_out_the_columns() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    let body = export_edl(&request).expect("exports").body;
    let reel = format!("{:<8}", "AX");
    let expected_second = format!(
        "002  {reel} V     C        00:00:08:00 00:00:09:04 00:00:02:00 00:00:03:04"
    );
    assert!(body.contains(&expected_second), "{body}");
    assert!(body.contains("003"), "{body}");
    assert!(!body.contains("004"), "{body}");
}

#[test]
fn edl_out_points_are_inclusive_so_a_fifty_frame_event_ends_on_frame_forty_nine() {
    let project = project();
    let segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let request = request(&project, &segments);

    let body = export_edl(&request).expect("exports").body;
    let line = body.lines().find(|line| line.starts_with("001")).expect("event 001");
    let fields: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(
        fields,
        [
            "001",
            "AX",
            "V",
            "C",
            "00:00:00:00",
            "00:00:01:24",
            "00:00:00:00",
            "00:00:01:24"
        ],
        "{body}"
    );
}

#[test]
fn edl_switches_between_drop_and_non_drop_with_its_timecodes() {
    let ntsc = ntsc_project();
    let segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let drop_request = ExportRequest {
        project: &ntsc,
        segments: segments.iter().collect(),
        timeline_start_frame: 0,
        sequence_name: "Rollup".to_owned(),
        timeline_rate: FrameRate::FPS_29_97,
        media_path_for: &IDENTITY,
    };
    let drop = export_edl(&drop_request).expect("exports").body;
    assert!(drop.starts_with("TITLE: Rollup\nFCM: DROP FRAME\n"), "{drop}");
    assert!(drop.contains("00:00:01;19"), "{drop}");
    assert!(!drop.contains("00:00:01:19"), "{drop}");

    let pal = project();
    let pal_segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let pal_request = request(&pal, &pal_segments);
    let non_drop = export_edl(&pal_request).expect("exports").body;
    assert!(
        non_drop.starts_with("TITLE: Rollup\nFCM: NON-DROP FRAME\n"),
        "{non_drop}"
    );
    assert!(non_drop.contains("00:00:01:24"), "{non_drop}");
}

#[test]
fn edl_names_the_file_behind_every_event_because_the_reel_column_cannot() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    let body = export_edl(&request).expect("exports").body;
    assert_eq!(body.matches("* FROM CLIP NAME: master.mp4").count(), 3, "{body}");
}

// --- CSV ------------------------------------------------------------------------------

#[test]
fn csv_quotes_a_name_containing_a_comma_and_a_quote_to_rfc_4180() {
    let project = project();
    let segments = [Segment::new(MASTER, "begging, take \"two\"", 0, 50)];
    let request = request(&project, &segments);

    let body = export_csv(&request).expect("exports").body;
    assert!(
        body.contains("\"begging, take \"\"two\"\"\""),
        "{body}"
    );
}

#[test]
fn csv_separates_rows_with_carriage_return_and_line_feed() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    let body = export_csv(&request).expect("exports").body;
    let rows: Vec<&str> = body.split("\r\n").collect();
    assert_eq!(rows.len(), 5, "a header and three rows, then the terminator:\n{body}");
    assert_eq!(rows[4], "");
    assert!(rows.iter().all(|row| !row.contains('\n')));
}

#[test]
fn csv_carries_the_columns_a_producer_asks_for_with_three_decimal_seconds() {
    let project = project();
    let mut segment = Segment::new(MASTER, "begging", 100, 150);
    segment.preset = Some("youtube_1080".to_owned());
    segment.tags = vec!["andy".to_owned(), "act one".to_owned()];
    segment.note = Some("the good take".to_owned());
    let segments = [segment];
    let request = request(&project, &segments);

    let body = export_csv(&request).expect("exports").body;
    let row = body.lines().nth(1).expect("one data row");
    assert_eq!(
        row,
        "1,begging,H:\\THEROLLUPFILES\\master.mp4,100,150,00:00:04:00,00:00:06:00,50,2.000,\
         youtube_1080,andy;act one,the good take"
    );
}

// --- validation and warnings ----------------------------------------------------------

#[test]
fn a_segment_on_a_source_the_project_does_not_hold_is_a_not_found_error() {
    let project = project();
    let segments = [Segment::new(r"H:\elsewhere\ghost.mp4", "ghost", 0, 50)];
    let request = request(&project, &segments);

    for format in ExportFormat::ALL {
        let error = export(&request, format).expect_err("must refuse");
        match error {
            trimmer_core::CoreError::NotFound { entity, id } => {
                assert_eq!(entity, "source");
                assert!(id.contains("ghost.mp4"), "{id}");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }
}

#[test]
fn a_segment_on_an_unprobed_source_is_skipped_with_a_warning_and_the_rest_still_exports() {
    let project = project();
    let segments = [
        Segment::new(MASTER, "cold open", 0, 50),
        Segment::new(OFFLINE, "from the offline drive", 0, 50),
        Segment::new(MASTER, "the button", 300, 320),
    ];
    let request = request(&project, &segments);

    for format in ExportFormat::ALL {
        let product = export(&request, format).expect("still exports");
        assert_eq!(product.clip_count, 2, "{format:?} skipped the wrong number of segments");
        assert_eq!(product.total_frames, 70, "{format:?}");
        assert_eq!(product.warnings.len(), 1, "{:?}", product.warnings);
        let warning = &product.warnings[0];
        assert!(warning.contains("from the offline drive"), "{warning}");
        assert!(warning.contains("has not been probed"), "{warning}");
        assert!(!product.body.contains("offline.mov"), "{format:?}");
    }
}

#[test]
fn a_rate_mismatch_is_a_warning_and_the_frames_are_not_rescaled() {
    let project = ntsc_project();
    let segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let request = request(&project, &segments);

    let product = export_premiere_xml(&request).expect("exports");
    assert_eq!(product.clip_count, 1);
    assert_eq!(product.total_frames, 50, "a mismatched rate must not rescale the cut");
    assert_eq!(product.warnings.len(), 1, "{:?}", product.warnings);
    let warning = &product.warnings[0];
    assert!(warning.contains("cold open"), "{warning}");
    assert!(warning.contains("30000/1001"), "{warning}");
    assert!(warning.contains(" 25 fps"), "{warning}");
    // The clip keeps its own source rate, so its rate element is 30/TRUE, not 25/FALSE.
    assert!(
        product.body.contains("<rate><timebase>30</timebase><ntsc>TRUE</ntsc></rate>"),
        "{}",
        product.body
    );
}

#[test]
fn total_frames_is_the_sum_of_the_lengths_on_the_timeline() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);

    for format in ExportFormat::ALL {
        let product = export(&request, format).expect("exports");
        assert_eq!(product.total_frames, 50 + 30 + 20, "{format:?}");
        assert_eq!(product.clip_count, 3);
    }
}

#[test]
fn handles_widen_the_range_because_they_are_part_of_what_the_segment_covers() {
    let project = project();
    let mut segment = Segment::new(MASTER, "begging", 100, 150);
    segment.handle_frames = 10;
    let segments = [segment];
    let request = request(&project, &segments);

    let product = export_premiere_xml(&request).expect("exports");
    assert_eq!(product.total_frames, 70);
    assert!(product.body.contains("<in>90</in>"), "{}", product.body);
    assert!(product.body.contains("<out>160</out>"), "{}", product.body);
}

#[test]
fn an_open_ended_segment_runs_to_the_end_of_its_source() {
    let project = project();
    let mut segment = Segment::new(MASTER, "to the end", 9_950, 0);
    segment.end_frame = None;
    let segments = [segment];
    let request = request(&project, &segments);

    let product = export_premiere_xml(&request).expect("exports");
    assert_eq!(product.total_frames, 50);
    assert!(product.body.contains("<out>10000</out>"), "{}", product.body);
}

#[test]
fn a_mark_past_the_end_of_the_source_is_clamped_and_reported() {
    let project = project();
    let segments = [Segment::new(MASTER, "over the end", 9_980, 10_500)];
    let request = request(&project, &segments);

    let product = export_premiere_xml(&request).expect("exports");
    assert_eq!(product.total_frames, 20);
    assert!(product.body.contains("<out>10000</out>"), "{}", product.body);
    assert_eq!(product.warnings.len(), 1, "{:?}", product.warnings);
    assert!(
        product.warnings.iter().any(|warning| warning.contains("clamped")),
        "{:?}",
        product.warnings
    );
}

// --- the caller's path resolver -------------------------------------------------------

#[test]
fn the_media_path_resolver_decides_what_the_document_names() {
    let project = project();
    let segments = [Segment::new(MASTER, "cold open", 0, 50)];
    let request = ExportRequest {
        project: &project,
        segments: segments.iter().collect(),
        timeline_start_frame: 0,
        sequence_name: "Rollup".to_owned(),
        timeline_rate: FrameRate::FPS_25,
        media_path_for: &REMAPPED,
    };

    let premiere = export_premiere_xml(&request).expect("exports").body;
    assert!(
        premiere.contains("<pathurl>file://localhost/mnt/masters/master.mp4</pathurl>"),
        "{premiere}"
    );
    let fcpxml = export_fcpxml(&request).expect("exports").body;
    assert!(
        fcpxml.contains("src=\"file:///mnt/masters/master.mp4\""),
        "{fcpxml}"
    );
    let csv = export_csv(&request).expect("exports").body;
    // The CSV is not a URL, so it carries whatever the resolver returned, separator and all.
    assert!(csv.contains("/mnt/masters\\master.mp4"), "{csv}");
}

// --- the one write helper -------------------------------------------------------------

#[test]
fn each_write_helper_puts_its_document_on_disk_unchanged() {
    let project = project();
    let segments = three_cuts();
    let request = request(&project, &segments);
    let directory = std::env::temp_dir().join(format!("trimmer-export-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("scratch directory");

    let premiere = export_premiere_xml(&request).expect("exports");
    let premiere_path = directory.join(format!("sequence.{}", premiere.suggested_extension));
    trimmer_export::write_premiere_xml(&premiere_path, &premiere.body).expect("writes");
    assert_eq!(
        std::fs::read_to_string(&premiere_path).expect("reads back"),
        premiere.body
    );

    let csv = export_csv(&request).expect("exports");
    let csv_path = directory.join(format!("cuts.{}", csv.suggested_extension));
    write_csv(&csv_path, &csv.body).expect("writes");
    let written = std::fs::read_to_string(&csv_path).expect("reads back");
    assert_eq!(written, csv.body);
    assert!(written.contains("\r\n"));

    assert!(premiere_path.exists() && csv_path.exists());
    std::fs::remove_dir_all(&directory).expect("cleans up");
}

#[test]
fn every_export_format_offers_a_unique_extension_and_a_label() {
    let extensions: Vec<&str> = ExportFormat::ALL
        .iter()
        .map(|format| format.extension())
        .collect();
    assert_eq!(extensions, ["xml", "fcpxml", "edl", "csv"]);
    for format in ExportFormat::ALL {
        assert!(!format.label().is_empty(), "{format:?}");
        assert_eq!(format.to_string(), format.label());
    }
}

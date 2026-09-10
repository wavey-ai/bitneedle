//! A record is pressed, given a sidecar, permuted, and read back.
//!
//! The sidecar checks in `test-spin` separate a record that opens from a
//! record that decodes alone. These tests therefore run against a pressed
//! record rather than against a hand-assembled container. Three faults occur in
//! practice: a reverse map that describes another groove, a pointer whose
//! digest covers an earlier stream, and an item whose payload differs from its
//! declared type.

use base64::Engine as _;

const PROFILE: &str = "single45";

/// The ECDC a golden record was cut from — real payload, so the groove has
/// the shape a groove has.
fn payload() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../goldenfiles/records/lori-asha-westside-single45-hq/lori-asha-westside-single45-hq.ecdc"
    ))
    .expect("the golden ECDC is checked in beside this test")
}

/// A pressed record, with nothing in its label but the groove.
fn press(payload: &[u8]) -> Vec<u8> {
    bitneedle_record_author::render_payload_container_to_png_native(
        payload,
        "ecdc",
        "encodec",
        "rgb",
        PROFILE,
        30.0,
        "{}",
    )
    .expect("the golden ECDC presses")
    .png_bytes
}

/// The same record with a sidecar painted into its label and lead-in.
fn with_sidecar(png: &[u8], items: serde_json::Value) -> Vec<u8> {
    let options = serde_json::json!({
        "sidecar": {
            "scheme": "pairsign-safe-luma-v2",
            "carriers": ["label", "leadIn"],
            "items": items,
        }
    });
    record_sidecar::rewrite_record_png(png, &options.to_string(), Some(PROFILE))
        .expect("the sidecar paints into the record")
        .0
}

fn text_item(name: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "utf8Text",
        "codec": "raw",
        "name": name,
        "text": text,
    })
}

#[test]
fn a_record_without_a_sidecar_is_not_a_record_with_a_broken_one() {
    let png = press(&payload());
    let report = test_spin::sidecars::inspect(&png, Some(PROFILE));

    assert!(!report.present(), "a bare press carries no sidecar");
    assert!(report.error.is_none(), "and that is not an error");
    assert!(report.ok(), "so the record passes");
}

#[test]
fn arbitrary_items_are_checked_as_closely_as_the_named_ones() {
    let png = press(&payload());
    // Every name here is outside the registry of this crate. The inspection
    // walks, types and reports an item that a presser invented, in the way
    // that it handles a named item.
    let png = with_sidecar(
        &png,
        serde_json::json!([
            text_item("liner-notes", "cut at 45, one take"),
            serde_json::json!({
                "type": "json",
                "codec": "raw",
                "name": "session",
                "json": { "room": "wavey", "takes": 3 },
            }),
            serde_json::json!({
                "type": "opaque",
                "codec": "raw",
                "name": "whatever",
                "dataBase64": base64::engine::general_purpose::STANDARD.encode([0u8, 1, 2, 3, 255]),
            }),
        ]),
    );

    let report = test_spin::sidecars::inspect(&png, Some(PROFILE));
    assert!(report.present(), "the sidecar is found");
    assert!(report.ok(), "and every check passes: {:#?}", report.checks);

    let inspection = report.inspection.as_ref().expect("present");
    assert_eq!(inspection.decoded.items.len(), 3);
    assert_eq!(inspection.arbitrary_items().len(), 3);

    // Each item is named in a check of its own, so a reader can see that
    // the unknown ones were looked at rather than skipped.
    for name in ["liner-notes", "session", "whatever"] {
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.label.contains(name) && check.passed),
            "{name} has a check of its own"
        );
    }

    let text = inspection.item("liner-notes").expect("carried");
    assert_eq!(text.text.as_deref(), Some("cut at 45, one take"));
    let json = inspection.item("session").expect("carried");
    assert_eq!(json.json.as_ref().expect("json")["takes"], 3);
}

#[test]
fn a_label_that_has_been_edited_no_longer_carries_its_sidecar() {
    let png = press(&payload());
    let png = with_sidecar(&png, serde_json::json!([text_item("note", "hello")]));
    assert!(
        test_spin::sidecars::inspect(&png, Some(PROFILE)).ok(),
        "the record is sound before it is touched"
    );

    // Repaint the middle of the label. The sidecar is hidden in the
    // brightness relationships between neighbouring pixels there, so a
    // record whose label has been edited is a record whose sidecar is gone
    // — and it must read as gone rather than as merely different.
    let edited = repaint_label(&png);
    let report = test_spin::sidecars::inspect(&edited, Some(PROFILE));

    assert!(
        !report.ok(),
        "an edited label must not pass: {:#?}",
        report.checks
    );
}

/// Flatten a square in the middle of the record — the label — to one colour.
fn repaint_label(png: &[u8]) -> Vec<u8> {
    let image = image::load_from_memory(png).expect("the pressed record is a PNG");
    let mut rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let (centre_x, centre_y) = (width / 2, height / 2);
    // Well inside the label radius at every profile this test presses.
    let reach = width / 10;
    for y in centre_y.saturating_sub(reach)..(centre_y + reach).min(height) {
        for x in centre_x.saturating_sub(reach)..(centre_x + reach).min(width) {
            let pixel = rgba.get_pixel_mut(x, y);
            pixel.0 = [128, 128, 128, pixel.0[3]];
        }
    }

    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("re-encodes");
    out.into_inner()
}

#[test]
fn the_patternize_map_is_run_against_the_groove_it_claims_to_describe() {
    let png = press(&payload());

    // A light permutation. `amount` is a percentage of blocks and the
    // reverse map grows with it: at the default 20% the map for a record
    // this small is four times what its carriers hold. 2% fits. (1.0 does
    // not mean one percent — `normalize_amount` reads a value that is not
    // greater than 1 as a fraction, so 1.0 is the whole groove.)
    //
    // Patternize writes the sidecar itself, so nothing is added first: the
    // map has to be the sidecar the record ends up carrying.
    let patternized = bitneedle_record_author::patternize_record_png_native(
        &png,
        "{\"amount\":2.0}",
        Some(PROFILE),
    )
    .expect("the groove permutes")
    .png_bytes;

    let report = test_spin::sidecars::inspect(&patternized, Some(PROFILE));
    assert!(report.present(), "the sidecar survived patternizing");
    assert!(report.ok(), "and every check passes: {:#?}", report.checks);

    assert!(
        report
            .checks
            .iter()
            .any(|check| check.label == "Patternize groove restores" && check.passed),
        "the map was actually run, not merely parsed"
    );
}

#[test]
fn the_manifest_reads_a_pressed_record_flat() {
    let png = press(&payload());
    let png = with_sidecar(&png, serde_json::json!([text_item("note", "hello")]));

    let json = test_spin::manifest_report_json(&png).expect("the manifest reads");
    let report: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    // Not `ok`: a record pressed from a bare ECDC with no codec metadata
    // fails the payload's own shape checks, and that is this fixture's
    // doing rather than the manifest's. What is asserted here is that the
    // manifest lays the record out — the checks themselves are asserted
    // against the sidecar above, where this test's fixture is sound.
    let titles = report["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .map(|section| section["title"].as_str().expect("title").to_owned())
        .collect::<Vec<_>>();
    for wanted in ["RECORD", "IDENTITY", "AUDIO", "SIDECAR", "SIDECAR ITEMS", "SIDECAR CHECKS", "CHECKS"] {
        assert!(titles.contains(&wanted.to_owned()), "{wanted} is a section");
    }

    // Every sidecar check is a row with a verdict on it, and on this record
    // every one of them passes.
    let sidecar_checks = report["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|section| section["title"] == "SIDECAR CHECKS")
        .expect("the sidecar was checked");
    let rows = sidecar_checks["rows"].as_array().expect("rows");
    assert!(!rows.is_empty());
    for row in rows {
        assert_eq!(row["status"], true, "{row}");
    }

    // And a failed check is reported rather than swallowed: the payload's
    // own checks carry verdicts too.
    let checks = report["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|section| section["title"] == "CHECKS")
        .expect("the payload was checked");
    assert!(checks["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .all(|row| row["status"].is_boolean()));
}

#[test]
fn long_values_are_cut_for_the_screen_and_kept_whole_for_the_share() {
    let png = press(&payload());
    let long = "codec/".repeat(40);
    let png = with_sidecar(&png, serde_json::json!([text_item("note", &long)]));

    let options = test_spin::InspectionOptions::verbose_defaults();
    let report = test_spin::manifest_report(&png, &options).expect("the manifest reads");

    let mut found = false;
    for section in &report.sections {
        for row in &section.rows {
            assert!(
                row.value.chars().count() <= test_spin::manifest::MAX_VALUE_WIDTH,
                "{:?} was shown untruncated",
                row.label
            );
            if row.full.is_some() {
                found = true;
                assert!(
                    row.full.as_deref().expect("full").chars().count()
                        > test_spin::manifest::MAX_VALUE_WIDTH
                );
            }
        }
    }
    assert!(found, "this record has at least one value worth cutting");

    // And the share carries the whole of it.
    let text = report.to_text();
    for section in &report.sections {
        for row in &section.rows {
            assert!(text.contains(row.whole()), "{:?} is shared whole", row.label);
        }
    }
}

/// Writes a real pressed record out, for driving the apps by hand.
///
/// Ignored by default: it is not a test, it is the fixture the phone and the
/// browser need to have a record to open. `cargo test -p test-spin --
/// --ignored write_a_fixture_record --nocapture` prints where it landed.
#[test]
#[ignore]
fn write_a_fixture_record() {
    let png = press(&payload());
    let png = with_sidecar(
        &png,
        serde_json::json!([
            text_item("liner-notes", "cut at 45, one take"),
            serde_json::json!({
                "type": "json",
                "codec": "raw",
                "name": "session",
                "json": { "room": "wavey", "takes": 3 },
            }),
        ]),
    );
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixture-record.png");
    std::fs::write(&out, &png).expect("writes");
    println!("wrote {} ({} bytes)", out.display(), png.len());
}

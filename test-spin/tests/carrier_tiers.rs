//! A toned record is pressed, filled tier by tier, and read back.
//!
//! The sidecar fills the groove carriers before the steg carriers, and the
//! label last. These tests press a real record so the bands have the geometry
//! a press gives them, then write across every tier and read the bytes back.

use base64::Engine as _;
use record_sidecar::{
    build_sidecar_plan, paint_sidecar_bytes_into_plan, read_sidecar_bytes_from_plan,
    sidecar_plan_capacities, SidecarCarrier, SidecarRun, SIDECAR_DEFAULT_SEED, SIDECAR_FILL_ORDER,
};

const PROFILE: &str = "single45";
const SCHEME: &str = "pairsign-safe-luma-v2";

/// The pressed record the full-capacity roundtrip reads.
///
/// A toned press whose programme stops short of the label, so all five
/// carriers exist: the lead-in, the dead wax, the run-out, the intergroove,
/// and the label. Kept in goldenfiles because the press is the input, and a
/// fixture regenerated in the test would hide a change in the press.
const CARRIER_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../goldenfiles/records/bitneedle-carrier-tiers/bitneedle-carrier-tiers.record.png"
);

/// A programme short enough that the cut stops well above the label.
///
/// A side that runs to the label leaves no silent groove and the silent groove is one of
/// the carriers under test. The bytes are opaque to the press: an unregistered
/// container name selects the extension container, which carries them as they
/// are.
fn payload() -> Vec<u8> {
    (0..24_000usize)
        .map(|i| (i.wrapping_mul(97) ^ 0x5c) as u8)
        .collect()
}

/// A pressed record with a wheel, so its silent groove and trailer carry a palette.
fn press_toned(payload: &[u8]) -> Vec<u8> {
    let options = serde_json::json!({
        "grooveToneSlots": [
            "#6d4b3a", "#7a5a44", "#8a6b52", "#5c3f31",
            "#6f5140", "#7d6149", "#8f7358", "#63483a"
        ],
    })
    .to_string();

    bitneedle_record_author::render_payload_container_to_png_native(
        payload,
        "bitneedle-carrier-test",
        "test",
        "rgb",
        PROFILE,
        30.0,
        &options,
    )
    .expect("the synthetic payload presses toned")
    .png_bytes
}

struct Pressed {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    descriptor: record_descriptor::RecordDescriptor,
    descriptor_bytes: usize,
}

fn pressed() -> Pressed {
    let png = press_toned(&payload());
    let (_, bytes) = record_decode::decode_record_descriptor_bytes_from_png(&png, None)
        .expect("the pressed record carries a descriptor");
    let descriptor =
        record_descriptor::decode_record_descriptor_bytes(&bytes).expect("the descriptor decodes");
    let (width, height, rgba) = record_decode::load_record_rgba(&png).expect("the PNG loads");
    Pressed {
        width,
        height,
        rgba,
        descriptor,
        descriptor_bytes: bytes.len(),
    }
}

fn plan_for(pressed: &Pressed) -> Vec<SidecarRun> {
    build_sidecar_plan(
        pressed.width,
        pressed.height,
        &pressed.descriptor,
        pressed.descriptor_bytes,
        &SIDECAR_FILL_ORDER,
        SIDECAR_DEFAULT_SEED,
        None,
    )
    .expect("a toned record offers every carrier")
}

/// Deterministic bytes, so a failure names the offset that moved.
fn filler(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i.wrapping_mul(31) ^ 0xa5) as u8).collect()
}

#[test]
fn a_toned_record_offers_every_carrier_in_fill_order() {
    let pressed = pressed();
    let plan = plan_for(&pressed);
    let order: Vec<SidecarCarrier> = plan.iter().map(|run| run.carrier()).collect();

    assert_eq!(
        order,
        vec![
            SidecarCarrier::LeadIn,
            SidecarCarrier::SilentGroove,
            SidecarCarrier::Trailer,
            SidecarCarrier::Intergroove,
            SidecarCarrier::Label,
        ],
        "a cut that stops short of the label offers all five carriers, in fill order; \
         this record stopped at {}",
        pressed.descriptor.cut_inner_radius
    );

    let capacities = sidecar_plan_capacities(&plan, SCHEME, &pressed.rgba)
        .expect("every run reports a capacity");
    println!(
        "cut stops at {} px, descriptor {} bytes",
        pressed.descriptor.cut_inner_radius, pressed.descriptor_bytes
    );
    for (carrier, capacity) in &capacities {
        println!(
            "  {:>12} {:>8} bytes",
            record_sidecar::sidecar_carrier_name(*carrier),
            capacity
        );
        assert!(
            *capacity > 0,
            "{} holds no bytes",
            record_sidecar::sidecar_carrier_name(*carrier)
        );
    }
    println!(
        "  {:>12} {:>8} bytes",
        "TOTAL",
        capacities.iter().map(|(_, c)| c).sum::<usize>()
    );
}

#[test]
fn bytes_written_across_every_tier_read_back() {
    let mut pressed = pressed();
    let plan = plan_for(&pressed);
    let capacities =
        sidecar_plan_capacities(&plan, SCHEME, &pressed.rgba).expect("capacities resolve");
    let total: usize = capacities.iter().map(|(_, capacity)| capacity).sum();

    // Past every groove carrier and into the steg tiers, so one stream crosses
    // both kinds of carrier and the joins between them are exercised.
    let groove: usize = capacities
        .iter()
        .filter(|(carrier, _)| carrier.is_groove())
        .map(|(_, capacity)| capacity)
        .sum();
    let length = (groove + 4096).min(total);
    let bytes = filler(length);

    paint_sidecar_bytes_into_plan(
        &mut pressed.rgba,
        pressed.width,
        pressed.height,
        &plan,
        SCHEME,
        &bytes,
    )
    .expect("the stream fits the plan");

    let back = read_sidecar_bytes_from_plan(
        &pressed.rgba,
        pressed.width,
        pressed.height,
        &plan,
        SCHEME,
        bytes.len(),
    )
    .expect("the stream reads back");

    assert_eq!(back.len(), bytes.len());
    assert_eq!(back, bytes, "the stream did not survive the carriers");
}

#[test]
fn filling_the_carriers_leaves_the_descriptor_readable() {
    let mut pressed = pressed();
    let plan = plan_for(&pressed);
    let capacities =
        sidecar_plan_capacities(&plan, SCHEME, &pressed.rgba).expect("capacities resolve");
    let groove: usize = capacities
        .iter()
        .filter(|(carrier, _)| carrier.is_groove())
        .map(|(_, capacity)| capacity)
        .sum();

    // Fill every groove carrier to the brim. The descriptor sits at the front
    // of the lead-in and the trailer, and the plan starts each run past it, so
    // a full carrier must leave the record's own bytes where they were.
    let bytes = filler(groove);
    paint_sidecar_bytes_into_plan(
        &mut pressed.rgba,
        pressed.width,
        pressed.height,
        &plan,
        SCHEME,
        &bytes,
    )
    .expect("the groove carriers hold their own capacity");

    let reread = record_descriptor::metadata_bytes_from_grayscale_rgba(
        &pressed.rgba,
        &record_core::build_lead_in_spiral_indices(
            pressed.width,
            pressed.height,
            &pressed.descriptor.record_profile,
            None,
            None,
            None,
        )
        .expect("the lead-in traces"),
        pressed.descriptor_bytes,
        "record descriptor",
    )
    .expect("the descriptor still reads off the lead-in");

    let descriptor =
        record_descriptor::decode_record_descriptor_bytes(&reread).expect("and still decodes");
    assert_eq!(descriptor.record_profile, pressed.descriptor.record_profile);
    assert_eq!(descriptor.cut_inner_radius, pressed.descriptor.cut_inner_radius);
}

/// The whole sidecar path, at the record's full capacity, on a kept record.
///
/// `rewrite_record_png` is the same path a press takes: a framed item is
/// written across the carriers and the descriptor is repainted around it.
/// The payload is deterministic mock bytes — not a picture — so the only
/// thing under test is whether every carrier returns what it was given.
#[test]
fn full_capacity_sidecar_survives_every_carrier() {
    let png = std::fs::read(CARRIER_FIXTURE).expect("the carrier fixture is checked in");
    let profile = record_decode::decode_record_png(&png)
        .expect("the fixture is a record")
        .record_profile;

    // Grooves first — lead-in, dead wax, run-out — then the intergroove and
    // the label, which is the order a press fills and a reader unwinds.
    let carriers = ["leadIn", "silent_groove", "trailer", "intergroove", "label"];
    let options_for = |data: &[u8]| {
        serde_json::json!({
            "sidecar": {
                "scheme": SCHEME,
                "carriers": carriers,
                "items": [{
                    "type": "opaque",
                    "codec": "raw",
                    "name": "roundtrip.bin",
                    "mime": "application/octet-stream",
                    "dataBase64": base64::engine::general_purpose::STANDARD.encode(data),
                }],
            },
        })
        .to_string()
    };

    // The largest mock payload the record holds, by bisection on the writer.
    let mut low = 0usize;
    let mut high = 200_000usize;
    while low < high {
        let mid = (low + high + 1) / 2;
        let fits =
            record_sidecar::rewrite_record_png(&png, &options_for(&filler(mid)), Some(&profile))
                .is_ok();
        if fits {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let data = filler(low);
    let (record, summary) =
        record_sidecar::rewrite_record_png(&png, &options_for(&data), Some(&profile))
            .expect("the payload fits the capacity it was sized to");
    for carrier in carriers {
        assert!(
            summary.carriers.iter().any(|used| used == carrier),
            "{carrier} was not filled: {:?}",
            summary.carriers
        );
    }
    assert!(summary.bsc1_bytes <= summary.capacity_bytes);

    // Every byte back, in order.
    let items_json = record_sidecar::decode_record_png_sidecar_items_json(&record, Some(&profile))
        .expect("the record that now bears the sidecar reads back");
    let parsed: serde_json::Value = serde_json::from_str(&items_json).expect("items are JSON");
    let back = base64::engine::general_purpose::STANDARD
        .decode(parsed["items"][0]["dataBase64"].as_str().expect("payload"))
        .expect("payload is base64");
    assert_eq!(back.len(), data.len(), "the stream length moved");
    assert_eq!(back, data, "the sidecar bytes did not survive the carriers");

    // And it is still a record: a full sidecar must not have written over the
    // descriptor cut into the lead-in.
    let decoded = record_decode::decode_record_png(&record).expect("the filled record still decodes");
    assert_eq!(decoded.record_profile, profile);
}

/// Writes the fixture the roundtrip test reads.
///
/// Ignored: it presses, so it is run by hand when the press changes, not on
/// every test run. `cargo test -p test-spin --test carrier_tiers --
/// --ignored write_the_carrier_fixture --nocapture` says where it landed.
#[test]
#[ignore]
fn write_the_carrier_fixture() {
    let png = press_toned(&payload());
    let path = std::path::Path::new(CARRIER_FIXTURE);
    std::fs::create_dir_all(path.parent().expect("the fixture has a directory")).unwrap();
    std::fs::write(path, &png).unwrap();
    println!("wrote {} ({} bytes)", path.display(), png.len());
}

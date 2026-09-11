//! Everything a record carries beside its groove, checked.
//!
//! The payload — BRD1, BRS1, the spirals — was the only thing this tool ever
//! read. A pressed record also carries a BSC1 sidecar hidden in the label,
//! the intergroove and the lead-in, and that sidecar is where the package's
//! display header and metadata live, where a cover sits, and where a presser
//! may put anything at all. None of it was inspected, so a record could pass
//! `record-test` with a sidecar that was truncated, mistyped or unattested —
//! which is to say, a record that does not open.
//!
//! The inspection checks every item, including the items with unknown names:
//!
//! * the container's framing, version, flags and declared length;
//! * the BRD1 pointer that declares it, and whether the stream's digest is
//!   the one the pointer promised;
//! * the type and codec pairing of each item against the registry, its
//!   declared raw length against its decompressed length, and its payload
//!   against its declared type: text is UTF-8, JSON parses, and an image is
//!   AVIF;
//! * the package display header's magic, version, length and both CRCs;
//! * the package metadata and cover;
//! * the attestation, and whether it covers the items that the sidecar holds;
//! * arbitrary items, which get the same checks as the named items. A typed
//!   container gives an unknown name the same checks as a known one.

use anyhow::{Context, Result};
use record_sidecar::{
    SidecarDecodedItem, SidecarInspection, DISPLAY_HEADER_LENGTH, DISPLAY_HEADER_MAGIC,
    DISPLAY_HEADER_NAME, DISPLAY_HEADER_VERSION, PACKAGE_COVER_ITEM_NAME,
    PACKAGE_METADATA_ITEM_NAME, SIDECAR_ATTESTATION_ITEM_NAME, SIDECAR_CONTAINER_VERSION,
    SIDECAR_MAGIC,
};

use crate::manifest::{ManifestRow, ManifestSection};

/// One thing that had to be true about a sidecar.
#[derive(Debug, Clone)]
pub struct SidecarCheck {
    pub passed: bool,
    pub label: String,
    pub detail: String,
}

impl SidecarCheck {
    fn pass(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            passed: true,
            label: label.into(),
            detail: detail.into(),
        }
    }

    fn fail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            passed: false,
            label: label.into(),
            detail: detail.into(),
        }
    }

    fn from(label: impl Into<String>, outcome: Result<String>) -> Self {
        match outcome {
            Ok(detail) => Self::pass(label, detail),
            Err(error) => Self::fail(label, format!("{error:#}")),
        }
    }
}

/// The sidecar as this tool reports it: what it holds, and what held true.
#[derive(Debug, Clone)]
pub struct SidecarReport {
    /// `None` when the record carries no sidecar — a valid record, and not
    /// the same thing as a sidecar that failed to read.
    pub inspection: Option<SidecarInspection>,
    /// Why the sidecar could not be read, when it could not be. A record
    /// with no sidecar leaves this empty.
    pub error: Option<String>,
    pub checks: Vec<SidecarCheck>,
}

impl SidecarReport {
    pub fn ok(&self) -> bool {
        self.error.is_none() && self.checks.iter().all(|check| check.passed)
    }

    pub fn present(&self) -> bool {
        self.inspection.is_some()
    }
}

/// Read a record's sidecar and check every part of it.
pub fn inspect(png: &[u8], record_profile: Option<&str>) -> SidecarReport {
    match record_sidecar::inspect_record_png_sidecar(png, record_profile) {
        Ok(None) => SidecarReport {
            inspection: None,
            error: None,
            checks: Vec::new(),
        },
        Ok(Some(inspection)) => {
            let checks = collect_checks(&inspection);
            SidecarReport {
                inspection: Some(inspection),
                error: None,
                checks,
            }
        }
        Err(error) => SidecarReport {
            inspection: None,
            error: Some(format!("{error:#}")),
            checks: vec![SidecarCheck::fail(
                "Sidecar decode",
                format!("{error:#}"),
            )],
        },
    }
}

fn collect_checks(inspection: &SidecarInspection) -> Vec<SidecarCheck> {
    let mut checks = Vec::new();
    let bytes = &inspection.bytes;

    // The container itself. Everything below this line is only meaningful
    // if the frame it was read out of is the frame that was written.
    checks.push(if bytes.len() >= 4 && &bytes[..4] == SIDECAR_MAGIC {
        SidecarCheck::pass("BSC1 container magic", "BSC1")
    } else {
        SidecarCheck::fail(
            "BSC1 container magic",
            format!("expected BSC1, found {}", ascii_magic(bytes)),
        )
    });

    let validation = &inspection.decoded.validation;
    checks.push(if validation.version == SIDECAR_CONTAINER_VERSION {
        SidecarCheck::pass(
            "BSC1 container version",
            format!("version {}", validation.version),
        )
    } else {
        SidecarCheck::fail(
            "BSC1 container version",
            format!(
                "version {} is not the supported {SIDECAR_CONTAINER_VERSION}",
                validation.version
            ),
        )
    });

    checks.push(if validation.total_length == bytes.len() {
        SidecarCheck::pass(
            "BSC1 declared length",
            format!("{} bytes, exactly the stream read back", bytes.len()),
        )
    } else {
        SidecarCheck::fail(
            "BSC1 declared length",
            format!(
                "container declares {} bytes, {} were read",
                validation.total_length,
                bytes.len()
            ),
        )
    });

    checks.push(
        if validation.item_count == inspection.decoded.items.len() {
            SidecarCheck::pass(
                "BSC1 item count",
                format!("{} items, all framed", validation.item_count),
            )
        } else {
            SidecarCheck::fail(
                "BSC1 item count",
                format!(
                    "header declares {} items, {} decoded",
                    validation.item_count,
                    inspection.decoded.items.len()
                ),
            )
        },
    );

    // The stream against the carriers it was hidden in. A sidecar larger
    // than its carriers is one that was written over itself.
    let decode = &inspection.decode;
    checks.push(if decode.bsc1_byte_length <= decode.capacity_bytes {
        SidecarCheck::pass(
            "Sidecar fits its carriers",
            format!(
                "{} of {} bytes across {} carrier pairs",
                decode.bsc1_byte_length, decode.capacity_bytes, decode.carrier_pairs
            ),
        )
    } else {
        SidecarCheck::fail(
            "Sidecar fits its carriers",
            format!(
                "{} bytes will not fit {} bytes of carrier",
                decode.bsc1_byte_length, decode.capacity_bytes
            ),
        )
    });

    // The pointer in BRD1. A record may carry a sidecar without declaring
    // one, so absence is reported rather than failed; a pointer that lies
    // about the digest is a failure.
    match (&inspection.pointer, inspection.pointer_digest_matches) {
        (Some(pointer), Some(true)) => checks.push(SidecarCheck::pass(
            "BRD1 sidecar pointer digest",
            format!(
                "SHA-256 matches, {} bytes over {}",
                pointer.length,
                carrier_names(pointer)
            ),
        )),
        (Some(_), Some(false)) => checks.push(SidecarCheck::fail(
            "BRD1 sidecar pointer digest",
            "the pointer's SHA-256 is not the digest of the stream that was read",
        )),
        _ => checks.push(SidecarCheck::pass(
            "BRD1 sidecar pointer",
            "absent; the sidecar was found by its own magic",
        )),
    }

    // Every item, named or not. `decode_sidecar_container_items` has
    // already run each payload through `validate_sidecar_item_payload`, so
    // reaching here means the type, codec, lengths and payload agreed; what
    // is reported per item is that agreement, item by item, so a reader can
    // see which items were checked rather than trusting a single tick.
    for item in &inspection.decoded.items {
        checks.push(SidecarCheck::pass(
            format!("Item {:?}", item.name),
            format!(
                "{} / {}, {} stored → {} decoded bytes{}",
                item.item_type_name,
                item.codec_name,
                item.stored_byte_length,
                item.decoded_byte_length,
                item.raw_byte_length
                    .map(|raw| format!(", declared raw {raw}"))
                    .unwrap_or_default()
            ),
        ));
    }

    // The package's display header: fixed 128 bytes, two CRCs, one over the
    // payload and one over the header with its own CRC field zeroed.
    match inspection.item(DISPLAY_HEADER_NAME) {
        Some(item) => checks.push(SidecarCheck::from(
            "Package display header",
            display_header_detail(item),
        )),
        None => checks.push(SidecarCheck::pass(
            "Package display header",
            "absent; this record was not packaged",
        )),
    }

    match inspection.item(PACKAGE_METADATA_ITEM_NAME) {
        Some(item) => checks.push(SidecarCheck::from(
            "Package metadata",
            package_metadata_detail(item),
        )),
        None => checks.push(SidecarCheck::pass("Package metadata", "absent")),
    }

    match inspection.item(PACKAGE_COVER_ITEM_NAME) {
        Some(item) => checks.push(SidecarCheck::from("Album cover", cover_detail(item))),
        None => checks.push(SidecarCheck::pass("Album cover", "absent")),
    }

    // The attestation, checked structurally. This check reports that the
    // signed digest covers the items that the sidecar holds, and that it binds
    // to the descriptor of this record. The holder of the key list decides the
    // trust of the key.
    match (&inspection.attestation, inspection.attestation_covers) {
        (Some(attestation), Some(true)) => checks.push(SidecarCheck::pass(
            "Sidecar attestation",
            format!(
                "covers all {} items, key {}",
                inspection.decoded.items.len().saturating_sub(1),
                attestation.key_id
            ),
        )),
        (Some(_), Some(false)) => checks.push(SidecarCheck::fail(
            "Sidecar attestation",
            "the signed commitment is not over the items this sidecar now holds",
        )),
        _ => checks.push(SidecarCheck::pass(
            "Sidecar attestation",
            "absent; the sidecar is unattested",
        )),
    }

    checks
}

fn carrier_names(pointer: &record_sidecar::SidecarHeaderPointer) -> String {
    pointer
        .carriers
        .iter()
        .map(|carrier| carrier.name())
        .collect::<Vec<_>>()
        .join(" + ")
}

fn display_header_detail(item: &SidecarDecodedItem) -> Result<String> {
    let bytes = record_sidecar::decode_base64_text(&item.data_base64, "display header")?;
    if bytes.len() != DISPLAY_HEADER_LENGTH {
        anyhow::bail!(
            "display header is {} bytes, not the fixed {DISPLAY_HEADER_LENGTH}",
            bytes.len()
        );
    }
    if &bytes[..4] != DISPLAY_HEADER_MAGIC {
        anyhow::bail!("display header magic is {}", ascii_magic(&bytes));
    }
    if bytes[4] != DISPLAY_HEADER_VERSION {
        anyhow::bail!(
            "display header version {} is not the supported {DISPLAY_HEADER_VERSION}",
            bytes[4]
        );
    }
    if bytes[5] as usize != DISPLAY_HEADER_LENGTH {
        anyhow::bail!(
            "display header declares length {}, not {DISPLAY_HEADER_LENGTH}",
            bytes[5]
        );
    }

    let payload_crc = u32::from_be_bytes(bytes[8..12].try_into().expect("slice length"));
    let actual_payload_crc = record_core::crc32_ieee(&bytes[16..]);
    if payload_crc != actual_payload_crc {
        anyhow::bail!(
            "display header payload CRC is {payload_crc:#010x}, computed {actual_payload_crc:#010x}"
        );
    }

    let header_crc = u32::from_be_bytes(bytes[12..16].try_into().expect("slice length"));
    let mut for_crc = bytes.clone();
    for_crc[12..16].fill(0);
    let actual_header_crc = record_core::crc32_ieee(&for_crc);
    if header_crc != actual_header_crc {
        anyhow::bail!(
            "display header CRC is {header_crc:#010x}, computed {actual_header_crc:#010x}"
        );
    }

    let design = ascii_field(&bytes[16..56]);
    Ok(format!(
        "BDH1 v{}, both CRCs match{}",
        bytes[4],
        if design.is_empty() {
            String::new()
        } else {
            format!(", design {design:?}")
        }
    ))
}

fn package_metadata_detail(item: &SidecarDecodedItem) -> Result<String> {
    let value = item
        .json
        .as_ref()
        .context("package metadata is not carried as JSON")?;
    let object = value
        .as_object()
        .context("package metadata is not a JSON object")?;
    Ok(format!(
        "{} bytes of JSON, {} top-level fields",
        item.decoded_byte_length,
        object.len()
    ))
}

fn cover_detail(item: &SidecarDecodedItem) -> Result<String> {
    let bytes = record_sidecar::decode_base64_text(&item.data_base64, "album cover")?;
    if !record_sidecar::looks_like_avif(&bytes) {
        anyhow::bail!("the album cover is not an AVIF file");
    }
    Ok(format!("AVIF, {} bytes", bytes.len()))
}

fn ascii_field(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\0', ' '])
        .to_owned()
}

fn ascii_magic(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(4)
        .map(|byte| {
            if byte.is_ascii_graphic() {
                *byte as char
            } else {
                '.'
            }
        })
        .collect()
}

// MARK: The sidecar as sections of the flat manifest

/// The sidecar's part of the manifest: what it declares, what it holds, and
/// every check that ran over it.
pub fn sections(report: &SidecarReport) -> Vec<ManifestSection> {
    let mut sections = Vec::new();

    let Some(inspection) = report.inspection.as_ref() else {
        let mut section = ManifestSection::new("SIDECAR");
        match report.error.as_deref() {
            Some(error) => {
                section.push(ManifestRow::check("Sidecar", error, false));
            }
            None => {
                section.push(ManifestRow::new("Sidecar", "none carried"));
            }
        }
        sections.push(section);
        return sections;
    };

    let decode = &inspection.decode;
    let validation = &inspection.decoded.validation;

    let mut carried = ManifestSection::new("SIDECAR");
    match inspection.pointer.as_ref() {
        Some(pointer) => {
            carried.push(ManifestRow::new("Declared by", "BRD1 pointer"));
            carried.push(ManifestRow::new("Scheme", &pointer.scheme));
            carried.push(ManifestRow::new("Carriers", carrier_names(pointer)));
            carried.push(ManifestRow::new("Seed", format!("{:#010x}", pointer.seed)));
            carried.push(ManifestRow::new("Digest", &pointer.sha256));
        }
        None => carried.push(ManifestRow::new("Declared by", "found by magic; no pointer")),
    }
    carried.push(ManifestRow::new(
        "Container",
        format!("BSC1 v{}", validation.version),
    ));
    carried.push(ManifestRow::new(
        "Stream",
        format!(
            "{} bytes of {} carried",
            decode.bsc1_byte_length, decode.capacity_bytes
        ),
    ));
    carried.push(ManifestRow::new(
        "Carrier pairs",
        format!(
            "{} pairs over {} pixels",
            decode.carrier_pairs, decode.carrier_pixels
        ),
    ));
    // Where the stream sits. The carriers fill in order, so a row that holds
    // its whole capacity is a carrier the stream filled and passed through,
    // and the first row short of its capacity is where the stream ended.
    for usage in &decode.carriers {
        carried.push(ManifestRow::new(
            format!("  {}", usage.carrier),
            format!(
                "{} of {} bytes over {} {}",
                usage.used_bytes, usage.capacity_bytes, usage.units, usage.kind
            ),
        ));
    }
    carried.push(ManifestRow::new("Items", validation.item_count.to_string()));
    carried.push(ManifestRow::new(
        "Attested",
        match (&inspection.attestation, inspection.attestation_covers) {
            (Some(attestation), Some(true)) => format!("yes, key {}", attestation.key_id),
            (Some(_), _) => "yes, but not over these items".to_owned(),
            _ => "no".to_owned(),
        },
    ));
    sections.push(carried);

    // Every item, one row each: what it is, and how big it is. The name is
    // the row's label, so an arbitrary item a presser added reads exactly
    // like one of ours.
    let mut items = ManifestSection::new("SIDECAR ITEMS");
    if inspection.decoded.items.is_empty() {
        items.push(ManifestRow::new("Items", "none"));
    }
    for item in &inspection.decoded.items {
        items.push(ManifestRow::new(
            item_label(item),
            format!(
                "{} · {} · {} bytes",
                item.item_type_name,
                item.codec_name,
                item.decoded_byte_length
            ),
        ));
    }
    sections.push(items);

    let mut checks = ManifestSection::new("SIDECAR CHECKS");
    for check in &report.checks {
        checks.push(ManifestRow::check(&check.label, &check.detail, check.passed));
    }
    sections.push(checks);

    sections
}

/// The name an item is shown under. The reserved names are spelled out —
/// `attestation` says nothing to a reader — and everything else keeps the name
/// it was stored with.
fn item_label(item: &SidecarDecodedItem) -> String {
    match item.name.as_str() {
        SIDECAR_ATTESTATION_ITEM_NAME => "Attestation".to_owned(),
        DISPLAY_HEADER_NAME => "Display header".to_owned(),
        PACKAGE_METADATA_ITEM_NAME => "Package metadata".to_owned(),
        PACKAGE_COVER_ITEM_NAME => "Album cover".to_owned(),
        other => other.to_owned(),
    }
}

//! Optional BPK1 transport container for exact Bitneedle record components.
//!
//! BPK1 preserves one BRD1 descriptor, one BRS1 record stream, and an optional
//! BSC1 sidecar.

use anyhow::{bail, Context, Result};
use record_descriptor::{decode_record_descriptor_bytes, RecordDescriptor};
use record_sidecar::{
    decode_sidecar_header_pointer, validate_sidecar_container, SidecarContainerValidation,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const PACKAGE_MAGIC: &[u8; 4] = b"BPK1";
pub const PACKAGE_VERSION: u8 = 1;
pub const PACKAGE_FILE_EXTENSION: &str = ".bpk";
pub const PACKAGE_MEDIA_TYPE: &str = "application/vnd.bitneedle.package";
pub const PACKAGE_PREFIX_LENGTH: usize = 24;
pub const PACKAGE_DIRECTORY_ENTRY_LENGTH: usize = 52;
pub const PACKAGE_OVERHEAD_WITHOUT_BSC1: usize =
    PACKAGE_PREFIX_LENGTH + 2 * PACKAGE_DIRECTORY_ENTRY_LENGTH;
pub const PACKAGE_OVERHEAD_WITH_BSC1: usize =
    PACKAGE_PREFIX_LENGTH + 3 * PACKAGE_DIRECTORY_ENTRY_LENGTH;
pub const PACKAGE_FLAG_HAS_BSC1: u8 = 0x01;
pub const PACKAGE_KNOWN_FLAGS: u8 = PACKAGE_FLAG_HAS_BSC1;

pub const PACKAGE_SECTION_BRD1: u8 = 1;
pub const PACKAGE_SECTION_BRS1: u8 = 2;
pub const PACKAGE_SECTION_BSC1: u8 = 3;

const PACKAGE_HEADER_CRC32_OFFSET: usize = 20;
const PACKAGE_HEADER_CRC32_END: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSectionKind {
    Brd1,
    Brs1,
    Bsc1,
}

impl PackageSectionKind {
    pub fn wire_code(self) -> u8 {
        match self {
            Self::Brd1 => PACKAGE_SECTION_BRD1,
            Self::Brs1 => PACKAGE_SECTION_BRS1,
            Self::Bsc1 => PACKAGE_SECTION_BSC1,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Brd1 => "BRD1",
            Self::Brs1 => "BRS1",
            Self::Bsc1 => "BSC1",
        }
    }

    pub fn from_name(name: &str) -> Result<Self> {
        match name.trim().to_ascii_uppercase().as_str() {
            "BRD1" => Ok(Self::Brd1),
            "BRS1" => Ok(Self::Brs1),
            "BSC1" => Ok(Self::Bsc1),
            other => bail!("unsupported BPK1 section name {other}"),
        }
    }

    fn from_wire_code(code: u8) -> Result<Self> {
        match code {
            PACKAGE_SECTION_BRD1 => Ok(Self::Brd1),
            PACKAGE_SECTION_BRS1 => Ok(Self::Brs1),
            PACKAGE_SECTION_BSC1 => Ok(Self::Bsc1),
            _ => bail!("unsupported BPK1 section type {code}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSectionDescriptor {
    pub kind: PackageSectionKind,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub sha256: [u8; 32],
}

#[derive(Debug)]
pub struct ParsedPackage<'a> {
    pub version: u8,
    pub flags: u8,
    pub header_byte_length: usize,
    pub sections: Vec<PackageSectionDescriptor>,
    pub record_descriptor: RecordDescriptor,
    pub sidecar_validation: Option<SidecarContainerValidation>,
    brd1: &'a [u8],
    brs1: &'a [u8],
    bsc1: Option<&'a [u8]>,
}

impl<'a> ParsedPackage<'a> {
    pub fn brd1(&self) -> &'a [u8] {
        self.brd1
    }

    pub fn brs1(&self) -> &'a [u8] {
        self.brs1
    }

    pub fn bsc1(&self) -> Option<&'a [u8]> {
        self.bsc1
    }

    pub fn section(&self, kind: PackageSectionKind) -> Option<&'a [u8]> {
        match kind {
            PackageSectionKind::Brd1 => Some(self.brd1),
            PackageSectionKind::Brs1 => Some(self.brs1),
            PackageSectionKind::Bsc1 => self.bsc1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageSectionInspection {
    pub section_type: u8,
    pub name: &'static str,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageDescriptorInspection {
    pub record_profile: String,
    pub stream_byte_length: usize,
    pub payload_encoding: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub catalog_number: Option<String>,
    pub label: Option<String>,
    pub artwork_credit: Option<String>,
    pub canonical_url: Option<String>,
    pub copyright_year: Option<u16>,
    pub copyright_holder: Option<String>,
    pub has_sidecar_pointer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInspection {
    pub format: &'static str,
    pub version: u8,
    pub flags: u8,
    pub header_byte_length: usize,
    pub total_byte_length: usize,
    pub container_overhead_bytes: usize,
    pub section_count: usize,
    pub sections: Vec<PackageSectionInspection>,
    pub descriptor: PackageDescriptorInspection,
    pub sidecar_item_count: Option<usize>,
}

/// Build a canonical BPK1 package from exact component bytes.
pub fn encode_package(brd1: &[u8], brs1: &[u8], bsc1: Option<&[u8]>) -> Result<Vec<u8>> {
    validate_components(brd1, brs1, bsc1)?;

    let components = [
        Some((PackageSectionKind::Brd1, brd1)),
        Some((PackageSectionKind::Brs1, brs1)),
        bsc1.map(|bytes| (PackageSectionKind::Bsc1, bytes)),
    ];
    let sections = components.into_iter().flatten().collect::<Vec<_>>();
    let section_count = u16::try_from(sections.len()).context("BPK1 section count exceeds u16")?;
    let header_byte_length = PACKAGE_PREFIX_LENGTH
        .checked_add(
            sections
                .len()
                .checked_mul(PACKAGE_DIRECTORY_ENTRY_LENGTH)
                .context("BPK1 directory length overflow")?,
        )
        .context("BPK1 header length overflow")?;
    let component_byte_length = sections.iter().try_fold(0usize, |total, (_, bytes)| {
        total
            .checked_add(bytes.len())
            .context("BPK1 component length overflow")
    })?;
    let total_byte_length = header_byte_length
        .checked_add(component_byte_length)
        .context("BPK1 total length overflow")?;

    let mut output = Vec::with_capacity(total_byte_length);
    output.extend_from_slice(PACKAGE_MAGIC);
    output.push(PACKAGE_VERSION);
    output.push(if bsc1.is_some() {
        PACKAGE_FLAG_HAS_BSC1
    } else {
        0
    });
    output.extend_from_slice(&section_count.to_be_bytes());
    output.extend_from_slice(
        &u32::try_from(header_byte_length)
            .context("BPK1 header length exceeds u32")?
            .to_be_bytes(),
    );
    output.extend_from_slice(
        &u64::try_from(total_byte_length)
            .context("BPK1 total length exceeds u64")?
            .to_be_bytes(),
    );
    output.extend_from_slice(&0u32.to_be_bytes());

    let mut byte_offset = header_byte_length;
    for (kind, bytes) in &sections {
        output.push(kind.wire_code());
        output.push(0);
        output.extend_from_slice(&0u16.to_be_bytes());
        output.extend_from_slice(
            &u64::try_from(byte_offset)
                .context("BPK1 section offset exceeds u64")?
                .to_be_bytes(),
        );
        output.extend_from_slice(
            &u64::try_from(bytes.len())
                .context("BPK1 section length exceeds u64")?
                .to_be_bytes(),
        );
        output.extend_from_slice(&Sha256::digest(bytes));
        byte_offset = byte_offset
            .checked_add(bytes.len())
            .context("BPK1 section end overflow")?;
    }
    debug_assert_eq!(output.len(), header_byte_length);

    let header_crc32 = package_header_crc32(&output)?;
    output[PACKAGE_HEADER_CRC32_OFFSET..PACKAGE_HEADER_CRC32_END]
        .copy_from_slice(&header_crc32.to_be_bytes());
    for (_, bytes) in sections {
        output.extend_from_slice(bytes);
    }
    debug_assert_eq!(output.len(), total_byte_length);

    parse_package(&output).context("encoded BPK1 package failed validation")?;
    Ok(output)
}

/// Recover exact record components from a picture-record PNG and package them.
pub fn encode_package_from_png(png: &[u8]) -> Result<Vec<u8>> {
    let decoded = record_decode::decode_record_png(png)
        .context("failed to decode picture-record PNG for BPK1")?;
    let (_, brd1) =
        record_decode::decode_record_descriptor_bytes_from_png(png, Some(&decoded.record_profile))
            .context("failed to recover the exact BRD1 descriptor")?;
    let bsc1 = if decoded.descriptor.bsc_pointer.is_some() {
        Some(
            record_sidecar::decode_record_png_sidecar_bytes(png, Some(&decoded.record_profile))
                .context("failed to recover the exact BSC1 sidecar")?,
        )
    } else {
        None
    };
    encode_package(&brd1, &decoded.chunk_stream.bytes, bsc1.as_deref())
}

/// Parse and validate a complete BPK1 package.
pub fn parse_package(bytes: &[u8]) -> Result<ParsedPackage<'_>> {
    if bytes.len() < PACKAGE_PREFIX_LENGTH {
        bail!("BPK1 package is too short");
    }
    if bytes.get(..4) != Some(PACKAGE_MAGIC.as_slice()) {
        bail!("BPK1 package magic is unsupported");
    }

    let version = bytes[4];
    if version != PACKAGE_VERSION {
        bail!("unsupported BPK1 version {version}");
    }
    let flags = bytes[5];
    if flags & !PACKAGE_KNOWN_FLAGS != 0 {
        bail!("BPK1 package contains unknown flags");
    }

    let section_count = read_u16be(bytes, 6, "BPK1 section count")? as usize;
    let has_bsc1 = flags & PACKAGE_FLAG_HAS_BSC1 != 0;
    let expected_section_count = if has_bsc1 { 3 } else { 2 };
    if section_count != expected_section_count {
        bail!(
            "BPK1 section count {section_count} does not match flags; expected {expected_section_count}"
        );
    }

    let header_byte_length = usize::try_from(read_u32be(bytes, 8, "BPK1 header length")?)
        .context("BPK1 header length exceeds usize")?;
    let expected_header_byte_length = PACKAGE_PREFIX_LENGTH
        .checked_add(
            section_count
                .checked_mul(PACKAGE_DIRECTORY_ENTRY_LENGTH)
                .context("BPK1 directory length overflow")?,
        )
        .context("BPK1 header length overflow")?;
    if header_byte_length != expected_header_byte_length {
        bail!(
            "BPK1 header length {header_byte_length} is not canonical; expected {expected_header_byte_length}"
        );
    }
    if header_byte_length > bytes.len() {
        bail!("BPK1 header is truncated");
    }

    let total_byte_length = usize::try_from(read_u64be(bytes, 12, "BPK1 total length")?)
        .context("BPK1 total length exceeds usize")?;
    if total_byte_length != bytes.len() {
        bail!("BPK1 total length does not match byte stream length");
    }

    let stored_header_crc32 = read_u32be(bytes, PACKAGE_HEADER_CRC32_OFFSET, "BPK1 header CRC-32")?;
    let actual_header_crc32 = package_header_crc32(&bytes[..header_byte_length])?;
    if stored_header_crc32 != actual_header_crc32 {
        bail!("BPK1 header CRC-32 mismatch");
    }

    let expected_kinds = if has_bsc1 {
        vec![
            PackageSectionKind::Brd1,
            PackageSectionKind::Brs1,
            PackageSectionKind::Bsc1,
        ]
    } else {
        vec![PackageSectionKind::Brd1, PackageSectionKind::Brs1]
    };
    let mut sections = Vec::with_capacity(section_count);
    let mut expected_byte_offset = header_byte_length;

    for (index, expected_kind) in expected_kinds.iter().enumerate() {
        let directory_offset = PACKAGE_PREFIX_LENGTH + index * PACKAGE_DIRECTORY_ENTRY_LENGTH;
        let kind = PackageSectionKind::from_wire_code(bytes[directory_offset])?;
        if kind != *expected_kind {
            bail!(
                "BPK1 section {index} is {}; expected {}",
                kind.name(),
                expected_kind.name()
            );
        }
        if bytes[directory_offset + 1] != 0 {
            bail!("BPK1 section {index} flags must be 0");
        }
        if bytes[directory_offset + 2..directory_offset + 4] != [0, 0] {
            bail!("BPK1 section {index} reserved bytes must be 0");
        }

        let byte_offset = usize::try_from(read_u64be(
            bytes,
            directory_offset + 4,
            "BPK1 section offset",
        )?)
        .context("BPK1 section offset exceeds usize")?;
        let byte_length = usize::try_from(read_u64be(
            bytes,
            directory_offset + 12,
            "BPK1 section length",
        )?)
        .context("BPK1 section length exceeds usize")?;
        if byte_length == 0 {
            bail!("BPK1 section {index} must not be empty");
        }
        if byte_offset != expected_byte_offset {
            bail!(
                "BPK1 section {index} offset {byte_offset} is not canonical; expected {expected_byte_offset}"
            );
        }
        let section_end = byte_offset
            .checked_add(byte_length)
            .context("BPK1 section end overflow")?;
        if section_end > bytes.len() {
            bail!("BPK1 section {index} is truncated");
        }

        let stored_sha256: [u8; 32] = bytes
            [directory_offset + 20..directory_offset + PACKAGE_DIRECTORY_ENTRY_LENGTH]
            .try_into()
            .expect("BPK1 SHA-256 field length");
        let actual_sha256: [u8; 32] = Sha256::digest(&bytes[byte_offset..section_end]).into();
        if stored_sha256 != actual_sha256 {
            bail!("BPK1 {} section SHA-256 mismatch", kind.name());
        }

        sections.push(PackageSectionDescriptor {
            kind,
            byte_offset,
            byte_length,
            sha256: stored_sha256,
        });
        expected_byte_offset = section_end;
    }
    if expected_byte_offset != bytes.len() {
        bail!("BPK1 package contains trailing bytes");
    }

    let brd1 = section_bytes(bytes, &sections, PackageSectionKind::Brd1)
        .context("BPK1 package has no BRD1 section")?;
    let brs1 = section_bytes(bytes, &sections, PackageSectionKind::Brs1)
        .context("BPK1 package has no BRS1 section")?;
    let bsc1 = section_bytes(bytes, &sections, PackageSectionKind::Bsc1);
    let (record_descriptor, sidecar_validation) = validate_components(brd1, brs1, bsc1)?;

    Ok(ParsedPackage {
        version,
        flags,
        header_byte_length,
        sections,
        record_descriptor,
        sidecar_validation,
        brd1,
        brs1,
        bsc1,
    })
}

/// Validate BPK1 and return a compact inspection model.
pub fn inspect_package(bytes: &[u8]) -> Result<PackageInspection> {
    let package = parse_package(bytes)?;
    let component_bytes: usize = package
        .sections
        .iter()
        .map(|section| section.byte_length)
        .sum();
    let descriptor = &package.record_descriptor;
    Ok(PackageInspection {
        format: "BPK1",
        version: package.version,
        flags: package.flags,
        header_byte_length: package.header_byte_length,
        total_byte_length: bytes.len(),
        container_overhead_bytes: bytes.len() - component_bytes,
        section_count: package.sections.len(),
        sections: package
            .sections
            .iter()
            .map(|section| PackageSectionInspection {
                section_type: section.kind.wire_code(),
                name: section.kind.name(),
                byte_offset: section.byte_offset,
                byte_length: section.byte_length,
                sha256: hex(&section.sha256),
            })
            .collect(),
        descriptor: PackageDescriptorInspection {
            record_profile: descriptor.record_profile.clone(),
            stream_byte_length: descriptor.stream_byte_length,
            payload_encoding: descriptor.payload_encoding.clone(),
            title: descriptor.title.clone(),
            artist: descriptor.artist.clone(),
            catalog_number: descriptor.catalog_number.clone(),
            label: descriptor.label.clone(),
            artwork_credit: descriptor.artwork_credit.clone(),
            canonical_url: descriptor.canonical_url.clone(),
            copyright_year: descriptor.copyright_year,
            copyright_holder: descriptor.copyright_holder.clone(),
            has_sidecar_pointer: descriptor.bsc_pointer.is_some(),
        },
        sidecar_item_count: package
            .sidecar_validation
            .as_ref()
            .map(|validation| validation.item_count),
    })
}

fn validate_components(
    brd1: &[u8],
    brs1: &[u8],
    bsc1: Option<&[u8]>,
) -> Result<(RecordDescriptor, Option<SidecarContainerValidation>)> {
    let descriptor = decode_record_descriptor_bytes(brd1).context("invalid BPK1 BRD1 section")?;
    record_core::parse_record_stream(brs1).context("invalid BPK1 BRS1 section")?;
    if descriptor.stream_byte_length != brs1.len() {
        bail!(
            "BRD1 stream length {} does not match BRS1 byte length {}",
            descriptor.stream_byte_length,
            brs1.len()
        );
    }

    let sidecar_validation = match bsc1 {
        Some(sidecar_bytes) => {
            let validation =
                validate_sidecar_container(sidecar_bytes).context("invalid BPK1 BSC1 section")?;
            let pointer_bytes = descriptor
                .bsc_pointer
                .as_deref()
                .context("BPK1 contains BSC1 but BRD1 has no sidecar pointer")?;
            let pointer = decode_sidecar_header_pointer(pointer_bytes)
                .context("invalid BRD1 BSC1 pointer")?;
            if pointer.length != sidecar_bytes.len() {
                bail!(
                    "BRD1 sidecar length {} does not match BSC1 byte length {}",
                    pointer.length,
                    sidecar_bytes.len()
                );
            }
            let sidecar_sha256: [u8; 32] = Sha256::digest(sidecar_bytes).into();
            if pointer.sha256_bytes != sidecar_sha256 {
                bail!("BRD1 sidecar SHA-256 does not match BSC1 bytes");
            }
            Some(validation)
        }
        None => {
            if descriptor.bsc_pointer.is_some() {
                bail!("BRD1 has a sidecar pointer but BPK1 has no BSC1 section");
            }
            None
        }
    };

    Ok((descriptor, sidecar_validation))
}

fn section_bytes<'a>(
    package: &'a [u8],
    sections: &[PackageSectionDescriptor],
    kind: PackageSectionKind,
) -> Option<&'a [u8]> {
    let section = sections.iter().find(|section| section.kind == kind)?;
    Some(&package[section.byte_offset..section.byte_offset + section.byte_length])
}

fn package_header_crc32(header: &[u8]) -> Result<u32> {
    if header.len() < PACKAGE_PREFIX_LENGTH {
        bail!("BPK1 header is too short for CRC-32");
    }
    let mut checksum_bytes = header.to_vec();
    checksum_bytes[PACKAGE_HEADER_CRC32_OFFSET..PACKAGE_HEADER_CRC32_END].fill(0);
    Ok(record_core::crc32_ieee(&checksum_bytes))
}

fn read_u16be(bytes: &[u8], offset: usize, label: &str) -> Result<u16> {
    let end = offset.checked_add(2).context("u16 offset overflow")?;
    let value = bytes
        .get(offset..end)
        .with_context(|| format!("{label} is truncated"))?;
    Ok(u16::from_be_bytes(value.try_into().expect("u16 length")))
}

fn read_u32be(bytes: &[u8], offset: usize, label: &str) -> Result<u32> {
    let end = offset.checked_add(4).context("u32 offset overflow")?;
    let value = bytes
        .get(offset..end)
        .with_context(|| format!("{label} is truncated"))?;
    Ok(u32::from_be_bytes(value.try_into().expect("u32 length")))
}

fn read_u64be(bytes: &[u8], offset: usize, label: &str) -> Result<u64> {
    let end = offset.checked_add(8).context("u64 offset overflow")?;
    let value = bytes
        .get(offset..end)
        .with_context(|| format!("{label} is truncated"))?;
    Ok(u64::from_be_bytes(value.try_into().expect("u64 length")))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use record_cut::descriptor::{encode_record_descriptor_stream, RecordDescriptorInput};
    use record_cut::{
        encode_record_stream, PayloadDescriptorInput, PayloadEntryInput, RecordStreamInput,
        TrackInput,
    };
    use record_sidecar::{
        encode_sidecar_header_pointer, SidecarCarrier, SidecarHeaderPointer, SIDECAR_DEFAULT_SEED,
        SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2,
    };

    fn brs1() -> Vec<u8> {
        encode_record_stream(
            &RecordStreamInput {
                payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
                tracks: vec![TrackInput {
                    title: "One".to_owned(),
                    first_revolution_index: None,
                    revolution_count: None,
                }],
                track_gaps: Vec::new(),
            },
            &[PayloadEntryInput::new(0, vec![1, 2, 3, 4])],
        )
        .unwrap()
    }

    fn bsc1() -> Vec<u8> {
        record_sidecar::build_sidecar_container_from_items_json(
            r#"[{"type":"json","codec":"raw","name":"credits.json","mime":"application/json","json":{"artist":"Test Artist"}}]"#,
        )
        .unwrap()
    }

    fn brd1(brs1: &[u8], bsc1: Option<&[u8]>) -> Vec<u8> {
        let bsc_pointer = bsc1.map(|bytes| {
            let sha256_bytes: [u8; 32] = Sha256::digest(bytes).into();
            encode_sidecar_header_pointer(&SidecarHeaderPointer {
                scheme: SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2.to_owned(),
                carriers: vec![SidecarCarrier::Label],
                seed: SIDECAR_DEFAULT_SEED,
                length: bytes.len(),
                sha256: String::new(),
                sha256_bytes,
            })
            .unwrap()
        });
        encode_record_descriptor_stream(
            1.0,
            &RecordDescriptorInput {
                record_profile: "single45".to_owned(),
                stream_byte_length: brs1.len(),
                payload_encoding: Some("rgb".to_owned()),
                title: Some("One".to_owned()),
                artist: Some("Artist".to_owned()),
                bsc_pointer,
                ..RecordDescriptorInput::default()
            },
            usize::MAX,
        )
        .unwrap()
    }

    #[test]
    fn package_with_sidecar_round_trips_exact_sections() {
        let stream = brs1();
        let sidecar = bsc1();
        let descriptor = brd1(&stream, Some(&sidecar));
        let encoded = encode_package(&descriptor, &stream, Some(&sidecar)).unwrap();
        let parsed = parse_package(&encoded).unwrap();

        assert_eq!(parsed.brd1(), descriptor);
        assert_eq!(parsed.brs1(), stream);
        assert_eq!(parsed.bsc1(), Some(sidecar.as_slice()));
        assert_eq!(parsed.header_byte_length, PACKAGE_OVERHEAD_WITH_BSC1);
        assert_eq!(
            encoded.len(),
            descriptor.len() + stream.len() + sidecar.len() + PACKAGE_OVERHEAD_WITH_BSC1
        );
        assert_eq!(
            inspect_package(&encoded).unwrap().container_overhead_bytes,
            PACKAGE_OVERHEAD_WITH_BSC1
        );
    }

    #[test]
    fn package_without_sidecar_round_trips_exact_sections() {
        let stream = brs1();
        let descriptor = brd1(&stream, None);
        let encoded = encode_package(&descriptor, &stream, None).unwrap();
        let parsed = parse_package(&encoded).unwrap();

        assert_eq!(parsed.brd1(), descriptor);
        assert_eq!(parsed.brs1(), stream);
        assert_eq!(parsed.bsc1(), None);
        assert_eq!(parsed.header_byte_length, PACKAGE_OVERHEAD_WITHOUT_BSC1);
    }

    #[test]
    fn package_rejects_changed_section_bytes() {
        let stream = brs1();
        let descriptor = brd1(&stream, None);
        let mut encoded = encode_package(&descriptor, &stream, None).unwrap();
        let last = encoded.len() - 1;
        encoded[last] ^= 1;
        let error = parse_package(&encoded).unwrap_err();
        assert!(error.to_string().contains("SHA-256 mismatch"));
    }

    #[test]
    fn package_rejects_changed_header_bytes() {
        let stream = brs1();
        let descriptor = brd1(&stream, None);
        let mut encoded = encode_package(&descriptor, &stream, None).unwrap();
        encoded[8] ^= 1;
        let error = parse_package(&encoded).unwrap_err();
        assert!(error.to_string().contains("header length"));
    }

    #[test]
    fn package_rejects_wrong_header_crc32() {
        let stream = brs1();
        let descriptor = brd1(&stream, None);
        let mut encoded = encode_package(&descriptor, &stream, None).unwrap();
        encoded[PACKAGE_HEADER_CRC32_OFFSET] ^= 1;
        let error = parse_package(&encoded).unwrap_err();
        assert!(error.to_string().contains("header CRC-32 mismatch"));
    }

    #[test]
    fn package_rejects_sidecar_without_brd1_pointer() {
        let stream = brs1();
        let descriptor = brd1(&stream, None);
        let sidecar = bsc1();
        let error = encode_package(&descriptor, &stream, Some(&sidecar)).unwrap_err();
        assert!(error.to_string().contains("BRD1 has no sidecar pointer"));
    }

    #[test]
    fn package_rejects_wrong_brd1_stream_length() {
        let stream = brs1();
        let other_stream = brs1();
        let descriptor = brd1(&stream[..stream.len() - 1], None);
        let error = encode_package(&descriptor, &other_stream, None).unwrap_err();
        assert!(error
            .to_string()
            .contains("does not match BRS1 byte length"));
    }
}

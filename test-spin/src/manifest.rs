//! The test-spin report, flat.
//!
//! `inspect_record_png` writes a diagnostic: hex prefixes, per-entry tables,
//! ANSI ticks, several hundred lines of it. That is the right shape for a
//! terminal and the wrong shape for a phone, so this is the same read said
//! once — labelled sections of label/value rows, in the order a person would
//! ask the questions.
//!
//! It is derived from the decoded record, not scraped out of the text
//! report: a screen parsing a diagnostic's formatting is a screen that
//! breaks the next time a column moves.
//!
//! Long values are truncated for display and kept whole for sharing. A codec
//! blob or a canonical URL runs past the width of a phone, and a row that
//! wraps to four lines costs more than the tail of the value is worth —
//! [`ManifestRow::full`] carries what was cut, and [`ManifestReport::to_text`]
//! writes the whole of it.

use anyhow::{Context, Result};
use record_descriptor::RecordDescriptor;
use serde::Serialize;
use std::fmt::Write as _;

use crate::sidecars;

/// How wide a value may be before the display copy is cut. Chosen against
/// the narrowest screen this is read on rather than a round number.
pub const MAX_VALUE_WIDTH: usize = 64;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestReport {
    /// Whether every check in the report passed. The report covers a record
    /// that reads and fails a check in full, and it names the failed check.
    pub ok: bool,
    pub checks_passed: usize,
    pub checks_failed: usize,
    pub sections: Vec<ManifestSection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSection {
    pub title: String,
    pub rows: Vec<ManifestRow>,
}

impl ManifestSection {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            rows: Vec::new(),
        }
    }

    pub fn push(&mut self, row: ManifestRow) {
        self.rows.push(row);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRow {
    pub label: String,
    /// The value as it should be shown: cut at [`MAX_VALUE_WIDTH`].
    pub value: String,
    /// The whole value, present only when `value` is a truncation of it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
    /// Set when this row is a check rather than a fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<bool>,
}

impl ManifestRow {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        let value = value.into();
        let (shown, full) = truncate(&value);
        Self {
            label: label.into(),
            value: shown,
            full,
            status: None,
        }
    }

    pub fn check(label: impl Into<String>, value: impl Into<String>, passed: bool) -> Self {
        let mut row = Self::new(label, value);
        row.status = Some(passed);
        row
    }

    /// The value as it was before truncation.
    pub fn whole(&self) -> &str {
        self.full.as_deref().unwrap_or(&self.value)
    }
}

fn truncate(value: &str) -> (String, Option<String>) {
    if value.chars().count() <= MAX_VALUE_WIDTH {
        return (value.to_owned(), None);
    }
    let cut: String = value.chars().take(MAX_VALUE_WIDTH - 1).collect();
    (format!("{cut}…"), Some(value.to_owned()))
}

impl ManifestReport {
    /// The whole report as plain text: every value in full, no ANSI, no
    /// truncation. This is what a share sheet hands on.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "BITNEEDLE MANIFEST");
        let _ = writeln!(
            out,
            "{} · {} checks passed, {} failed",
            if self.ok { "VALID" } else { "FAILED" },
            self.checks_passed,
            self.checks_failed
        );

        for section in &self.sections {
            let _ = writeln!(out, "\n{}", section.title);
            let width = section
                .rows
                .iter()
                .map(|row| row.label.chars().count())
                .max()
                .unwrap_or(0);
            for row in &section.rows {
                let mark = match row.status {
                    Some(true) => "[ok]   ",
                    Some(false) => "[FAIL] ",
                    None => "",
                };
                let _ = writeln!(
                    out,
                    "  {mark}{:width$}  {}",
                    row.label,
                    row.whole(),
                    width = width
                );
            }
        }
        out
    }
}

/// Read a pressed record and lay its manifest out flat.
pub fn manifest_report(png: &[u8], options: &crate::InspectionOptions<'_>) -> Result<ManifestReport> {
    // Patternize first: a permuted groove will not parse, and the reverse
    // map lives in the sidecar.
    //
    // A restore that fails is not on its own a record that fails. The
    // permutation is optional, and a record that was never permuted can
    // still trip this — a sidecar that is not a reverse map, a descriptor
    // this build does not know — so the plain decode is tried before
    // anything is reported. What is reported then is the decode's own
    // error, with the restore's beside it: naming the last thing tried
    // rather than the thing that was wrong sends a reader hunting a
    // Patternize map on a record that has none.
    let decoded = match record_sidecar::restore_patternized_record_png(png, None) {
        Ok(restored) => record_decode::decode_record_png(restored.as_deref().unwrap_or(png))
            .context("failed to decode the record PNG")?,
        Err(restore_error) => record_decode::decode_record_png(png)
            .with_context(|| format!("this file could not be read as a record: {restore_error:#}"))?,
    };
    let descriptor = &decoded.descriptor;
    let parsed = record_core::parse_record_stream(&decoded.chunk_stream.bytes)
        .context("failed to parse the BRS1 record stream")?;

    let sidecar = sidecars::inspect(png, Some(&decoded.record_profile));

    let mut sections = Vec::new();
    sections.push(record_section(png, &decoded, options.png_name));
    sections.push(identity_section(descriptor));
    sections.push(audio_section(&parsed));
    sections.push(programme_section(&parsed));
    sections.extend(sidecars::sections(&sidecar));
    sections.push(signing_section(descriptor, &sidecar));

    // The payload's own checks and the sidecar's, in one list: a record is
    // one thing, and a reader should not have to know which half of it a
    // failure came from to see that there was one.
    let spec = crate::collect_spec_checks(&parsed, &decoded.record_profile);
    let mut checks = ManifestSection::new("CHECKS");
    for check in &spec {
        checks.push(ManifestRow::check(&check.label, &check.detail, check.passed));
    }
    let passed = spec.iter().filter(|check| check.passed).count()
        + sidecar.checks.iter().filter(|check| check.passed).count();
    let failed = spec.len() + sidecar.checks.len() - passed;
    sections.push(checks);

    Ok(ManifestReport {
        ok: failed == 0 && sidecar.error.is_none(),
        checks_passed: passed,
        checks_failed: failed,
        sections,
    })
}

/// The manifest as JSON, for a phone or a browser to lay out itself.
pub fn manifest_report_json(png: &[u8]) -> Result<String> {
    let options = crate::InspectionOptions::verbose_defaults();
    let report = manifest_report(png, &options)?;
    serde_json::to_string(&report).context("failed to serialize the record manifest")
}

fn record_section(
    png: &[u8],
    decoded: &record_decode::DecodedRecord,
    name: Option<&str>,
) -> ManifestSection {
    let mut section = ManifestSection::new("RECORD");
    if let Some(name) = name {
        section.push(ManifestRow::new("File", name));
    }
    section.push(ManifestRow::new("Bytes", format_bytes(png.len())));
    if let Some((width, height, depth, colour)) = crate::png_ihdr(png) {
        section.push(ManifestRow::new("Canvas", format!("{width} × {height}")));
        section.push(ManifestRow::new(
            "Pixels",
            format!("{depth}-bit, colour type {colour}"),
        ));
    }
    section.push(ManifestRow::new("Profile", &decoded.record_profile));
    section.push(ManifestRow::new(
        "Groove pixels",
        decoded.chunk_stream.pixel_count.to_string(),
    ));
    section.push(ManifestRow::new(
        "Stream",
        format_bytes(decoded.chunk_stream.bytes.len()),
    ));
    section
}

fn identity_section(descriptor: &RecordDescriptor) -> ManifestSection {
    let mut section = ManifestSection::new("IDENTITY");
    section.push(ManifestRow::new(
        "Title",
        descriptor.title.as_deref().unwrap_or("—"),
    ));
    section.push(ManifestRow::new(
        "Artist",
        descriptor.artist.as_deref().unwrap_or("—"),
    ));
    section.push(ManifestRow::new(
        "Label",
        descriptor.label.as_deref().unwrap_or("—"),
    ));
    section.push(ManifestRow::new(
        "Catalogue",
        descriptor.catalog_number.as_deref().unwrap_or("—"),
    ));
    section.push(ManifestRow::new(
        "Release ID",
        descriptor
            .release_id
            .map(record_descriptor::release_id_to_text)
            .unwrap_or_else(|| "—".to_owned()),
    ));
    section.push(ManifestRow::new(
        "YL code",
        descriptor
            .canonical_url
            .as_deref()
            .and_then(crate::yl_catalogue_code_from_url)
            .unwrap_or_else(|| "—".to_owned()),
    ));
    section.push(ManifestRow::new(
        "Canonical URL",
        descriptor.canonical_url.as_deref().unwrap_or("—"),
    ));
    section.push(ManifestRow::new(
        "Artwork credit",
        descriptor.artwork_credit.as_deref().unwrap_or("—"),
    ));
    section.push(ManifestRow::new(
        "Descriptor",
        format!("BRD1 v{}, b={}", descriptor.version, descriptor.b_value()),
    ));
    section
}

fn audio_section(parsed: &record_core::RecordStream) -> ManifestSection {
    let mut section = ManifestSection::new("AUDIO");
    let metadata = &parsed.metadata;

    // The codecs as one line rather than a descriptor apiece. A record cut
    // at two bitrates carries several descriptors that differ only in their
    // block size, and printing each in full is how a summary becomes a
    // table. The whole list is kept for the share.
    let codecs = metadata
        .payload_descriptors
        .iter()
        .map(|descriptor| {
            let codec = descriptor.codec.as_deref().unwrap_or("—");
            format!("{} {codec}", descriptor.container)
        })
        .collect::<Vec<_>>();
    let mut unique = codecs.clone();
    unique.sort();
    unique.dedup();
    section.push(ManifestRow::new("Codecs", unique.join(", ")));

    let rate = metadata
        .payload_descriptors
        .iter()
        .find_map(|descriptor| descriptor.sample_rate);
    let channels = metadata
        .payload_descriptors
        .iter()
        .find_map(|descriptor| descriptor.channels);
    section.push(ManifestRow::new(
        "Sample rate",
        rate.map(|value| format!("{value} Hz"))
            .unwrap_or_else(|| "—".to_owned()),
    ));
    section.push(ManifestRow::new(
        "Channels",
        channels
            .map(|value| value.to_string())
            .unwrap_or_else(|| "—".to_owned()),
    ));
    section.push(ManifestRow::new(
        "Encrypted",
        if metadata.encrypted { "yes" } else { "no" },
    ));
    section.push(ManifestRow::new(
        "Payload entries",
        metadata.payload_entries.len().to_string(),
    ));
    section.push(ManifestRow::new(
        "Transport chunks",
        parsed.chunks.len().to_string(),
    ));
    section.push(ManifestRow::new(
        "Track gaps",
        metadata.track_gaps.len().to_string(),
    ));
    section
}

fn programme_section(parsed: &record_core::RecordStream) -> ManifestSection {
    let mut section = ManifestSection::new("PROGRAMME");
    if parsed.metadata.tracks.is_empty() {
        section.push(ManifestRow::new("Tracks", "none"));
        return section;
    }
    for (index, track) in parsed.metadata.tracks.iter().enumerate() {
        section.push(ManifestRow::new(
            format!("A{}", index + 1),
            format!(
                "{} · {} revolutions",
                if track.title.is_empty() {
                    "untitled"
                } else {
                    track.title.as_str()
                },
                track.revolution_count
            ),
        ));
    }
    section
}

fn signing_section(
    descriptor: &RecordDescriptor,
    sidecar: &sidecars::SidecarReport,
) -> ManifestSection {
    let mut section = ManifestSection::new("SIGNING");
    match descriptor.signed_release_reference.as_ref() {
        Some(reference) => {
            section.push(ManifestRow::new("Release reference", "present"));
            section.push(ManifestRow::new(
                "Key",
                crate::printable_utf8(&reference.key_id).unwrap_or("<binary>"),
            ));
            section.push(ManifestRow::check(
                "Reference is well formed",
                match reference.validate() {
                    Ok(()) => "the signed release reference validates".to_owned(),
                    Err(error) => format!("{error:#}"),
                },
                reference.validate().is_ok(),
            ));
        }
        None => section.push(ManifestRow::new("Release reference", "absent")),
    }
    section.push(ManifestRow::new(
        "Sidecar attestation",
        match (
            sidecar.inspection.as_ref().and_then(|found| found.attestation.as_ref()),
            sidecar.inspection.as_ref().and_then(|found| found.attestation_covers),
        ) {
            (Some(_), Some(true)) => "present, covers the sidecar".to_owned(),
            (Some(_), _) => "present, does not cover the sidecar".to_owned(),
            _ => "absent".to_owned(),
        },
    ));
    section
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kib = bytes as f64 / 1024.0;
    if kib < 1024.0 {
        return format!("{kib:.1} KiB");
    }
    format!("{:.2} MiB", kib / 1024.0)
}

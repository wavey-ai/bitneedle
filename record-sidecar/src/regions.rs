//! Capacity and utilization of the writable regions, in fill order.
use super::*;

fn live_groove_pixels(
    width: usize,
    height: usize,
    descriptor: &record_descriptor::RecordDescriptor,
) -> Result<Vec<bool>> {
    let count = match descriptor.payload_encoding.as_str() {
        record_core::PAYLOAD_ENCODING_RGB => {
            bytes2rgb::pixel_count_for_byte_length(descriptor.stream_byte_length)
        }
        record_descriptor::PAYLOAD_ENCODING_TONED_V1 => record_descriptor::resolve_tone_spans(
            &descriptor.tone_spans,
            Some(descriptor.stream_byte_length),
        )?
        .last()
        .map_or(0, |span| span.pixel_offset + span.pixel_count),
        record_descriptor::PAYLOAD_ENCODING_TONED_V2 => record_descriptor::tone_clock_pixel_count(
            descriptor
                .tone_clock
                .as_ref()
                .context("toned-v2 record has no tone clock")?,
            descriptor.stream_byte_length,
        )?,
        other => bail!("unsupported sidecar payload encoding: {other}"),
    };
    let mask = record_core::build_spiral_mask_at(
        width,
        height,
        descriptor.b_value(),
        &descriptor.spiral_family,
        &descriptor.record_profile,
        None,
        None,
        None,
        descriptor.spiral_clockwise,
        descriptor.lead_out_geometry_revision,
    )?;
    if count > mask.ordered_pixel_indices.len() {
        bail!("record stream exceeds its groove");
    }
    let mut protected = vec![false; width * height];
    for &pixel in mask.ordered_pixel_indices.iter().take(count) {
        protected[pixel] = true;
    }
    Ok(protected)
}

/// Protect the actual stream pixels at the rounded cut boundary. Each later
/// carrier must also exclude pixels already assigned to an earlier carrier.
pub(super) fn protect_groove_and_carriers(
    width: usize,
    height: usize,
    descriptor: &record_descriptor::RecordDescriptor,
    runs: &mut [SidecarRun],
) -> Result<()> {
    let mut protected = live_groove_pixels(width, height, descriptor)?;
    for run in runs {
        match run {
            SidecarRun::Groove { indices, .. } => indices.retain(|&pixel| {
                if protected[pixel] {
                    return false;
                }
                protected[pixel] = true;
                true
            }),
            SidecarRun::Steg { pairs, .. } => pairs.retain(|&(a, b)| {
                if protected[a] || protected[b] {
                    return false;
                }
                protected[a] = true;
                protected[b] = true;
                true
            }),
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    pixels: usize,
    pairs: usize,
    capacity_bytes: usize,
    used_bytes: usize,
    image_percent: f64,
    utilization_percent: Option<f64>,
}

fn row(
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    pixels: usize,
    capacity: usize,
    used: usize,
    image_pixels: usize,
) -> Region {
    Region {
        id,
        name,
        kind,
        pixels,
        pairs: if kind == "pairs" { pixels / 2 } else { 0 },
        capacity_bytes: capacity,
        used_bytes: used,
        image_percent: pixels as f64 * 100.0 / image_pixels as f64,
        utilization_percent: (capacity > 0).then(|| used as f64 * 100.0 / capacity as f64),
    }
}

/// Byte capacities use the writer's groove encodings and pairwise scoring.
/// Utilization includes the sidecar container and all existing sidecar items.
pub fn record_png_carrier_regions_json(png: &[u8], profile: Option<&str>) -> Result<String> {
    let context = decode_record_png_context(png, profile)?;
    serde_json::to_string(&carrier_regions(&context)?)
        .context("could not serialize carrier regions")
}

fn carrier_regions(context: &RecordPngContext) -> Result<serde_json::Value> {
    let pointer = sidecar_pointer_from_descriptor(&context.descriptor)?;
    let seed = pointer.as_ref().map_or(SIDECAR_DEFAULT_SEED, |p| p.seed);
    let text_avoid = text_avoid_spec_from_descriptor(&context.descriptor);
    let plan = build_sidecar_plan(
        context.width,
        context.height,
        &context.descriptor,
        context.descriptor_bytes,
        &SIDECAR_FILL_ORDER,
        seed,
        text_avoid.as_ref(),
    )?;
    let active = pointer
        .as_ref()
        .map(|p| p.carriers.as_slice())
        .unwrap_or(&[]);
    let capacities =
        sidecar_plan_capacities(&plan, SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2, &context.rgba)?;
    let mut remaining = pointer.as_ref().map_or(0, |p| p.length);
    let trace = record_core::build_lead_out_trace_at(
        context.width,
        context.height,
        context.descriptor.b_value(),
        &context.descriptor.spiral_family,
        &context.record_profile,
        sidecar_cut_inner_radius(&context.descriptor),
        record_core::LeadOutExtent::Fill,
        context.descriptor.spiral_clockwise,
        context.descriptor.lead_out_geometry_revision,
    )?;
    let mut locked = vec![false; context.width * context.height];
    for &pixel in &trace.indices[trace.lock_start..] {
        locked[pixel] = true;
    }
    let mut rows = Vec::new();
    let image_pixels = context.width * context.height;
    for carrier in SIDECAR_FILL_ORDER {
        let found = plan.iter().position(|run| run.carrier() == carrier);
        let run = found.map(|i| &plan[i]);
        let capacity = found.map_or(0, |i| capacities[i].1);
        let used = if active.contains(&carrier) {
            remaining.min(capacity)
        } else {
            0
        };
        remaining -= used;
        let pixels = match run {
            Some(SidecarRun::Groove { indices, .. }) => indices.len(),
            Some(SidecarRun::Steg { pairs, .. }) => pairs.len() * 2,
            None => 0,
        };
        match carrier {
            SidecarCarrier::Trailer => {
                let (run_out_pixels, run_out_capacity) = match run {
                    Some(SidecarRun::Groove {
                        indices, encoding, ..
                    }) => {
                        let count = indices.iter().take_while(|&&pixel| !locked[pixel]).count();
                        (count, encoding.byte_capacity(count).min(capacity))
                    }
                    _ => (0, 0),
                };
                // Assign a byte crossing the boundary to the locked groove.
                let run_out_used = used.min(run_out_capacity);
                rows.push(row(
                    "runOut",
                    "Run-out",
                    "groove",
                    run_out_pixels,
                    run_out_capacity,
                    run_out_used,
                    image_pixels,
                ));
                rows.push(row(
                    "lockedGroove",
                    "Locked groove",
                    "groove",
                    pixels - run_out_pixels,
                    capacity - run_out_capacity,
                    used - run_out_used,
                    image_pixels,
                ));
            }
            _ => {
                let (id, name, kind) = match carrier {
                    SidecarCarrier::LeadIn => ("leadIn", "Lead-in", "groove"),
                    SidecarCarrier::SilentGroove => ("silentGroove", "Silent groove", "groove"),
                    SidecarCarrier::Intergroove => ("intergroove", "Intergroove", "pairs"),
                    SidecarCarrier::Label => ("label", "Label", "pairs"),
                    _ => unreachable!(),
                };
                rows.push(row(id, name, kind, pixels, capacity, used, image_pixels));
            }
        }
    }
    Ok(serde_json::json!({ "imagePixels": image_pixels,
        "capacityBytes": rows.iter().map(|r| r.capacity_bytes).sum::<usize>(),
        "usedBytes": rows.iter().map(|r| r.used_bytes).sum::<usize>(), "regions": rows }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn descriptor() -> record_descriptor::RecordDescriptor {
        serde_json::from_str(include_str!("../tests/fixtures/toned-boundary.json")).unwrap()
    }
    #[test]
    fn silent_groove_boundary_excludes_music_and_carriers_are_disjoint() {
        for clockwise in [false, true] {
            let mut d = descriptor();
            d.spiral_clockwise = clockwise;
            let protected = live_groove_pixels(576, 576, &d).unwrap();
            let mut claimed = protected.clone();
            let plan = build_sidecar_plan(
                576,
                576,
                &d,
                581,
                &SIDECAR_FILL_ORDER,
                SIDECAR_DEFAULT_SEED,
                None,
            )
            .unwrap();
            assert_eq!(
                plan.iter().map(|run| run.carrier()).collect::<Vec<_>>(),
                SIDECAR_FILL_ORDER
            );
            for run in plan {
                let pixels = match run {
                    SidecarRun::Groove { indices, .. } => indices,
                    SidecarRun::Steg { pairs, .. } => {
                        pairs.into_iter().flat_map(|(a, b)| [a, b]).collect()
                    }
                };
                assert!(!pixels.is_empty());
                for pixel in pixels {
                    assert!(!claimed[pixel], "overlap at {pixel}, clockwise={clockwise}");
                    claimed[pixel] = true;
                }
            }
        }
    }
    #[test]
    fn all_carriers_fill_before_label_and_the_stream_roundtrips() {
        let d = descriptor();
        let plan = build_sidecar_plan(
            576,
            576,
            &d,
            581,
            &SIDECAR_FILL_ORDER,
            SIDECAR_DEFAULT_SEED,
            None,
        )
        .unwrap();
        let mut rgba = vec![200_u8; 576 * 576 * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let original = rgba.clone();
        let capacities =
            sidecar_plan_capacities(&plan, SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2, &rgba).unwrap();
        let before_label: usize = capacities
            .iter()
            .filter(|(c, _)| *c != SidecarCarrier::Label)
            .map(|(_, n)| *n)
            .sum();
        let bytes: Vec<_> = (0..before_label + 32).map(|i| (i % 251) as u8).collect();
        let spans = sidecar_plan_split(
            &plan,
            SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2,
            &rgba,
            bytes.len(),
        )
        .unwrap();
        for ((_, capacity), (_, used)) in capacities.iter().zip(&spans).take(4) {
            assert_eq!(capacity, used);
        }
        assert_eq!(spans.last().unwrap().1, 32);
        paint_sidecar_bytes_into_plan(
            &mut rgba,
            576,
            576,
            &plan,
            SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2,
            &bytes,
        )
        .unwrap();
        let read = read_sidecar_bytes_from_plan(
            &rgba,
            576,
            576,
            &plan,
            SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2,
            bytes.len(),
        )
        .unwrap();
        assert_eq!(bytes, read);
        for (pixel, live) in live_groove_pixels(576, 576, &d)
            .unwrap()
            .into_iter()
            .enumerate()
        {
            if live {
                assert_eq!(
                    &rgba[pixel * 4..pixel * 4 + 4],
                    &original[pixel * 4..pixel * 4 + 4]
                );
            }
        }
    }
    #[test]
    fn region_metrics_match_the_writer_and_decoder_byte_counts() {
        let mut context = RecordPngContext {
            width: 576,
            height: 576,
            rgba: vec![200; 576 * 576 * 4],
            record_profile: "single45".to_owned(),
            descriptor: descriptor(),
            descriptor_bytes: 581,
        };
        for pixel in context.rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let plan = build_sidecar_plan(
            576,
            576,
            &context.descriptor,
            581,
            &SIDECAR_FILL_ORDER,
            SIDECAR_DEFAULT_SEED,
            None,
        )
        .unwrap();
        let capacities =
            sidecar_plan_capacities(&plan, SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2, &context.rgba)
                .unwrap();
        let length = capacities
            .iter()
            .take(4)
            .map(|(_, bytes)| bytes)
            .sum::<usize>()
            + 32;
        context.descriptor.bsc_pointer = Some(
            encode_sidecar_header_pointer(&SidecarHeaderPointer {
                scheme: SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2.to_owned(),
                carriers: SIDECAR_FILL_ORDER.to_vec(),
                seed: SIDECAR_DEFAULT_SEED,
                length,
                sha256: sha256_base64url(&[]),
                sha256_bytes: sha256_digest_bytes(&[]),
            })
            .unwrap(),
        );
        let report = carrier_regions(&context).unwrap();
        let rows = report["regions"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "leadIn",
                "silentGroove",
                "runOut",
                "lockedGroove",
                "intergroove",
                "label"
            ]
        );
        assert_eq!(
            report["capacityBytes"],
            capacities.iter().map(|(_, bytes)| bytes).sum::<usize>()
        );
        assert_eq!(report["usedBytes"], length);
        assert_eq!(
            rows[2]["capacityBytes"].as_u64().unwrap() + rows[3]["capacityBytes"].as_u64().unwrap(),
            capacities[2].1 as u64
        );
        assert!(rows[2]["pixels"].as_u64().unwrap() > 0);
        assert!(rows[3]["pixels"].as_u64().unwrap() > 0);
        for r in rows.iter().take(5) {
            assert_eq!(r["usedBytes"], r["capacityBytes"]);
        }
        assert_eq!(rows[5]["usedBytes"], 32);
        let percent = rows
            .iter()
            .map(|r| r["imagePercent"].as_f64().unwrap())
            .sum::<f64>();
        assert!(percent > 0.0 && percent < 100.0);
        for r in rows {
            let used = r["usedBytes"].as_f64().unwrap();
            let capacity = r["capacityBytes"].as_f64().unwrap();
            assert!(
                (r["utilizationPercent"].as_f64().unwrap() - used * 100.0 / capacity).abs() < 1e-9
            );
        }
    }
}

//! Sweep every profile from a very short programme to the longest that fits,
//! render each cut, and write a browsable page of the results.
//!
//! Everything lands in a temporary folder — no golden is touched.
//!
//! cargo run -p record-render --example record_sweep -- [profiles] [outDir]

use anyhow::Result;
use std::f64::consts::PI;
use std::fmt::Write as _;
use std::path::PathBuf;

/// One ECDC frame, in seconds. The programme is cut one frame per revolution.
const CHUNK_SECONDS: f64 = 1.3333;

/// The raw ECDC frames of a real stream, header stripped. Each is one 1.33 s
/// unit, so the byte rate and the playtime both come from the file itself
/// rather than from a guessed bytes-per-second.
fn load_packets(path: &str) -> Result<Vec<Vec<u8>>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 9 || &bytes[..4] != b"ECDC" {
        anyhow::bail!("not an ECDC stream: {path}");
    }
    let meta_len = u32::from_be_bytes(bytes[5..9].try_into().expect("header length")) as usize;
    let mut pos = 9 + meta_len;
    let mut packets = Vec::new();
    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().expect("packet length")) as usize;
        let end = pos + 8 + len;
        if end > bytes.len() {
            break;
        }
        packets.push(bytes[pos..end].to_vec());
        pos = end;
    }
    if packets.is_empty() {
        anyhow::bail!("no ECDC frames in {path}");
    }
    Ok(packets)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let profiles: Vec<String> = std::env::var("SWEEP_PROFILES")
        .ok()
        .map(|value| value.split(',').map(str::to_string).collect())
        .filter(|list: &Vec<String>| !list.is_empty())
        .unwrap_or_else(|| {
            ["single45", "single45vintage", "ten", "lp"]
                .iter()
                .map(|p| p.to_string())
                .collect()
        });
    let out_dir = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "/tmp/bitneedle-sweep".to_string()),
    );
    let sample_path = args.next().unwrap_or_else(|| {
        "../yl.vin/.tmp-local/preload-pray4me/pray4me-confirmation.12kbps.1333ms.ecdc".to_string()
    });
    std::fs::create_dir_all(&out_dir)?;
    let packets = load_packets(&sample_path)?;

    // The wheel lab's default artwork, shown under every cut so the artwork's
    // visibility through the groove can be judged. Copied beside the page.
    let mut art = String::new();
    for candidate in [
        std::path::PathBuf::from("../bitneedle-app/web/img/sample.avif"),
        std::path::PathBuf::from("web/img/sample.avif"),
    ] {
        if candidate.exists() {
            std::fs::copy(&candidate, out_dir.join("sample.avif"))?;
            art = "sample.avif".to_string();
            break;
        }
    }

    // Programme sizes come from the sample's own frames: one frame is one
    // 1.33 s revolution, so asking for a duration is asking for that many
    // frames. Frames past the sample's end wrap around, so the byte rate stays
    // the sample's real rate all the way to five minutes.
    let frame_bytes = packets.iter().map(Vec::len).sum::<usize>() as f64 / packets.len() as f64;
    println!(
        "sample {sample_path}\n  {} frames · {:.0} bytes/frame · {:.2} min of real audio",
        packets.len(),
        frame_bytes,
        packets.len() as f64 * CHUNK_SECONDS / 60.0
    );

    let durations_seconds: [f64; 14] = [
        15.0, 30.0, 45.0, 60.0, 75.0, 90.0, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0, 360.0,
    ];

    let mut sections = String::new();

    for profile in &profiles {
        let geometry = record_core::describe_record_profile(profile)?;
        let outer = record_core::payload_outer_radius_from_geometry(&geometry) as f64;
        let inner = record_core::programme_inner_radius(profile)? as f64;
        println!("profile {profile}  band {inner:.0}..{outer:.0} px");

        let mut rows = String::new();
        for (index, &seconds) in durations_seconds.iter().enumerate() {
            let frames = ((seconds / CHUNK_SECONDS).round() as usize).max(1);
            let mut codes = Vec::with_capacity(frames * frame_bytes as usize);
            for frame in 0..frames {
                codes.extend_from_slice(&packets[frame % packets.len()]);
            }
            let bytes = codes.len();
            let playtime = frames as f64 * CHUNK_SECONDS;
            // A release id per row, so the page shows the pitch variation: two
            // rows of the same length would otherwise cut one identical spiral.
            let release_id = format!("rel_000000000000000000000000{index:02}");
            let options = format!(
                r#"{{"labelReference":true,"headerReleaseId":"{release_id}"}}"#
            );

            let rendered = match record_render::render_payload_codes_to_png(
                &codes,
                "rgb",
                profile,
                playtime,
                Some(options.as_str()),
            ) {
                Ok(rendered) => rendered,
                Err(error) => {
                    println!("{:>5.1}s  {:>7} bytes  SKIP  {error:#}", playtime, bytes);
                    continue;
                }
            };

            let name = format!("{profile}-{index:02}-{seconds:.0}s.png");
            std::fs::write(out_dir.join(&name), &rendered.png_bytes)?;
            let payload = &rendered.payload;
            let separation = 2.0 * PI * payload.b_value;
            let total_seconds = playtime.round() as usize;
            let minutes = total_seconds / 60;
            let secs = total_seconds % 60;
            println!(
                "{minutes}:{secs:02}  {:>7} bytes  b={:.5}  {:.2} px/turn  cut {:>3}  silent {:>4.1}  {}",
                bytes, payload.b_value, separation, payload.cut_inner_radius, payload.silent_groove_turns, payload.status
            );

            let _ = write!(
                rows,
                r#"<figure><img src="{name}" loading="lazy"><figcaption><b>{minutes}:{secs:02}</b> · {bytes} B<br>{:.2} px/turn<br>cut {} · silent {:.1}<br><span class="s">{}</span></figcaption></figure>"#,
                separation, payload.cut_inner_radius, payload.silent_groove_turns, payload.status,
            );
        }

        let _ = write!(
            sections,
            r#"<section id="{profile}"><h2>{profile} · band {inner:.0}–{outer:.0} px</h2><div class="grid">{rows}</div></section>"#
        );
    }

    let nav = profiles
        .iter()
        .map(|p| format!(r##"<a href="#{p}">{p}</a>"##))
        .collect::<Vec<_>>()
        .join(" · ");

    let art_css = if art.is_empty() {
        "background:#000;".to_string()
    } else {
        format!("background:#000 url('{art}') center/cover no-repeat;")
    };
    let page = format!(
        r#"<!doctype html><meta charset="utf-8"><title>bitneedle sweep</title>
<style>
  :root {{ color-scheme: dark; }}
  body {{ margin: 0; background: #000; color: #b9b9b9;
         font: 12px/1.35 ui-monospace, SFMono-Regular, Menlo, monospace; }}
  header {{ position: sticky; top: 0; background: #000e; padding: 12px 16px; border-bottom: 1px solid #222; }}
  header b {{ color: #e8dc5a; }}
  header .links {{ margin-top: 6px; }}
  header a {{ color: #9fd0ff; text-decoration: none; }}
  h2 {{ margin: 24px 16px 0; color: #fff; font-weight: 600; }}
  .grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
           gap: 14px; padding: 16px; }}
  figure {{ margin: 0; background: #000; border: 1px solid #1c1c1c; border-radius: 6px; overflow: hidden; }}
  figure img {{ display: block; width: 100%; aspect-ratio: 1; {art_css} }}
  figcaption {{ padding: 8px 10px; border-top: 1px solid #1c1c1c; }}
  figcaption b {{ color: #fff; }}
  .s {{ color: #8a8a8a; }}
</style>
<header>bitneedle cut sweep — <b>{nav}</b> · 15 s → 6 min</header>
{sections}
"#
    );
    let page_path = out_dir.join("index.html");
    std::fs::write(&page_path, page)?;
    println!("\nwrote {}", page_path.display());

    Ok(())
}

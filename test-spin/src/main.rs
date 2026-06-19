//! record-test — verbose Bitneedle PNG, BRD1, BRS1 and payload inspector.
//!
//! Usage:
//!   record-test <record.png> [bundle.json] [manifest]
//!
//! `manifest` is optional because BRD1 now embeds only a binary
//! SignedReleaseReference. Supplying the external manifest allows the tool to
//! render its human-readable representation beside the reference.

use anyhow::{Context, Result};
use test_spin::{ExternalManifest, InspectionOptions};

fn main() {
    if let Err(error) = run() {
        eprintln!("\n!! record-test failed: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);

    let png_path = args
        .next()
        .context("usage: record-test <record.png> [bundle.json] [manifest]")?;
    let bundle_path = args.next();
    let manifest_path = args.next();

    let png = std::fs::read(&png_path)
        .with_context(|| format!("failed to read {png_path}"))?;

    let bundle_metadata = bundle_path
        .as_deref()
        .map(test_spin::load_bundle_metadata)
        .transpose()?;

    let manifest_bytes = manifest_path
        .as_deref()
        .map(|path| {
            std::fs::read(path)
                .with_context(|| format!("failed to read manifest {path}"))
        })
        .transpose()?;

    let mut options = InspectionOptions::verbose_defaults();
    options.png_name = Some(&png_path);
    options.bundle_metadata = bundle_metadata.as_ref();

    if let (Some(path), Some(bytes)) =
        (manifest_path.as_deref(), manifest_bytes.as_deref())
    {
        options.manifest = Some(ExternalManifest {
            name: path,
            bytes,
        });
    }

    let report = test_spin::inspect_record_png(&png, &options)?;
    print!("{report}");

    Ok(())
}

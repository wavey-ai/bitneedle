use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use record_descriptor::CACHE_ENCRYPTION_SECRET_LENGTH;
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let mut arguments = env::args_os().skip(1);
    let first = arguments
        .next()
        .map(PathBuf::from)
        .context("usage: patch_cache_encryption [--verify] <input.png> <output.png>")?;
    let verify_only = first.as_os_str() == "--verify";
    let input = if verify_only {
        arguments
            .next()
            .map(PathBuf::from)
            .context("usage: patch_cache_encryption --verify <input.png> <output.png>")?
    } else {
        first
    };
    let output = arguments
        .next()
        .map(PathBuf::from)
        .context("usage: patch_cache_encryption [--verify] <input.png> <output.png>")?;
    if arguments.next().is_some() {
        bail!("usage: patch_cache_encryption [--verify] <input.png> <output.png>");
    }
    if verify_only {
        let source =
            fs::read(&input).with_context(|| format!("failed to read {}", input.display()))?;
        let patched =
            fs::read(&output).with_context(|| format!("failed to read {}", output.display()))?;
        verify_patch(&source, &patched)?;
        println!("verified {}", output.display());
        return Ok(());
    }
    if output.exists() {
        bail!("refusing to overwrite {}", output.display());
    }

    let source = fs::read(&input).with_context(|| format!("failed to read {}", input.display()))?;
    let (_, original_descriptor) = record_decode::decode_record_descriptor_from_png(&source, None)
        .context("source record descriptor did not decode")?;
    if original_descriptor.cache_encryption().is_some() {
        bail!("source record already has a cache-encryption descriptor");
    }

    let mut secret = [0_u8; CACHE_ENCRYPTION_SECRET_LENGTH];
    getrandom::fill(&mut secret)
        .map_err(|error| anyhow::anyhow!("failed to generate cache-encryption secret: {error}"))?;
    let patched = record_sidecar::patch_record_png_cache_encryption_secret(
        &source,
        &URL_SAFE_NO_PAD.encode(secret),
        None,
    )?;
    verify_patch(&source, &patched)?;

    fs::write(&output, patched).with_context(|| format!("failed to write {}", output.display()))?;
    println!("patched {}", output.display());
    Ok(())
}

fn verify_patch(source: &[u8], patched: &[u8]) -> Result<()> {
    let (source_profile, source_descriptor) =
        record_decode::decode_record_descriptor_from_png(source, None)
            .context("source record descriptor did not decode")?;
    let (patched_profile, patched_descriptor) =
        record_decode::decode_record_descriptor_from_png(patched, None)
            .context("patched record descriptor did not decode")?;
    patched_descriptor.validate_cache_encryption()?;
    if patched_descriptor.cache_encryption().is_none() {
        bail!("patched record is missing its cache-encryption descriptor");
    }

    let mut expected_descriptor = source_descriptor.clone();
    expected_descriptor.cache_encryption = patched_descriptor.cache_encryption.clone();
    if patched_descriptor != expected_descriptor {
        bail!("patch changed descriptor fields other than cache encryption");
    }
    if patched_profile != source_profile {
        bail!("patch changed the record profile");
    }

    let (_, source_stream) = record_decode::decode_record_png_to_chunk_stream(source)
        .context("source record payload did not decode")?;
    let (_, patched_stream) = record_decode::decode_record_png_to_chunk_stream(patched)
        .context("patched record payload did not decode")?;
    if patched_stream.bytes != source_stream.bytes {
        bail!("patch changed the encoded audio payload");
    }

    if source_descriptor.bsc_pointer.is_some() {
        let source_sidecar = record_sidecar::decode_record_png_sidecar_bytes(source, None)
            .context("source sidecar did not decode")?;
        let patched_sidecar = record_sidecar::decode_record_png_sidecar_bytes(patched, None)
            .context("patched sidecar did not decode")?;
        if patched_sidecar != source_sidecar {
            bail!("patch changed the embedded sidecar");
        }
    }
    Ok(())
}

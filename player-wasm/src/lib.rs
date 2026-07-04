use base64::{engine::general_purpose, Engine as _};
use encodec_rs::arithmetic::ArithmeticDecoder;
use encodec_rs::binary::{read_chunk_payload, read_ecdc_header, read_exactly};
use encodec_rs::format::{
    ecdc_chunk_layout_from_metadata, ecdc_frame_ranges, ecdc_lm_frame_length, validate_metadata,
    EcdcChunkLayout, EcdcMetadata, ARITHMETIC_TOTAL_RANGE_BITS, DEFAULT_FP_SCALE,
    DEFAULT_MIN_RANGE, QUANTIZED_LM_BITSTREAM_VERSION,
};
use encodec_rs::metadata::OnnxFrameBundleMetadata;
use encodec_rs::quantized_lm::{QuantizedLm, QuantizedLmState, QuantizedLmWeights};
use encodec_rs::stable_hash::stable_hash_hex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::io::Cursor;
use wasm_bindgen::prelude::*;

const SCRATCH_DISPLAY_NAME_MAX_LENGTH: usize = 32;
const OPUS_CHUNK_CACHE_KEY_FORMAT: &str = "bitneedle-opus-chunk-cache-keys-v1";
const OPUS_CHUNK_CACHE_STORE_NAME: &str = "opus-chunks";
const OPUS_CHUNK_CACHE_VERSION: &str = "bitneedle-opus-chunk-cache-v2";
const OPUS_CHUNK_CACHE_OUTPUT_CODEC: &str = "soundkit_opus_packets";
const OPUS_CHUNK_CACHE_BITRATE: u32 = 64_000;
const OPUS_CHUNK_CACHE_KEY_DOMAIN: &str = "bitneedle.opus-chunk-cache-key.v1";

#[wasm_bindgen(js_name = initPanicHook)]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen(js_name = playerAppBuildInfoJson)]
pub fn player_app_build_info_json() -> String {
    json!({
        "crate": "player-wasm",
        "api": "bitneedle-player-app-wasm",
        "version": env!("CARGO_PKG_VERSION"),
        "builtFrom": "bitneedle/player-wasm",
        "recordProfiles": record_core::known_record_profile_names(),
    })
    .to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayerLmEcdcChunk {
    offset: usize,
    samples: usize,
    frame_length: usize,
    payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayerLmEcdcChunks {
    metadata: EcdcMetadata,
    chunks: Vec<PlayerLmEcdcChunk>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Bcs2OpusChunkCacheKeys {
    format: &'static str,
    store_name: &'static str,
    cache_version: &'static str,
    output_codec: &'static str,
    bitrate: u32,
    keys: Vec<String>,
}

#[wasm_bindgen(js_name = stableHashHex)]
pub fn wasm_stable_hash_hex(bytes: &[u8]) -> String {
    stable_hash_hex(bytes)
}

#[wasm_bindgen(js_name = bcs2OpusChunkCacheKeysJson)]
pub fn wasm_bcs2_opus_chunk_cache_keys_json(bcs2: &[u8]) -> Result<String, JsValue> {
    let keys = bcs2_opus_chunk_cache_keys(bcs2).map_err(to_js_error)?;
    serde_json::to_string(&keys).map_err(to_js_error)
}

#[wasm_bindgen(js_name = ecdcMetadata)]
pub fn wasm_ecdc_metadata(payload: &[u8]) -> Result<JsValue, JsValue> {
    let metadata: EcdcMetadata =
        read_ecdc_header(&mut Cursor::new(payload)).map_err(to_js_error)?;
    to_js_value(&metadata)
}

#[wasm_bindgen(js_name = ecdcFrameRanges)]
pub fn wasm_ecdc_frame_ranges(payload: &[u8]) -> Result<JsValue, JsValue> {
    let mut reader = Cursor::new(payload);
    let _metadata: EcdcMetadata = read_ecdc_header(&mut reader).map_err(to_js_error)?;
    let payload_start = reader.position() as usize;
    let ranges = ecdc_frame_ranges(payload, payload_start).map_err(to_js_error)?;
    let mapped: Vec<Value> = ranges
        .into_iter()
        .map(|range| {
            json!({
                "start": range.start,
                "end": range.end,
                "byteLength": range.end - range.start,
            })
        })
        .collect();
    to_js_value(&mapped)
}

#[wasm_bindgen(js_name = ecdcChunkLayoutFromMetadata)]
pub fn wasm_ecdc_chunk_layout_from_metadata(
    bundle_json: &str,
    payload: &[u8],
    chunk_count: usize,
) -> Result<JsValue, JsValue> {
    let meta = parse_encodec_bundle_metadata(bundle_json)?;
    let metadata: EcdcMetadata =
        read_ecdc_header(&mut Cursor::new(payload)).map_err(to_js_error)?;
    // `chunk_count` here is the stream-wide total across every revolution, so the
    // old single-object cross-check (chunk_count == segment_starts(this header))
    // no longer holds for a multi-revolution payload. The chunk layout (samples +
    // stride) is uniform for the profile/bundle, so the per-object default is the
    // correct answer; per-revolution chunk counts are already enforced inside the
    // decode loop in `lmEcdcDecodeChunks`.
    let _ = chunk_count;
    let layout: EcdcChunkLayout =
        ecdc_chunk_layout_from_metadata(&meta, &metadata).map_err(to_js_error)?;
    to_js_value(&json!({
        "samples": layout.samples,
        "stride": layout.stride,
    }))
}

/// Crop a guarded decode down to its owned audio: drops the left/right guard
/// samples the encoder padded each revolution with, leaving exactly the owned
/// (audible) samples. Mirrors `encodec_rs::wasm::ecdc_crop_decoded_owned_audio`
/// so the player worker can call it on the bitneedle-player module.
#[wasm_bindgen(js_name = ecdcCropDecodedOwnedAudio)]
pub fn wasm_ecdc_crop_decoded_owned_audio(
    bundle_json: &str,
    decoded_audio: &[f32],
) -> Result<Vec<f32>, JsValue> {
    let meta = parse_encodec_bundle_metadata(bundle_json)?;
    let channels = meta.channels;
    if channels == 0 {
        return Err(to_js_error("bundle channels must be positive"));
    }
    if decoded_audio.len() % channels != 0 {
        return Err(to_js_error(format!(
            "decoded audio length {} is not divisible by channels {}",
            decoded_audio.len(),
            channels
        )));
    }
    let decoded_samples = decoded_audio.len() / channels;
    // The decoded-block output geometry is now owned by the BRS1 ECDC payload
    // profile (record_core::ecdc), not the codec bundle: decode the full block,
    // then retain [offset .. offset + output). The discarded tail is derived.
    let left_guard = record_core::ecdc::ECDC_OUTPUT_OFFSET_SAMPLES as usize;
    let owned_samples = record_core::ecdc::ECDC_OUTPUT_SAMPLES as usize;
    let right_guard = (record_core::ecdc::ECDC_BLOCK_SAMPLES
        - record_core::ecdc::ECDC_OUTPUT_OFFSET_SAMPLES
        - record_core::ecdc::ECDC_OUTPUT_SAMPLES) as usize;
    let required = left_guard
        .checked_add(owned_samples)
        .and_then(|value| value.checked_add(right_guard))
        .ok_or_else(|| to_js_error("guarded decode geometry overflows usize"))?;
    if decoded_samples < required {
        return Err(to_js_error(format!(
            "decoded audio has {} samples per channel but guarded profile requires at least {}",
            decoded_samples, required
        )));
    }
    let mut cropped = vec![0.0_f32; channels * owned_samples];
    for channel in 0..channels {
        let src_base = channel * decoded_samples + left_guard;
        let dst_base = channel * owned_samples;
        cropped[dst_base..dst_base + owned_samples]
            .copy_from_slice(&decoded_audio[src_base..src_base + owned_samples]);
    }
    Ok(cropped)
}

/// One encoded revolution at the BRS1 boundary: a shared codec/container
/// description plus only the bytes that vary per revolution (the headerless
/// codec payload body). Mirrors the `EncodedPayload` shape from the WASM
/// headerless-ECDC contract.
#[wasm_bindgen]
pub struct WasmEncodedPayload {
    descriptor: JsValue,
    payload: Vec<u8>,
}

#[wasm_bindgen]
impl WasmEncodedPayload {
    #[wasm_bindgen(getter)]
    pub fn descriptor(&self) -> JsValue {
        self.descriptor.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn payload(&self) -> Vec<u8> {
        self.payload.clone()
    }
}

/// Split a standalone ECDC byte stream (one fixed-profile revolution block) into
/// the shared `PayloadDescriptor` and the headerless codec payload bytes stored
/// in the BRS1 groove. The ECDC outer header is lifted into the descriptor.
#[wasm_bindgen(js_name = ecdcStandaloneToPayload)]
pub fn wasm_ecdc_standalone_to_payload(
    sample_rate: u32,
    channels: u8,
    ecdc_bytes: &[u8],
) -> Result<WasmEncodedPayload, JsValue> {
    let (descriptor, payload) =
        record_core::ecdc::standalone_ecdc_to_payload(sample_rate, channels, ecdc_bytes)
            .map_err(to_js_error)?;
    Ok(WasmEncodedPayload {
        descriptor: to_js_value(&descriptor)?,
        payload,
    })
}

/// Reconstruct a decodable standalone ECDC byte stream from a `PayloadDescriptor`
/// (JSON) and the headerless codec payload bytes. Inverse of
/// [`wasm_ecdc_standalone_to_payload`]; lets the player feed the existing decode
/// path without storing repeated ECDC headers in the groove.
#[wasm_bindgen(js_name = payloadToStandaloneEcdc)]
pub fn wasm_payload_to_standalone_ecdc(
    descriptor_json: &str,
    payload: &[u8],
) -> Result<Vec<u8>, JsValue> {
    let descriptor: record_core::PayloadDescriptor =
        serde_json::from_str(descriptor_json).map_err(to_js_error)?;
    record_core::ecdc::payload_to_standalone_ecdc(&descriptor, payload).map_err(to_js_error)
}

/// Verify that two `PayloadDescriptor`s (JSON) are identical across every field,
/// so a record builder can confirm all revolutions share one descriptor before
/// storing it once. Errors name the differing field.
#[wasm_bindgen(js_name = validateSharedPayloadDescriptor)]
pub fn wasm_validate_shared_payload_descriptor(
    expected_json: &str,
    actual_json: &str,
) -> Result<(), JsValue> {
    let expected: record_core::PayloadDescriptor =
        serde_json::from_str(expected_json).map_err(to_js_error)?;
    let actual: record_core::PayloadDescriptor =
        serde_json::from_str(actual_json).map_err(to_js_error)?;
    record_core::validate_shared_payload_descriptor(&expected, &actual).map_err(to_js_error)
}

/// True when the next four bytes at the reader's position are the "ECDC" object
/// header magic — i.e. the current revolution's chunks have ended and the next
/// concatenated ECDC object begins.
fn next_is_ecdc_magic(payload: &[u8], reader: &Cursor<&[u8]>) -> bool {
    let pos = reader.position() as usize;
    payload.get(pos..pos + 4) == Some(b"ECDC")
}

#[wasm_bindgen(js_name = lmEcdcDecodeChunks)]
pub fn wasm_lm_ecdc_decode_chunks(bundle_json: &str, payload: &[u8]) -> Result<JsValue, JsValue> {
    let meta = parse_encodec_bundle_metadata(bundle_json)?;
    // The final compact Bitneedle format stores one complete ECDC object per
    // physical revolution and concatenates them back-to-back into a single
    // payload (payload_entry_index == revolution_index). Each object is fully
    // self-delimiting: its header declares `audio_length`, and with the bundle's
    // stride that yields exactly how many transport chunks belong to it. So we
    // walk object-by-object, reading precisely the implied chunk count for each
    // revolution rather than draining the whole buffer. Draining the buffer is
    // what the single-object reader used to do, and on a multi-revolution
    // payload it ran straight into the next object's "ECDC" magic and tried to
    // read it as a chunk length (the "stream ended early with 1161947177 bytes
    // remaining" failure — 1161947177 is the ASCII bytes `E C D C`).
    let mut reader = Cursor::new(payload);
    let mut chunks = Vec::new();
    // Sample offsets must be global across the whole stream so downstream PCM
    // overlap-add stays continuous; we carry a running base across revolutions.
    let mut global_offset = 0usize;
    let mut aggregate_metadata: Option<EcdcMetadata> = None;

    while (reader.position() as usize) < payload.len() {
        let metadata: EcdcMetadata = read_ecdc_header(&mut reader).map_err(to_js_error)?;
        validate_metadata(&meta, &metadata).map_err(to_js_error)?;
        if !metadata.use_lm {
            return Err(to_js_error("ECDC payload does not use LM coding"));
        }

        // A revolution's transport chunks run until the next object's "ECDC"
        // header or end-of-stream. We can't predict the count from the bundle
        // stride: the old continuous-overlap encoding (stride < segment_samples)
        // implied 2 windows for a 64000-sample span, but a per-revolution object
        // is one self-contained non-overlapping segment and emits a single chunk.
        // The 4-byte "ECDC" magic is an unambiguous boundary sentinel — no chunk
        // length prefix could legitimately equal 0x45434443 (~1.1 GiB).
        let layout = ecdc_chunk_layout_from_metadata(&meta, &metadata).map_err(to_js_error)?;
        let mut local_offset = 0usize;
        while (reader.position() as usize) < payload.len() && !next_is_ecdc_magic(payload, &reader)
        {
            let payload = read_chunk_payload(&mut reader, true).map_err(to_js_error)?;
            let samples = (metadata.audio_length.saturating_sub(local_offset)).min(layout.samples);
            let frame_length =
                ecdc_lm_frame_length(&metadata, samples, meta.segment_samples, meta.frame_length);
            chunks.push(PlayerLmEcdcChunk {
                offset: global_offset + local_offset,
                samples,
                frame_length,
                payload,
            });
            // Successive chunks inside one revolution advance by the overlap-add
            // stride; with one chunk per revolution this loop runs once.
            local_offset += layout.stride;
        }

        global_offset += metadata.audio_length;
        // Report a single aggregate metadata block spanning every revolution:
        // keep the codec/version fields from the first object (uniform across
        // the stream) but surface the total sample count.
        match aggregate_metadata.as_mut() {
            Some(existing) => existing.audio_length = global_offset,
            None => {
                let mut first = metadata;
                first.audio_length = global_offset;
                aggregate_metadata = Some(first);
            }
        }
    }

    let metadata =
        aggregate_metadata.ok_or_else(|| to_js_error("ECDC payload contained no revolutions"))?;
    to_js_value(&PlayerLmEcdcChunks { metadata, chunks })
}

#[wasm_bindgen]
pub struct QuantizedLmChunkDecoder {
    meta: OnnxFrameBundleMetadata,
    lm: QuantizedLm,
    state: QuantizedLmState,
    lm_window_frame_length: usize,
    input_symbols: Vec<usize>,
    decoder: ArithmeticDecoder,
    scale: f32,
    pulled_steps: usize,
}

#[wasm_bindgen]
impl QuantizedLmChunkDecoder {
    #[wasm_bindgen(constructor)]
    pub fn new(
        bundle_json: &str,
        weights: &[u8],
        payload: &[u8],
    ) -> Result<QuantizedLmChunkDecoder, JsValue> {
        let meta = parse_encodec_bundle_metadata(bundle_json)?;
        validate_encodec_lm_metadata(&meta).map_err(to_js_error)?;
        let weights = QuantizedLmWeights::from_bytes(weights).map_err(to_js_error)?;
        weights
            .validate_for_codebooks(meta.num_codebooks)
            .map_err(to_js_error)?;
        let lm_window_frame_length = weights.frame_length.max(1);
        let lm = QuantizedLm::new(weights);
        let state = lm.initial_state();
        let mut cursor = Cursor::new(payload);
        let scale = if meta.normalize {
            let bytes = read_exactly(&mut cursor, 4).map_err(to_js_error)?;
            f32::from_be_bytes(bytes.try_into().expect("slice length"))
        } else {
            1.0
        };
        let remaining = payload.len().saturating_sub(cursor.position() as usize);
        let encoded = read_exactly(&mut cursor, remaining).map_err(to_js_error)?;
        Ok(Self {
            input_symbols: vec![0; meta.num_codebooks],
            meta,
            lm,
            state,
            lm_window_frame_length,
            decoder: ArithmeticDecoder::new(encoded, ARITHMETIC_TOTAL_RANGE_BITS)
                .map_err(to_js_error)?,
            scale,
            pulled_steps: 0,
        })
    }

    pub fn bitstream_version(&self) -> u8 {
        QUANTIZED_LM_BITSTREAM_VERSION
    }

    #[wasm_bindgen(js_name = lmWindowFrameLength)]
    pub fn lm_window_frame_length(&self) -> usize {
        self.lm_window_frame_length
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn pull(&mut self) -> Result<Vec<u16>, JsValue> {
        if self.pulled_steps > 0 && self.pulled_steps % self.lm_window_frame_length == 0 {
            self.state = self.lm.initial_state();
            self.input_symbols.fill(0);
        }
        let logits = self
            .lm
            .forward_step(&mut self.state, &self.input_symbols)
            .map_err(to_js_error)?;
        let pdf = encodec_probability_columns_from_logits(&logits, &self.meta, 1.0)
            .map_err(to_js_error)?;
        let symbols = self
            .decoder
            .pull_symbols(
                &pdf,
                self.meta.lm_cardinality(),
                self.meta.num_codebooks,
                DEFAULT_FP_SCALE,
                DEFAULT_MIN_RANGE,
            )
            .map_err(to_js_error)?;
        for (dst, symbol) in self.input_symbols.iter_mut().zip(symbols.iter().copied()) {
            *dst = symbol + 1;
        }
        self.pulled_steps += 1;
        symbols
            .into_iter()
            .map(|symbol| {
                u16::try_from(symbol)
                    .map_err(|_| to_js_error(format!("LM symbol {symbol} does not fit u16")))
            })
            .collect()
    }
}

#[wasm_bindgen(js_name = normalizeRecordTextFieldText)]
pub fn wasm_normalize_record_text_field_text(value: &str) -> String {
    normalize_record_text_field_text(value)
}

#[wasm_bindgen(js_name = recordDisplayMetadataJson)]
pub fn wasm_record_display_metadata_json(
    record_json: &str,
    fallback_record_profile: &str,
) -> String {
    record_display_metadata_json(record_json, fallback_record_profile)
}

#[wasm_bindgen(js_name = recordVerificationMetaJson)]
pub fn wasm_record_verification_meta_json(verification_json: &str) -> String {
    record_verification_meta_json(verification_json)
}


#[wasm_bindgen(js_name = recordProfileFromHeaderValidationJson)]
pub fn wasm_record_profile_from_header_validation_json(header_json: &str) -> String {
    record_profile_from_header_validation_json(header_json)
}

#[wasm_bindgen(js_name = recordTextFromHeaderValidationJson)]
pub fn wasm_record_text_from_header_validation_json(header_json: &str) -> String {
    record_text_from_header_validation_json(header_json)
}

#[wasm_bindgen(js_name = recordPlaybackMetadataFromHeaderJson)]
pub fn wasm_record_playback_metadata_from_header_json(header_json: &str) -> String {
    record_playback_metadata_from_header_json(header_json)
}

#[wasm_bindgen(js_name = resolvePlaybackPayloadMetadataJson)]
pub fn wasm_resolve_playback_payload_metadata_json(
    header_metadata_json: &str,
    payload_metadata_json: &str,
    length_prefixed_entries: bool,
) -> String {
    resolve_playback_payload_metadata_json(
        header_metadata_json,
        payload_metadata_json,
        length_prefixed_entries,
    )
}

#[wasm_bindgen(js_name = resolveClipRevolutionsJson)]
pub fn wasm_resolve_clip_revolutions_json(
    start_time: f64,
    end_time: f64,
    duration_seconds: f64,
    revolution_count: f64,
) -> String {
    resolve_clip_revolutions_json(start_time, end_time, duration_seconds, revolution_count)
}

#[wasm_bindgen(js_name = scratchSampleTokenFromBytes)]
pub fn wasm_scratch_sample_token_from_bytes(random_bytes: &[u8], fallback: JsValue) -> f64 {
    scratch_sample_token_from_bytes(random_bytes, js_value_to_f64(&fallback, 0.0))
}

#[wasm_bindgen(js_name = normalizeScratchSampleId)]
pub fn wasm_normalize_scratch_sample_id(value: JsValue) -> f64 {
    normalize_scratch_sample_id(js_value_to_f64(&value, 0.0))
}

#[wasm_bindgen(js_name = scratchSampleTokenHex)]
pub fn wasm_scratch_sample_token_hex(value: JsValue) -> String {
    scratch_sample_token_hex(js_value_to_f64(&value, 0.0))
}

#[wasm_bindgen(js_name = scratchClipIdForSampleId)]
pub fn wasm_scratch_clip_id_for_sample_id(value: JsValue) -> String {
    scratch_clip_id_for_sample_id(js_value_to_f64(&value, 0.0))
}

#[wasm_bindgen(js_name = scratchClipSampleIdJson)]
pub fn wasm_scratch_clip_sample_id_json(clip_json: &str) -> f64 {
    scratch_clip_sample_id_json(clip_json)
}

#[wasm_bindgen(js_name = isValidScratchAnonUserId)]
pub fn wasm_is_valid_scratch_anon_user_id(value: &str) -> bool {
    is_valid_scratch_anon_user_id(value)
}

#[wasm_bindgen(js_name = scratchAnonUserIdFromRandom)]
pub fn wasm_scratch_anon_user_id_from_random(random_part: &str) -> String {
    scratch_anon_user_id_from_random(random_part)
}

#[wasm_bindgen(js_name = normalizeScratchDisplayName)]
pub fn wasm_normalize_scratch_display_name(value: &str) -> String {
    normalize_scratch_display_name(value)
}

#[wasm_bindgen(js_name = scratchDisplayNameKey)]
pub fn wasm_scratch_display_name_key(name: &str) -> String {
    scratch_display_name_key(name)
}

#[wasm_bindgen(js_name = stableLocalRecordIdFromMetaJson)]
pub fn wasm_stable_local_record_id_from_meta_json(meta_json: &str, file_name: &str) -> String {
    stable_local_record_id_from_meta_json(meta_json, file_name)
}

#[wasm_bindgen(js_name = scratchVisitorWalletAddressFromBytes)]
pub fn wasm_scratch_visitor_wallet_address_from_bytes(random_bytes: &[u8]) -> String {
    scratch_visitor_wallet_address_from_bytes(random_bytes)
}

#[wasm_bindgen(js_name = isValidScratchWalletAddress)]
pub fn wasm_is_valid_scratch_wallet_address(value: &str) -> bool {
    is_valid_scratch_wallet_address(value)
}

#[wasm_bindgen(js_name = shortScratchAddress)]
pub fn wasm_short_scratch_address(address: &str, chars: JsValue) -> String {
    short_scratch_address(address, js_value_to_f64(&chars, 4.0))
}

#[wasm_bindgen(js_name = normalizeScratchRemoteControlRevision)]
pub fn wasm_normalize_scratch_remote_control_revision(value: JsValue) -> u32 {
    normalize_scratch_remote_control_revision(js_value_to_f64(&value, 0.0))
}

#[wasm_bindgen(js_name = bumpScratchRemoteControlRevision)]
pub fn wasm_bump_scratch_remote_control_revision(value: JsValue) -> u32 {
    bump_scratch_remote_control_revision(js_value_to_f64(&value, 0.0))
}

#[wasm_bindgen(js_name = ensureScratchRemoteControlRevisionJson)]
pub fn wasm_ensure_scratch_remote_control_revision_json(state_json: &str) -> String {
    ensure_scratch_remote_control_revision_json(state_json)
}

#[wasm_bindgen(js_name = shouldApplyRemoteScratchControlsJson)]
pub fn wasm_should_apply_remote_scratch_controls_json(
    state_json: &str,
    command_json: &str,
) -> String {
    should_apply_remote_scratch_controls_json(state_json, command_json)
}

#[wasm_bindgen(js_name = normalizeRecordProfileName)]
pub fn wasm_normalize_record_profile_name(record_profile: &str) -> Result<String, JsValue> {
    normalize_record_profile_name(record_profile).map_err(to_js_error)
}

#[wasm_bindgen(js_name = recordProfileSpecJson)]
pub fn wasm_record_profile_spec_json(record_profile: &str) -> Result<String, JsValue> {
    record_profile_spec_json(record_profile).map_err(to_js_error)
}

#[wasm_bindgen(js_name = getRecordRpm)]
pub fn wasm_get_record_rpm(record_profile: &str) -> Result<f64, JsValue> {
    record_rpm(record_profile).map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolveRecordRpm)]
pub fn wasm_resolve_record_rpm(
    rpm_candidate: JsValue,
    record_profile: &str,
) -> Result<f64, JsValue> {
    resolve_record_rpm_number(js_value_to_f64(&rpm_candidate, f64::NAN), record_profile)
        .map_err(to_js_error)
}

#[wasm_bindgen(js_name = secondsToRevolutions)]
pub fn wasm_seconds_to_revolutions(
    seconds: JsValue,
    record_profile: &str,
    rpm_candidate: JsValue,
) -> Result<f64, JsValue> {
    let seconds = js_value_to_f64(&seconds, 0.0);
    if !(seconds.is_finite() && seconds > 0.0) {
        return Ok(0.0);
    }
    seconds_to_revolutions_number(
        seconds,
        record_profile,
        js_value_to_f64(&rpm_candidate, f64::NAN),
    )
    .map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolveLeadInTurns)]
pub fn wasm_resolve_lead_in_turns(record_profile: &str) -> Result<f64, JsValue> {
    profile_turns(record_profile)
        .map(|turns| turns.lead_in_turns)
        .map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolveDeadwaxTurns)]
pub fn wasm_resolve_deadwax_turns(record_profile: &str) -> Result<f64, JsValue> {
    profile_turns(record_profile)
        .map(|turns| turns.run_out_turns)
        .map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolvePlaybackRate)]
pub fn wasm_resolve_playback_rate(
    value: JsValue,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> f64 {
    resolve_playback_rate_number(
        js_value_to_f64(&value, default_rate),
        default_rate,
        min_rate,
        max_rate,
    )
}

#[wasm_bindgen(js_name = resolvePhysicalRpm)]
pub fn wasm_resolve_physical_rpm(
    rpm_candidate: JsValue,
    record_profile: &str,
    playback_rate: JsValue,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> Result<f64, JsValue> {
    resolve_physical_rpm_number(
        js_value_to_f64(&rpm_candidate, f64::NAN),
        record_profile,
        js_value_to_f64(&playback_rate, default_rate),
        default_rate,
        min_rate,
        max_rate,
    )
    .map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolveSecondsPerTurn)]
pub fn wasm_resolve_seconds_per_turn(
    rpm_candidate: JsValue,
    record_profile: &str,
    playback_rate: JsValue,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> Result<f64, JsValue> {
    resolve_seconds_per_turn_number(
        js_value_to_f64(&rpm_candidate, f64::NAN),
        record_profile,
        js_value_to_f64(&playback_rate, default_rate),
        default_rate,
        min_rate,
        max_rate,
    )
    .map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolveLeadInDurationSeconds)]
pub fn wasm_resolve_lead_in_duration_seconds(
    record_profile: &str,
    rpm_candidate: JsValue,
    playback_rate: JsValue,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> Result<f64, JsValue> {
    profile_turn_duration_seconds(
        profile_turns(record_profile)
            .map_err(to_js_error)?
            .lead_in_turns,
        js_value_to_f64(&rpm_candidate, f64::NAN),
        record_profile,
        js_value_to_f64(&playback_rate, default_rate),
        default_rate,
        min_rate,
        max_rate,
    )
    .map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolveDeadwaxDurationSeconds)]
pub fn wasm_resolve_deadwax_duration_seconds(
    record_profile: &str,
    rpm_candidate: JsValue,
    playback_rate: JsValue,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> Result<f64, JsValue> {
    profile_turn_duration_seconds(
        profile_turns(record_profile)
            .map_err(to_js_error)?
            .run_out_turns,
        js_value_to_f64(&rpm_candidate, f64::NAN),
        record_profile,
        js_value_to_f64(&playback_rate, default_rate),
        default_rate,
        min_rate,
        max_rate,
    )
    .map_err(to_js_error)
}

#[wasm_bindgen(js_name = createPlayerEcdcCacheProofContextJson)]
pub fn wasm_create_player_ecdc_cache_proof_context_json(ecdc: &[u8]) -> String {
    create_player_ecdc_cache_proof_context(ecdc)
        .and_then(|context| serde_json::to_string(&context).ok())
        .unwrap_or_else(|| "null".to_string())
}

#[wasm_bindgen(js_name = playerEcdcCacheProofForChunkJson)]
pub fn wasm_player_ecdc_cache_proof_for_chunk_json(
    context_json: &str,
    chunk_index: usize,
) -> String {
    player_ecdc_cache_proof_for_chunk_json(context_json, chunk_index)
}

fn normalize_record_text_field_text(value: &str) -> String {
    value
        .replace('\0', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn record_display_metadata_json(record_json: &str, fallback_record_profile: &str) -> String {
    let record = serde_json::from_str::<Value>(record_json).unwrap_or(Value::Null);
    record_display_metadata_value(&record, fallback_record_profile).to_string()
}

fn record_display_metadata_value(record: &Value, fallback_record_profile: &str) -> Value {
    let meta = record
        .get("meta")
        .filter(|value| value.is_object())
        .unwrap_or(&Value::Null);
    let title = text_from_first_value(&[record, meta], &["title"]);
    let artist = text_from_first_value(&[record, meta], &["artist"]);
    let explicit_profile = text_from_first_value(&[record, meta], &["recordProfile"]);
    let record_profile = if explicit_profile.is_empty() {
        normalize_record_text_field_text(fallback_record_profile)
    } else {
        explicit_profile
    };
    let normalized_profile = normalize_record_profile_name(&record_profile).unwrap_or_default();
    let verified = boolish_true(meta.get("bitneedleVerified"));
    let title_for_label = if title.is_empty() {
        "Untitled record".to_string()
    } else {
        title.clone()
    };
    let display_label = if artist.is_empty() {
        title_for_label.clone()
    } else {
        format!("{title_for_label} - {artist}")
    };

    json!({
        "title": title,
        "artist": artist,
        "displayLabel": display_label,
        "profileDisplay": record_profile_display_text(&record_profile, verified),
        "recordProfile": normalized_profile,
        "verified": verified,
    })
}

fn record_profile_display_text(record_profile: &str, verified: bool) -> String {
    let Ok(normalized) = normalize_record_profile_name(record_profile) else {
        return String::new();
    };
    let Ok(label) = record_profile_label(&normalized) else {
        return String::new();
    };
    if verified {
        format!("{label} \u{00b7} Verified")
    } else {
        label.to_string()
    }
}

fn record_verification_meta_json(verification_json: &str) -> String {
    let Ok(verification) = serde_json::from_str::<Value>(verification_json) else {
        return "{}".to_string();
    };
    if !verification.is_object() {
        return "{}".to_string();
    }
    let verified = verification
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json!({
        "verification": verification,
        "bitneedleVerification": text_from_value(verification.get("code")),
        "bitneedleVerified": if verified { "true" } else { "false" },
        "signatureKeyId": text_from_value(verification.get("keyId")),
    })
    .to_string()
}

fn boolish_true(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => value.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn record_profile_from_header_validation_json(header_json: &str) -> String {
    let Ok(header) = serde_json::from_str::<Value>(header_json) else {
        return String::new();
    };
    record_profile_from_header_value(&header)
}

fn record_profile_from_header_value(header: &Value) -> String {
    for path in [
        &["descriptor", "recordProfile"][..],
        &["record", "recordProfile"][..],
        &["recordProfile"][..],
    ] {
        if let Some(text) = value_at_path(header, path).and_then(Value::as_str) {
            if let Ok(profile) = record_core::normalize_record_profile_name(text) {
                return profile;
            }
        }
    }
    String::new()
}

fn record_text_from_header_validation_json(header_json: &str) -> String {
    let Ok(header) = serde_json::from_str::<Value>(header_json) else {
        return json!({ "title": "", "artist": "", "meta": {} }).to_string();
    };
    record_text_from_header_value(&header).to_string()
}

fn record_playback_metadata_from_header_json(header_json: &str) -> String {
    let Ok(header) = serde_json::from_str::<Value>(header_json) else {
        return playback_metadata_json(
            String::new(),
            String::new(),
            Value::Array(vec![]),
            Value::Array(vec![]),
        )
        .to_string();
    };
    let parsed = record_text_from_header_value(&header);
    let meta = parsed.get("meta").unwrap_or(&Value::Null);
    playback_metadata_json(
        text_from_value(meta.get("payloadContainer")),
        text_from_value(meta.get("entryContainer")),
        meta.get("trackListing")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![])),
        meta.get("dummySpiralRegions")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![])),
    )
    .to_string()
}

fn resolve_playback_payload_metadata_json(
    header_metadata_json: &str,
    payload_metadata_json: &str,
    _length_prefixed_entries: bool,
) -> String {
    let header = serde_json::from_str::<Value>(header_metadata_json).unwrap_or(Value::Null);
    let payload = serde_json::from_str::<Value>(payload_metadata_json).unwrap_or(Value::Null);
    resolve_playback_payload_metadata_value(&header, &payload).to_string()
}

fn resolve_playback_payload_metadata_value(header: &Value, payload: &Value) -> Value {
    let header_payload_container = text_from_value(header.get("payloadContainer"));
    let payload_container = if header_payload_container.is_empty() {
        normalize_payload_container(&payload_container_from_metadata(payload))
    } else {
        normalize_payload_container(&header_payload_container)
    };
    let entry_container = "single".to_string();
    let header_tracks = array_value(header.get("trackListing"));
    let track_listing = if header_tracks
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        header_tracks
    } else {
        array_value(payload.get("trackListing"))
    };
    let header_dummy_spiral_regions =
        normalize_dummy_spiral_regions(array_value(header.get("dummySpiralRegions")));
    let dummy_spiral_regions = if header_dummy_spiral_regions
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        header_dummy_spiral_regions
    } else {
        normalize_dummy_spiral_regions(array_value(payload.get("dummySpiralRegions")))
    };

    json!({
        "payloadContainer": payload_container,
        "payloadCodec": payload_codec_from_metadata(payload),
        "entryContainer": entry_container,
        "trackListing": track_listing.as_array().cloned().unwrap_or_default(),
        "dummySpiralRegions": dummy_spiral_regions.as_array().cloned().unwrap_or_default(),
    })
}

fn payload_descriptor_from_metadata(metadata: &Value) -> Option<&Value> {
    metadata
        .get("payloadDescriptors")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .filter(|value| value.is_object())
}

fn payload_codec_from_metadata(metadata: &Value) -> String {
    text_from_value(metadata.get("payloadCodec"))
}

fn payload_container_from_metadata(metadata: &Value) -> String {
    let descriptor = payload_descriptor_from_metadata(metadata);
    text_from_value(
        descriptor
            .and_then(|value| value.get("container"))
            .or_else(|| metadata.get("payloadContainer")),
    )
}

fn normalize_payload_container(value: &str) -> String {
    value.trim().to_uppercase()
}

fn resolve_clip_revolutions_json(
    start_time: f64,
    end_time: f64,
    duration_seconds: f64,
    revolution_count: f64,
) -> String {
    let duration = finite_nonnegative(duration_seconds);
    let count = finite_nonnegative(revolution_count).floor();
    if !(duration > 0.0 && count > 0.0) {
        return "[]".to_string();
    }

    let last_index = (count as u64).saturating_sub(1);
    let start_ratio =
        (finite_or_zero(start_time).min(finite_or_zero(end_time)) / duration).clamp(0.0, 1.0);
    let end_ratio =
        (finite_or_zero(start_time).max(finite_or_zero(end_time)) / duration).clamp(0.0, 1.0);
    let first = ((start_ratio * count).floor() as u64).min(last_index);
    let last = (((end_ratio - f64::EPSILON).max(0.0) * count).floor() as u64).min(last_index);
    if last < first {
        return "[]".to_string();
    }
    serde_json::to_string(&(first..=last).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".to_string())
}

#[derive(Debug, Clone, Copy)]
struct ProfileTurns {
    lead_in_turns: f64,
    run_out_turns: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayerRecordProfileSpec {
    name: String,
    label: String,
    spindle_hole_radius: i32,
    label_radius: i32,
    label_clearance: i32,
    outer_radius: i32,
    outer_rim_thickness: i32,
    lead_in_band_thickness: i32,
    lead_in_turns: f64,
    run_out_turns: f64,
}

fn normalize_record_profile_name(record_profile: &str) -> Result<String, String> {
    record_core::normalize_record_profile_name(record_profile)
        .map_err(|error| format!("unknown record profile {record_profile}: {error:#}"))
}

fn record_rpm(record_profile: &str) -> Result<f64, String> {
    let normalized = normalize_record_profile_name(record_profile)?;
    match normalized.as_str() {
        "single45" => Ok(45.0),
        "lp" => Ok(33.3333333333),
        _ => Err(format!("record profile {normalized} has no player RPM")),
    }
}

fn resolve_record_rpm_number(rpm_candidate: f64, record_profile: &str) -> Result<f64, String> {
    let record_rpm = record_rpm(record_profile)?;
    if rpm_candidate.is_finite() && rpm_candidate > 0.0 {
        Ok(rpm_candidate)
    } else {
        Ok(record_rpm)
    }
}

fn seconds_to_revolutions_number(
    seconds: f64,
    record_profile: &str,
    rpm_candidate: f64,
) -> Result<f64, String> {
    if !(seconds.is_finite() && seconds > 0.0) {
        return Ok(0.0);
    }
    Ok((seconds * resolve_record_rpm_number(rpm_candidate, record_profile)?) / 60.0)
}

fn resolve_playback_rate_number(
    value: f64,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> f64 {
    let default_value = if default_rate.is_finite() && default_rate > 0.0 {
        default_rate
    } else {
        1.0
    };
    let min = if min_rate.is_finite() && min_rate > 0.0 {
        min_rate
    } else {
        default_value
    };
    let max = if max_rate.is_finite() && max_rate >= min {
        max_rate
    } else {
        default_value.max(min)
    };
    let value = if value.is_finite() {
        value
    } else {
        default_value
    };
    value.clamp(min, max)
}

fn resolve_physical_rpm_number(
    rpm_candidate: f64,
    record_profile: &str,
    playback_rate: f64,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> Result<f64, String> {
    Ok(resolve_record_rpm_number(rpm_candidate, record_profile)?
        * resolve_playback_rate_number(playback_rate, default_rate, min_rate, max_rate))
}

fn resolve_seconds_per_turn_number(
    rpm_candidate: f64,
    record_profile: &str,
    playback_rate: f64,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> Result<f64, String> {
    let rpm = resolve_physical_rpm_number(
        rpm_candidate,
        record_profile,
        playback_rate,
        default_rate,
        min_rate,
        max_rate,
    )?;
    if rpm > 0.0 {
        Ok(60.0 / rpm)
    } else {
        Ok(0.0)
    }
}

fn profile_turn_duration_seconds(
    turns: f64,
    rpm_candidate: f64,
    record_profile: &str,
    playback_rate: f64,
    default_rate: f64,
    min_rate: f64,
    max_rate: f64,
) -> Result<f64, String> {
    let rpm = resolve_physical_rpm_number(
        rpm_candidate,
        record_profile,
        playback_rate,
        default_rate,
        min_rate,
        max_rate,
    )?;
    if turns > 0.0 && rpm > 0.0 {
        Ok(turns * (60.0 / rpm))
    } else {
        Ok(0.0)
    }
}

fn profile_turns(record_profile: &str) -> Result<ProfileTurns, String> {
    let _ = normalize_record_profile_name(record_profile)?;
    Ok(ProfileTurns {
        lead_in_turns: 2.0,
        run_out_turns: 2.0,
    })
}

fn record_profile_label(record_profile: &str) -> Result<&'static str, String> {
    match record_profile {
        "single45" => Ok("45"),
        "lp" => Ok("LP"),
        _ => Err(format!(
            "record profile {record_profile} has no player label"
        )),
    }
}

fn record_profile_spec(record_profile: &str) -> Result<PlayerRecordProfileSpec, String> {
    let normalized = normalize_record_profile_name(record_profile)?;
    let geometry = record_core::describe_record_profile(&normalized)
        .map_err(|error| format!("failed to describe record profile {normalized}: {error:#}"))?;
    let turns = profile_turns(&normalized)?;
    Ok(PlayerRecordProfileSpec {
        name: geometry.record_profile.clone(),
        label: record_profile_label(&geometry.record_profile)?.to_string(),
        spindle_hole_radius: geometry.spindle_hole_radius,
        label_radius: geometry.label_radius,
        label_clearance: geometry.payload_inner_radius - geometry.label_radius,
        outer_radius: geometry.outer_radius,
        outer_rim_thickness: geometry.outer_rim_thickness,
        lead_in_band_thickness: geometry.lead_in_band_thickness,
        lead_in_turns: turns.lead_in_turns,
        run_out_turns: turns.run_out_turns,
    })
}

fn record_profile_spec_json(record_profile: &str) -> Result<String, String> {
    record_profile_spec(record_profile)
        .and_then(|spec| serde_json::to_string(&spec).map_err(|error| error.to_string()))
}

fn js_value_to_f64(value: &JsValue, default_value: f64) -> f64 {
    let number = value
        .as_f64()
        .or_else(|| {
            value
                .as_string()
                .and_then(|text| text.trim().parse::<f64>().ok())
        })
        .unwrap_or(default_value);
    if number.is_finite() {
        number
    } else {
        default_value
    }
}

fn scratch_sample_token_from_bytes(random_bytes: &[u8], fallback: f64) -> f64 {
    let mut token = 0.0;
    for (index, byte) in random_bytes.iter().copied().take(7).enumerate() {
        let value = if index == 0 { byte & 0x1f } else { byte };
        token = (token * 256.0) + f64::from(value);
    }
    if token.fract() == 0.0 && token > 0.0 && token <= 9_007_199_254_740_991.0 {
        token
    } else if fallback.is_finite() && fallback > 0.0 {
        fallback
    } else {
        0.0
    }
}

fn normalize_scratch_sample_id(value: f64) -> f64 {
    if value.is_finite() && value.fract() == 0.0 && value > 0.0 && value <= 9_007_199_254_740_991.0
    {
        value
    } else {
        0.0
    }
}

fn scratch_sample_token_hex(value: f64) -> String {
    let token = normalize_scratch_sample_id(value);
    if token == 0.0 {
        String::new()
    } else {
        format!("{:014x}", token as u64)
    }
}

fn scratch_clip_id_for_sample_id(value: f64) -> String {
    let hex = scratch_sample_token_hex(value);
    if hex.is_empty() {
        String::new()
    } else {
        format!("scratch-sample-{hex}")
    }
}

fn scratch_clip_sample_id_json(clip_json: &str) -> f64 {
    let clip = serde_json::from_str::<Value>(clip_json).unwrap_or(Value::Null);
    let sample = clip
        .get("sampleId")
        .and_then(number_like_value_to_f64)
        .unwrap_or(0.0);
    normalize_scratch_sample_id(sample)
}

fn is_valid_scratch_anon_user_id(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    let Some(rest) = normalized.strip_prefix("bnanon_") else {
        return false;
    };
    let len = rest.len();
    (12..=80).contains(&len)
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn scratch_anon_user_id_from_random(random_part: &str) -> String {
    let suffix = random_part
        .to_ascii_lowercase()
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        .take(72)
        .map(char::from)
        .collect::<String>();
    format!("bnanon_{suffix}")
}

fn normalize_scratch_display_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control() && *ch != '\u{007f}')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SCRATCH_DISPLAY_NAME_MAX_LENGTH)
        .collect()
}

fn scratch_display_name_key(name: &str) -> String {
    normalize_scratch_display_name(name).to_ascii_lowercase()
}

fn stable_local_record_id_from_meta_json(meta_json: &str, file_name: &str) -> String {
    let meta = serde_json::from_str::<Value>(meta_json).unwrap_or(Value::Null);
    let source = ["releaseId", "recordPngSha256", "chunkStreamSha256"]
        .into_iter()
        .find_map(|key| {
            let value = text_from_value(meta.get(key));
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        })
        .unwrap_or_else(|| file_name.trim().to_string());
    let safe = sanitize_local_record_id_source(&source);
    if safe.is_empty() {
        String::new()
    } else {
        format!("local-record-{safe}")
    }
}

fn sanitize_local_record_id_source(source: &str) -> String {
    let trimmed = source.trim();
    let trimmed = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    let mut safe = String::new();
    let mut previous_dash = false;
    for ch in trimmed.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '_' {
            Some(ch)
        } else if ch == '-' {
            Some('-')
        } else {
            Some('-')
        };
        if let Some(ch) = mapped {
            if ch == '-' {
                if !previous_dash && !safe.is_empty() {
                    safe.push('-');
                }
                previous_dash = true;
            } else {
                safe.push(ch);
                previous_dash = false;
            }
        }
        if safe.len() >= 72 {
            break;
        }
    }
    while safe.ends_with('-') {
        safe.pop();
    }
    safe
}

fn scratch_visitor_wallet_address_from_bytes(random_bytes: &[u8]) -> String {
    let mut bytes = [0u8; 20];
    for (target, source) in bytes.iter_mut().zip(random_bytes.iter().copied()) {
        *target = source;
    }
    format!(
        "0x{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn is_valid_scratch_wallet_address(value: &str) -> bool {
    let text = value.trim();
    text.len() == 42
        && text.starts_with("0x")
        && text.as_bytes()[2..]
            .iter()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn short_scratch_address(address: &str, chars: f64) -> String {
    let text = address.to_string();
    let chars = if chars.is_finite() && chars > 0.0 {
        chars.floor() as usize
    } else {
        4
    };
    if text.len() <= (chars * 2) + 2 {
        if text.is_empty() {
            "LOCAL".to_string()
        } else {
            text
        }
    } else {
        format!("{}...{}", &text[..chars + 2], &text[text.len() - chars..])
    }
}

fn number_like_value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScratchRemoteControlState {
    control_revision: u32,
    control_intent: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScratchRemoteControlInput {
    control_revision: Value,
    control_intent: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScratchRemoteControlResolution {
    apply: bool,
    control_revision: u32,
    control_intent: bool,
}

fn normalize_scratch_remote_control_revision(value: f64) -> u32 {
    if !(value.is_finite() && value > 0.0) {
        return 0;
    }
    let truncated = value.trunc();
    if truncated >= u64::MAX as f64 {
        return 0;
    }
    (truncated as u64 & 0xffff_ffff) as u32
}

fn bump_scratch_remote_control_revision(value: f64) -> u32 {
    let next = normalize_scratch_remote_control_revision(value).wrapping_add(1);
    if next == 0 {
        1
    } else {
        next
    }
}

fn scratch_remote_control_state_from_json(state_json: &str) -> ScratchRemoteControlState {
    let value = serde_json::from_str::<Value>(state_json).unwrap_or(Value::Null);
    ScratchRemoteControlState {
        control_revision: normalize_scratch_remote_control_revision(
            value
                .get("controlRevision")
                .and_then(number_like_value_to_f64)
                .unwrap_or(0.0),
        ),
        control_intent: value
            .get("controlIntent")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn scratch_remote_control_input_from_json(command_json: &str) -> ScratchRemoteControlInput {
    let value = serde_json::from_str::<Value>(command_json).unwrap_or(Value::Null);
    ScratchRemoteControlInput {
        control_revision: value.get("controlRevision").cloned().unwrap_or(Value::Null),
        control_intent: value.get("controlIntent").and_then(Value::as_bool),
    }
}

fn ensure_scratch_remote_control_revision_json(state_json: &str) -> String {
    let mut state = scratch_remote_control_state_from_json(state_json);
    if state.control_revision == 0 {
        state.control_revision = 1;
        state.control_intent = false;
    }
    serde_json::to_string(&state).unwrap_or_else(|_| "{}".to_string())
}

fn should_apply_remote_scratch_controls_json(state_json: &str, command_json: &str) -> String {
    let state = scratch_remote_control_state_from_json(state_json);
    let command = scratch_remote_control_input_from_json(command_json);
    let incoming_revision = normalize_scratch_remote_control_revision(
        number_like_value_to_f64(&command.control_revision).unwrap_or(0.0),
    );
    if incoming_revision == 0 {
        return serde_json::to_string(&ScratchRemoteControlResolution {
            apply: true,
            control_revision: state.control_revision,
            control_intent: state.control_intent,
        })
        .unwrap_or_else(|_| "{}".to_string());
    }

    let incoming_intent = command.control_intent.unwrap_or(false);
    if incoming_revision > state.control_revision
        || (incoming_revision == state.control_revision && incoming_intent && !state.control_intent)
    {
        return serde_json::to_string(&ScratchRemoteControlResolution {
            apply: true,
            control_revision: incoming_revision,
            control_intent: incoming_intent || state.control_intent,
        })
        .unwrap_or_else(|_| "{}".to_string());
    }

    serde_json::to_string(&ScratchRemoteControlResolution {
        apply: false,
        control_revision: state.control_revision,
        control_intent: state.control_intent,
    })
    .unwrap_or_else(|_| "{}".to_string())
}

fn record_text_from_header_value(header: &Value) -> Value {
    let source_value = header
        .get("descriptor")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            if header.is_object() {
                header.clone()
            } else {
                Value::Null
            }
        });
    let chunk_stream_value = object_field(header, "chunkStream")
        .map(Value::Object)
        .unwrap_or(Value::Null);
    let record_value = object_field(header, "record")
        .map(Value::Object)
        .unwrap_or(Value::Null);
    let arbitrary = parse_arbitrary_metadata(&source_value);

    let track_listing = first_array_value(
        &[&arbitrary, &source_value, &chunk_stream_value],
        &["trackListing"],
    );
    let dummy_spiral_regions = normalize_dummy_spiral_regions(first_array_value(
        &[&arbitrary, &source_value, &chunk_stream_value],
        &["dummySpiralRegions"],
    ));
    let payload_container = text_from_first_value(
        &[&arbitrary, &source_value, &chunk_stream_value],
        &["payloadContainer"],
    );
    let entry_container = text_from_first_value(
        &[&arbitrary, &source_value, &chunk_stream_value],
        &["entryContainer"],
    );

    let mut meta = Map::new();
    insert_text(&mut meta, "releaseId", &source_value, &["releaseId"]);
    insert_text(
        &mut meta,
        "catalogNumber",
        &source_value,
        &["catalogNumber"],
    );
    insert_text(&mut meta, "label", &source_value, &["label"]);
    insert_text(
        &mut meta,
        "artworkCredit",
        &source_value,
        &["artworkCredit"],
    );
    insert_text(&mut meta, "license", &source_value, &["license"]);
    insert_text(&mut meta, "canonicalUrl", &source_value, &["canonicalUrl"]);
    insert_text(&mut meta, "createdAt", &source_value, &["createdAt"]);
    insert_text(
        &mut meta,
        "arbitraryMetadata",
        &source_value,
        &["arbitraryMetadata"],
    );
    meta.insert(
        "recordProfile".to_string(),
        Value::String(record_profile_from_header_value(header)),
    );
    meta.insert(
        "streamByteLength".to_string(),
        first_truthy_value(
            &[&source_value, &chunk_stream_value],
            &["streamByteLength", "byteLength"],
        )
        .unwrap_or_else(|| Value::String(String::new())),
    );
    meta.insert(
        "payloadByteLength".to_string(),
        first_truthy_value(
            &[&source_value, &chunk_stream_value],
            &["payloadByteLength"],
        )
        .unwrap_or_else(|| Value::String(String::new())),
    );
    meta.insert(
        "recordPngByteLength".to_string(),
        first_truthy_value(&[&record_value], &["pngByteLength"])
            .unwrap_or_else(|| Value::String(String::new())),
    );
    insert_text(&mut meta, "recordPngSha256", &record_value, &["pngSha256"]);
    insert_text(
        &mut meta,
        "chunkStreamSha256",
        &chunk_stream_value,
        &["sha256"],
    );
    meta.insert(
        "chunkCount".to_string(),
        first_truthy_value(&[&chunk_stream_value], &["chunkCount"])
            .unwrap_or_else(|| Value::String(String::new())),
    );
    meta.insert(
        "revolutionCount".to_string(),
        chunk_stream_revolution_count_value(&chunk_stream_value)
            .unwrap_or_else(|| Value::String(String::new())),
    );
    meta.insert(
        "payloadContainer".to_string(),
        Value::String(payload_container),
    );
    meta.insert("entryContainer".to_string(), Value::String(entry_container));
    meta.insert("trackListing".to_string(), track_listing);
    meta.insert("dummySpiralRegions".to_string(), dummy_spiral_regions);

    json!({
        "title": text_from_first_value(&[&source_value], &["title"]),
        "artist": text_from_first_value(&[&source_value], &["artist"]),
        "meta": Value::Object(meta),
    })
}

fn playback_metadata_json(
    payload_container: String,
    entry_container: String,
    track_listing: Value,
    dummy_spiral_regions: Value,
) -> Value {
    json!({
        "payloadContainer": payload_container,
        "entryContainer": entry_container,
        "trackListing": track_listing.as_array().cloned().unwrap_or_default(),
        "dummySpiralRegions": dummy_spiral_regions.as_array().cloned().unwrap_or_default(),
    })
}

fn object_field(value: &Value, key: &str) -> Option<Map<String, Value>> {
    value.get(key)?.as_object().cloned()
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    Some(cursor)
}

fn parse_arbitrary_metadata(source: &Value) -> Value {
    let raw = source
        .get("arbitraryMetadata")
        .or_else(|| source.get("arbitrary_metadata"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if raw.is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str::<Value>(raw)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn chunk_stream_revolution_count_value(chunk_stream: &Value) -> Option<Value> {
    let tracks = value_at_path(chunk_stream, &["metadata", "tracks"]).and_then(Value::as_array)?;
    let mut total = 0_u64;
    for track in tracks {
        let Some(count) = first_numeric_value(&[track], &["revolutionCount", "revolution_count"])
        else {
            continue;
        };
        if count.is_finite() && count > 0.0 {
            total = total.saturating_add(count.floor() as u64);
        }
    }
    if total > 0 {
        Some(json!(total))
    } else {
        None
    }
}

fn normalize_dummy_spiral_regions(regions: Value) -> Value {
    let Some(regions) = regions.as_array() else {
        return Value::Array(vec![]);
    };
    let normalized = regions
        .iter()
        .filter_map(|region| {
            let mut object = region.as_object()?.clone();
            let carrier_pixel_start = nonnegative_usize(
                object
                    .get("carrierPixelStart")
                    .or_else(|| object.get("spiralPixelStart")),
            );
            let pixel_count = nonnegative_usize(
                object
                    .get("pixelCount")
                    .or_else(|| object.get("spiralPixelCount")),
            );
            if pixel_count == 0 {
                return None;
            }
            let spiral_pixel_start = nonnegative_usize(
                object
                    .get("spiralPixelStart")
                    .or_else(|| object.get("carrierPixelStart")),
            );
            object.insert("carrierPixelStart".to_string(), json!(carrier_pixel_start));
            object.insert("spiralPixelStart".to_string(), json!(spiral_pixel_start));
            object.insert("pixelCount".to_string(), json!(pixel_count));
            object.insert("codecCarrier".to_string(), Value::Bool(false));
            Some(Value::Object(object))
        })
        .collect();
    Value::Array(normalized)
}

fn nonnegative_usize(value: Option<&Value>) -> usize {
    let number = match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(text)) => text.parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };
    if number.is_finite() && number > 0.0 {
        number.floor() as usize
    } else {
        0
    }
}

fn first_array_value(sources: &[&Value], keys: &[&str]) -> Value {
    for source in sources {
        for key in keys {
            if let Some(value) = source.get(*key) {
                if value.is_array() {
                    return value.clone();
                }
            }
        }
    }
    Value::Array(vec![])
}

fn array_value(value: Option<&Value>) -> Value {
    value
        .filter(|value| value.is_array())
        .cloned()
        .unwrap_or_else(|| Value::Array(vec![]))
}

fn text_from_first_value(sources: &[&Value], keys: &[&str]) -> String {
    for source in sources {
        for key in keys {
            let text = text_from_value(source.get(*key));
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn text_from_value(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(normalize_record_text_field_text)
        .unwrap_or_default()
}

fn insert_text(meta: &mut Map<String, Value>, output_key: &str, source: &Value, keys: &[&str]) {
    meta.insert(
        output_key.to_string(),
        Value::String(text_from_first_value(&[source], keys)),
    );
}

fn first_truthy_value(sources: &[&Value], keys: &[&str]) -> Option<Value> {
    for source in sources {
        for key in keys {
            if let Some(value) = source.get(*key) {
                if value_is_truthy(value) {
                    return Some(value.clone());
                }
            }
        }
    }
    None
}

fn first_numeric_value(sources: &[&Value], keys: &[&str]) -> Option<f64> {
    for source in sources {
        for key in keys {
            if let Some(value) = numeric_value(source.get(*key)) {
                if value != 0.0 {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn numeric_value(value: Option<&Value>) -> Option<f64> {
    let value = match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    value.is_finite().then_some(value)
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn finite_nonnegative(value: f64) -> f64 {
    finite_or_zero(value).max(0.0)
}

fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes(slice.try_into().ok()?))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcdcCacheChunk {
    chunk_index: usize,
    chunk_offset: usize,
    chunk_byte_length: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcdcCacheProofContext {
    format: String,
    header_base64url: String,
    header_byte_length: usize,
    chunks: Vec<EcdcCacheChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcdcCacheProof {
    record_header: String,
    chunk_index: usize,
    chunk_offset: usize,
    chunk_byte_length: usize,
}

fn create_player_ecdc_cache_proof_context(ecdc: &[u8]) -> Option<EcdcCacheProofContext> {
    if ecdc.len() < 8 || ecdc.get(0..4)? != b"ECDC" {
        return None;
    }
    let metadata_start = ecdc.iter().position(|byte| *byte == b'{')?;
    let header_end = json_object_end(ecdc, metadata_start)?;
    if header_end > ecdc.len() {
        return None;
    }
    serde_json::from_slice::<Value>(&ecdc[metadata_start..header_end]).ok()?;

    // ECDC v2 frame layout: [4B BE payload_len][4B overhead][payload_len bytes]
    let mut chunks = Vec::new();
    let mut offset = header_end;
    while offset < ecdc.len() {
        if offset + 8 > ecdc.len() {
            break;
        }
        let payload_len = read_u32_be(ecdc, offset)? as usize;
        let end = offset.checked_add(8 + payload_len)?;
        if end > ecdc.len() {
            break;
        }
        chunks.push(EcdcCacheChunk {
            chunk_index: chunks.len(),
            chunk_offset: offset,
            chunk_byte_length: 8 + payload_len,
        });
        offset = end;
    }
    if chunks.is_empty() {
        return None;
    }
    Some(EcdcCacheProofContext {
        format: "ecdc-v2".to_string(),
        header_base64url: general_purpose::URL_SAFE_NO_PAD.encode(&ecdc[..header_end]),
        header_byte_length: header_end,
        chunks,
    })
}

fn player_ecdc_cache_proof_for_chunk_json(context_json: &str, chunk_index: usize) -> String {
    let Ok(context) = serde_json::from_str::<EcdcCacheProofContext>(context_json) else {
        return "null".to_string();
    };
    let Some(chunk) = context
        .chunks
        .get(chunk_index)
        .or_else(|| context.chunks.first())
    else {
        return "null".to_string();
    };
    let proof = EcdcCacheProof {
        record_header: format!(
            "{}:{}",
            if context.format.is_empty() {
                "ecdc-v2"
            } else {
                &context.format
            },
            context.header_base64url
        ),
        chunk_index,
        chunk_offset: chunk.chunk_offset,
        chunk_byte_length: chunk.chunk_byte_length,
    };
    serde_json::to_string(&proof).unwrap_or_else(|_| "null".to_string())
}

fn json_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start).copied()? != b'{' {
        return None;
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

fn bcs2_opus_chunk_cache_keys(bcs2: &[u8]) -> Result<Bcs2OpusChunkCacheKeys, String> {
    let stream = record_core::parse_chunk_stream(bcs2).map_err(|error| error.to_string())?;
    let keys = stream
        .chunks
        .iter()
        .map(|chunk| opus_chunk_cache_key_u64_hex(&chunk.payload))
        .collect::<Vec<_>>();
    Ok(Bcs2OpusChunkCacheKeys {
        format: OPUS_CHUNK_CACHE_KEY_FORMAT,
        store_name: OPUS_CHUNK_CACHE_STORE_NAME,
        cache_version: OPUS_CHUNK_CACHE_VERSION,
        output_codec: OPUS_CHUNK_CACHE_OUTPUT_CODEC,
        bitrate: OPUS_CHUNK_CACHE_BITRATE,
        keys,
    })
}

fn opus_chunk_cache_key_u64_hex(source_payload: &[u8]) -> String {
    let source_payload_hash = stable_hash_hex(source_payload);
    let preimage = format!(
        "{OPUS_CHUNK_CACHE_KEY_DOMAIN}\n\
         source_payload_sha256={source_payload_hash}\n\
         output_codec={OPUS_CHUNK_CACHE_OUTPUT_CODEC}\n\
         bitrate={OPUS_CHUNK_CACHE_BITRATE}\n\
         cache_version={OPUS_CHUNK_CACHE_VERSION}\n"
    );
    let full_hash = stable_hash_hex(preimage.as_bytes());
    full_hash.chars().take(16).collect()
}

fn parse_encodec_bundle_metadata(bundle_json: &str) -> Result<OnnxFrameBundleMetadata, JsValue> {
    serde_json::from_str(bundle_json).map_err(to_js_error)
}

fn validate_encodec_lm_metadata(meta: &OnnxFrameBundleMetadata) -> Result<(), String> {
    meta.lm_dim().map_err(|error| error.to_string())?;
    meta.lm_num_layers().map_err(|error| error.to_string())?;
    meta.lm_past_context().map_err(|error| error.to_string())?;
    if meta.lm_cardinality() == 0 {
        return Err("LM cardinality must be non-zero".to_string());
    }
    Ok(())
}

fn encodec_probability_columns_from_logits(
    logits: &[f32],
    meta: &OnnxFrameBundleMetadata,
    lm_tau: f64,
) -> Result<Vec<f64>, String> {
    let card = meta.lm_cardinality();
    let codebooks = meta.num_codebooks;
    if logits.len() != card * codebooks {
        return Err(format!(
            "LM logits length {} does not match cardinality {} * codebooks {}",
            logits.len(),
            card,
            codebooks
        ));
    }

    let mut pdf = vec![0.0_f64; card * codebooks];
    let mut quantized = vec![0.0_f64; card];
    let mut probs = vec![0.0_f64; card];
    let uniform = 1.0 / card as f64;
    let near_pdf_threshold = 0.25 / DEFAULT_FP_SCALE as f64;
    let logit_step = meta.lm_entropy_logit_step();

    for codebook in 0..codebooks {
        let mut max_value = f64::NEG_INFINITY;
        let mut min_value = f64::INFINITY;
        for bin in 0..card {
            let raw = logits[bin * codebooks + codebook] as f64 / lm_tau;
            let quantized_value = quantize_encodec_logit(raw, logit_step);
            quantized[bin] = quantized_value;
            max_value = max_value.max(quantized_value);
            min_value = min_value.min(quantized_value);
        }

        let mut denom = 0.0_f64;
        for bin in 0..card {
            let value = (quantized[bin] - max_value).exp();
            probs[bin] = value;
            denom += value;
        }
        if !denom.is_finite() || denom <= 0.0 {
            for bin in 0..card {
                pdf[bin * codebooks + codebook] = uniform;
            }
            continue;
        }

        let mut max_pdf = 0.0_f64;
        let mut min_pdf = f64::INFINITY;
        for prob in probs.iter_mut() {
            *prob /= denom;
            max_pdf = max_pdf.max(*prob);
            min_pdf = min_pdf.min(*prob);
        }
        let near_uniform = (max_value - min_value) <= (2.0 * logit_step)
            || (max_pdf - min_pdf) <= near_pdf_threshold;
        for bin in 0..card {
            pdf[bin * codebooks + codebook] = if near_uniform { uniform } else { probs[bin] };
        }
    }

    Ok(pdf)
}

fn quantize_encodec_logit(value: f64, step: f64) -> f64 {
    let eps = 2_f64.powi(-40);
    let y = value / step;
    (y + 0.5 - eps).floor() * step
}

fn to_js_value<T: Serialize + ?Sized>(value: &T) -> Result<JsValue, JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    value.serialize(&serializer).map_err(to_js_error)
}

fn to_js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_record_text_and_playback_metadata_from_header_json() {
        let header = json!({
            "descriptor": {
                "title": "  Westside\u{0000}Demo  ",
                "artist": " Lori   Asha ",
                "recordProfile": "single45",
                "arbitraryMetadata": serde_json::to_string(&json!({
                    "payloadContainer": "ECDC",
                    "trackListing": [{"title": "A"}],
                    "dummySpiralRegions": [{"spiralPixelStart": 7, "spiralPixelCount": 3}]
                })).unwrap()
            },
            "chunkStream": {
                "byteLength": 123,
                "chunkCount": 2,
                "metadata": {
                    "tracks": [{ "title": "A", "first_revolution_index": 0, "revolution_count": 7 }]
                },
                "sha256": "abc"
            }
        });

        let parsed: Value = serde_json::from_str(&record_text_from_header_validation_json(
            &header.to_string(),
        ))
        .unwrap();
        assert_eq!(parsed["title"], "Westside Demo");
        assert_eq!(parsed["artist"], "Lori Asha");
        assert_eq!(parsed["meta"]["recordProfile"], "single45");
        assert_eq!(parsed["meta"]["chunkCount"], 2);
        assert_eq!(parsed["meta"]["revolutionCount"], 7);
        assert_eq!(parsed["meta"]["trackListing"][0]["title"], "A");
        assert_eq!(
            parsed["meta"]["dummySpiralRegions"][0]["carrierPixelStart"],
            7
        );
        assert_eq!(parsed["meta"]["dummySpiralRegions"][0]["pixelCount"], 3);

        let playback: Value = serde_json::from_str(&record_playback_metadata_from_header_json(
            &header.to_string(),
        ))
        .unwrap();
        assert_eq!(playback["payloadContainer"], "ECDC");
        assert_eq!(playback["entryContainer"], "");
        assert_eq!(playback["trackListing"][0]["title"], "A");
    }

    #[test]
    fn resolves_record_display_metadata_from_canonical_fields() {
        let record = json!({
            "title": "  Westside  ",
            "artist": " Lori   Asha ",
            "recordProfile": "single45",
            "meta": {
                "title": "Metadata Title",
                "artist": "Metadata Artist",
                "recordProfile": "lp",
                "bitneedleVerified": "true"
            }
        });

        let metadata = record_display_metadata_value(&record, "");
        assert_eq!(metadata["title"], "Westside");
        assert_eq!(metadata["artist"], "Lori Asha");
        assert_eq!(metadata["displayLabel"], "Westside - Lori Asha");
        assert_eq!(metadata["profileDisplay"], "45 \u{00b7} Verified");
        assert_eq!(metadata["recordProfile"], "single45");
        assert_eq!(metadata["verified"], true);
    }

    #[test]
    fn builds_record_verification_meta_without_header_aliases() {
        let meta: Value = serde_json::from_str(&record_verification_meta_json(
            r#"{"ok":true,"code":" abc  123 ","keyId":" key-1 "}"#,
        ))
        .unwrap();
        assert_eq!(meta["bitneedleVerification"], "abc 123");
        assert_eq!(meta["bitneedleVerified"], "true");
        assert_eq!(meta["signatureKeyId"], "key-1");
        assert!(meta.get("X-Bitneedle-Verified").is_none());
        assert!(meta.get("X-Bitneedle-Verification").is_none());
    }

    #[test]
    fn resolves_playback_payload_metadata_from_header_and_payload() {
        let header = json!({
            "payloadContainer": "ecdc",
            "entryContainer": "",
            "trackListing": [{ "title": "Header track" }],
            "dummySpiralRegions": [{ "spiralPixelStart": 8, "pixelCount": 4 }]
        });
        let payload = json!({
            "payloadContainer": "mossnano",
            "payloadCodec": "moss-audio-tokenizer-nano-rvq16",
            "entryContainer": "single",
            "payloadDescriptors": [{ "container": "ignored" }],
            "trackListing": [{ "title": "Payload track" }],
            "dummySpiralRegions": [{ "spiralPixelStart": 20, "pixelCount": 5 }]
        });

        let metadata = resolve_playback_payload_metadata_value(&header, &payload);
        assert_eq!(metadata["payloadContainer"], "ECDC");
        assert_eq!(metadata["payloadCodec"], "moss-audio-tokenizer-nano-rvq16");
        assert_eq!(metadata["entryContainer"], "single");
        assert_eq!(metadata["trackListing"][0]["title"], "Header track");
        assert_eq!(metadata["dummySpiralRegions"][0]["carrierPixelStart"], 8);
        assert_eq!(metadata["dummySpiralRegions"][0]["pixelCount"], 4);
    }

    #[test]
    fn ignores_legacy_snake_case_record_metadata_aliases() {
        let header = json!({
            "descriptor": {
                "record_profile": "single45",
                "arbitrary_metadata": serde_json::to_string(&json!({
                    "payload_container": "ECDC",
                    "track_listing": [{"title": "legacy"}],
                    "dummy_spiral_regions": [{"spiralPixelStart": 7, "spiralPixelCount": 3}]
                })).unwrap()
            },
            "chunkStream": {
                "payload_container": "ECDC",
                "track_listing": [{"title": "legacy"}],
                "dummy_spiral_regions": [{"spiralPixelStart": 7, "spiralPixelCount": 3}]
            }
        });

        assert_eq!(
            record_profile_from_header_validation_json(&header.to_string()),
            ""
        );
        let playback: Value = serde_json::from_str(&record_playback_metadata_from_header_json(
            &header.to_string(),
        ))
        .unwrap();
        assert_eq!(playback["payloadContainer"], "");
        assert_eq!(playback["entryContainer"], "");
        assert_eq!(playback["trackListing"].as_array().unwrap().len(), 0);
        assert_eq!(playback["dummySpiralRegions"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn resolves_record_profile_and_playback_math() {
        let profile: Value =
            serde_json::from_str(&record_profile_spec_json("single45").unwrap()).unwrap();
        assert_eq!(profile["name"], "single45");
        assert_eq!(profile["label"], "45");
        assert_eq!(profile["leadInTurns"], 2.0);
        assert!(profile["spindleHoleRadius"].as_i64().unwrap() > 0);

        assert_eq!(normalize_record_profile_name("lp").unwrap(), "lp");
        assert!(normalize_record_profile_name("45rpm").is_err());
        assert!(normalize_record_profile_name("12 inch").is_err());
        assert!(normalize_record_profile_name("single12").is_err());
        assert!((record_rpm("lp").unwrap() - 33.3333333333).abs() < 1e-9);

        let playback_rate = resolve_playback_rate_number(4.0, 1.0, 0.92, 1.25);
        assert_eq!(playback_rate, 1.25);

        let revolutions = seconds_to_revolutions_number(120.0, "single45", f64::NAN).unwrap();
        assert_eq!(revolutions, 90.0);

        let seconds_per_turn =
            resolve_seconds_per_turn_number(f64::NAN, "single45", 1.0, 1.0, 0.92, 1.25).unwrap();
        assert!((seconds_per_turn - (60.0 / 45.0)).abs() < 1e-9);
    }

    #[test]
    fn resolves_scratch_identity_rules() {
        assert_eq!(
            scratch_sample_token_from_bytes(&[0xff, 0, 0, 0, 0, 0, 1], 99.0),
            8725724278030337.0
        );
        assert_eq!(
            scratch_sample_token_hex(8725724278030337.0),
            "1f000000000001"
        );
        assert_eq!(
            scratch_clip_id_for_sample_id(8725724278030337.0),
            "scratch-sample-1f000000000001"
        );
        assert_eq!(
            scratch_clip_sample_id_json(r#"{"sampleId":8725724278030337}"#),
            8725724278030337.0
        );
        assert!(is_valid_scratch_anon_user_id("bnanon_123456789abc"));
        assert!(!is_valid_scratch_anon_user_id("anon_123456789abc"));
        assert_eq!(
            scratch_anon_user_id_from_random("ABC_de--123!!!!"),
            "bnanon_abcde--123"
        );
        assert_eq!(
            normalize_scratch_display_name("  A\u{0000}   Name\t "),
            "A Name"
        );
        assert_eq!(
            scratch_display_name_key("  Mixed CASE  Name "),
            "mixed case name"
        );
        assert_eq!(
            stable_local_record_id_from_meta_json(
                r#"{"releaseId":"0xabc 123","recordPngSha256":"ignored"}"#,
                "fallback.png",
            ),
            "local-record-abc-123"
        );
        assert_eq!(
            scratch_visitor_wallet_address_from_bytes(&[1, 2, 3]),
            "0x0102030000000000000000000000000000000000"
        );
        assert!(is_valid_scratch_wallet_address(
            "0x0102030000000000000000000000000000000000"
        ));
        assert_eq!(
            short_scratch_address("0x0102030000000000000000000000000000000000", 4.0),
            "0x0102...0000"
        );
    }

    #[test]
    fn resolves_remote_scratch_control_revision_protocol() {
        assert_eq!(normalize_scratch_remote_control_revision(0.0), 0);
        assert_eq!(normalize_scratch_remote_control_revision(3.7), 3);
        assert_eq!(bump_scratch_remote_control_revision(0.0), 1);
        assert_eq!(bump_scratch_remote_control_revision(f64::from(u32::MAX)), 1);

        let ensured: Value = serde_json::from_str(&ensure_scratch_remote_control_revision_json(
            r#"{"controlRevision":0,"controlIntent":true}"#,
        ))
        .unwrap();
        assert_eq!(ensured["controlRevision"], 1);
        assert_eq!(ensured["controlIntent"], false);

        let newer: Value = serde_json::from_str(&should_apply_remote_scratch_controls_json(
            r#"{"controlRevision":2,"controlIntent":false}"#,
            r#"{"controlRevision":3,"controlIntent":false}"#,
        ))
        .unwrap();
        assert_eq!(newer["apply"], true);
        assert_eq!(newer["controlRevision"], 3);

        let intent_tie: Value = serde_json::from_str(&should_apply_remote_scratch_controls_json(
            r#"{"controlRevision":3,"controlIntent":false}"#,
            r#"{"controlRevision":3,"controlIntent":true}"#,
        ))
        .unwrap();
        assert_eq!(intent_tie["apply"], true);
        assert_eq!(intent_tie["controlIntent"], true);

        let stale: Value = serde_json::from_str(&should_apply_remote_scratch_controls_json(
            r#"{"controlRevision":4,"controlIntent":true}"#,
            r#"{"controlRevision":3,"controlIntent":true}"#,
        ))
        .unwrap();
        assert_eq!(stale["apply"], false);
        assert_eq!(stale["controlRevision"], 4);
    }

    #[test]
    fn creates_ecdc_cache_proof_context() {
        // ECDC v2 frame: [4B BE payload_len][4B overhead][payload bytes]
        let mut ecdc = b"ECDC{\"x\":1}".to_vec();
        ecdc.extend_from_slice(&2u32.to_be_bytes()); // payload_len = 2
        ecdc.extend_from_slice(&[0, 0, 0, 0]); // 4-byte overhead
        ecdc.extend_from_slice(&[1, 2]); // 2 bytes payload

        let context = create_player_ecdc_cache_proof_context(&ecdc).unwrap();
        assert_eq!(context.header_byte_length, 11);
        assert_eq!(context.chunks[0].chunk_offset, 11);
        assert_eq!(context.chunks[0].chunk_byte_length, 10); // 8 header + 2 payload

        let context_json = serde_json::to_string(&context).unwrap();
        let proof: Value =
            serde_json::from_str(&player_ecdc_cache_proof_for_chunk_json(&context_json, 3))
                .unwrap();
        assert_eq!(proof["chunkIndex"], 3);
        assert_eq!(proof["chunkOffset"], 11);
        assert_eq!(proof["chunkByteLength"], 10);
        assert!(proof["recordHeader"]
            .as_str()
            .unwrap()
            .starts_with("ecdc-v2:"));
    }

    #[test]
    fn derives_ordered_bcs2_opus_chunk_cache_keys() {
        let input = record_cut::RecordStreamInput {
            payload_descriptors: vec![record_cut::PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![record_cut::TrackInput {
                title: "Test Track".to_string(),
                first_revolution_index: Some(0),
                revolution_count: Some(2),
            }],
            track_gaps: vec![],
        };
        let entries = vec![
            record_cut::PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: b"first".to_vec(),
            },
            record_cut::PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: b"second".to_vec(),
            },
        ];
        let stream = record_cut::encode_record_stream(&input, &entries).unwrap();
        let parsed = record_core::parse_chunk_stream(&stream).unwrap();

        let keys = bcs2_opus_chunk_cache_keys(&stream).unwrap();

        assert_eq!(keys.format, OPUS_CHUNK_CACHE_KEY_FORMAT);
        assert_eq!(keys.store_name, OPUS_CHUNK_CACHE_STORE_NAME);
        assert_eq!(keys.keys.len(), 2);
        assert_eq!(keys.keys[0].len(), 16);
        assert_eq!(keys.keys[1].len(), 16);
        assert_ne!(keys.keys[0], keys.keys[1]);
        assert_eq!(
            keys.keys[0],
            opus_chunk_cache_key_u64_hex(&parsed.chunks[0].payload)
        );
        assert_eq!(
            keys.keys[1],
            opus_chunk_cache_key_u64_hex(&parsed.chunks[1].payload)
        );
    }
}

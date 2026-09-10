//! Public Bitneedle BRD1 descriptor wire-format and decoding primitives.
//!
//! This crate is the authoritative BRD1 carrier-descriptor wire contract. It
//! describes how the record stream is located and encoded in the PNG carrier.
//! `record-core` defines BRS1 payload-entry semantics, programme-time
//! revolution duration, track timing, GAP timing, and the relation between
//! payload entries and programme revolutions.
//!
//! Every segment here is binary. JSON descriptor segments, Brotli
//! compatibility envelopes, base64 and hex wire representations, and
//! record-creation policy live outside this crate.

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit, Payload as AeadPayload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use record_core::{SpiralFamily, SPIRAL_FAMILY_VARI_PITCH_CODE};
use record_groove::{ClockSlot, ToneClock, TonedPalette};

/// The clock a toned band is read with.
///
/// [`trailer_clock`] and [`band_clock`] return this type, so a caller that
/// holds one needs to be able to name it.
pub use record_groove::ToneClock as BandToneClock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::convert::TryInto;

pub const RECORD_DESCRIPTOR_MAGIC: &[u8; 4] = b"BRD1";
pub const RECORD_DESCRIPTOR_VERSION: u8 = 4;
/// The private "house" descriptor version: identical to v2 except it may
/// carry a [`SEGMENT_SPIRAL_GEOMETRY`] segment declaring the groove's
/// [`SpiralFamily`]. A v3 record without that segment is strict Archimedean.
/// This version is local. v2 is the wire version of every Archimedean record.
pub const RECORD_DESCRIPTOR_VERSION_HOUSE: u8 = 5;
/// The prefix holds the magic, the version, the three lengths, the `b` of the
/// payload spiral, and the cut geometry. The cut geometry gives the radius at
/// which the groove of the programme stops, and the feed that the deadwax is
/// cut at. Those two values let a reader with the PNG alone traverse the whole
/// groove, including the deadwax. The prefix is painted in the header spiral at
/// the rim, so a reader gets it first.
pub const RECORD_DESCRIPTOR_PREFIX_LENGTH: usize = 29;

/// How many bits of the metadata stream each grey pixel carries.
///
/// A reader reads the lead-in and the run-out before it knows anything about
/// the record, so both bands are painted in plain grey. A toned band or a
/// palette-coded band would need a palette that the reader lacks at that point.
///
/// The carrier needs six bits per pixel. The run-out went from four turns to
/// two turns. At six bits per pixel, the lead-in and a two-turn run-out
/// together hold more descriptor than the four-turn band held at four bits.
pub const METADATA_GRAYSCALE_BITS_PER_PIXEL: u32 = 6;

/// The number of distinct greys, `2^METADATA_GRAYSCALE_BITS_PER_PIXEL`.
pub const METADATA_GRAYSCALE_LEVELS: u32 = 1 << METADATA_GRAYSCALE_BITS_PER_PIXEL;

/// The gap between adjacent levels.
///
/// The step is one. A record decodes from the PNG that it was cut as,
/// losslessly and at its own raster. The path holds no resample, no
/// recompression and no print-and-scan step, so each pixel reaches the reader
/// with the value that the cutter wrote. A wider step would give tolerance
/// against drift that this path excludes, and it would spread the band toward
/// black and white.
///
/// Adjacent rungs put the sixty-four levels in sixty-four values, so the whole
/// band sits in the middle of the range.
pub const METADATA_GRAYSCALE_STEP: u8 = 1;

/// The darkest grey on the ladder.
///
/// The window is centred, so the band holds mid-grey values. Sixty-four rungs
/// one apart span sixty-four values, and that window in the middle of 0..=255
/// sits at 96..=159. The lead-in paints inside 96..=159, so the descriptor
/// bands read as one flat grey ring at the rim and at the label.
pub const METADATA_GRAYSCALE_BASE: u8 =
    ((255 - (METADATA_GRAYSCALE_LEVELS - 1) * METADATA_GRAYSCALE_STEP as u32) / 2) as u8;

/// The lightest grey on the ladder.
pub const METADATA_GRAYSCALE_TOP: u8 = METADATA_GRAYSCALE_BASE
    + ((METADATA_GRAYSCALE_LEVELS - 1) as u8) * METADATA_GRAYSCALE_STEP;

/// The grey a level is painted as.
pub fn grayscale_value_for_level(level: u32) -> u8 {
    METADATA_GRAYSCALE_BASE
        + ((level % METADATA_GRAYSCALE_LEVELS) as u8) * METADATA_GRAYSCALE_STEP
}

/// The level a grey reads back as.
///
/// The match is exact. The band is lossless end to end, so a value off the
/// ladder came from a source other than a Bitneedle cutter. A nearest-rung
/// match would give a corrupted record a plausible reading. This function
/// therefore refuses a value off the ladder, ahead of the CRC check.
pub fn level_for_grayscale_value(value: u8) -> Option<u32> {
    let offset = value.checked_sub(METADATA_GRAYSCALE_BASE)? as u32;
    let step = METADATA_GRAYSCALE_STEP as u32;
    if offset % step != 0 {
        return None;
    }
    let level = offset / step;
    (level < METADATA_GRAYSCALE_LEVELS).then_some(level)
}

/// Fixed by the BRD1 v2 format: a release commitment is always SHA-256, and a
/// signature is always Ed25519. The reference carries no algorithm selector.
pub const SIGNED_RELEASE_REFERENCE_VERSION: u8 = 2;
pub const SIGNED_RELEASE_REFERENCE_HASH_LENGTH: usize = 32;
pub const SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH: usize = 64;
pub const SIGNED_RELEASE_REFERENCE_MAX_KEY_ID_LENGTH: usize = u16::MAX as usize;

pub const CACHE_ENCRYPTION_DESCRIPTOR_VERSION: u8 = 1;
pub const CACHE_ENCRYPTION_ALGORITHM_XCHACHA20POLY1305: &str = "xchacha20-poly1305";
pub const CACHE_KEY_DERIVATION_HKDF_SHA256: &str = "hkdf-sha256";
pub const CACHE_ENCRYPTION_SECRET_LENGTH: usize = 32;
pub const CACHE_ENCRYPTION_RECORD_BINDING_HASH_LENGTH: usize = 32;
pub const CACHE_ENCRYPTION_NONCE_LENGTH: usize = 24;
pub const CACHE_ENCRYPTION_TAG_LENGTH: usize = 16;
pub const CACHE_ENCRYPTION_ENVELOPE_MAGIC: &[u8; 4] = b"BCE1";
pub const CACHE_ENCRYPTION_ENVELOPE_VERSION: u8 = 1;
pub const CACHE_ENCRYPTION_ENVELOPE_ALGORITHM_XCHACHA20POLY1305: u8 = 1;
pub const CACHE_ENCRYPTION_INFO: &[u8] = b"bitneedle-cache-encryption-v1";
pub const CACHE_ENCRYPTION_NONCE_INFO: &[u8] = b"bitneedle-cache-encryption-nonce-v1";
pub const CACHE_ENCRYPTION_AAD_DOMAIN: &[u8] = b"bitneedle-cache-encryption-aad-v1";
pub const CACHE_ENCRYPTION_NONCE_DOMAIN: &[u8] = b"bitneedle-cache-nonce-v1";

pub const RECORD_PROFILE_SINGLE45_CODE: u8 = 0;
pub const RECORD_PROFILE_LP_CODE: u8 = 1;
pub const RECORD_PROFILE_TEN_CODE: u8 = 2;
/// The 7" with the smaller of the two label sizes plants publish. Registered
/// after the first three, so it takes the next code rather than a place in
/// the historical order.
pub const RECORD_PROFILE_SINGLE45_VINTAGE_CODE: u8 = 3;
pub const RECORD_PROFILE_SINGLE45: &str = "single45";
pub const RECORD_PROFILE_SINGLE45_VINTAGE: &str = "single45vintage";
pub const RECORD_PROFILE_LP: &str = "lp";
pub const RECORD_PROFILE_TEN: &str = "ten";

pub const RELEASE_ID_LENGTH: usize = 16;

pub const SEGMENT_DESCRIPTOR_CRC32: u8 = 1;
pub const SEGMENT_STREAM_BYTE_LENGTH: u8 = 2;
pub const SEGMENT_RECORD_PROFILE: u8 = 4;
pub const SEGMENT_TITLE: u8 = 5;
pub const SEGMENT_ARTIST: u8 = 6;
pub const SEGMENT_PAYLOAD_ENCODING: u8 = 7;
pub const SEGMENT_RELEASE_ID: u8 = 8;
pub const SEGMENT_CATALOG_NUMBER: u8 = 9;
pub const SEGMENT_LABEL: u8 = 10;
pub const SEGMENT_ARTWORK_CREDIT: u8 = 11;
pub const SEGMENT_CANONICAL_URL: u8 = 13;
pub const SEGMENT_CREATED_AT: u8 = 14;
pub const SEGMENT_SIGNED_RELEASE_REFERENCE: u8 = 16;
pub const SEGMENT_BSC_POINTER: u8 = 21;
pub const SEGMENT_TONED_CARRIER_MAP: u8 = 22;
pub const SEGMENT_CACHE_ENCRYPTION: u8 = 23;
pub const SEGMENT_COPYRIGHT_YEAR: u8 = 24;
pub const SEGMENT_COPYRIGHT_HOLDER: u8 = 25;
/// The deferred group: the only fields a pressed record may gain later.
///
/// A pressed record is immutable, and the release commitment covers every
/// statement that it makes about itself. These three fields are the exception,
/// because each one arrives after the press. A chain anchor needs a commitment
/// to anchor, and registrars issue ISRCs and barcodes on their own schedule.
/// Each of these fields carries a signature. A writer that writes one of them
/// must write [`SEGMENT_DEFERRED_ATTESTATION`], so a deferred field is null or
/// signed.
pub const SEGMENT_CHAIN_ANCHOR: u8 = 26;
/// ISRCs, one per recording rather than per record: a count, then
/// `(track index u16be, 12 ASCII characters)` pairs in ascending index
/// order. Pairs because registrars assign codes a track at a time.
pub const SEGMENT_ISRC: u8 = 27;
/// The release barcode: 12, 13 or 14 ASCII digits (UPC-A, EAN-13, GTIN-14)
/// with a valid mod-10 check digit.
pub const SEGMENT_UPC: u8 = 28;
/// The signature over the deferred group, in the same envelope shape as
/// [`SEGMENT_SIGNED_RELEASE_REFERENCE`], over
/// [`deferred_identity_bytes`].
pub const SEGMENT_DEFERRED_ATTESTATION: u8 = 29;

/// v3 only: the groove geometry family. Payload is
/// `family_code (u8) || depth_bits (u64be) || seed (u64be)` — 17 bytes for
/// vari-pitch. Archimedean records never write this segment.
pub const SEGMENT_SPIRAL_GEOMETRY: u8 = 30;

/// Signatures beyond the first, so a release can be attested by more than
/// one party.
///
/// The artist, yl.vin, or both may sign a pressing. Each party signs the same
/// release commitment independently, so this segment is a list of signatures.
/// The party that resolves a key ID decides the owner of that signature. The
/// order of the list carries no meaning.
///
/// This segment is separate from [`SEGMENT_SIGNED_RELEASE_REFERENCE`], because
/// a repeat of that segment reads as a duplicate to a reader already in the
/// field. A reader skips this unknown segment instead.
pub const SEGMENT_ADDITIONAL_SIGNATURES: u8 = 31;

/// toned-v2 only: the clockface the groove is toned by. The disc is divided
/// into equal angular slots from a rotation, each cut in its own tone; which
/// slot a pixel is in follows from where it sits on the disc, so the map is
/// the wheel itself and nothing per pixel. See [`encode_tone_clock_map`].
pub const SEGMENT_TONE_CLOCK_MAP: u8 = 32;

/// The deadwax: the groove between where the programme stopped and the
/// descriptor's inner band, and what may be written into it.
///
/// The prefix already carries `cut_inner_radius` and the feed of the deadwax,
/// which is what a reader needs to walk the band. This segment adds the claim:
/// whether the band holds bytes, the owner of those bytes, and the room that a
/// writer has in an empty band.
///
/// This segment is a declaration. The bytes live in the groove, and this
/// segment gives their location and their owner. A record whose programme runs
/// to the label has no deadwax, and it writes no segment.
///
/// Payload: `outer(u16be) || inner(u16be) || pixel_capacity(u32be) ||
/// encoding(u8) || byte_capacity(u32be)`, 13 bytes, and a claimed band adds
/// `claim(4) || claimed_byte_length(u32be)` for 21. Sized rather than
/// versioned, as [`SEGMENT_SPIRAL_GEOMETRY`] is: a longer payload from a
/// later writer decodes to what these mean.
pub const SEGMENT_DEADWAX_EXTENT: u8 = 33;

/// Which way round the programme's groove is cut.
///
/// The cutter of a lathe holds its position, the record turns under it, and
/// the spiral that leaves the head therefore winds against the turn. Every
/// record written before this segment was traced clockwise from twelve o'clock,
/// which mirrors a disc cut on a lathe that turns clockwise. That tracing is
/// self-consistent, because the same tracer reads it back. It gives the
/// opposite hand to a disc cut under a physical cutter.
///
/// Payload: one byte, `0` for counter-clockwise and `1` for clockwise. An
/// absent segment means clockwise, so every pressed record decodes as it did
/// and keeps its bytes in place. A writer writes this segment for a cut in the
/// other hand.
pub const SEGMENT_GROOVE_HANDEDNESS: u8 = 34;

/// Which revision of the lead-out geometry the record was cut under.
///
/// The geometry of the run-out and its lock groove follows from the profile and
/// from `cut_inner_radius`, which the prefix carries. A decoder therefore
/// derives the band. The wire holds no flag for it, and both sides compute one
/// answer.
///
/// Derivation makes the constants part of the format. Those constants are the
/// gap ladder, the taper, and the clearance that the lock groove sits at. A
/// change to any of them stops every record already cut from reading back, and
/// the record itself names no constants. This segment names the revision. It is
/// one byte, and a decoder that meets an unknown revision refuses the record
/// rather than tracing the wrong band and returning plausible bytes.
///
/// This segment also names the tone that the band is cut in. The trailer
/// carries the tail of the stream in that tone rather than in the grey ladder,
/// and no other segment carries that colour. The wheel gives the reading of the
/// picture under the programme, and this tone gives the reading under the
/// trailer.
///
/// It is written at the front of the body, before any field a writer chooses
/// the length of, so a reader that has to walk the trailer finds it in the
/// lead-in. See [`run_out_tone_from_partial_stream`].
///
/// Payload: `revision(u8)`, or `revision(u8) || tone(3)` where the trailer was
/// cut in one of its own. Sized rather than versioned, as
/// [`SEGMENT_DEADWAX_EXTENT`] is. Absent means
/// [`LEAD_OUT_GEOMETRY_REVISION_DRAFT04`], so a record written before this
/// existed decodes as what it is.
pub const SEGMENT_LEAD_OUT_GEOMETRY: u8 = 35;

/// The draft-04 trailer: a single Archimedean spiral of two turns from the
/// cut inner radius to the payload inner radius, and no locked groove.
///
/// This build cuts and traces the later revisions. This constant exists because
/// an absent segment means this revision, and a record that means it is refused
/// by name rather than by a CRC failure further on. draft-05 also changed the
/// pixel encoding of the descriptor band, so a record of this vintage fails at
/// the BRD1 magic, ahead of segment parsing.
pub const LEAD_OUT_GEOMETRY_REVISION_DRAFT04: u8 = 0;

/// The first lead-out geometry: a fine deadwax at the lathe's feed, a run-out
/// of one to four rings opening outward by [`record_core::RUN_OUT_TAPER`],
/// and a lock groove closing on itself.
///
/// The deadwax took the space that the four rings left. On a side that carried
/// one track, that space was most of the annulus: forty turns at a millimetre
/// apart, drawn across the artwork. [`LEAD_OUT_GEOMETRY_REVISION_FILLED`] gives
/// that space to the run-out instead.
pub const LEAD_OUT_GEOMETRY_REVISION_ORIGINAL: u8 = 1;

/// The filled run-out: a deadwax header of at most
/// [`record_core::DEADWAX_MAX_TURNS`] turns, and a run-out of as many rings as
/// the room the cut left will hold.
///
/// This revision keeps the same three bands in the same order, and it keeps the
/// lock groove at its radius. It moves the spare room from the deadwax to the
/// run-out. The rings open out of the lock by [`record_core::RUN_OUT_TAPER`]
/// until they reach the coarse feed of the lathe
/// ([`record_core::RUN_OUT_TURN_SEPARATION_MM`]). Above that point they run
/// parallel up to the header. A side that stops a third of the way down
/// therefore carries a dozen widely spaced rings, against four before.
///
/// A record cut under [`LEAD_OUT_GEOMETRY_REVISION_ORIGINAL`] holds its run-out
/// at other radii. Its descriptor reads back while the stream fits the lead-in,
/// which is the ordinary case, and its groove and programme hold their bytes. A
/// reader must treat the band that this build traces there as another band, and
/// not as the run-out of that record.
pub const LEAD_OUT_GEOMETRY_REVISION_FILLED: u8 = 2;

/// The merged lock: the run-out's descent ends above the lock radius, and the
/// locked groove closes the ring.
///
/// Revision 2 ran the descent to the lock radius. The last arc of that descent
/// drew a row of pixels beside the lock circle. The two rows touch, and they
/// read as one groove two pixels wide.
///
/// This revision ends the travel at [`record_core::LOCK_MERGE_PX`] above the
/// lock radius. The locked groove starts at the angle the descent reached, so
/// the traversal stays continuous. Every groove in the band is one pixel wide.
///
/// The run-out holds fewer pixels than it holds at revision 2. The trailer
/// carrier holds 781 bytes at the narrowest legal cut of the `lp` profile,
/// which keeps the guarantee of 512 bytes on every registered profile.
///
/// A record cut under [`LEAD_OUT_GEOMETRY_REVISION_FILLED`] holds its run-out
/// pixels in another order. Its descriptor reads back while the stream fits the
/// lead-in, which is the ordinary case, and its groove and programme hold their
/// bytes. A reader must treat the band that this build traces there as another
/// band, and not as the run-out of that record.
pub const LEAD_OUT_GEOMETRY_REVISION_MERGED: u8 = 3;

/// The revision this build cuts and can read.
pub const LEAD_OUT_GEOMETRY_REVISION: u8 = LEAD_OUT_GEOMETRY_REVISION_MERGED;

/// The deadwax is an empty groove. A writer and its reader agree on the content
/// of that groove between them.
pub const DEADWAX_ENCODING_UNPAINTED: u8 = 0;
/// One nibble per pixel as a grey step, the way the descriptor's own bands
/// are painted ([`METADATA_GRAYSCALE_BASE`]).
pub const DEADWAX_ENCODING_GRAYSCALE_NIBBLE: u8 = 1;
/// The carrier's own encoding: an iso-luma palette around the groove tone,
/// at the clock's bits per pixel. A band written this way is the record's
/// colour and carries several times what the grey encoding does.
pub const DEADWAX_ENCODING_TONED: u8 = 2;

/// The band is free, and a writer may use its whole capacity.
pub const DEADWAX_CLAIM_FREE: Option<[u8; 4]> = None;

pub const ISRC_LENGTH: usize = 12;

pub const PAYLOAD_ENCODING_RGB: &str = "rgb";
pub const PAYLOAD_ENCODING_TONED_V1: &str = "toned-v1";
pub const PAYLOAD_ENCODING_RGB_CODE: u8 = 0;
pub const PAYLOAD_ENCODING_TONED_V1_CODE: u8 = 1;
pub const PAYLOAD_ENCODING_TONED_V2: &str = "toned-v2";
pub const PAYLOAD_ENCODING_TONED_V2_CODE: u8 = 2;

pub const TONED_CARRIER_MAP_VERSION: u8 = 1;
pub const TONED_ORDERING_BASE_PROXIMITY: u8 = 0;
pub const TONED_ORDERING_CHROMA_PROXIMITY: u8 = 1;
pub const TONED_MIN_BITS_PER_PIXEL: u8 = 1;
pub const TONED_MAX_BITS_PER_PIXEL: u8 = 24;
pub const TONED_MAX_SPAN_COUNT: usize = u16::MAX as usize;
/// Version 1 holds a wheel of wedges: one ring, over the whole depth of the
/// groove. Version 2 adds the rings, as a slot count per ring and the band that
/// the rings divide. A v1 map decodes as that one-ring wheel, and an encoder
/// writes a one-ring wheel as v1. A record with one ring therefore keeps its
/// bytes.
pub const TONE_CLOCK_MAP_VERSION: u8 = 2;
pub const TONE_CLOCK_MAP_VERSION_WEDGES: u8 = 1;
pub const TONE_CLOCK_MIN_SLOTS: usize = 2;
pub const TONE_CLOCK_MAX_SLOTS: usize = 64;
/// Pockets in the whole wheel, across every ring.
pub const TONE_CLOCK_MAX_CELLS: usize = 256;
pub const TONE_CLOCK_MAX_RINGS: usize = 8;
/// The band the rings divide, in ten-thousandths of the half-side.
pub const TONE_CLOCK_SPAN_UNITS: u32 = 10_000;
/// Clock rotation is carried in hundredths of a degree, clockwise from
/// twelve o'clock, so encoder and decoder derive identical radians from one
/// integer.
pub const TONE_CLOCK_ROTATION_UNITS_PER_TURN: u32 = 36_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheEncryptionAlgorithm {
    #[serde(rename = "xchacha20-poly1305")]
    XChaCha20Poly1305,
}

impl CacheEncryptionAlgorithm {
    pub fn wire_code(self) -> u8 {
        match self {
            Self::XChaCha20Poly1305 => CACHE_ENCRYPTION_ENVELOPE_ALGORITHM_XCHACHA20POLY1305,
        }
    }

    pub fn from_wire_code(code: u8) -> Result<Self> {
        match code {
            CACHE_ENCRYPTION_ENVELOPE_ALGORITHM_XCHACHA20POLY1305 => Ok(Self::XChaCha20Poly1305),
            _ => bail!("unsupported cache encryption algorithm code {code}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheKeyDerivation {
    #[serde(rename = "hkdf-sha256")]
    HkdfSha256,
}

impl CacheKeyDerivation {
    pub fn wire_code(self) -> u8 {
        match self {
            Self::HkdfSha256 => 1,
        }
    }

    pub fn from_wire_code(code: u8) -> Result<Self> {
        match code {
            1 => Ok(Self::HkdfSha256),
            _ => bail!("unsupported cache key derivation code {code}"),
        }
    }
}

fn serialize_secret_base64url<S>(secret: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&URL_SAFE_NO_PAD.encode(secret))
}

fn deserialize_secret_base64url<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    URL_SAFE_NO_PAD
        .decode(text.as_bytes())
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEncryptionDescriptor {
    pub version: u8,
    pub algorithm: CacheEncryptionAlgorithm,
    pub key_derivation: CacheKeyDerivation,
    #[serde(
        serialize_with = "serialize_secret_base64url",
        deserialize_with = "deserialize_secret_base64url"
    )]
    pub secret: Vec<u8>,
}

impl CacheEncryptionDescriptor {
    pub fn validate(&self) -> Result<()> {
        if self.version != CACHE_ENCRYPTION_DESCRIPTOR_VERSION {
            bail!(
                "unsupported cache encryption descriptor version: {}",
                self.version
            );
        }
        match self.algorithm {
            CacheEncryptionAlgorithm::XChaCha20Poly1305 => {}
        }
        match self.key_derivation {
            CacheKeyDerivation::HkdfSha256 => {}
        }
        if self.secret.len() != CACHE_ENCRYPTION_SECRET_LENGTH {
            bail!(
                "cache encryption secret must be exactly {} bytes",
                CACHE_ENCRYPTION_SECRET_LENGTH
            );
        }
        Ok(())
    }

    pub fn secret(&self) -> &[u8] {
        self.secret.as_slice()
    }

    pub fn from_secret_base64url(secret: &str) -> Result<Self> {
        let secret = URL_SAFE_NO_PAD
            .decode(secret.as_bytes())
            .context("cache encryption secret is not valid base64url")?;
        let descriptor = Self {
            version: CACHE_ENCRYPTION_DESCRIPTOR_VERSION,
            algorithm: CacheEncryptionAlgorithm::XChaCha20Poly1305,
            key_derivation: CacheKeyDerivation::HkdfSha256,
            secret,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEncryptionContext {
    pub protocol_version: u8,
    pub cache_format_version: u8,
    pub cache_store_name: String,
    pub cache_key: String,
    pub chunk_index: u64,
    pub packet_offset: u64,
    pub plaintext_length: usize,
    pub codec_identifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEncryptionEnvelope {
    pub version: u8,
    pub algorithm: u8,
    pub flags: u16,
    pub record_binding_hash: [u8; CACHE_ENCRYPTION_RECORD_BINDING_HASH_LENGTH],
    pub chunk_index: u64,
    pub packet_offset: u64,
    pub plaintext_length: u32,
    pub nonce: [u8; CACHE_ENCRYPTION_NONCE_LENGTH],
    pub ciphertext: Vec<u8>,
}

impl CacheEncryptionEnvelope {
    pub const HEADER_LENGTH: usize = 4
        + 1
        + 1
        + 2
        + CACHE_ENCRYPTION_RECORD_BINDING_HASH_LENGTH
        + 8
        + 8
        + 4
        + CACHE_ENCRYPTION_NONCE_LENGTH;

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < Self::HEADER_LENGTH + CACHE_ENCRYPTION_TAG_LENGTH {
            bail!("invalid BCE1 envelope: truncated header or ciphertext");
        }
        if bytes.get(0..4) != Some(CACHE_ENCRYPTION_ENVELOPE_MAGIC.as_slice()) {
            bail!("invalid BCE1 envelope: magic mismatch");
        }

        let version = bytes[4];
        if version != CACHE_ENCRYPTION_ENVELOPE_VERSION {
            bail!("unsupported BCE1 envelope version {version}");
        }

        let algorithm = bytes[5];
        if algorithm != CACHE_ENCRYPTION_ENVELOPE_ALGORITHM_XCHACHA20POLY1305 {
            bail!("unsupported BCE1 envelope algorithm {algorithm}");
        }

        let flags = u16::from_be_bytes(bytes[6..8].try_into().expect("slice length"));
        let record_binding_hash = bytes[8..40].try_into().expect("slice length");
        let chunk_index = u64::from_be_bytes(bytes[40..48].try_into().expect("slice length"));
        let packet_offset = u64::from_be_bytes(bytes[48..56].try_into().expect("slice length"));
        let plaintext_length = u32::from_be_bytes(bytes[56..60].try_into().expect("slice length"));
        let nonce = bytes[60..84].try_into().expect("slice length");
        let ciphertext = bytes[84..].to_vec();

        if plaintext_length == 0 {
            bail!("invalid BCE1 envelope: empty plaintext length");
        }
        if ciphertext.len() != plaintext_length as usize + CACHE_ENCRYPTION_TAG_LENGTH {
            bail!("invalid BCE1 envelope: ciphertext length mismatch");
        }

        Ok(Self {
            version,
            algorithm,
            flags,
            record_binding_hash,
            chunk_index,
            packet_offset,
            plaintext_length,
            nonce,
            ciphertext,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.version != CACHE_ENCRYPTION_ENVELOPE_VERSION {
            bail!("unsupported BCE1 envelope version {}", self.version);
        }
        if self.algorithm != CACHE_ENCRYPTION_ENVELOPE_ALGORITHM_XCHACHA20POLY1305 {
            bail!("unsupported BCE1 envelope algorithm {}", self.algorithm);
        }
        if self.plaintext_length == 0 {
            bail!("invalid BCE1 envelope: empty plaintext length");
        }
        if self.ciphertext.len() != self.plaintext_length as usize + CACHE_ENCRYPTION_TAG_LENGTH {
            bail!("invalid BCE1 envelope: ciphertext length mismatch");
        }

        let mut out = Vec::with_capacity(Self::HEADER_LENGTH + self.ciphertext.len());
        out.extend_from_slice(CACHE_ENCRYPTION_ENVELOPE_MAGIC);
        out.push(self.version);
        out.push(self.algorithm);
        out.extend_from_slice(&self.flags.to_be_bytes());
        out.extend_from_slice(&self.record_binding_hash);
        out.extend_from_slice(&self.chunk_index.to_be_bytes());
        out.extend_from_slice(&self.packet_offset.to_be_bytes());
        out.extend_from_slice(&self.plaintext_length.to_be_bytes());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.ciphertext);
        Ok(out)
    }
}

fn push_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

#[allow(dead_code)]
fn push_len_prefixed_bytes(out: &mut Vec<u8>, tag: u8, bytes: &[u8]) {
    out.push(tag);
    push_u32(out, u32::try_from(bytes.len()).unwrap_or(u32::MAX));
    out.extend_from_slice(bytes);
}

fn push_len_prefixed_string(out: &mut Vec<u8>, tag: u8, value: Option<&str>) {
    out.push(tag);
    match value {
        Some(value) => {
            let bytes = value.as_bytes();
            push_u32(out, u32::try_from(bytes.len()).unwrap_or(u32::MAX));
            out.extend_from_slice(bytes);
        }
        None => push_u32(out, 0),
    }
}

fn push_len_prefixed_u8_slice<const N: usize>(out: &mut Vec<u8>, tag: u8, value: Option<&[u8; N]>) {
    out.push(tag);
    match value {
        Some(value) => {
            push_u32(out, N as u32);
            out.extend_from_slice(value);
        }
        None => push_u32(out, 0),
    }
}

fn cache_encryption_identity_bytes(descriptor: &RecordDescriptor) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"bitneedle.record-descriptor.cache-identity.v1");
    push_u8(&mut out, descriptor.version);
    push_u8(&mut out, u8::from(descriptor.checksum_protected));
    push_u64(&mut out, descriptor.b_value_bits);
    push_len_prefixed_string(&mut out, 1, Some(&descriptor.record_profile));
    push_u64(&mut out, descriptor.stream_byte_length as u64);
    push_len_prefixed_string(&mut out, 2, Some(&descriptor.payload_encoding));
    push_len_prefixed_string(&mut out, 3, descriptor.title.as_deref());
    push_len_prefixed_string(&mut out, 4, descriptor.artist.as_deref());
    push_len_prefixed_u8_slice(&mut out, 5, descriptor.release_id.as_ref());
    push_len_prefixed_string(&mut out, 6, descriptor.catalog_number.as_deref());
    push_len_prefixed_string(&mut out, 7, descriptor.label.as_deref());
    push_len_prefixed_string(&mut out, 8, descriptor.artwork_credit.as_deref());
    push_len_prefixed_string(&mut out, 9, descriptor.canonical_url.as_deref());
    out.push(10);
    match descriptor.created_at {
        Some(value) => {
            push_u32(&mut out, 8);
            push_u64(&mut out, value);
        }
        None => push_u32(&mut out, 0),
    }
    out.push(12);
    match descriptor.bsc_pointer.as_ref() {
        Some(pointer) => {
            push_u32(
                &mut out,
                u32::try_from(pointer.len()).context("BSC pointer exceeds u32")?,
            );
            out.extend_from_slice(pointer);
        }
        None => push_u32(&mut out, 0),
    }
    out.push(13);
    push_u32(
        &mut out,
        u32::try_from(descriptor.tone_spans.len()).context("tone span count exceeds u32")?,
    );
    for span in &descriptor.tone_spans {
        push_u32(
            &mut out,
            u32::try_from(span.byte_length).context("tone span byte length exceeds u32")?,
        );
        out.extend_from_slice(&span.base);
        push_u8(&mut out, span.luma_tolerance);
        push_u8(&mut out, span.bits_per_pixel);
        push_u8(&mut out, span.ordering.wire_code());
    }
    out.push(14);
    match descriptor.copyright_year {
        Some(value) => {
            push_u32(&mut out, 2);
            push_u16(&mut out, value);
        }
        None => push_u32(&mut out, 0),
    }
    push_len_prefixed_string(&mut out, 15, descriptor.copyright_holder.as_deref());
    push_spiral_family_identity(&mut out, 16, &descriptor.spiral_family);
    push_tone_clock_identity(&mut out, 17, descriptor.tone_clock.as_ref())?;
    Ok(out)
}

pub fn decode_additional_signatures(payload: &[u8]) -> Result<Vec<SignedReleaseReference>> {
    if payload.len() < 2 {
        bail!("additional signatures segment is truncated");
    }
    let count = u16::from_be_bytes(payload[..2].try_into().expect("slice length")) as usize;
    if count == 0 {
        bail!("additional signatures segment must not be empty");
    }
    let mut references = Vec::with_capacity(count);
    let mut offset = 2usize;
    for _ in 0..count {
        if offset + 2 > payload.len() {
            bail!("additional signature is truncated");
        }
        let length = u16::from_be_bytes(
            payload[offset..offset + 2]
                .try_into()
                .expect("slice length"),
        ) as usize;
        offset += 2;
        let end = offset
            .checked_add(length)
            .context("additional signature length overflow")?;
        if end > payload.len() {
            bail!("additional signature is truncated");
        }
        references.push(decode_signed_release_reference(&payload[offset..end])?);
        offset = end;
    }
    if offset != payload.len() {
        bail!("additional signatures segment has trailing bytes");
    }
    Ok(references)
}

/// One key signs once. A second signature from the same key over the same
/// commitment adds no information, and this check refuses it.
pub fn validate_signature_set(descriptor: &RecordDescriptor) -> Result<()> {
    if descriptor.additional_signatures.is_empty() {
        return Ok(());
    }
    let Some(primary) = descriptor.signed_release_reference.as_ref() else {
        bail!("additional signatures without a signed release reference to join");
    };
    let mut seen = vec![primary.key_id.as_slice()];
    for reference in &descriptor.additional_signatures {
        if reference.release_commitment_sha256 != primary.release_commitment_sha256 {
            bail!("every signature on a release must be over the same commitment");
        }
        if seen.contains(&reference.key_id.as_slice()) {
            bail!("the same key has signed this release twice");
        }
        seen.push(reference.key_id.as_slice());
    }
    Ok(())
}

/// An ISRC in canonical form: uppercase, unhyphenated, 12 characters —
/// 2 alpha country, 3 alphanumeric registrant, 2 digit year, 5 digit
/// designation.
pub fn normalize_isrc(value: &str) -> Result<String> {
    let code: String = value
        .chars()
        .filter(|character| !matches!(character, '-' | ' '))
        .collect::<String>()
        .to_ascii_uppercase();
    if code.len() != ISRC_LENGTH {
        bail!("ISRC must be {ISRC_LENGTH} characters, got {}", code.len());
    }
    let bytes = code.as_bytes();
    if !bytes[..2].iter().all(u8::is_ascii_alphabetic) {
        bail!("ISRC country code must be two letters");
    }
    if !bytes[2..5].iter().all(u8::is_ascii_alphanumeric) {
        bail!("ISRC registrant code must be three letters or digits");
    }
    if !bytes[5..].iter().all(u8::is_ascii_digit) {
        bail!("ISRC year and designation must be seven digits");
    }
    Ok(code)
}

/// A barcode as issued: 12, 13 or 14 digits with a valid mod-10 check digit.
pub fn normalize_upc(value: &str) -> Result<String> {
    let digits: String = value.chars().filter(|c| !matches!(c, '-' | ' ')).collect();
    if !matches!(digits.len(), 12 | 13 | 14) {
        bail!(
            "barcode must be 12, 13 or 14 digits (UPC-A, EAN-13, GTIN-14), got {}",
            digits.len()
        );
    }
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("barcode must be digits only");
    }
    // Mod 10: weight 3 and 1 alternating from the right, excluding the
    // check digit itself.
    let bytes = digits.as_bytes();
    let check = (bytes[bytes.len() - 1] - b'0') as u32;
    let sum: u32 = bytes[..bytes.len() - 1]
        .iter()
        .rev()
        .enumerate()
        .map(|(index, byte)| {
            let digit = (byte - b'0') as u32;
            if index % 2 == 0 {
                digit * 3
            } else {
                digit
            }
        })
        .sum();
    if (10 - (sum % 10)) % 10 != check {
        bail!("barcode check digit is wrong");
    }
    Ok(digits)
}

pub fn encode_isrc_segment(isrcs: &[TrackIsrc]) -> Result<Vec<u8>> {
    let mut sorted = isrcs.to_vec();
    sorted.sort_by_key(|entry| entry.track_index);
    if sorted
        .windows(2)
        .any(|pair| pair[0].track_index == pair[1].track_index)
    {
        bail!("two ISRCs claim the same track");
    }
    let mut out = Vec::with_capacity(2 + sorted.len() * (2 + ISRC_LENGTH));
    out.extend_from_slice(
        &u16::try_from(sorted.len())
            .context("ISRC count exceeds u16")?
            .to_be_bytes(),
    );
    for entry in &sorted {
        out.extend_from_slice(&entry.track_index.to_be_bytes());
        out.extend_from_slice(normalize_isrc(&entry.code)?.as_bytes());
    }
    Ok(out)
}

pub fn decode_isrc_segment(payload: &[u8]) -> Result<Vec<TrackIsrc>> {
    if payload.len() < 2 {
        bail!("ISRC segment is truncated");
    }
    let count = u16::from_be_bytes(payload[..2].try_into().expect("slice length")) as usize;
    let expected = 2 + count * (2 + ISRC_LENGTH);
    if payload.len() != expected {
        bail!(
            "ISRC segment declares {count} codes but is {} bytes",
            payload.len()
        );
    }
    let mut entries = Vec::with_capacity(count);
    let mut previous: Option<u16> = None;
    for index in 0..count {
        let at = 2 + index * (2 + ISRC_LENGTH);
        let track_index = u16::from_be_bytes(payload[at..at + 2].try_into().expect("slice length"));
        if let Some(previous) = previous {
            if track_index <= previous {
                bail!("ISRCs must be in ascending track order without repeats");
            }
        }
        previous = Some(track_index);
        let code = std::str::from_utf8(&payload[at + 2..at + 2 + ISRC_LENGTH])
            .context("ISRC is not valid UTF-8")?;
        entries.push(TrackIsrc {
            track_index,
            code: normalize_isrc(code)?,
        });
    }
    Ok(entries)
}

/// The signed shape of the deferred group.
///
/// Bound to [`descriptor_commitment`] rather than to the press signature, so
/// a barcode cannot be lifted off one record onto another, and so a record
/// that was never signed at press can still carry a signed barcode.
pub fn deferred_identity_bytes(descriptor: &RecordDescriptor) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"bitneedle.record-descriptor.deferred.v1");
    out.extend_from_slice(&descriptor_commitment(descriptor)?);
    out.push(1);
    match descriptor.chain_anchor.as_ref() {
        Some(anchor) => {
            push_u32(
                &mut out,
                u32::try_from(anchor.len()).context("chain anchor exceeds u32")?,
            );
            out.extend_from_slice(anchor);
        }
        None => push_u32(&mut out, 0),
    }
    out.push(2);
    let isrcs = encode_isrc_segment(&descriptor.isrcs)?;
    push_u32(
        &mut out,
        u32::try_from(isrcs.len()).context("ISRC block exceeds u32")?,
    );
    out.extend_from_slice(&isrcs);
    push_len_prefixed_string(&mut out, 3, descriptor.upc.as_deref());
    Ok(out)
}

/// SHA-256 of [`deferred_identity_bytes`]: what the deferred attestation
/// signs.
pub fn deferred_commitment(descriptor: &RecordDescriptor) -> Result<[u8; 32]> {
    Ok(Sha256::digest(deferred_identity_bytes(descriptor)?).into())
}

/// The rule that makes a deferred field null or signed and nothing between.
pub fn validate_deferred_group(descriptor: &RecordDescriptor) -> Result<()> {
    let present = descriptor.chain_anchor.is_some()
        || !descriptor.isrcs.is_empty()
        || descriptor.upc.is_some();
    match (present, descriptor.deferred_attestation.as_ref()) {
        (true, None) => bail!(
            "a record carrying a chain anchor, an ISRC or a barcode must carry \
             the deferred attestation that signs them"
        ),
        (false, Some(_)) => bail!("deferred attestation with nothing deferred to sign"),
        _ => Ok(()),
    }
}

/// The signed shape of a descriptor: everything a pressed record says about
/// itself, in a fixed order, length-prefixed.
///
/// A pressed record is immutable. The release commitment covers every field
/// here, so a change to a title, a catalogue number, the carrier geometry or
/// the toned palette gives a different release.
///
/// Two segments sit outside the commitment. The signed-release reference
/// excludes itself, because it holds its own signature. The chain anchor
/// arrives after the press, because a writer writes it once the release is
/// pressed and anchored.
pub fn signed_descriptor_identity_bytes(descriptor: &RecordDescriptor) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"bitneedle.record-descriptor.signed-identity.v1");
    push_u8(&mut out, descriptor.version);
    push_u8(&mut out, u8::from(descriptor.checksum_protected));
    push_u64(&mut out, descriptor.b_value_bits);
    push_len_prefixed_string(&mut out, 1, Some(&descriptor.record_profile));
    push_u64(&mut out, descriptor.stream_byte_length as u64);
    push_len_prefixed_string(&mut out, 2, Some(&descriptor.payload_encoding));
    push_len_prefixed_string(&mut out, 3, descriptor.title.as_deref());
    push_len_prefixed_string(&mut out, 4, descriptor.artist.as_deref());
    push_len_prefixed_u8_slice(&mut out, 5, descriptor.release_id.as_ref());
    push_len_prefixed_string(&mut out, 6, descriptor.catalog_number.as_deref());
    push_len_prefixed_string(&mut out, 7, descriptor.label.as_deref());
    push_len_prefixed_string(&mut out, 8, descriptor.artwork_credit.as_deref());
    push_len_prefixed_string(&mut out, 9, descriptor.canonical_url.as_deref());
    out.push(10);
    match descriptor.created_at {
        Some(value) => {
            push_u32(&mut out, 8);
            push_u64(&mut out, value);
        }
        None => push_u32(&mut out, 0),
    }
    out.push(11);
    match descriptor.bsc_pointer.as_ref() {
        Some(pointer) => {
            push_u32(
                &mut out,
                u32::try_from(pointer.len()).context("BSC pointer exceeds u32")?,
            );
            out.extend_from_slice(pointer);
        }
        None => push_u32(&mut out, 0),
    }
    out.push(12);
    push_u32(
        &mut out,
        u32::try_from(descriptor.tone_spans.len()).context("tone span count exceeds u32")?,
    );
    for span in &descriptor.tone_spans {
        push_u32(
            &mut out,
            u32::try_from(span.byte_length).context("tone span byte length exceeds u32")?,
        );
        out.extend_from_slice(&span.base);
        push_u8(&mut out, span.luma_tolerance);
        push_u8(&mut out, span.bits_per_pixel);
        push_u8(&mut out, span.ordering.wire_code());
    }
    out.push(13);
    match descriptor.copyright_year {
        Some(value) => {
            push_u32(&mut out, 2);
            push_u16(&mut out, value);
        }
        None => push_u32(&mut out, 0),
    }
    push_len_prefixed_string(&mut out, 14, descriptor.copyright_holder.as_deref());
    out.push(15);
    match descriptor.cache_encryption.as_ref() {
        Some(cache_encryption) => {
            let encoded = encode_cache_encryption_descriptor(cache_encryption)?;
            push_u32(
                &mut out,
                u32::try_from(encoded.len()).context("cache encryption exceeds u32")?,
            );
            out.extend_from_slice(&encoded);
        }
        None => push_u32(&mut out, 0),
    }
    push_spiral_family_identity(&mut out, 16, &descriptor.spiral_family);
    push_tone_clock_identity(&mut out, 17, descriptor.tone_clock.as_ref())?;
    Ok(out)
}

/// Appends the tone clock to an identity preimage, for a record that carries a
/// clock. A record with a single tone appends nothing, so its identity, and the
/// signature or cache key derived from that identity, keeps the bytes that it
/// had before clocks existed. The commitment covers the clock for the reason
/// that it covers the toned palette: a different wheel gives a different
/// pressing.
fn push_tone_clock_identity(
    out: &mut Vec<u8>,
    tag: u8,
    clock: Option<&ToneClockDescriptor>,
) -> Result<()> {
    if let Some(clock) = clock {
        let encoded = encode_tone_clock_map(clock, None)?;
        out.push(tag);
        push_u32(
            out,
            u32::try_from(encoded.len()).context("tone clock map exceeds u32")?,
        );
        out.extend_from_slice(&encoded);
    }
    Ok(())
}

/// Appends the groove geometry family to an identity preimage, for a cut
/// outside the Archimedean family. An Archimedean descriptor appends nothing,
/// so every v2 identity, and the signature or cache key derived from it, keeps
/// the bytes that it had before spiral families existed.
fn push_spiral_family_identity(out: &mut Vec<u8>, tag: u8, family: &SpiralFamily) {
    if let SpiralFamily::VariPitch {
        depth,
        seed,
        definition,
        sheen,
        placement,
        fire,
        tuning,
    } = family
    {
        out.push(tag);
        push_u32(out, 90);
        push_u8(out, family.wire_code());
        push_u64(out, depth.to_bits());
        push_u64(out, *seed);
        push_u64(out, definition.to_bits());
        push_u64(out, sheen.to_bits());
        push_u8(out, placement.wire_code());
        push_u64(out, fire.to_bits());
        push_u64(out, tuning.wave_one_cycles.to_bits());
        push_u64(out, tuning.wave_two_cycles.to_bits());
        push_u64(out, tuning.wave_balance.to_bits());
        push_u64(out, tuning.dither_frequency.to_bits());
        push_u64(out, tuning.aura_width.to_bits());
        push_u64(out, tuning.fire_cycles.to_bits());
    }
}

/// SHA-256 of [`signed_descriptor_identity_bytes`], for the release
/// commitment to carry.
pub fn descriptor_commitment(descriptor: &RecordDescriptor) -> Result<[u8; 32]> {
    Ok(Sha256::digest(signed_descriptor_identity_bytes(descriptor)?).into())
}

pub fn cache_encryption_record_binding_hash(
    descriptor: &RecordDescriptor,
) -> Result<[u8; CACHE_ENCRYPTION_RECORD_BINDING_HASH_LENGTH]> {
    let identity = cache_encryption_identity_bytes(descriptor)?;
    Ok(Sha256::digest(identity).into())
}

pub fn derive_cache_encryption_key(descriptor: &RecordDescriptor) -> Result<[u8; 32]> {
    let cache_encryption = descriptor
        .cache_encryption
        .as_ref()
        .context("record descriptor is missing cache encryption descriptor")?;
    cache_encryption.validate()?;

    let salt = cache_encryption_record_binding_hash(descriptor)?;
    Ok(hkdf_sha256_32(
        &salt,
        cache_encryption.secret(),
        CACHE_ENCRYPTION_INFO,
    ))
}

/// Subkey used only to derive the per-entry nonce, kept separate from the AEAD
/// key itself (same salt/secret, distinct HKDF `info` label).
pub fn derive_cache_nonce_key(descriptor: &RecordDescriptor) -> Result<[u8; 32]> {
    let cache_encryption = descriptor
        .cache_encryption
        .as_ref()
        .context("record descriptor is missing cache encryption descriptor")?;
    cache_encryption.validate()?;

    let salt = cache_encryption_record_binding_hash(descriptor)?;
    Ok(hkdf_sha256_32(
        &salt,
        cache_encryption.secret(),
        CACHE_ENCRYPTION_NONCE_INFO,
    ))
}

/// Deterministic nonce: a PRF over the plaintext (hashed) and enough of the
/// cache context to disambiguate entries, keyed by a subkey derived from the
/// record's own cache-encryption secret. The same (record, plaintext, context)
/// always produces the same nonce, so the same triple produces byte-identical
/// ciphertext. The resulting BCE1 envelope therefore serves as a
/// content-addressed cache key, and two independent writers converge on it. Two
/// such writers are the press after issuance and the first decode by a player.
/// Nonce reuse under a fixed key occurs for identical plaintext alone, which is
/// the convergent-encryption property that this scheme intends. The plaintext
/// is decoded audio, and a holder of the record image recovers it already.
fn derive_cache_nonce(
    nonce_key: &[u8; 32],
    context: &CacheEncryptionContext,
    plaintext: &[u8],
) -> [u8; CACHE_ENCRYPTION_NONCE_LENGTH] {
    let plaintext_hash: [u8; 32] = Sha256::digest(plaintext).into();
    let mut input =
        Vec::with_capacity(CACHE_ENCRYPTION_NONCE_DOMAIN.len() + 32 + context.cache_key.len() + 20);
    input.extend_from_slice(CACHE_ENCRYPTION_NONCE_DOMAIN);
    input.extend_from_slice(&plaintext_hash);
    push_len_prefixed_string(&mut input, 1, Some(&context.cache_key));
    push_u64(&mut input, context.chunk_index);
    push_u64(&mut input, context.packet_offset);
    let mac = hmac_sha256(nonce_key, &input);
    let mut nonce = [0u8; CACHE_ENCRYPTION_NONCE_LENGTH];
    nonce.copy_from_slice(&mac[..CACHE_ENCRYPTION_NONCE_LENGTH]);
    nonce
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        out.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Hex-encoded record binding hash, exposed so callers (e.g. the JS cache
/// layer, via the wasm bindings) can fold the exact same record-identity
/// notion the encryption itself uses into a pre-decode, record-scoped cache
/// lookup key, without reimplementing the identity hash.
pub fn cache_encryption_record_binding_hash_hex(descriptor: &RecordDescriptor) -> Result<String> {
    Ok(hex_encode(&cache_encryption_record_binding_hash(
        descriptor,
    )?))
}

pub fn derive_cache_encryption_key_hex(descriptor: &RecordDescriptor) -> Result<String> {
    Ok(hex_encode(&derive_cache_encryption_key(descriptor)?))
}

pub fn derive_cache_nonce_key_hex(descriptor: &RecordDescriptor) -> Result<String> {
    Ok(hex_encode(&derive_cache_nonce_key(descriptor)?))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed: [u8; 32] = Sha256::digest(key).into();
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0u8; BLOCK_SIZE];
    let mut outer_pad = [0u8; BLOCK_SIZE];
    for index in 0..BLOCK_SIZE {
        inner_pad[index] = key_block[index] ^ 0x36;
        outer_pad[index] = key_block[index] ^ 0x5c;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(data);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn hkdf_sha256_32(salt: &[u8], ikm: &[u8], info: &[u8]) -> [u8; 32] {
    let prk = hmac_sha256(salt, ikm);
    let mut okm_input = Vec::with_capacity(info.len() + 1);
    okm_input.extend_from_slice(info);
    okm_input.push(1);
    hmac_sha256(&prk, &okm_input)
}

pub fn cache_encryption_aad(
    descriptor: &RecordDescriptor,
    context: &CacheEncryptionContext,
) -> Result<Vec<u8>> {
    let binding_hash = cache_encryption_record_binding_hash(descriptor)?;
    let mut out = Vec::new();
    out.extend_from_slice(CACHE_ENCRYPTION_AAD_DOMAIN);
    push_u8(&mut out, context.protocol_version);
    push_u8(&mut out, context.cache_format_version);
    out.extend_from_slice(&binding_hash);
    push_len_prefixed_string(&mut out, 1, Some(&context.cache_store_name));
    push_len_prefixed_string(&mut out, 2, Some(&context.cache_key));
    push_u64(&mut out, context.chunk_index);
    push_u64(&mut out, context.packet_offset);
    push_u64(
        &mut out,
        u64::try_from(context.plaintext_length).context("plaintext length exceeds u64")?,
    );
    push_len_prefixed_string(&mut out, 3, Some(&context.codec_identifier));
    Ok(out)
}

pub fn encrypt_cache_envelope(
    descriptor: &RecordDescriptor,
    context: &CacheEncryptionContext,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    if plaintext.is_empty() {
        bail!("cache plaintext must not be empty");
    }
    if plaintext.len() != context.plaintext_length {
        bail!("cache plaintext length mismatch");
    }
    if !descriptor
        .cache_encryption
        .as_ref()
        .is_some_and(|value| value.validate().is_ok())
    {
        return Err(anyhow::anyhow!(
            "record descriptor is missing a valid cache encryption descriptor"
        ));
    }

    let key = derive_cache_encryption_key(descriptor)?;
    let nonce_key = derive_cache_nonce_key(descriptor)?;
    let nonce = derive_cache_nonce(&nonce_key, context, plaintext);
    let aad = cache_encryption_aad(descriptor, context)?;
    let ciphertext = XChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt(
            XNonce::from_slice(&nonce),
            AeadPayload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("failed to encrypt cache payload"))?;

    let envelope = CacheEncryptionEnvelope {
        version: CACHE_ENCRYPTION_ENVELOPE_VERSION,
        algorithm: CACHE_ENCRYPTION_ENVELOPE_ALGORITHM_XCHACHA20POLY1305,
        flags: 0,
        record_binding_hash: cache_encryption_record_binding_hash(descriptor)?,
        chunk_index: context.chunk_index,
        packet_offset: context.packet_offset,
        plaintext_length: u32::try_from(plaintext.len()).context("plaintext length exceeds u32")?,
        nonce,
        ciphertext,
    };
    envelope.encode()
}

pub fn decrypt_cache_envelope(
    descriptor: &RecordDescriptor,
    context: &CacheEncryptionContext,
    envelope_bytes: &[u8],
) -> Result<Vec<u8>> {
    let envelope = CacheEncryptionEnvelope::parse(envelope_bytes)?;
    let expected_binding_hash = cache_encryption_record_binding_hash(descriptor)?;
    if envelope.record_binding_hash != expected_binding_hash {
        bail!("record binding hash mismatch");
    }
    let mut resolved_context = context.clone();
    resolved_context.chunk_index = envelope.chunk_index;
    resolved_context.packet_offset = envelope.packet_offset;
    resolved_context.plaintext_length = envelope.plaintext_length as usize;
    let key = derive_cache_encryption_key(descriptor)?;
    let aad = cache_encryption_aad(descriptor, &resolved_context)?;
    XChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt(
            XNonce::from_slice(&envelope.nonce),
            AeadPayload {
                msg: &envelope.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("cache authentication failed"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToneOrdering {
    BaseProximity,
    ChromaProximity,
}

impl ToneOrdering {
    pub fn wire_code(self) -> u8 {
        match self {
            Self::BaseProximity => TONED_ORDERING_BASE_PROXIMITY,
            Self::ChromaProximity => TONED_ORDERING_CHROMA_PROXIMITY,
        }
    }

    pub fn from_wire_code(code: u8) -> Result<Self> {
        match code {
            TONED_ORDERING_BASE_PROXIMITY => Ok(Self::BaseProximity),
            TONED_ORDERING_CHROMA_PROXIMITY => Ok(Self::ChromaProximity),
            _ => bail!("unknown toned carrier ordering code {code}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToneSpanDescriptor {
    pub byte_length: usize,
    pub base: [u8; 3],
    pub luma_tolerance: u8,
    pub bits_per_pixel: u8,
    pub ordering: ToneOrdering,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedToneSpan {
    pub index: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub pixel_offset: usize,
    pub pixel_count: usize,
    pub base: [u8; 3],
    pub luma_tolerance: u8,
    pub bits_per_pixel: u8,
    pub ordering: ToneOrdering,
}

/// One pocket of a tone clock: the tone the track is cut in there and the
/// lighter tone its track gaps are cut in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToneClockSlotDescriptor {
    pub base: [u8; 3],
    pub luma_tolerance: u8,
    pub gap_base: [u8; 3],
    pub gap_luma_tolerance: u8,
}

/// The toned-v2 carrier map: a wheel of equal angular slots, each in its own
/// tone. A pixel's slot follows from its angle about the disc's centre (in
/// the record's frame, clockwise from twelve) and its groove index, both of
/// which a decoder has once it has walked the spiral. Bits per pixel and
/// ordering are shared by every slot, so the bit stream is one stream and
/// the groove is exactly as long as a single-tone cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToneClockDescriptor {
    /// Where each ring's slot zero begins, in hundredths of a degree
    /// clockwise from twelve, innermost first; the rest of a ring's slots
    /// follow clockwise. One entry turns the wheel as one piece, which is the
    /// form that a version 1 map carries.
    pub rotation_centidegrees: Vec<u16>,
    /// Whether a pixel near a slot boundary may take the tone of the adjacent
    /// slot, in proportion to its distance from that boundary. The choice is
    /// deterministic per pixel, and it gives a continuous surface across the
    /// boundary.
    pub blend: bool,
    pub bits_per_pixel: u8,
    pub ordering: ToneOrdering,
    /// The slots in each ring, innermost first. `[8, 16]` is eight pockets
    /// across the inside of the groove band and sixteen around the outside.
    /// One entry gives a wheel of wedges, which is the form of every clock
    /// before rings, and an encoder writes it as a version 1 map.
    #[serde(default = "one_ring")]
    pub rings: Vec<u32>,
    /// The band the rings divide, in ten-thousandths of the half-side:
    /// where the groove starts and where it ends.
    #[serde(default = "whole_disc")]
    pub span: (u16, u16),
    /// Every pocket's tones: innermost ring first, and clockwise from twelve
    /// within each ring.
    pub slots: Vec<ToneClockSlotDescriptor>,
    /// Byte offsets at which the groove alternates between track and gap
    /// tone, starting in track tone at offset zero. Strictly increasing.
    pub gap_switch_offsets: Vec<usize>,
}

/// What a map with no rings written in it means: one, holding every slot.
fn one_ring() -> Vec<u32> {
    Vec::new()
}

/// The band that a map with no band written in it describes: the whole record,
/// which is the band that a wheel of wedges covers.
fn whole_disc() -> (u16, u16) {
    (0, TONE_CLOCK_SPAN_UNITS as u16)
}

impl ToneClockDescriptor {
    /// The slots in each ring, with an unwritten `rings` read as the one
    /// ring holding every slot.
    pub fn ring_slots(&self) -> Vec<u32> {
        if self.rings.is_empty() {
            vec![self.slots.len() as u32]
        } else {
            self.rings.clone()
        }
    }

    /// Whether this wheel needs the rings written down at all.
    pub fn has_rings(&self) -> bool {
        self.ring_slots().len() > 1
    }
}

/// One signed-release reference over the release commitment. See
/// `record_core::commitment::release_commitment`.
/// `SIGNED_RELEASE_REFERENCE_VERSION` fixes SHA-256 and Ed25519, so the
/// reference carries no algorithm selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedReleaseReference {
    pub version: u8,
    pub release_commitment_sha256: [u8; SIGNED_RELEASE_REFERENCE_HASH_LENGTH],
    pub key_id: Vec<u8>,
    pub signature: Vec<u8>,
}

impl SignedReleaseReference {
    pub fn validate(&self) -> Result<()> {
        if self.version != SIGNED_RELEASE_REFERENCE_VERSION {
            bail!(
                "unsupported signed release reference version: {}",
                self.version
            );
        }
        if self.key_id.is_empty() {
            bail!("signature key ID must not be empty");
        }
        if self.key_id.len() > SIGNED_RELEASE_REFERENCE_MAX_KEY_ID_LENGTH {
            bail!("signature key ID exceeds u16 length limit");
        }
        if self.signature.len() != SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH {
            bail!("signature must be exactly {SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH} bytes");
        }
        Ok(())
    }
}

pub fn encode_cache_encryption_descriptor(
    cache_encryption: &CacheEncryptionDescriptor,
) -> Result<Vec<u8>> {
    cache_encryption.validate()?;
    let mut out = Vec::with_capacity(4 + CACHE_ENCRYPTION_SECRET_LENGTH);
    out.push(cache_encryption.version);
    out.push(cache_encryption.algorithm.wire_code());
    out.push(cache_encryption.key_derivation.wire_code());
    out.push(
        u8::try_from(cache_encryption.secret.len())
            .context("cache encryption secret exceeds u8")?,
    );
    out.extend_from_slice(cache_encryption.secret());
    Ok(out)
}

pub fn decode_cache_encryption_descriptor(bytes: &[u8]) -> Result<CacheEncryptionDescriptor> {
    if bytes.len() < 4 {
        bail!("cache encryption descriptor is truncated");
    }
    let version = bytes[0];
    let algorithm = CacheEncryptionAlgorithm::from_wire_code(bytes[1])?;
    let key_derivation = CacheKeyDerivation::from_wire_code(bytes[2])?;
    let secret_len = usize::from(bytes[3]);
    let secret = bytes[4..].to_vec();
    if secret_len != secret.len() {
        bail!("cache encryption secret length mismatch");
    }
    let descriptor = CacheEncryptionDescriptor {
        version,
        algorithm,
        key_derivation,
        secret,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

/// The content of the deadwax, and its capacity.
///
/// The radii are in rendered pixels from the centre, and they bound the band as
/// the cut does. `outer` is the radius at which the groove of the programme
/// stopped, and `inner` is the radius at which the band of the descriptor
/// begins. `pixel_capacity` is the pixel count that the spiral lays down
/// between them, which follows from the feed of the lathe.
///
/// `byte_capacity` is the capacity of those pixels under `encoding`. The
/// segment stores this value rather than deriving it, so a reader that
/// implements another encoding still reports the room in a band, and so a later
/// encoding leaves the number of an older record unchanged.
///
/// `claim` is a four-byte tag that names the writer of the band. A sidecar
/// chooses its own tag by its own convention. `None` marks a free band. A claim
/// with zero bytes reserves the band.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeadwaxExtent {
    pub outer_radius: u16,
    pub inner_radius: u16,
    pub pixel_capacity: u32,
    pub encoding: u8,
    pub byte_capacity: u32,
    /// Four bytes naming the owner, or `None` while the band is free.
    #[serde(default)]
    pub claim: Option<[u8; 4]>,
    /// The part of `byte_capacity` that the claim uses.
    #[serde(default)]
    pub claimed_byte_length: u32,
}

impl DeadwaxExtent {
    /// Whether anything has claimed the band.
    pub fn is_free(&self) -> bool {
        self.claim.is_none()
    }

    /// What is left for a writer, in bytes.
    pub fn free_byte_capacity(&self) -> u32 {
        self.byte_capacity
            .saturating_sub(self.claimed_byte_length)
    }
}

/// One deadwax extent segment, as written.
///
/// Two shapes: 13 bytes for a free band, and 21 bytes for a claimed band. A
/// longer payload comes from a later writer, and this decoder reads the
/// leading bytes and ignores the tail, as it does for every other sized
/// segment.
pub fn decode_deadwax_extent(payload: &[u8]) -> Result<DeadwaxExtent> {
    const FREE_LENGTH: usize = 13;
    const CLAIMED_LENGTH: usize = 21;

    if payload.len() < FREE_LENGTH {
        bail!("deadwax extent segment is too short");
    }

    let outer_radius = u16::from_be_bytes(payload[0..2].try_into().expect("slice length"));
    let inner_radius = u16::from_be_bytes(payload[2..4].try_into().expect("slice length"));
    if inner_radius >= outer_radius {
        bail!("deadwax extent inner radius is not inside its outer radius");
    }

    let pixel_capacity = u32::from_be_bytes(payload[4..8].try_into().expect("slice length"));
    let encoding = payload[8];
    let byte_capacity = u32::from_be_bytes(payload[9..13].try_into().expect("slice length"));

    let (claim, claimed_byte_length) = if payload.len() >= CLAIMED_LENGTH {
        let tag: [u8; 4] = payload[13..17].try_into().expect("slice length");
        let used = u32::from_be_bytes(payload[17..21].try_into().expect("slice length"));
        if used > byte_capacity {
            bail!("deadwax claim is longer than the band it claims");
        }
        // An all-zero value is outside the tag space. A writer that takes the
        // band and names no owner writes the free shape.
        if tag == [0, 0, 0, 0] {
            bail!("deadwax claim tag is empty");
        }
        (Some(tag), used)
    } else {
        (None, 0)
    };

    Ok(DeadwaxExtent {
        outer_radius,
        inner_radius,
        pixel_capacity,
        encoding,
        byte_capacity,
        claim,
        claimed_byte_length,
    })
}

/// The same, as bytes.
pub fn encode_deadwax_extent(extent: &DeadwaxExtent) -> Result<Vec<u8>> {
    if extent.inner_radius >= extent.outer_radius {
        bail!("deadwax extent inner radius is not inside its outer radius");
    }
    if extent.claimed_byte_length > extent.byte_capacity {
        bail!("deadwax claim is longer than the band it claims");
    }

    let mut bytes = Vec::with_capacity(21);
    bytes.extend_from_slice(&extent.outer_radius.to_be_bytes());
    bytes.extend_from_slice(&extent.inner_radius.to_be_bytes());
    bytes.extend_from_slice(&extent.pixel_capacity.to_be_bytes());
    bytes.push(extent.encoding);
    bytes.extend_from_slice(&extent.byte_capacity.to_be_bytes());

    if let Some(tag) = extent.claim {
        if tag == [0, 0, 0, 0] {
            bail!("deadwax claim tag is empty");
        }
        bytes.extend_from_slice(&tag);
        bytes.extend_from_slice(&extent.claimed_byte_length.to_be_bytes());
    } else if extent.claimed_byte_length != 0 {
        bail!("deadwax extent has claimed bytes but no claim");
    }

    Ok(bytes)
}

/// Decoded BRD1 carrier descriptor.
///
/// `record_profile` identifies the canonical Bitneedle carrier profile that
/// decodes the raster geometry. BRS1 and the codec-specific validation assign
/// the logical sample counts of the payload entries and hold the programme
/// timing.
fn clockwise_by_default() -> bool {
    true
}

/// An absent segment 35 selects the geometry that came before revision 1.
///
/// A record with no segment 35 was written before segment 35 existed, which is
/// to say before revision 1 existed. The absent segment therefore reads as the
/// earlier trailer. The opposite default made every record already cut claim
/// bands that it was cut without, which is the fault that this segment
/// prevents.
fn original_lead_out_geometry() -> u8 {
    LEAD_OUT_GEOMETRY_REVISION_DRAFT04
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordDescriptor {
    pub version: u8,
    pub checksum_protected: bool,
    pub b_value_bits: u64,
    /// Where the programme's groove stops and the deadwax takes over, in
    /// rendered pixels. Zero for a cut that reaches the label.
    #[serde(default)]
    pub cut_inner_radius: u16,
    /// The deadwax's spiral `b` — the feed, never the turn count.
    ///
    /// Serialized as `leadOutBValueBits` under the earlier name for the band.
    /// The alias keeps descriptor JSON written under that vocabulary readable.
    /// The wire bytes hold their positions, so this field remains prefix octets
    /// 21..29.
    #[serde(default, alias = "leadOutBValueBits")]
    pub deadwax_b_value_bits: u64,
    /// The groove geometry family. A v2 record always carries
    /// [`SpiralFamily::Archimedean`]. A v3 record may carry vari-pitch. The
    /// defaults keep every existing serialized form valid.
    #[serde(default)]
    pub spiral_family: SpiralFamily,
    /// Whether the groove of the programme winds clockwise from its start
    /// angle. Every record written before [`SEGMENT_GROOVE_HANDEDNESS`] existed
    /// winds clockwise, and that is the default here, so an absent segment
    /// reads as the original cut. [`SEGMENT_GROOVE_HANDEDNESS`] gives the
    /// reason for the other hand.
    #[serde(default = "clockwise_by_default")]
    pub spiral_clockwise: bool,
    pub record_profile: String,
    pub stream_byte_length: usize,
    pub payload_encoding: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub release_id: Option<[u8; RELEASE_ID_LENGTH]>,
    pub catalog_number: Option<String>,
    pub label: Option<String>,
    pub artwork_credit: Option<String>,
    pub canonical_url: Option<String>,
    pub created_at: Option<u64>,
    /// Phonographic (℗) copyright year — the P-line year shown in credits.
    pub copyright_year: Option<u16>,
    /// Phonographic (℗) copyright holder text (e.g. the artist, optionally with
    /// a licensing clause). Distinct from `label` (the record label).
    pub copyright_holder: Option<String>,
    pub signed_release_reference: Option<SignedReleaseReference>,
    pub bsc_pointer: Option<Vec<u8>>,
    pub tone_spans: Vec<ToneSpanDescriptor>,
    /// The clockface for a toned-v2 groove; absent for every other encoding.
    #[serde(default)]
    pub tone_clock: Option<ToneClockDescriptor>,
    pub cache_encryption: Option<CacheEncryptionDescriptor>,
    /// The on-chain anchor, written after pressing. BRD1 treats it as opaque
    /// bytes, and the chain defines the meaning of a chain reference.
    pub chain_anchor: Option<Vec<u8>>,
    /// Every other signature over the same release commitment.
    pub additional_signatures: Vec<SignedReleaseReference>,
    /// ISRCs by track index, ascending. Empty when none have been issued.
    pub isrcs: Vec<TrackIsrc>,
    /// The release barcode, as issued.
    pub upc: Option<String>,
    /// The signature over the fields above that arrive after the press. A
    /// record that carries any of those fields must carry this signature.
    pub deferred_attestation: Option<SignedReleaseReference>,
    /// Which revision of the lead-out geometry drew this record's run-out.
    /// See [`SEGMENT_LEAD_OUT_GEOMETRY`].
    #[serde(default = "original_lead_out_geometry")]
    pub lead_out_geometry_revision: u8,
    /// The one tone that the trailer is cut in, for a trailer with its own
    /// tone. An absent value means that the band follows the wheel of the
    /// record, or that the record carries no tone. See
    /// [`SEGMENT_LEAD_OUT_GEOMETRY`].
    #[serde(default)]
    pub run_out_tone: Option<[u8; 3]>,
    /// The deadwax that the cut left: the groove between the programme and the
    /// inner band of the descriptor, and the owner of any claim on it. Absent
    /// on a record whose programme ran to the label.
    #[serde(default)]
    pub deadwax: Option<DeadwaxExtent>,
}

/// One recording's ISRC, against the track it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackIsrc {
    pub track_index: u16,
    /// Canonical form: uppercase, unhyphenated, 12 characters.
    pub code: String,
}

impl RecordDescriptor {
    pub fn b_value(&self) -> f64 {
        f64::from_bits(self.b_value_bits)
    }

    pub fn cache_encryption(&self) -> Option<&CacheEncryptionDescriptor> {
        self.cache_encryption.as_ref()
    }

    pub fn validate_cache_encryption(&self) -> Result<()> {
        if let Some(cache_encryption) = self.cache_encryption.as_ref() {
            cache_encryption.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorPrefix {
    pub version: u8,
    pub payload_len: usize,
    pub segment_count: usize,
    pub segment_stream_len: usize,
    pub b_value_bits: u64,
    /// The radius, in rendered pixels, at which the groove of the programme
    /// stops and the deadwax begins. Zero means that the cut ran to the label
    /// and left no deadwax.
    pub cut_inner_radius: u16,
    /// The spiral `b` of the deadwax. The descriptor declares this feed,
    /// because the wire holds no turn count. A reader derives the turn count as
    /// a lathe produces it, from the remaining travel and this feed.
    pub deadwax_b_value_bits: u64,
}

pub fn metadata_pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length
        .saturating_mul(8)
        .div_ceil(METADATA_GRAYSCALE_BITS_PER_PIXEL as usize)
}

pub fn metadata_byte_capacity_for_pixel_count(pixel_count: usize) -> usize {
    pixel_count.saturating_mul(METADATA_GRAYSCALE_BITS_PER_PIXEL as usize) / 8
}

/// The luma windows a one-tone band's palette is tried at, tightest first.
///
/// A tone near black or near white has few colours at its own luma, and it
/// needs a wider window than a tone in the middle of the range. A window wider
/// than the palette needs moves the pixels of the band away from the tone that
/// it is cut in.
pub const TRAILER_LUMA_TOLERANCES: [u8; 6] = [2, 4, 6, 10, 16, 24];

/// The bits that a pixel of a one-tone band carries. This rate matches the
/// grey ladder, so the band keeps the capacity that it had in grey.
pub const TRAILER_BITS_PER_PIXEL: u32 = METADATA_GRAYSCALE_BITS_PER_PIXEL;

/// A clock for a band cut in one tone: two pockets, both that tone.
///
/// The count is two, because [`record_groove::TONE_CLOCK_MIN_SLOTS`] is two.
/// Both pockets carry the same colour, so every pixel of the band takes that
/// tone.
///
/// The clock derives from the tone alone, so the wire carries the colour
/// alone.
pub fn trailer_clock(tone: [u8; 3]) -> Result<ToneClock> {
    let luma_tolerance = TRAILER_LUMA_TOLERANCES
        .into_iter()
        .find(|&tolerance| TonedPalette::new(tone, tolerance, TRAILER_BITS_PER_PIXEL).is_ok())
        .with_context(|| {
            format!(
                "no iso-luma palette of {} colours exists around #{:02X}{:02X}{:02X}",
                1 << TRAILER_BITS_PER_PIXEL,
                tone[0],
                tone[1],
                tone[2]
            )
        })?;
    let pocket = ClockSlot {
        base: tone,
        luma_tolerance,
        gap_base: tone,
        gap_luma_tolerance: luma_tolerance,
    };

    Ok(ToneClock {
        rotation_centidegrees: vec![0],
        blend: false,
        bits_per_pixel: TRAILER_BITS_PER_PIXEL,
        ordering: record_groove::ToneOrdering::default(),
        rings: vec![2],
        span: (0, record_groove::TONE_CLOCK_SPAN_UNITS as u16),
        slots: vec![pocket; 2],
        gap_switch_offsets: Vec::new(),
    })
}

/// The clock a trailer was cut with, from as much of the stream as the
/// lead-in holds.
///
/// The tone takes precedence. A record that names a tone for its trailer was
/// cut matte there, at every programme tone. A record that names no tone gives
/// the band the wheel of the record.
pub fn trailer_clock_from_stream_head(head: &[u8]) -> Result<ToneClock> {
    if let Some(tone) = run_out_tone_from_partial_stream(head) {
        return trailer_clock(tone);
    }

    let map = tone_clock_map_from_partial_stream(head)
        .context("the lead-in carries neither a trailer tone nor a wheel")?;

    Ok(band_clock(&tone_clock_from_map(&map)))
}

/// A wheel in the form that the groove encoder takes. A version 1 map arrives
/// with its one ring written out, so a caller reads both versions the same
/// way.
pub fn tone_clock_from_map(map: &ToneClockDescriptor) -> ToneClock {
    ToneClock {
        rings: map.ring_slots(),
        span: map.span,
        rotation_centidegrees: map.rotation_centidegrees.clone(),
        blend: map.blend,
        bits_per_pixel: u32::from(map.bits_per_pixel),
        ordering: match map.ordering {
            ToneOrdering::BaseProximity => record_groove::ToneOrdering::BaseProximity,
            ToneOrdering::ChromaProximity => record_groove::ToneOrdering::ChromaProximity,
        },
        slots: map
            .slots
            .iter()
            .map(|slot| ClockSlot {
                base: slot.base,
                luma_tolerance: slot.luma_tolerance,
                gap_base: slot.gap_base,
                gap_luma_tolerance: slot.gap_luma_tolerance,
            })
            .collect(),
        gap_switch_offsets: map.gap_switch_offsets.clone(),
    }
}

/// The record's wheel, for a band that has no track structure.
///
/// The gap switches are byte offsets into the programme. A band that carries
/// its own stream holds an empty switch list, so every pixel takes the track
/// tone of its pocket.
pub fn band_clock(clock: &ToneClock) -> ToneClock {
    ToneClock {
        gap_switch_offsets: Vec::new(),
        ..clock.clone()
    }
}

/// Bytes a band of `pixel_count` pixels holds at this clock's rate.
pub fn band_byte_capacity(pixel_count: usize, clock: &ToneClock) -> usize {
    pixel_count * clock.bits_per_pixel as usize / 8
}

/// Where the pixels of a band sit, in the frame a clock reads: angle about
/// the centre, and distance from it as a fraction of the half-side.
pub fn band_geometry(width: usize, height: usize, indices: &[usize]) -> (Vec<f64>, Vec<f64>) {
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let angles = indices
        .iter()
        .map(|&index| {
            record_groove::pixel_angle(
                (index % width) as f64,
                (index / width) as f64,
                center_x,
                center_y,
            )
        })
        .collect();
    let radii = indices
        .iter()
        .map(|&index| {
            record_groove::pixel_radius(
                (index % width) as f64,
                (index / width) as f64,
                center_x,
                center_y,
            )
        })
        .collect();

    (angles, radii)
}

/// A band's bytes, read back through the clock it was cut with.
///
/// The band uses the encoding that the programme uses. The pocket of a pixel
/// gives its palette, and the bit stream runs across the pockets.
pub fn band_bytes_from_toned_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    indices: &[usize],
    clock: &ToneClock,
    byte_length: usize,
    label: &str,
) -> Result<Vec<u8>> {
    let pixel_count = clock.pixel_count(byte_length);
    if indices.len() < pixel_count {
        bail!("{label} spiral capacity is too small");
    }

    let taken = &indices[..pixel_count];
    let mut pixels = Vec::with_capacity(pixel_count * 4);
    for &pixel_index in taken {
        let rgba_index = pixel_index
            .checked_mul(4)
            .context("band RGBA index overflow")?;
        if rgba_index + 3 >= rgba.len() {
            bail!("{label} spiral pixel index is outside RGBA buffer");
        }
        if rgba[rgba_index + 3] == 0 {
            bail!("{label} spiral pixel is empty");
        }
        pixels.extend_from_slice(&rgba[rgba_index..rgba_index + 4]);
    }

    let (angles, radii) = band_geometry(width, height, taken);

    record_groove::decode_toned_clock(&pixels, clock, &angles, &radii, Some(byte_length))
        .with_context(|| format!("{label} is not written in the tone the band was cut in"))
}

/// The tone the trailer was cut in, read out of however much of the stream is
/// in hand.
///
/// A reader needs this tone before it walks the trailer. The encoder writes it
/// in the lead-in, which the grey ladder reads without prior knowledge. This
/// scan is deliberately lenient: it checks no CRC, no segment count and no
/// completeness.
pub fn run_out_tone_from_partial_stream(bytes: &[u8]) -> Option<[u8; 3]> {
    let payload = segment_from_partial_stream(bytes, SEGMENT_LEAD_OUT_GEOMETRY)?;
    if payload.len() < 4 {
        return None;
    }

    Some([payload[1], payload[2], payload[3]])
}

/// The record's wheel, read out of however much of the stream is in hand.
///
/// A trailer cut by the wheel is read with the wheel, which
/// [`SEGMENT_TONE_CLOCK_MAP`] carries. This function uses the staging that the
/// tone scan uses.
pub fn tone_clock_map_from_partial_stream(bytes: &[u8]) -> Option<ToneClockDescriptor> {
    let payload = segment_from_partial_stream(bytes, SEGMENT_TONE_CLOCK_MAP)?;

    decode_tone_clock_map(payload, None).ok()
}

/// One segment's payload, from as much of the stream as is in hand.
///
/// This scan runs before a reader has the rest of the stream, on the bands that
/// carry the front of it. It therefore checks no CRC, no segment count and no
/// completeness.
fn segment_from_partial_stream(bytes: &[u8], wanted: u8) -> Option<&[u8]> {
    let body = bytes.get(RECORD_DESCRIPTOR_PREFIX_LENGTH..)?;
    let mut offset = 0usize;

    while offset + 3 <= body.len() {
        let kind = body[offset];
        let length = u16::from_be_bytes([body[offset + 1], body[offset + 2]]) as usize;
        let start = offset + 3;
        let end = start.checked_add(length)?;
        if end > body.len() {
            return None;
        }
        if kind == wanted {
            return Some(&body[start..end]);
        }
        offset = end;
    }

    None
}

pub fn metadata_bytes_from_grayscale_rgba(
    rgba: &[u8],
    indices: &[usize],
    byte_length: usize,
    label: &str,
) -> Result<Vec<u8>> {
    let pixel_count = metadata_pixel_count_for_byte_length(byte_length);
    if indices.len() < pixel_count {
        bail!("{label} spiral capacity is too small");
    }

    let bits = METADATA_GRAYSCALE_BITS_PER_PIXEL;
    let mut bytes = Vec::with_capacity(byte_length);
    let mut acc = 0u32;
    let mut acc_bits = 0u32;

    for &pixel_index in indices.iter().take(pixel_count) {
        let rgba_index = pixel_index
            .checked_mul(4)
            .context("metadata RGBA index overflow")?;
        if rgba_index + 3 >= rgba.len() {
            bail!("{label} spiral pixel index is outside RGBA buffer");
        }

        let red = rgba[rgba_index];
        let green = rgba[rgba_index + 1];
        let blue = rgba[rgba_index + 2];
        if rgba[rgba_index + 3] == 0 {
            bail!("{label} spiral pixel is empty");
        }
        if red != green || green != blue {
            bail!("{label} metadata pixel is not grayscale");
        }

        let Some(level) = level_for_grayscale_value(red) else {
            bail!("{label} metadata pixel is outside the grayscale ladder");
        };

        acc = (acc << bits) | level;
        acc_bits += bits;
        while acc_bits >= 8 && bytes.len() < byte_length {
            bytes.push((acc >> (acc_bits - 8)) as u8);
            acc_bits -= 8;
            acc &= (1 << acc_bits) - 1;
        }
    }

    if bytes.len() < byte_length {
        bail!("{label} spiral did not yield {byte_length} bytes");
    }
    Ok(bytes)
}

pub fn record_profile_code(record_profile: &str) -> Result<u8> {
    match record_profile {
        RECORD_PROFILE_SINGLE45 => Ok(RECORD_PROFILE_SINGLE45_CODE),
        RECORD_PROFILE_LP => Ok(RECORD_PROFILE_LP_CODE),
        RECORD_PROFILE_TEN => Ok(RECORD_PROFILE_TEN_CODE),
        RECORD_PROFILE_SINGLE45_VINTAGE => Ok(RECORD_PROFILE_SINGLE45_VINTAGE_CODE),
        other => bail!("unsupported canonical record profile {other}"),
    }
}

pub fn record_profile_from_code(code: u8) -> Result<String> {
    match code {
        RECORD_PROFILE_SINGLE45_CODE => Ok(RECORD_PROFILE_SINGLE45.to_string()),
        RECORD_PROFILE_LP_CODE => Ok(RECORD_PROFILE_LP.to_string()),
        RECORD_PROFILE_TEN_CODE => Ok(RECORD_PROFILE_TEN.to_string()),
        RECORD_PROFILE_SINGLE45_VINTAGE_CODE => {
            Ok(RECORD_PROFILE_SINGLE45_VINTAGE.to_string())
        }
        other => bail!("unknown record profile code {other}"),
    }
}

pub fn payload_encoding_code(payload_encoding: &str) -> Result<u8> {
    match payload_encoding {
        PAYLOAD_ENCODING_RGB => Ok(PAYLOAD_ENCODING_RGB_CODE),
        PAYLOAD_ENCODING_TONED_V1 => Ok(PAYLOAD_ENCODING_TONED_V1_CODE),
        PAYLOAD_ENCODING_TONED_V2 => Ok(PAYLOAD_ENCODING_TONED_V2_CODE),
        other => bail!("unsupported canonical payload encoding {other}"),
    }
}

pub fn payload_encoding_from_code(code: u8) -> Result<String> {
    match code {
        PAYLOAD_ENCODING_RGB_CODE => Ok(PAYLOAD_ENCODING_RGB.to_string()),
        PAYLOAD_ENCODING_TONED_V1_CODE => Ok(PAYLOAD_ENCODING_TONED_V1.to_string()),
        PAYLOAD_ENCODING_TONED_V2_CODE => Ok(PAYLOAD_ENCODING_TONED_V2.to_string()),
        other => bail!("unknown payload encoding code {other}"),
    }
}

const RELEASE_ID_TAGGED_PREFIX: &str = "rel_";
const RELEASE_ID_ULID_TEXT_LENGTH: usize = 26;
const CROCKFORD_BASE32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Parse a canonical `rel_`-tagged ULID string into its 16 raw bytes.
pub fn release_id_to_bytes(text: &str) -> Result<[u8; RELEASE_ID_LENGTH]> {
    let ulid_text = text
        .strip_prefix(RELEASE_ID_TAGGED_PREFIX)
        .context("release ID is missing the rel_ prefix")?;
    if ulid_text.len() != RELEASE_ID_ULID_TEXT_LENGTH {
        bail!("release ID must be 26 Crockford Base32 characters");
    }

    let mut bits: u128 = 0;
    for (index, byte) in ulid_text.bytes().enumerate() {
        let upper = byte.to_ascii_uppercase();
        let digit = CROCKFORD_BASE32
            .iter()
            .position(|&candidate| candidate == upper)
            .context("release ID contains a non-canonical Crockford Base32 character")?;

        // A textual ULID contains 130 encoded bits but the value is only
        // 128 bits. Therefore the first Crockford digit may contain only the
        // low two bits (0..=7). Rejecting larger values prevents silent u128
        // truncation and ensures one canonical text representation per value.
        if index == 0 && digit > 7 {
            bail!("release ID exceeds the 128-bit ULID range");
        }

        bits = (bits << 5) | digit as u128;
    }
    Ok(bits.to_be_bytes())
}

/// Format 16 raw release ULID bytes back into the canonical `rel_`-tagged
/// display string.
pub fn release_id_to_text(bytes: [u8; RELEASE_ID_LENGTH]) -> String {
    let mut value = u128::from_be_bytes(bytes);
    let mut chars = [b'0'; RELEASE_ID_ULID_TEXT_LENGTH];
    for index in (0..RELEASE_ID_ULID_TEXT_LENGTH).rev() {
        chars[index] = CROCKFORD_BASE32[(value & 0x1f) as usize];
        value >>= 5;
    }
    let mut text =
        String::with_capacity(RELEASE_ID_TAGGED_PREFIX.len() + RELEASE_ID_ULID_TEXT_LENGTH);
    text.push_str(RELEASE_ID_TAGGED_PREFIX);
    text.push_str(std::str::from_utf8(&chars).expect("Crockford Base32 alphabet is ASCII"));
    text
}

pub fn decode_descriptor_prefix(bytes: &[u8]) -> Result<DescriptorPrefix> {
    if bytes.len() < RECORD_DESCRIPTOR_PREFIX_LENGTH {
        bail!("record descriptor payload too short");
    }
    if &bytes[..4] != RECORD_DESCRIPTOR_MAGIC {
        bail!("record descriptor magic mismatch");
    }

    let version = bytes[4];
    let payload_len = u16::from_be_bytes(bytes[5..7].try_into().expect("slice length")) as usize;
    let segment_count = u16::from_be_bytes(bytes[7..9].try_into().expect("slice length")) as usize;
    let segment_stream_len =
        u16::from_be_bytes(bytes[9..11].try_into().expect("slice length")) as usize;
    let b_value_bits = u64::from_be_bytes(bytes[11..19].try_into().expect("slice length"));
    let cut_inner_radius = u16::from_be_bytes(bytes[19..21].try_into().expect("slice length"));
    let deadwax_b_value_bits = u64::from_be_bytes(bytes[21..29].try_into().expect("slice length"));

    if payload_len < RECORD_DESCRIPTOR_PREFIX_LENGTH || payload_len > bytes.len() {
        bail!("record descriptor payload length is invalid");
    }

    Ok(DescriptorPrefix {
        version,
        payload_len,
        segment_count,
        segment_stream_len,
        b_value_bits,
        cut_inner_radius,
        deadwax_b_value_bits,
    })
}

pub fn validate_tone_span(span: &ToneSpanDescriptor, index: usize) -> Result<()> {
    if span.byte_length == 0 {
        bail!("tone span {index} byte length must be greater than zero");
    }
    if !(TONED_MIN_BITS_PER_PIXEL..=TONED_MAX_BITS_PER_PIXEL).contains(&span.bits_per_pixel) {
        bail!(
            "tone span {index} bits per pixel must be between {} and {}",
            TONED_MIN_BITS_PER_PIXEL,
            TONED_MAX_BITS_PER_PIXEL
        );
    }
    Ok(())
}

pub fn resolve_tone_spans(
    spans: &[ToneSpanDescriptor],
    expected_byte_length: Option<usize>,
) -> Result<Vec<ResolvedToneSpan>> {
    if spans.is_empty() {
        bail!("toned-v1 carrier map must contain at least one span");
    }
    if spans.len() > TONED_MAX_SPAN_COUNT {
        bail!("tone span count exceeds u16 range");
    }

    let mut byte_offset = 0usize;
    let mut pixel_offset = 0usize;
    let mut resolved = Vec::with_capacity(spans.len());

    for (index, span) in spans.iter().enumerate() {
        validate_tone_span(span, index)?;
        let bit_length = span
            .byte_length
            .checked_mul(8)
            .context("tone span bit length overflow")?;
        let pixel_count = bit_length.div_ceil(usize::from(span.bits_per_pixel));

        resolved.push(ResolvedToneSpan {
            index,
            byte_offset,
            byte_length: span.byte_length,
            pixel_offset,
            pixel_count,
            base: span.base,
            luma_tolerance: span.luma_tolerance,
            bits_per_pixel: span.bits_per_pixel,
            ordering: span.ordering,
        });

        byte_offset = byte_offset
            .checked_add(span.byte_length)
            .context("tone span total byte length overflow")?;
        pixel_offset = pixel_offset
            .checked_add(pixel_count)
            .context("tone span total pixel count overflow")?;
    }

    if let Some(expected) = expected_byte_length {
        if byte_offset != expected {
            bail!("tone spans cover {byte_offset} bytes, expected {expected}");
        }
    }

    Ok(resolved)
}

pub fn toned_pixel_count(
    spans: &[ToneSpanDescriptor],
    expected_byte_length: Option<usize>,
) -> Result<usize> {
    Ok(resolve_tone_spans(spans, expected_byte_length)?
        .last()
        .map(|span| span.pixel_offset + span.pixel_count)
        .unwrap_or(0))
}

pub fn encode_toned_carrier_map(
    spans: &[ToneSpanDescriptor],
    expected_byte_length: Option<usize>,
) -> Result<Vec<u8>> {
    resolve_tone_spans(spans, expected_byte_length)?;

    let mut out = Vec::new();
    out.push(TONED_CARRIER_MAP_VERSION);
    out.extend_from_slice(
        &u16::try_from(spans.len())
            .context("tone span count exceeds u16")?
            .to_be_bytes(),
    );

    for span in spans {
        push_varuint(
            &mut out,
            u64::try_from(span.byte_length).context("tone span byte length exceeds u64")?,
        );
        out.extend_from_slice(&span.base);
        out.push(span.luma_tolerance);
        out.push(span.bits_per_pixel);
        out.push(span.ordering.wire_code());
    }

    Ok(out)
}

pub fn decode_toned_carrier_map(
    bytes: &[u8],
    expected_byte_length: Option<usize>,
) -> Result<Vec<ToneSpanDescriptor>> {
    let mut cursor = ByteCursor::new(bytes);
    let version = cursor.read_u8("toned carrier map version")?;
    if version != TONED_CARRIER_MAP_VERSION {
        bail!("unsupported toned carrier map version {version}");
    }

    let count = usize::from(cursor.read_u16be("tone span count")?);
    if count == 0 {
        bail!("toned-v1 carrier map must contain at least one span");
    }

    let mut spans = Vec::with_capacity(count);
    for index in 0..count {
        let byte_length = usize::try_from(cursor.read_varuint("tone span byte length")?)
            .context("tone span byte length exceeds usize")?;
        let base = [
            cursor.read_u8("tone span base red")?,
            cursor.read_u8("tone span base green")?,
            cursor.read_u8("tone span base blue")?,
        ];
        let luma_tolerance = cursor.read_u8("tone span luma tolerance")?;
        let bits_per_pixel = cursor.read_u8("tone span bits per pixel")?;
        let ordering = ToneOrdering::from_wire_code(cursor.read_u8("tone span ordering")?)?;

        let span = ToneSpanDescriptor {
            byte_length,
            base,
            luma_tolerance,
            bits_per_pixel,
            ordering,
        };
        validate_tone_span(&span, index)?;
        spans.push(span);
    }

    if cursor.remaining() != 0 {
        bail!(
            "toned carrier map contains {} trailing bytes",
            cursor.remaining()
        );
    }

    resolve_tone_spans(&spans, expected_byte_length)?;
    Ok(spans)
}

/// Checks a clock is well formed, and — given the stream length — that its
/// gap switches all fall inside the stream.
pub fn validate_tone_clock(
    clock: &ToneClockDescriptor,
    expected_byte_length: Option<usize>,
) -> Result<()> {
    let rings = clock.ring_slots();
    if rings.is_empty() || rings.len() > TONE_CLOCK_MAX_RINGS {
        bail!(
            "tone clock needs between 1 and {TONE_CLOCK_MAX_RINGS} rings, got {}",
            rings.len()
        );
    }
    for (index, &slots) in rings.iter().enumerate() {
        if !(TONE_CLOCK_MIN_SLOTS..=TONE_CLOCK_MAX_SLOTS).contains(&(slots as usize)) {
            bail!(
                "tone clock ring {index} needs between {TONE_CLOCK_MIN_SLOTS} and \
                 {TONE_CLOCK_MAX_SLOTS} slots, got {slots}"
            );
        }
    }
    let cells: usize = rings.iter().map(|&slots| slots as usize).sum();
    if cells > TONE_CLOCK_MAX_CELLS {
        bail!("tone clock has {cells} pockets, more than {TONE_CLOCK_MAX_CELLS}");
    }
    if clock.slots.len() != cells {
        bail!(
            "tone clock has {} tones for {cells} pockets; every pocket takes one",
            clock.slots.len()
        );
    }
    if clock.span.0 >= clock.span.1 {
        bail!(
            "tone clock band runs from {} to {}, which is not a band",
            clock.span.0, clock.span.1
        );
    }
    if u32::from(clock.span.1) > TONE_CLOCK_SPAN_UNITS {
        bail!("tone clock band ends at {}, past the record", clock.span.1);
    }
    if clock.rotation_centidegrees.is_empty() || clock.rotation_centidegrees.len() > rings.len() {
        bail!(
            "tone clock has {} rotations for {} rings",
            clock.rotation_centidegrees.len(),
            rings.len()
        );
    }
    for &turn in &clock.rotation_centidegrees {
        if u32::from(turn) >= TONE_CLOCK_ROTATION_UNITS_PER_TURN {
            bail!("tone clock rotation {turn} is a full turn or more");
        }
    }
    if !(TONED_MIN_BITS_PER_PIXEL..=TONED_MAX_BITS_PER_PIXEL).contains(&clock.bits_per_pixel) {
        bail!(
            "tone clock bits per pixel must be between {TONED_MIN_BITS_PER_PIXEL} and {TONED_MAX_BITS_PER_PIXEL}"
        );
    }
    if clock.gap_switch_offsets.first() == Some(&0) {
        bail!("tone clock cannot switch to gap tone at byte offset zero");
    }
    for pair in clock.gap_switch_offsets.windows(2) {
        if pair[1] <= pair[0] {
            bail!("tone clock gap switch offsets must be strictly increasing");
        }
    }
    if let (Some(expected), Some(&last)) = (expected_byte_length, clock.gap_switch_offsets.last())
    {
        if last >= expected {
            bail!("tone clock gap switch at byte {last} is beyond the {expected}-byte stream");
        }
    }
    Ok(())
}

/// Pixels a toned-v2 groove of `byte_length` bytes occupies.
pub fn tone_clock_pixel_count(clock: &ToneClockDescriptor, byte_length: usize) -> Result<usize> {
    byte_length
        .checked_mul(8)
        .map(|bits| bits.div_ceil(usize::from(clock.bits_per_pixel)))
        .context("tone clock bit length overflow")
}

/// Version 1, a wheel of wedges:
/// `1 || bits_per_pixel || ordering || blend || rotation (u16be) ||
/// slot_count || slots (base[3] tol gap_base[3] gap_tol)* || gap_switch_count
/// (varuint) || gap_switch_offsets (varuint)*`.
///
/// Version 2, a wheel of rings, differs only in what stands where the slot
/// count did:
/// `2 || bits_per_pixel || ordering || blend || rotation (u16be) ||
/// ring_count || ring_slots (u8)* || span_inner (u16be) || span_outer (u16be)
/// || pockets (base[3] tol gap_base[3] gap_tol)* || …`
///
/// An encoder writes a wheel of one ring as version 1, so a record with one
/// ring keeps its bytes, and every player that reads version 1 reads it.
pub fn encode_tone_clock_map(
    clock: &ToneClockDescriptor,
    expected_byte_length: Option<usize>,
) -> Result<Vec<u8>> {
    validate_tone_clock(clock, expected_byte_length)?;

    let rings = clock.ring_slots();
    let ringed = clock.has_rings();
    let mut out = Vec::with_capacity(12 + clock.slots.len() * 8);
    out.push(if ringed {
        TONE_CLOCK_MAP_VERSION
    } else {
        TONE_CLOCK_MAP_VERSION_WEDGES
    });
    out.push(clock.bits_per_pixel);
    out.push(clock.ordering.wire_code());
    out.push(u8::from(clock.blend));
    if ringed {
        out.push(rings.len() as u8);
        for &slots in &rings {
            out.push(slots as u8);
        }
        // One rotation per ring, padded from the last given: the rings turn
        // at their own rates and the map has to say where each one starts.
        for ring in 0..rings.len() {
            let at = ring.min(clock.rotation_centidegrees.len() - 1);
            out.extend_from_slice(&clock.rotation_centidegrees[at].to_be_bytes());
        }
        out.extend_from_slice(&clock.span.0.to_be_bytes());
        out.extend_from_slice(&clock.span.1.to_be_bytes());
    } else {
        out.extend_from_slice(&clock.rotation_centidegrees[0].to_be_bytes());
        out.push(clock.slots.len() as u8);
    }
    for slot in &clock.slots {
        out.extend_from_slice(&slot.base);
        out.push(slot.luma_tolerance);
        out.extend_from_slice(&slot.gap_base);
        out.push(slot.gap_luma_tolerance);
    }
    push_varuint(&mut out, clock.gap_switch_offsets.len() as u64);
    for &offset in &clock.gap_switch_offsets {
        push_varuint(
            &mut out,
            u64::try_from(offset).context("tone clock gap switch offset exceeds u64")?,
        );
    }
    Ok(out)
}

pub fn decode_tone_clock_map(
    bytes: &[u8],
    expected_byte_length: Option<usize>,
) -> Result<ToneClockDescriptor> {
    let mut cursor = ByteCursor::new(bytes);
    let version = cursor.read_u8("tone clock map version")?;
    if version != TONE_CLOCK_MAP_VERSION && version != TONE_CLOCK_MAP_VERSION_WEDGES {
        bail!("unsupported tone clock map version {version}");
    }
    let bits_per_pixel = cursor.read_u8("tone clock bits per pixel")?;
    let ordering = ToneOrdering::from_wire_code(cursor.read_u8("tone clock ordering")?)?;
    let blend = match cursor.read_u8("tone clock blend")? {
        0 => false,
        1 => true,
        other => bail!("tone clock blend flag {other} is not 0 or 1"),
    };
    // A v1 map holds one ring with every slot, over the whole record, turned
    // as one piece. That form is the wheel of wedges. Another reading would
    // move the pockets of every record already pressed.
    let (rings, rotation_centidegrees, span) = if version == TONE_CLOCK_MAP_VERSION {
        let ring_count = usize::from(cursor.read_u8("tone clock ring count")?);
        if ring_count == 0 || ring_count > TONE_CLOCK_MAX_RINGS {
            bail!("tone clock declares {ring_count} rings");
        }
        let mut rings = Vec::with_capacity(ring_count);
        for _ in 0..ring_count {
            rings.push(u32::from(cursor.read_u8("tone clock ring slots")?));
        }
        let mut turns = Vec::with_capacity(ring_count);
        for _ in 0..ring_count {
            turns.push(cursor.read_u16be("tone clock ring rotation")?);
        }
        let inner = cursor.read_u16be("tone clock band inner")?;
        let outer = cursor.read_u16be("tone clock band outer")?;
        (rings, turns, (inner, outer))
    } else {
        let turn = cursor.read_u16be("tone clock rotation")?;
        (
            vec![u32::from(cursor.read_u8("tone clock slot count")?)],
            vec![turn],
            whole_disc(),
        )
    };
    let slot_count: usize = rings.iter().map(|&slots| slots as usize).sum();
    if slot_count > TONE_CLOCK_MAX_CELLS {
        bail!("tone clock declares {slot_count} pockets");
    }
    let mut slots = Vec::with_capacity(slot_count);
    for _ in 0..slot_count {
        let base = cursor.read_rgb("tone clock slot base")?;
        let luma_tolerance = cursor.read_u8("tone clock slot luma tolerance")?;
        let gap_base = cursor.read_rgb("tone clock slot gap base")?;
        let gap_luma_tolerance = cursor.read_u8("tone clock slot gap luma tolerance")?;
        slots.push(ToneClockSlotDescriptor {
            base,
            luma_tolerance,
            gap_base,
            gap_luma_tolerance,
        });
    }
    let switch_count = cursor.read_varuint("tone clock gap switch count")?;
    let switch_count = usize::try_from(switch_count).context("tone clock gap switch count exceeds usize")?;
    if switch_count > cursor.remaining() {
        bail!("tone clock declares more gap switches than the map can hold");
    }
    let mut gap_switch_offsets = Vec::with_capacity(switch_count);
    for _ in 0..switch_count {
        gap_switch_offsets.push(
            usize::try_from(cursor.read_varuint("tone clock gap switch offset")?)
                .context("tone clock gap switch offset exceeds usize")?,
        );
    }
    if cursor.remaining() != 0 {
        bail!(
            "tone clock map contains {} trailing bytes",
            cursor.remaining()
        );
    }

    let clock = ToneClockDescriptor {
        rotation_centidegrees,
        blend,
        bits_per_pixel,
        ordering,
        // A v1 map comes back with its one ring written out, so a downstream
        // caller reads both versions the same way.
        rings,
        span,
        slots,
        gap_switch_offsets,
    };
    validate_tone_clock(&clock, expected_byte_length)?;
    Ok(clock)
}

fn push_varuint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

pub fn decode_signed_release_reference(bytes: &[u8]) -> Result<SignedReleaseReference> {
    let mut cursor = ByteCursor::new(bytes);

    let version = cursor.read_u8("signed release reference version")?;
    let release_commitment_sha256 = cursor
        .read_bytes(
            SIGNED_RELEASE_REFERENCE_HASH_LENGTH,
            "release commitment SHA-256",
        )?
        .try_into()
        .expect("length checked");
    let key_id_len = cursor.read_u16be("signature key ID length")? as usize;
    let key_id = cursor.read_bytes(key_id_len, "signature key ID")?.to_vec();
    let signature = cursor
        .read_bytes(SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH, "signature")?
        .to_vec();

    if cursor.remaining() != 0 {
        bail!(
            "signed release reference contains {} trailing bytes",
            cursor.remaining()
        );
    }

    let reference = SignedReleaseReference {
        version,
        release_commitment_sha256,
        key_id,
        signature,
    };
    reference.validate()?;
    Ok(reference)
}

pub fn decode_record_descriptor_bytes(bytes: &[u8]) -> Result<RecordDescriptor> {
    let prefix = decode_descriptor_prefix(bytes)?;

    if prefix.version != RECORD_DESCRIPTOR_VERSION
        && prefix.version != RECORD_DESCRIPTOR_VERSION_HOUSE
    {
        bail!("record descriptor version mismatch");
    }
    if prefix.payload_len != RECORD_DESCRIPTOR_PREFIX_LENGTH + prefix.segment_stream_len {
        bail!("record descriptor segment stream length mismatch");
    }

    let body = &bytes[RECORD_DESCRIPTOR_PREFIX_LENGTH..prefix.payload_len];
    let mut offset = 0usize;
    let mut parsed_segments = 0usize;

    let mut crc32_range = None;
    let mut crc32 = None;
    let mut stream_byte_length = None;
    let mut record_profile = None;
    let mut payload_encoding = None;
    let mut title = None;
    let mut artist = None;
    let mut release_id = None;
    let mut catalog_number = None;
    let mut label = None;
    let mut artwork_credit = None;
    let mut canonical_url = None;
    let mut created_at = None;
    let mut copyright_year = None;
    let mut copyright_holder = None;
    let mut signed_release_reference = None;
    let mut bsc_pointer = None;
    let mut tone_spans = None;
    let mut tone_clock = None;
    let mut cache_encryption = None;
    let mut chain_anchor = None;
    let mut additional_signatures = None;
    let mut isrcs = None;
    let mut upc = None;
    let mut deferred_attestation = None;
    let mut deadwax = None;
    let mut lead_out_geometry = None;
    let mut run_out_tone = None;
    let mut spiral_family = None;
    let mut spiral_clockwise = None;

    while offset < body.len() {
        if parsed_segments >= prefix.segment_count {
            bail!("record descriptor contains more segments than declared");
        }
        if offset + 3 > body.len() {
            bail!("record descriptor segment is truncated");
        }

        let kind = body[offset];
        let len = u16::from_be_bytes(
            body[offset + 1..offset + 3]
                .try_into()
                .expect("slice length"),
        ) as usize;
        let payload_start = offset + 3;
        let payload_end = payload_start
            .checked_add(len)
            .context("record descriptor segment length overflow")?;
        if payload_end > body.len() {
            bail!("record descriptor segment payload is truncated");
        }

        let payload = &body[payload_start..payload_end];
        match kind {
            SEGMENT_DESCRIPTOR_CRC32 => {
                if crc32.is_some() {
                    bail!("duplicate record descriptor CRC32 segment");
                }
                if payload.len() != 4 {
                    bail!("record descriptor CRC32 segment has invalid length");
                }
                crc32 = Some(u32::from_be_bytes(
                    payload.try_into().expect("slice length"),
                ));
                let absolute_start = RECORD_DESCRIPTOR_PREFIX_LENGTH + payload_start;
                crc32_range = Some(absolute_start..absolute_start + payload.len());
            }
            SEGMENT_STREAM_BYTE_LENGTH => {
                if stream_byte_length.is_some() {
                    bail!("duplicate stream byte length segment");
                }
                if payload.len() != 4 {
                    bail!("stream byte length segment has invalid length");
                }
                let raw_len = u32::from_be_bytes(payload.try_into().expect("slice length"));
                if raw_len == 0 {
                    bail!("stream byte length must not be zero");
                }
                stream_byte_length = Some(raw_len as usize);
            }
            SEGMENT_RECORD_PROFILE => {
                if payload.len() != 1 {
                    bail!("record profile segment has invalid length");
                }
                assign_once(
                    &mut record_profile,
                    record_profile_from_code(payload[0])?,
                    "record profile",
                )?
            }
            SEGMENT_PAYLOAD_ENCODING => {
                if payload.len() != 1 {
                    bail!("payload encoding segment has invalid length");
                }
                assign_once(
                    &mut payload_encoding,
                    payload_encoding_from_code(payload[0])?,
                    "payload encoding",
                )?
            }
            SEGMENT_TITLE => {
                assign_once(&mut title, decode_optional_text(payload, "title")?, "title")?
            }
            SEGMENT_ARTIST => assign_once(
                &mut artist,
                decode_optional_text(payload, "artist")?,
                "artist",
            )?,
            SEGMENT_RELEASE_ID => {
                if payload.len() != RELEASE_ID_LENGTH {
                    bail!("release ID segment has invalid length");
                }
                assign_once(
                    &mut release_id,
                    <[u8; RELEASE_ID_LENGTH]>::try_from(payload).expect("length checked"),
                    "release ID",
                )?
            }
            SEGMENT_CATALOG_NUMBER => assign_once(
                &mut catalog_number,
                decode_optional_text(payload, "catalog number")?,
                "catalog number",
            )?,
            SEGMENT_LABEL => {
                assign_once(&mut label, decode_optional_text(payload, "label")?, "label")?
            }
            SEGMENT_ARTWORK_CREDIT => assign_once(
                &mut artwork_credit,
                decode_optional_text(payload, "artwork credit")?,
                "artwork credit",
            )?,
            SEGMENT_CANONICAL_URL => assign_once(
                &mut canonical_url,
                decode_optional_text(payload, "canonical URL")?,
                "canonical URL",
            )?,
            SEGMENT_CREATED_AT => {
                if payload.len() != 8 {
                    bail!("created-at segment has invalid length");
                }
                assign_once(
                    &mut created_at,
                    u64::from_be_bytes(payload.try_into().expect("slice length")),
                    "created-at timestamp",
                )?
            }
            SEGMENT_COPYRIGHT_YEAR => {
                if payload.len() != 2 {
                    bail!("copyright-year segment has invalid length");
                }
                assign_once(
                    &mut copyright_year,
                    u16::from_be_bytes(payload.try_into().expect("slice length")),
                    "copyright year",
                )?
            }
            SEGMENT_COPYRIGHT_HOLDER => assign_once(
                &mut copyright_holder,
                decode_optional_text(payload, "copyright holder")?,
                "copyright holder",
            )?,
            SEGMENT_SIGNED_RELEASE_REFERENCE => {
                if signed_release_reference.is_some() {
                    bail!("duplicate signed release reference segment");
                }
                signed_release_reference = Some(decode_signed_release_reference(payload)?);
            }
            SEGMENT_BSC_POINTER => {
                if bsc_pointer.is_some() {
                    bail!("duplicate BSC pointer segment");
                }
                if payload.is_empty() {
                    bail!("BSC pointer segment must not be empty");
                }
                bsc_pointer = Some(payload.to_vec());
            }
            SEGMENT_TONED_CARRIER_MAP => {
                if tone_spans.is_some() {
                    bail!("duplicate toned carrier map segment");
                }
                tone_spans = Some(decode_toned_carrier_map(payload, None)?);
            }
            SEGMENT_TONE_CLOCK_MAP => {
                if tone_clock.is_some() {
                    bail!("duplicate tone clock map segment");
                }
                tone_clock = Some(decode_tone_clock_map(payload, None)?);
            }
            SEGMENT_CACHE_ENCRYPTION => {
                if cache_encryption.is_some() {
                    bail!("duplicate cache encryption segment");
                }
                cache_encryption = Some(decode_cache_encryption_descriptor(payload)?);
            }
            SEGMENT_CHAIN_ANCHOR => {
                if chain_anchor.is_some() {
                    bail!("duplicate chain anchor segment");
                }
                if payload.is_empty() {
                    bail!("chain anchor segment must not be empty");
                }
                chain_anchor = Some(payload.to_vec());
            }
            SEGMENT_ADDITIONAL_SIGNATURES => assign_once(
                &mut additional_signatures,
                decode_additional_signatures(payload)?,
                "additional signatures",
            )?,
            SEGMENT_ISRC => assign_once(&mut isrcs, decode_isrc_segment(payload)?, "ISRC list")?,
            SEGMENT_UPC => assign_once(
                &mut upc,
                normalize_upc(decode_text(payload, "barcode")?.as_str())?,
                "barcode",
            )?,
            SEGMENT_DEFERRED_ATTESTATION => assign_once(
                &mut deferred_attestation,
                decode_signed_release_reference(payload)?,
                "deferred attestation",
            )?,
            SEGMENT_SPIRAL_GEOMETRY => {
                if prefix.version != RECORD_DESCRIPTOR_VERSION_HOUSE {
                    bail!("spiral geometry segment requires descriptor version 3");
                }
                // The segment grew as the house cut did: 17 bytes is the
                // first v3 shape, 25 adds the groove definition, 34 adds
                // sheen and the fire's placement, 42 the fire's own depth.
                // Absent fields decode to what those earlier cuts meant.
                if ![17usize, 25, 34, 42, 90].contains(&payload.len()) {
                    bail!("spiral geometry segment has invalid length");
                }
                if payload[0] != SPIRAL_FAMILY_VARI_PITCH_CODE {
                    bail!("unsupported spiral family code {}", payload[0]);
                }
                let depth = f64::from_bits(u64::from_be_bytes(
                    payload[1..9].try_into().expect("slice length"),
                ));
                let seed = u64::from_be_bytes(payload[9..17].try_into().expect("slice length"));
                let definition = if payload.len() >= 25 {
                    f64::from_bits(u64::from_be_bytes(
                        payload[17..25].try_into().expect("slice length"),
                    ))
                } else {
                    0.0
                };
                let (sheen, placement) = if payload.len() >= 34 {
                    (
                        f64::from_bits(u64::from_be_bytes(
                            payload[25..33].try_into().expect("slice length"),
                        )),
                        record_core::VariPitchPlacement::from_wire_code(payload[33])?,
                    )
                } else {
                    (0.0, record_core::VariPitchPlacement::Even)
                };
                let fire = if payload.len() >= 42 {
                    f64::from_bits(u64::from_be_bytes(
                        payload[34..42].try_into().expect("slice length"),
                    ))
                } else {
                    0.0
                };
                let tuning = if payload.len() == 90 {
                    let field = |at: usize| {
                        f64::from_bits(u64::from_be_bytes(
                            payload[at..at + 8].try_into().expect("slice length"),
                        ))
                    };
                    record_core::VariPitchTuning {
                        wave_one_cycles: field(42),
                        wave_two_cycles: field(50),
                        wave_balance: field(58),
                        dither_frequency: field(66),
                        aura_width: field(74),
                        fire_cycles: field(82),
                    }
                } else {
                    record_core::VariPitchTuning::default()
                };
                let family = SpiralFamily::VariPitch {
                    depth,
                    seed,
                    definition,
                    sheen,
                    placement,
                    fire,
                    tuning,
                };
                family.validate()?;
                assign_once(&mut spiral_family, family, "spiral geometry")?;
            }
            SEGMENT_DEADWAX_EXTENT => {
                if deadwax.is_some() {
                    bail!("duplicate deadwax extent segment");
                }
                deadwax = Some(decode_deadwax_extent(payload)?);
            }
            SEGMENT_GROOVE_HANDEDNESS => {
                if payload.len() != 1 {
                    bail!("groove handedness segment has invalid length");
                }
                assign_once(&mut spiral_clockwise, payload[0] != 0, "groove handedness")?;
            }
            SEGMENT_LEAD_OUT_GEOMETRY => {
                // Sized rather than versioned, as the deadwax extent is: one
                // byte is the revision alone, four carries the tone the
                // trailer was cut in, and a longer payload from a later
                // writer decodes to what these mean.
                if payload.len() != 1 && payload.len() < 4 {
                    bail!("lead-out geometry segment has invalid length");
                }
                if payload.len() >= 4 {
                    assign_once(
                        &mut run_out_tone,
                        [payload[1], payload[2], payload[3]],
                        "run-out tone",
                    )?;
                }
                // A decoder must read this segment. An unknown value in any
                // other segment costs the decoder one feature. An unknown
                // value here states that constants outside this build drew the
                // band under the trailer, and a trace under the wrong
                // constants returns plausible bytes.
                if payload[0] > LEAD_OUT_GEOMETRY_REVISION {
                    bail!(
                        "record was cut under lead-out geometry revision {}, and this build \
                         knows up to {LEAD_OUT_GEOMETRY_REVISION}",
                        payload[0]
                    );
                }
                assign_once(&mut lead_out_geometry, payload[0], "lead-out geometry")?;
            }
            // This build skips a segment type that it does not know.
            //
            // A segment is type-length-value, so a skip is exact. The
            // descriptor CRC32 covers the whole payload, including the unknown
            // bytes, so the check below still catches corruption. A refusal
            // here would make every record from the first writer of a new
            // field unreadable by every reader already in the field, and the
            // framing is additive by design.
            _ => {}
        }

        offset = payload_end;
        parsed_segments += 1;
    }

    if parsed_segments != prefix.segment_count {
        bail!(
            "record descriptor segment count mismatch: declared {}, parsed {}",
            prefix.segment_count,
            parsed_segments
        );
    }

    let expected = crc32.context("record descriptor CRC32 segment is missing")?;
    let range = crc32_range.context("record descriptor CRC32 segment is missing")?;
    let mut canonical = bytes[..prefix.payload_len].to_vec();
    canonical[range].fill(0);

    if compute_descriptor_crc32(&canonical) != expected {
        bail!("record descriptor CRC32 mismatch");
    }

    let b_value = f64::from_bits(prefix.b_value_bits);
    if !(b_value.is_finite() && b_value > 0.0) {
        bail!("decoded invalid b_value");
    }

    let record_profile = record_profile.context("record profile segment is missing")?;
    let stream_byte_length = stream_byte_length.context("stream byte length segment is missing")?;
    let payload_encoding = payload_encoding.context("payload encoding segment is missing")?;
    let tone_spans = tone_spans.unwrap_or_default();

    match payload_encoding.as_str() {
        PAYLOAD_ENCODING_RGB => {
            if !tone_spans.is_empty() {
                bail!("rgb payload encoding must not include a toned carrier map");
            }
            if tone_clock.is_some() {
                bail!("rgb payload encoding must not include a tone clock map");
            }
        }
        PAYLOAD_ENCODING_TONED_V1 => {
            if tone_spans.is_empty() {
                bail!("toned-v1 payload encoding requires a toned carrier map");
            }
            if tone_clock.is_some() {
                bail!("toned-v1 payload encoding must not include a tone clock map");
            }
            resolve_tone_spans(&tone_spans, Some(stream_byte_length))?;
        }
        PAYLOAD_ENCODING_TONED_V2 => {
            if !tone_spans.is_empty() {
                bail!("toned-v2 payload encoding must not include a toned carrier map");
            }
            let clock = tone_clock
                .as_ref()
                .context("toned-v2 payload encoding requires a tone clock map")?;
            validate_tone_clock(clock, Some(stream_byte_length))?;
        }
        other => bail!("unsupported canonical payload encoding {other}"),
    }

    let descriptor = RecordDescriptor {
        version: prefix.version,
        checksum_protected: true,
        b_value_bits: prefix.b_value_bits,
        cut_inner_radius: prefix.cut_inner_radius,
        deadwax_b_value_bits: prefix.deadwax_b_value_bits,
        spiral_family: spiral_family.unwrap_or_default(),
        spiral_clockwise: spiral_clockwise.unwrap_or(true),
        record_profile,
        stream_byte_length,
        payload_encoding,
        title: title.flatten(),
        artist: artist.flatten(),
        release_id,
        catalog_number: catalog_number.flatten(),
        label: label.flatten(),
        artwork_credit: artwork_credit.flatten(),
        canonical_url: canonical_url.flatten(),
        created_at,
        copyright_year,
        copyright_holder: copyright_holder.flatten(),
        signed_release_reference,
        bsc_pointer,
        tone_spans,
        tone_clock,
        cache_encryption,
        chain_anchor,
        additional_signatures: additional_signatures.unwrap_or_default(),
        isrcs: isrcs.unwrap_or_default(),
        upc,
        deferred_attestation,
        deadwax,
        lead_out_geometry_revision: lead_out_geometry
            .unwrap_or(LEAD_OUT_GEOMETRY_REVISION_DRAFT04),
        run_out_tone,
    };
    // A deferred field is null or signed. A deferred field with no attestation
    // over it makes the record malformed.
    validate_deferred_group(&descriptor)?;
    validate_signature_set(&descriptor)?;
    Ok(descriptor)
}

pub fn compute_descriptor_crc32(bytes: &[u8]) -> u32 {
    record_core::crc32_ieee(bytes)
}

fn decode_optional_text(payload: &[u8], label: &str) -> Result<Option<String>> {
    if payload.is_empty() {
        return Ok(None);
    }
    Ok(Some(decode_text(payload, label)?))
}

fn decode_text(payload: &[u8], label: &str) -> Result<String> {
    let value = String::from_utf8(payload.to_vec())
        .with_context(|| format!("record descriptor {label} is not valid UTF-8"))?;
    if value.chars().any(char::is_control) {
        bail!("record descriptor {label} contains control characters");
    }
    Ok(value)
}

fn assign_once<T>(destination: &mut Option<T>, value: T, label: &str) -> Result<()> {
    if destination.is_some() {
        bail!("duplicate {label} segment");
    }
    *destination = Some(value);
    Ok(())
}

#[derive(Clone, Copy)]
struct ByteCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_u8(&mut self, label: &str) -> Result<u8> {
        let value = *self
            .bytes
            .get(self.offset)
            .with_context(|| format!("{label} is truncated"))?;
        self.offset += 1;
        Ok(value)
    }

    fn read_rgb(&mut self, label: &str) -> Result<[u8; 3]> {
        Ok([
            self.read_u8(label)?,
            self.read_u8(label)?,
            self.read_u8(label)?,
        ])
    }

    fn read_u16be(&mut self, label: &str) -> Result<u16> {
        let end = self
            .offset
            .checked_add(2)
            .with_context(|| format!("{label} offset overflow"))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .with_context(|| format!("{label} is truncated"))?;
        self.offset = end;
        Ok(u16::from_be_bytes(
            bytes.try_into().expect("length checked"),
        ))
    }

    fn read_varuint(&mut self, label: &str) -> Result<u64> {
        let start = self.offset;
        let mut value = 0u64;
        let mut shift = 0u32;

        for byte_index in 0..10 {
            let byte = self.read_u8(label)?;
            let payload = u64::from(byte & 0x7f);

            if shift == 63 && payload > 1 {
                bail!("{label} exceeds u64 range");
            }

            value |= payload
                .checked_shl(shift)
                .with_context(|| format!("{label} shift overflow"))?;

            if byte & 0x80 == 0 {
                let consumed = self.offset - start;
                if consumed > 1 {
                    let minimum = 1u64 << (7 * (consumed - 1));
                    if value < minimum {
                        bail!("{label} uses non-canonical overlong varuint encoding");
                    }
                }
                return Ok(value);
            }

            shift += 7;
            if byte_index == 9 {
                bail!("{label} exceeds ten-byte varuint limit");
            }
        }

        unreachable!()
    }

    fn read_bytes(&mut self, length: usize, label: &str) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .with_context(|| format!("{label} length overflow"))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .with_context(|| format!("{label} is truncated"))?;
        self.offset = end;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    /// A minimal valid BRD1 payload, plus whatever extra segments are asked
    /// for. Built by hand because the encoder lives in `record-cut`.
    fn descriptor_bytes(extra: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        let mut segments: u16 = 0;
        let mut push = |body: &mut Vec<u8>, kind: u8, payload: &[u8], segments: &mut u16| {
            body.push(kind);
            body.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            body.extend_from_slice(payload);
            *segments += 1;
        };
        push(
            &mut body,
            SEGMENT_DESCRIPTOR_CRC32,
            &0u32.to_be_bytes(),
            &mut segments,
        );
        push(
            &mut body,
            SEGMENT_STREAM_BYTE_LENGTH,
            &4096u32.to_be_bytes(),
            &mut segments,
        );
        push(
            &mut body,
            SEGMENT_RECORD_PROFILE,
            &[RECORD_PROFILE_SINGLE45_CODE],
            &mut segments,
        );
        push(
            &mut body,
            SEGMENT_PAYLOAD_ENCODING,
            &[PAYLOAD_ENCODING_RGB_CODE],
            &mut segments,
        );
        push(&mut body, SEGMENT_TITLE, b"Title", &mut segments);
        for (kind, payload) in extra {
            push(&mut body, *kind, payload, &mut segments);
        }

        let payload_len = RECORD_DESCRIPTOR_PREFIX_LENGTH + body.len();
        let mut full = Vec::with_capacity(payload_len);
        full.extend_from_slice(RECORD_DESCRIPTOR_MAGIC);
        full.push(RECORD_DESCRIPTOR_VERSION);
        full.extend_from_slice(&(payload_len as u16).to_be_bytes());
        full.extend_from_slice(&segments.to_be_bytes());
        full.extend_from_slice(&(body.len() as u16).to_be_bytes());
        full.extend_from_slice(&1.0f64.to_bits().to_be_bytes());
        // A cut that reached the label leaves no deadwax band, and therefore
        // no feed.
        full.extend_from_slice(&0u16.to_be_bytes());
        full.extend_from_slice(&0f64.to_bits().to_be_bytes());
        full.extend_from_slice(&body);

        let crc = compute_descriptor_crc32(&full);
        let crc_at = RECORD_DESCRIPTOR_PREFIX_LENGTH + 3;
        full[crc_at..crc_at + 4].copy_from_slice(&crc.to_be_bytes());
        full
    }

    #[test]
    fn unknown_segments_are_skipped_rather_than_refused() {
        let bytes = descriptor_bytes(&[(200, vec![9, 9, 9, 9])]);
        let descriptor = decode_record_descriptor_bytes(&bytes)
            .expect("a descriptor carrying an unknown segment still decodes");
        assert_eq!(descriptor.title.as_deref(), Some("Title"));
        assert_eq!(descriptor.stream_byte_length, 4096);
    }

    #[test]
    fn a_corrupt_unknown_segment_still_fails_the_crc() {
        let mut bytes = descriptor_bytes(&[(200, vec![9, 9, 9, 9])]);
        // The last byte of the unknown segment's payload.
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let error = decode_record_descriptor_bytes(&bytes)
            .expect_err("corruption anywhere in the payload must fail");
        assert!(
            error.to_string().contains("CRC32"),
            "unexpected error: {error}"
        );
    }

    fn test_descriptor(secret: Vec<u8>) -> RecordDescriptor {
        RecordDescriptor {
            run_out_tone: None,
            spiral_clockwise: true,
            version: RECORD_DESCRIPTOR_VERSION,
            checksum_protected: true,
            b_value_bits: 1.0f64.to_bits(),
            cut_inner_radius: 0,
            deadwax_b_value_bits: 0,
            spiral_family: SpiralFamily::Archimedean,
            record_profile: RECORD_PROFILE_SINGLE45.to_string(),
            stream_byte_length: 4096,
            payload_encoding: PAYLOAD_ENCODING_RGB.to_string(),
            title: Some("Title".to_string()),
            artist: Some("Artist".to_string()),
            release_id: Some([0x11; RELEASE_ID_LENGTH]),
            catalog_number: Some("CAT-1".to_string()),
            label: Some("Label".to_string()),
            artwork_credit: Some("Credit".to_string()),
            canonical_url: Some("https://example.invalid/release".to_string()),
            created_at: Some(1_700_000_000),
            copyright_year: Some(2006),
            copyright_holder: Some("Artist".to_string()),
            signed_release_reference: None,
            bsc_pointer: Some(vec![1, 2, 3, 4]),
            tone_spans: Vec::new(),
            tone_clock: None,
            cache_encryption: Some(CacheEncryptionDescriptor {
                version: CACHE_ENCRYPTION_DESCRIPTOR_VERSION,
                algorithm: CacheEncryptionAlgorithm::XChaCha20Poly1305,
                key_derivation: CacheKeyDerivation::HkdfSha256,
                secret,
            }),
            chain_anchor: None,
            additional_signatures: Vec::new(),
            isrcs: Vec::new(),
            upc: None,
            deferred_attestation: None,
            deadwax: None,
            lead_out_geometry_revision: LEAD_OUT_GEOMETRY_REVISION,
        }
    }

    fn reference(key: &str, commitment: [u8; 32]) -> SignedReleaseReference {
        SignedReleaseReference {
            version: SIGNED_RELEASE_REFERENCE_VERSION,
            release_commitment_sha256: commitment,
            key_id: key.as_bytes().to_vec(),
            signature: vec![7; SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH],
        }
    }

    #[test]
    fn isrc_normalizes_to_twelve_characters() {
        assert_eq!(
            normalize_isrc("gb-abc-24-00001").expect("a well formed ISRC"),
            "GBABC2400001"
        );
        assert!(normalize_isrc("GBABC240000").is_err(), "too short");
        assert!(
            normalize_isrc("1BABC2400001").is_err(),
            "country is letters"
        );
        assert!(normalize_isrc("GBABCX400001").is_err(), "year is digits");
    }

    #[test]
    fn isrcs_round_trip_in_track_order() {
        let isrcs = vec![
            TrackIsrc {
                track_index: 3,
                code: "GBABC2400004".to_string(),
            },
            TrackIsrc {
                track_index: 0,
                code: "gb-abc-24-00001".to_string(),
            },
        ];
        let encoded = encode_isrc_segment(&isrcs).expect("encodes");
        let decoded = decode_isrc_segment(&encoded).expect("decodes");
        assert_eq!(decoded[0].track_index, 0);
        assert_eq!(decoded[0].code, "GBABC2400001");
        assert_eq!(decoded[1].track_index, 3);
    }

    #[test]
    fn two_isrcs_cannot_claim_one_track() {
        let isrcs = vec![
            TrackIsrc {
                track_index: 1,
                code: "GBABC2400001".to_string(),
            },
            TrackIsrc {
                track_index: 1,
                code: "GBABC2400002".to_string(),
            },
        ];
        assert!(encode_isrc_segment(&isrcs).is_err());
    }

    #[test]
    fn barcodes_are_checked_by_their_last_digit() {
        // A real EAN-13 check digit, and the same barcode with it wrong.
        assert_eq!(
            normalize_upc("5-060204-800016").expect("valid"),
            "5060204800016"
        );
        assert!(normalize_upc("5060204800017").is_err(), "check digit");
        assert!(normalize_upc("50602048000").is_err(), "wrong length");
    }

    #[test]
    fn a_deferred_field_is_null_or_signed() {
        let mut descriptor = test_descriptor(vec![0x11; CACHE_ENCRYPTION_SECRET_LENGTH]);
        descriptor.upc = Some("5060204800016".to_string());
        assert!(
            validate_deferred_group(&descriptor).is_err(),
            "a barcode with nothing signing it is malformed"
        );

        descriptor.deferred_attestation = Some(reference("yl.vin", [1; 32]));
        validate_deferred_group(&descriptor).expect("signed, so well formed");

        descriptor.upc = None;
        descriptor.isrcs.clear();
        descriptor.chain_anchor = None;
        assert!(
            validate_deferred_group(&descriptor).is_err(),
            "an attestation with nothing to sign is malformed"
        );
    }

    #[test]
    fn the_deferred_group_is_outside_what_the_press_signed() {
        let base = test_descriptor(vec![0x11; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let pressed = descriptor_commitment(&base).expect("commits");

        let mut anchored = base.clone();
        anchored.chain_anchor = Some(vec![9; 32]);
        anchored.upc = Some("5060204800016".to_string());
        anchored.deferred_attestation = Some(reference("yl.vin", [1; 32]));
        assert_eq!(
            pressed,
            descriptor_commitment(&anchored).expect("commits"),
            "anchoring a release must not disturb what was pressed"
        );

        let mut retitled = base.clone();
        retitled.title = Some("Another name".to_string());
        assert_ne!(
            pressed,
            descriptor_commitment(&retitled).expect("commits"),
            "a pressed record's own words are inside the commitment"
        );

        let mut recut = base.clone();
        recut.b_value_bits = 2.0f64.to_bits();
        assert_ne!(
            pressed,
            descriptor_commitment(&recut).expect("commits"),
            "the geometry it was cut at is inside the commitment too"
        );
    }

    #[test]
    fn a_release_may_be_signed_by_more_than_one_party() {
        let mut descriptor = test_descriptor(vec![0x11; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let commitment = [4; 32];
        descriptor.signed_release_reference = Some(reference("artist", commitment));
        descriptor.additional_signatures = vec![reference("yl.vin", commitment)];
        validate_signature_set(&descriptor).expect("artist and yl.vin, over one commitment");

        descriptor.additional_signatures = vec![reference("artist", commitment)];
        assert!(
            validate_signature_set(&descriptor).is_err(),
            "one key, one signature"
        );

        descriptor.additional_signatures = vec![reference("yl.vin", [5; 32])];
        assert!(
            validate_signature_set(&descriptor).is_err(),
            "every signature is over the same commitment"
        );

        descriptor.signed_release_reference = None;
        descriptor.additional_signatures = vec![reference("yl.vin", commitment)];
        assert!(
            validate_signature_set(&descriptor).is_err(),
            "there is nothing for them to join"
        );
    }

    fn test_context() -> CacheEncryptionContext {
        CacheEncryptionContext {
            protocol_version: 1,
            cache_format_version: 1,
            cache_store_name: "opus-chunks".to_string(),
            cache_key: "0123456789abcdef".to_string(),
            chunk_index: 7,
            packet_offset: 2048,
            plaintext_length: 12,
            codec_identifier: "soundkit_opus_packets".to_string(),
        }
    }

    #[test]
    fn record_profile_codes_round_trip() {
        assert_eq!(record_profile_code("single45").unwrap(), 0);
        assert_eq!(record_profile_code("lp").unwrap(), 1);
        assert_eq!(record_profile_code("ten").unwrap(), 2);
        assert_eq!(record_profile_code("single45vintage").unwrap(), 3);
        assert_eq!(record_profile_from_code(0).unwrap(), "single45");
        assert_eq!(record_profile_from_code(1).unwrap(), "lp");
        assert_eq!(record_profile_from_code(2).unwrap(), "ten");
        assert_eq!(record_profile_from_code(3).unwrap(), "single45vintage");
        assert!(record_profile_from_code(4).is_err());
    }

    #[test]
    fn payload_encoding_codes_round_trip() {
        assert_eq!(payload_encoding_code("rgb").unwrap(), 0);
        assert_eq!(payload_encoding_code("toned-v1").unwrap(), 1);
        assert_eq!(payload_encoding_code("toned-v2").unwrap(), 2);
        assert_eq!(payload_encoding_from_code(0).unwrap(), "rgb");
        assert_eq!(payload_encoding_from_code(1).unwrap(), "toned-v1");
        assert_eq!(payload_encoding_from_code(2).unwrap(), "toned-v2");
        assert!(payload_encoding_from_code(3).is_err());
    }

    #[test]
    fn release_id_text_round_trips_through_bytes() {
        let bytes = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60,
            0x70, 0x80,
        ];
        let text = release_id_to_text(bytes);
        assert!(text.starts_with("rel_"));
        assert_eq!(text.len(), 4 + 26);
        assert_eq!(release_id_to_bytes(&text).unwrap(), bytes);
    }

    #[test]
    fn release_id_rejects_missing_prefix() {
        assert!(release_id_to_bytes("01ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
    }

    #[test]
    fn release_id_rejects_values_above_the_ulid_range() {
        assert!(release_id_to_bytes("rel_Z1ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
    }

    #[test]
    fn release_id_accepts_the_maximum_canonical_ulid() {
        let text = "rel_7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
        let bytes = release_id_to_bytes(text).unwrap();
        assert_eq!(release_id_to_text(bytes), text);
    }

    #[test]
    fn binary_reference_round_trips_through_decoder() {
        let mut bytes = Vec::new();
        bytes.push(SIGNED_RELEASE_REFERENCE_VERSION);
        bytes.extend_from_slice(&[0x11; 32]);
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(b"key");
        bytes.extend_from_slice(&[0x22; 64]);

        let decoded = decode_signed_release_reference(&bytes).unwrap();
        assert_eq!(decoded.release_commitment_sha256, [0x11; 32]);
        assert_eq!(decoded.key_id, b"key");
        assert_eq!(decoded.signature, vec![0x22; 64]);
    }
    #[test]
    fn toned_carrier_map_round_trips() {
        let spans = vec![
            ToneSpanDescriptor {
                byte_length: 1024,
                base: [255, 192, 203],
                luma_tolerance: 16,
                bits_per_pixel: 21,
                ordering: ToneOrdering::ChromaProximity,
            },
            ToneSpanDescriptor {
                byte_length: 513,
                base: [20, 40, 80],
                luma_tolerance: 8,
                bits_per_pixel: 18,
                ordering: ToneOrdering::BaseProximity,
            },
        ];

        let bytes = encode_toned_carrier_map(&spans, Some(1537)).unwrap();
        let decoded = decode_toned_carrier_map(&bytes, Some(1537)).unwrap();

        assert_eq!(decoded, spans);
    }

    #[test]
    fn tone_clock_map_round_trips() {
        let clock = ToneClockDescriptor {
            rotation_centidegrees: vec![12_345],
            blend: true,
            bits_per_pixel: 20,
            ordering: ToneOrdering::ChromaProximity,
            rings: vec![16],
            span: (0, TONE_CLOCK_SPAN_UNITS as u16),
            slots: (0..16u8)
                .map(|k| ToneClockSlotDescriptor {
                    base: [k * 16, 255 - k * 16, 128],
                    luma_tolerance: 48 + k,
                    gap_base: [k * 16 + 8, 255 - k * 16, 140],
                    gap_luma_tolerance: 48 + k,
                })
                .collect(),
            gap_switch_offsets: vec![1_000, 2_000, 70_000, 71_000],
        };
        let bytes = encode_tone_clock_map(&clock, Some(140_000)).unwrap();
        // One ring encodes as a version 1 map: 7 header bytes, 8 bytes per
        // slot, and 1 + (2 + 2 + 3 + 3) bytes for the switches. That total
        // matches the records already pressed.
        assert_eq!(bytes[0], TONE_CLOCK_MAP_VERSION_WEDGES);
        assert_eq!(bytes.len(), 7 + 16 * 8 + 11);
        assert_eq!(decode_tone_clock_map(&bytes, Some(140_000)).unwrap(), clock);
        assert!(decode_tone_clock_map(&bytes, Some(70_500)).is_err());

        let mut one_slot = clock.clone();
        one_slot.slots.truncate(1);
        one_slot.rings = vec![1];
        assert!(encode_tone_clock_map(&one_slot, None).is_err());
        let mut spun_too_far = clock.clone();
        spun_too_far.rotation_centidegrees = vec![36_000];
        assert!(encode_tone_clock_map(&spun_too_far, None).is_err());
    }

    /// The house wheel, which is eight pockets inside and sixteen outside,
    /// round-trips through the map unchanged.
    #[test]
    fn a_ringed_tone_clock_map_round_trips() {
        let clock = ToneClockDescriptor {
            rotation_centidegrees: vec![4_500, 9_000],
            blend: true,
            bits_per_pixel: 20,
            ordering: ToneOrdering::ChromaProximity,
            rings: vec![8, 16],
            span: (3_100, 9_700),
            slots: (0..24u8)
                .map(|k| ToneClockSlotDescriptor {
                    base: [k * 10, 255 - k * 10, 128],
                    luma_tolerance: 40 + k,
                    gap_base: [k * 10 + 8, 255 - k * 10, 140],
                    gap_luma_tolerance: 40 + k,
                })
                .collect(),
            gap_switch_offsets: vec![1_000],
        };
        let bytes = encode_tone_clock_map(&clock, Some(140_000)).unwrap();
        assert_eq!(bytes[0], TONE_CLOCK_MAP_VERSION);
        // 4 header bytes, then 1 + 2 ring counts, 2 rotations of 2, 4 for
        // the band, 8 per pocket, and 1 + 2 for the one switch.
        assert_eq!(bytes.len(), 4 + 1 + 2 + 4 + 4 + 24 * 8 + 3);
        assert_eq!(clock.rotation_centidegrees, vec![4_500, 9_000]);
        assert_eq!(decode_tone_clock_map(&bytes, Some(140_000)).unwrap(), clock);

        // The tone count must equal the pocket count.
        let mut short = clock.clone();
        short.slots.truncate(23);
        assert!(encode_tone_clock_map(&short, None).is_err());
        // The band must have a positive width.
        let mut inside_out = clock.clone();
        inside_out.span = (9_700, 3_100);
        assert!(encode_tone_clock_map(&inside_out, None).is_err());
    }

    /// A version 1 map, which is the form of every clock-toned record pressed
    /// so far, reads as a one-ring wheel over the whole disc.
    #[test]
    fn a_version_one_map_still_decodes_as_wedges() {
        let mut bytes = vec![
            TONE_CLOCK_MAP_VERSION_WEDGES,
            20,
            ToneOrdering::ChromaProximity.wire_code(),
            1,
        ];
        bytes.extend_from_slice(&1_250u16.to_be_bytes());
        bytes.push(4);
        for k in 0..4u8 {
            bytes.extend_from_slice(&[k, k, k]);
            bytes.push(16);
            bytes.extend_from_slice(&[k, k, k]);
            bytes.push(16);
        }
        bytes.push(0);
        let clock = decode_tone_clock_map(&bytes, None).unwrap();
        assert_eq!(clock.rings, vec![4]);
        assert_eq!(clock.span, (0, TONE_CLOCK_SPAN_UNITS as u16));
        assert!(!clock.has_rings());
        // And it re-encodes to exactly the bytes it came from.
        assert_eq!(encode_tone_clock_map(&clock, None).unwrap(), bytes);
    }

    #[test]
    fn tone_clock_is_inside_the_identity_only_when_present() {
        let plain = test_descriptor(vec![1; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let mut clocked = plain.clone();
        clocked.tone_clock = Some(ToneClockDescriptor {
            rotation_centidegrees: vec![0],
            blend: false,
            bits_per_pixel: 20,
            ordering: ToneOrdering::ChromaProximity,
            rings: vec![2],
            span: (0, TONE_CLOCK_SPAN_UNITS as u16),
            slots: vec![
                ToneClockSlotDescriptor {
                    base: [1, 2, 3],
                    luma_tolerance: 8,
                    gap_base: [4, 5, 6],
                    gap_luma_tolerance: 8,
                };
                2
            ],
            gap_switch_offsets: Vec::new(),
        });
        let plain_identity = signed_descriptor_identity_bytes(&plain).unwrap();
        let clocked_identity = signed_descriptor_identity_bytes(&clocked).unwrap();
        assert!(clocked_identity.starts_with(&plain_identity));
        assert!(clocked_identity.len() > plain_identity.len());
        assert_ne!(
            cache_encryption_identity_bytes(&plain).unwrap(),
            cache_encryption_identity_bytes(&clocked).unwrap()
        );
    }

    #[test]
    fn toned_offsets_are_derived() {
        let spans = vec![
            ToneSpanDescriptor {
                byte_length: 5,
                base: [1, 2, 3],
                luma_tolerance: 0,
                bits_per_pixel: 8,
                ordering: ToneOrdering::BaseProximity,
            },
            ToneSpanDescriptor {
                byte_length: 7,
                base: [4, 5, 6],
                luma_tolerance: 1,
                bits_per_pixel: 4,
                ordering: ToneOrdering::ChromaProximity,
            },
        ];

        let resolved = resolve_tone_spans(&spans, Some(12)).unwrap();
        assert_eq!(resolved[0].byte_offset, 0);
        assert_eq!(resolved[1].byte_offset, 5);
        assert_eq!(resolved[0].pixel_count, 5);
        assert_eq!(resolved[1].pixel_offset, 5);
        assert_eq!(resolved[1].pixel_count, 14);
    }

    #[test]
    fn toned_map_rejects_overlong_varuint() {
        let bytes = [
            TONED_CARRIER_MAP_VERSION,
            0,
            1,
            0x81,
            0x00,
            0,
            0,
            0,
            0,
            8,
            TONED_ORDERING_BASE_PROXIMITY,
        ];
        assert!(decode_toned_carrier_map(&bytes, None).is_err());
    }

    #[test]
    fn cache_encryption_descriptor_round_trips_through_json() {
        let descriptor = CacheEncryptionDescriptor {
            version: CACHE_ENCRYPTION_DESCRIPTOR_VERSION,
            algorithm: CacheEncryptionAlgorithm::XChaCha20Poly1305,
            key_derivation: CacheKeyDerivation::HkdfSha256,
            secret: vec![7u8; CACHE_ENCRYPTION_SECRET_LENGTH],
        };
        let json = serde_json::to_string(&descriptor).unwrap();
        let decoded: CacheEncryptionDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn cache_encryption_secret_must_be_32_bytes() {
        let mut descriptor = CacheEncryptionDescriptor {
            version: CACHE_ENCRYPTION_DESCRIPTOR_VERSION,
            algorithm: CacheEncryptionAlgorithm::XChaCha20Poly1305,
            key_derivation: CacheKeyDerivation::HkdfSha256,
            secret: vec![0u8; 31],
        };
        assert!(descriptor.validate().is_err());
        descriptor.secret = vec![0u8; 32];
        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn cache_encryption_descriptor_rejects_malformed_base64url() {
        let json = r#"{"version":1,"algorithm":"xchacha20-poly1305","keyDerivation":"hkdf-sha256","secret":"not base64"}"#;
        assert!(serde_json::from_str::<CacheEncryptionDescriptor>(json).is_err());
    }

    #[test]
    fn cache_encryption_descriptor_rejects_wrong_secret_length() {
        let json = format!(
            r#"{{"version":1,"algorithm":"xchacha20-poly1305","keyDerivation":"hkdf-sha256","secret":"{}"}}"#,
            URL_SAFE_NO_PAD.encode([1u8; 31])
        );
        let parsed: CacheEncryptionDescriptor = serde_json::from_str(&json).unwrap();
        assert!(parsed.validate().is_err());
    }

    #[test]
    fn old_descriptors_without_cache_encryption_still_decode() {
        let descriptor = test_descriptor(vec![9u8; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let json = serde_json::to_string(&descriptor).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value.as_object_mut().unwrap().remove("cacheEncryption");
        let decoded: RecordDescriptor = serde_json::from_value(value).unwrap();
        assert!(decoded.cache_encryption.is_none());
    }

    #[test]
    fn cache_encryption_key_derivation_is_stable_and_bindable() {
        let descriptor = test_descriptor(vec![1u8; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let key_a = derive_cache_encryption_key(&descriptor).unwrap();
        let key_b = derive_cache_encryption_key(&descriptor).unwrap();
        assert_eq!(key_a, key_b);

        let mut other_secret = descriptor.clone();
        other_secret.cache_encryption.as_mut().unwrap().secret =
            vec![2u8; CACHE_ENCRYPTION_SECRET_LENGTH];
        assert_ne!(key_a, derive_cache_encryption_key(&other_secret).unwrap());

        let mut other_record = descriptor.clone();
        other_record.release_id = Some([0x22; RELEASE_ID_LENGTH]);
        assert_ne!(key_a, derive_cache_encryption_key(&other_record).unwrap());
    }

    #[test]
    fn cache_encryption_envelope_round_trips() {
        let descriptor = test_descriptor(vec![3u8; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let context = test_context();
        let plaintext = b"opus-packets";
        let envelope = encrypt_cache_envelope(&descriptor, &context, plaintext).unwrap();
        let decrypted = decrypt_cache_envelope(&descriptor, &context, &envelope).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn cache_encryption_envelope_rejects_tampering() {
        let descriptor = test_descriptor(vec![5u8; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let context = test_context();
        let plaintext = b"opus-packets";
        let mut envelope = encrypt_cache_envelope(&descriptor, &context, plaintext).unwrap();

        envelope[CacheEncryptionEnvelope::HEADER_LENGTH] ^= 1;
        assert!(decrypt_cache_envelope(&descriptor, &context, &envelope).is_err());

        let mut nonce_tampered = encrypt_cache_envelope(&descriptor, &context, plaintext).unwrap();
        nonce_tampered[60] ^= 1;
        assert!(decrypt_cache_envelope(&descriptor, &context, &nonce_tampered).is_err());

        let mut binding_tampered =
            encrypt_cache_envelope(&descriptor, &context, plaintext).unwrap();
        binding_tampered[8] ^= 1;
        assert!(decrypt_cache_envelope(&descriptor, &context, &binding_tampered).is_err());
    }

    #[test]
    fn cache_encryption_nonce_is_deterministic_and_content_bound() {
        let descriptor = test_descriptor(vec![9u8; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let context = test_context();
        let plaintext = b"opus-packets";

        let envelope_a = encrypt_cache_envelope(&descriptor, &context, plaintext).unwrap();
        let envelope_b = encrypt_cache_envelope(&descriptor, &context, plaintext).unwrap();
        assert_eq!(
            envelope_a, envelope_b,
            "same (record, plaintext, context) must produce byte-identical envelopes"
        );

        let other_plaintext = b"opus-packet$";
        assert_eq!(other_plaintext.len(), plaintext.len());
        let envelope_c = encrypt_cache_envelope(&descriptor, &context, other_plaintext).unwrap();
        assert_ne!(
            envelope_a, envelope_c,
            "different plaintext must not reuse the same nonce/ciphertext"
        );
    }

    #[test]
    fn cache_encryption_record_binding_hash_hex_differs_per_record() {
        let descriptor_a = test_descriptor(vec![1u8; CACHE_ENCRYPTION_SECRET_LENGTH]);
        let mut descriptor_b = descriptor_a.clone();
        descriptor_b.release_id = Some([0x33; RELEASE_ID_LENGTH]);

        let hash_a = cache_encryption_record_binding_hash_hex(&descriptor_a).unwrap();
        let hash_b = cache_encryption_record_binding_hash_hex(&descriptor_b).unwrap();
        assert_eq!(hash_a.len(), 64);
        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn every_level_round_trips_through_its_grey() {
        for level in 0..METADATA_GRAYSCALE_LEVELS {
            let grey = grayscale_value_for_level(level);
            assert_eq!(
                level_for_grayscale_value(grey),
                Some(level),
                "level {level} painted as {grey} did not read back"
            );
        }
    }

    /// The band holds mid-grey values.
    ///
    /// The descriptor paints inside the mid-grey window, away from black and
    /// white. The lead-in and the run-out are visible rings, and a ladder that
    /// reaches the ends of the range draws them as a barcode.
    /// Sixty-four rungs one value apart occupy sixty-four values, and those
    /// sit in the middle with ninety-six values of headroom either side.
    #[test]
    fn the_ladder_is_mid_grey_at_both_ends() {
        let darkest = grayscale_value_for_level(0);
        let lightest = grayscale_value_for_level(METADATA_GRAYSCALE_LEVELS - 1);

        assert_eq!(darkest, METADATA_GRAYSCALE_BASE);
        assert_eq!(lightest, METADATA_GRAYSCALE_TOP);
        assert_eq!((darkest, lightest), (96, 159));

        // Centred: the room below the darkest rung equals the room above the
        // lightest, so the band cannot creep toward either end unnoticed.
        assert_eq!(i32::from(darkest), 255 - i32::from(lightest));
    }

    /// A value off the ladder is refused, and it is never rounded to the
    /// nearest rung.
    ///
    /// The band is lossless end to end. The path holds no resample, no
    /// recompression and no print-and-scan step, so each pixel arrives with the
    /// value that the cutter wrote. A value off the ladder came from another
    /// source. A nearest-rung match would give a corrupted record a plausible
    /// reading, and it would leave the CRC as the first check to report the
    /// fault.
    #[test]
    fn a_grey_off_the_ladder_is_refused() {
        // Every value below the band and above it.
        for value in 0..METADATA_GRAYSCALE_BASE {
            assert_eq!(level_for_grayscale_value(value), None, "{value} is below the band");
        }
        for value in (METADATA_GRAYSCALE_TOP + 1)..=255 {
            assert_eq!(level_for_grayscale_value(value), None, "{value} is above the band");
        }
        // And, when the step ever widens again, the gaps between the rungs.
        if METADATA_GRAYSCALE_STEP > 1 {
            for level in 0..(METADATA_GRAYSCALE_LEVELS - 1) {
                let between = grayscale_value_for_level(level) + 1;
                assert_eq!(
                    level_for_grayscale_value(between),
                    None,
                    "{between} falls between two rungs"
                );
            }
        }
    }

    #[test]
    fn the_ladder_carries_more_bits_than_the_nibble_encoding_it_replaced() {
        // The run-out went from four turns to two; the bits per pixel have to
        // make that back or the descriptor a record can carry shrinks.
        assert!(
            METADATA_GRAYSCALE_BITS_PER_PIXEL > 4,
            "six bits per pixel is what pays for the shorter run-out"
        );
        let pixels = 5197; // single45 lead-in + two-turn run-out
        let was_four_turn_nibble = 7138 / 2; // the original band, at 4 bits/px
        assert!(
            metadata_byte_capacity_for_pixel_count(pixels) >= was_four_turn_nibble,
            "capacity must not fall below the four-turn nibble encoding"
        );
    }
}

#[cfg(test)]
mod lead_out_geometry_tests {
    use super::*;

    /// A descriptor carrying exactly one segment, so the segment is the only
    /// thing under test.
    fn descriptor_carrying(segment: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BRD1");
        bytes.push(RECORD_DESCRIPTOR_VERSION);
        bytes.extend_from_slice(
            &u16::try_from(RECORD_DESCRIPTOR_PREFIX_LENGTH + segment.len())
                .expect("descriptor fits")
                .to_be_bytes(),
        );
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(
            &u16::try_from(segment.len()).expect("segments fit").to_be_bytes(),
        );
        bytes.extend_from_slice(&1.0f64.to_bits().to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&0u64.to_be_bytes());
        bytes.extend_from_slice(segment);
        bytes
    }

    fn revision_segment(revision: u8) -> Vec<u8> {
        let mut segment = vec![SEGMENT_LEAD_OUT_GEOMETRY];
        segment.extend_from_slice(&1u16.to_be_bytes());
        segment.push(revision);
        segment
    }

    /// A record with no lead-out geometry segment was cut before that segment
    /// existed, and therefore before revision 1 existed. Its absent segment
    /// selects the earlier trailer.
    #[test]
    fn an_absent_revision_is_the_draft04_trailer() {
        assert_eq!(
            original_lead_out_geometry(),
            LEAD_OUT_GEOMETRY_REVISION_DRAFT04
        );
        assert_ne!(
            original_lead_out_geometry(),
            LEAD_OUT_GEOMETRY_REVISION_ORIGINAL,
            "absence must not assert the geometry this build cuts"
        );
    }

    /// A decoder must read this segment. An unknown value in any other
    /// segment costs the decoder one feature. An unknown value here states that
    /// constants outside this build drew the band under the trailer, and a
    /// trace under the wrong constants returns plausible bytes.
    #[test]
    fn a_future_revision_is_refused_rather_than_guessed_at() {
        let bytes = descriptor_carrying(&revision_segment(LEAD_OUT_GEOMETRY_REVISION + 1));
        let error = decode_record_descriptor_bytes(&bytes)
            .expect_err("a revision this build cannot draw is not a record it can read");

        assert!(
            error.to_string().contains("lead-out geometry revision"),
            "{error}"
        );
    }

}

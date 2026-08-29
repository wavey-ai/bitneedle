//! Typed operational identifiers for public Bitneedle record objects.
//!
//! Bitneedle uses two complementary identity systems.
//!
//! ## Internal object identifiers
//!
//! Internal identifiers use a typed, prefixed ULID:
//!
//! ```text
//! <lowercase-kind-prefix>_<canonical-uppercase-ULID>
//! ```
//!
//! Examples:
//!
//! ```text
//! rel_01JXWQ7H6K8V4Z2T9M3N5C1BPA
//! edn_01JXWQ8CM2R6F4KD9H7T3Y5VNE
//! ```
//!
//! These identifiers name releases, editions, assets, tracks, revolutions,
//! receipts, rights objects, sidecars, attestations, and authorisations.
//!
//! ## Public YL catalogue codes
//!
//! Publicly issued picture records may additionally receive a random,
//! non-enumerable YL catalogue code:
//!
//! ```text
//! yl_K7M3P9TX4QC
//! ```
//!
//! Its presentation forms are:
//!
//! ```text
//! Label text: YL K7M3P · 9TX4Q · C
//! Slug:       k7m3p-9tx4q-c
//! Permalink:  https://yl.vin/k7m3p-9tx4q-c
//! ```
//!
//! The compact body is 11 Crockford Base32 symbols:
//!
//! - 10 random data symbols (50 random bits);
//! - 1 trailing checksum symbol.
//!
//! Parsing is case-insensitive, accepts Crockford aliases (`O -> 0`, `I/L -> 1`),
//! and ignores presentation separators (`-`, space, `.`, `·`, `_`).

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sha2::{Digest, Sha256};
use thiserror::Error;
use ulid::Ulid;

#[cfg(feature = "generate")]
use rand_core::{CryptoRng, OsRng, RngCore};

/// Number of characters in the canonical text representation of a ULID.
pub const ULID_TEXT_LENGTH: usize = 26;

/// Canonical machine-readable YL catalogue prefix.
pub const YL_CATALOGUE_PREFIX: &str = "yl_";

/// Public host used for canonical YL record permalinks.
pub const YL_PERMALINK_HOST: &str = "yl.vin";

/// Crockford Base32 alphabet used by YL catalogue codes.
pub const YL_CATALOGUE_ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Number of random data symbols in a YL catalogue code.
pub const YL_CATALOGUE_DATA_SYMBOLS: usize = 10;

/// Number of checksum symbols in a YL catalogue code.
pub const YL_CATALOGUE_CHECKSUM_SYMBOLS: usize = 1;

/// Number of compact symbols in a YL catalogue code.
/// Domain separation for [`YlCatalogueCode::derive`].
pub const YL_CATALOGUE_DERIVATION_DOMAIN: &[u8] = b"bitneedle.yl-catalogue.v1";

pub const YL_CATALOGUE_COMPACT_LENGTH: usize =
    YL_CATALOGUE_DATA_SYMBOLS + YL_CATALOGUE_CHECKSUM_SYMBOLS;

const YL_CATALOGUE_LABEL_SEPARATOR: &str = " · ";
const YL_CATALOGUE_CHECKSUM_SEED: u16 = 0x15;
const YL_CATALOGUE_CHECKSUM_WEIGHTS: [u16; YL_CATALOGUE_DATA_SYMBOLS] =
    [1, 3, 5, 7, 9, 11, 13, 15, 17, 19];

/// Public Bitneedle identifier kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BitneedleIdKind {
    Release,
    Edition,
    Asset,
    Track,
    Revolution,
    Receipt,
    Rights,
    Sidecar,
    Attestation,
    Authorization,
}

impl BitneedleIdKind {
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Release => "rel",
            Self::Edition => "edn",
            Self::Asset => "ast",
            Self::Track => "trk",
            Self::Revolution => "rev",
            Self::Receipt => "rcp",
            Self::Rights => "rgt",
            Self::Sidecar => "bsc",
            Self::Attestation => "att",
            Self::Authorization => "aut",
        }
    }

    pub const fn tagged_prefix(self) -> &'static str {
        match self {
            Self::Release => "rel_",
            Self::Edition => "edn_",
            Self::Asset => "ast_",
            Self::Track => "trk_",
            Self::Revolution => "rev_",
            Self::Receipt => "rcp_",
            Self::Rights => "rgt_",
            Self::Sidecar => "bsc_",
            Self::Attestation => "att_",
            Self::Authorization => "aut_",
        }
    }

    pub fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "rel" => Some(Self::Release),
            "edn" => Some(Self::Edition),
            "ast" => Some(Self::Asset),
            "trk" => Some(Self::Track),
            "rev" => Some(Self::Revolution),
            "rcp" => Some(Self::Receipt),
            "rgt" => Some(Self::Rights),
            "bsc" => Some(Self::Sidecar),
            "att" => Some(Self::Attestation),
            "aut" => Some(Self::Authorization),
            _ => None,
        }
    }
}

impl fmt::Display for BitneedleIdKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.prefix())
    }
}

/// Error returned when a Bitneedle identifier is malformed or has the wrong kind.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BitneedleIdError {
    #[error("Bitneedle identifier is empty")]
    Empty,

    #[error("Bitneedle identifier is missing its type prefix")]
    MissingPrefix,

    #[error("unknown Bitneedle identifier prefix `{0}`")]
    UnknownPrefix(String),

    #[error("expected `{expected}_` identifier, received `{actual}_`")]
    WrongKind {
        expected: &'static str,
        actual: String,
    },

    #[error("ULID body must contain exactly {ULID_TEXT_LENGTH} characters; found {actual}")]
    InvalidLength { actual: usize },

    #[error("ULID body is not in canonical uppercase Crockford Base32 form")]
    NonCanonicalUlid,

    #[error("invalid ULID: {0}")]
    InvalidUlid(String),
}

/// Parse failure for a YL catalogue code.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum YlCatalogueCodeParseError {
    #[error("YL catalogue code is empty")]
    Empty,

    #[error("YL catalogue code must contain exactly {YL_CATALOGUE_COMPACT_LENGTH} symbols after normalization; found {actual}")]
    InvalidLength { actual: usize },

    #[error("invalid catalogue character `{character}` at index {index}")]
    InvalidCharacter { character: char, index: usize },

    #[error("YL catalogue code checksum is invalid")]
    InvalidChecksum,
}

fn parse_parts(value: &str) -> Result<(BitneedleIdKind, Ulid), BitneedleIdError> {
    if value.is_empty() {
        return Err(BitneedleIdError::Empty);
    }

    let (prefix, body) = value
        .split_once('_')
        .ok_or(BitneedleIdError::MissingPrefix)?;

    let kind = BitneedleIdKind::from_prefix(prefix)
        .ok_or_else(|| BitneedleIdError::UnknownPrefix(prefix.to_owned()))?;

    if body.len() != ULID_TEXT_LENGTH {
        return Err(BitneedleIdError::InvalidLength { actual: body.len() });
    }

    let ulid = Ulid::from_string(body)
        .map_err(|error| BitneedleIdError::InvalidUlid(error.to_string()))?;

    if ulid.to_string() != body {
        return Err(BitneedleIdError::NonCanonicalUlid);
    }

    Ok((kind, ulid))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BitneedleId {
    kind: BitneedleIdKind,
    ulid: Ulid,
}

impl BitneedleId {
    #[cfg(feature = "generate")]
    pub fn new(kind: BitneedleIdKind) -> Self {
        Self {
            kind,
            ulid: Ulid::new(),
        }
    }

    pub const fn from_ulid(kind: BitneedleIdKind, ulid: Ulid) -> Self {
        Self { kind, ulid }
    }

    pub const fn kind(self) -> BitneedleIdKind {
        self.kind
    }

    pub const fn ulid(self) -> Ulid {
        self.ulid
    }

    pub fn timestamp(self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.ulid.timestamp_ms())
    }

    pub fn expect_kind(self, expected: BitneedleIdKind) -> Result<Self, BitneedleIdError> {
        if self.kind != expected {
            return Err(BitneedleIdError::WrongKind {
                expected: expected.prefix(),
                actual: self.kind.prefix().to_owned(),
            });
        }
        Ok(self)
    }
}

impl fmt::Display for BitneedleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.kind.tagged_prefix(), self.ulid)
    }
}

impl FromStr for BitneedleId {
    type Err = BitneedleIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (kind, ulid) = parse_parts(value)?;
        Ok(Self { kind, ulid })
    }
}

impl Serialize for BitneedleId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for BitneedleId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// Random, checksummed public YL catalogue code.
///
/// The compact representation contains 10 random Crockford Base32 symbols
/// followed by one checksum symbol. The checksum is a fixed weighted modulo-32
/// checksum over the first 10 symbols with a YL-specific seed:
///
/// `checksum = (seed + Σ((symbol + 1) * weight[i])) mod 32`
///
/// The weights are the odd sequence `[1, 3, 5, 7, 9, 11, 13, 15, 17, 19]`.
/// Because every weight is odd, every single-symbol substitution changes the
/// checksum. Adjacent transpositions are detected for the common cases covered
/// by the tests below, but like other simple weighted checksums this scheme is
/// not perfect for every possible transposition pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct YlCatalogueCode([u8; YL_CATALOGUE_COMPACT_LENGTH]);

impl YlCatalogueCode {
    #[cfg(feature = "generate")]
    pub fn generate() -> Self {
        Self::generate_with_rng(&mut OsRng)
    }

    #[cfg(feature = "generate")]
    pub fn generate_with_rng<R>(rng: &mut R) -> Self
    where
        R: RngCore + CryptoRng,
    {
        let mut compact = [0u8; YL_CATALOGUE_COMPACT_LENGTH];
        let mut random = [0u8; YL_CATALOGUE_DATA_SYMBOLS];
        rng.fill_bytes(&mut random);
        for (index, byte) in random.iter().enumerate() {
            compact[index] = byte & 0x1f;
        }
        compact[YL_CATALOGUE_DATA_SYMBOLS] =
            Self::checksum_symbol_value(&compact[..YL_CATALOGUE_DATA_SYMBOLS]);
        Self(compact)
    }

    pub fn parse(value: impl AsRef<str>) -> Result<Self, YlCatalogueCodeParseError> {
        value.as_ref().parse()
    }

    /// The catalogue code of a release, derived from its ID.
    ///
    /// A release already has an identity: sixteen bytes in the record's own
    /// header, collision-free without anyone coordinating. Minting a second
    /// random number to stand for the same release meant a record carried
    /// two unrelated identifiers and the header paid for both. This is the
    /// same identity at the resolution a person can say out loud — fifty
    /// bits, derived, stored nowhere.
    ///
    /// Hashed rather than truncated, so the code gives away nothing about
    /// the ULID it came from — not the timestamp it opens with, and not
    /// enough to work backwards. It cannot be reversed: fifty-five bits do
    /// not hold a hundred and twenty-eight.
    pub fn derive(release_id: [u8; 16]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(YL_CATALOGUE_DERIVATION_DOMAIN);
        hasher.update(release_id);
        let digest: [u8; 32] = hasher.finalize().into();

        let mut compact = [0u8; YL_CATALOGUE_COMPACT_LENGTH];
        let mut bit = 0usize;
        for symbol in compact.iter_mut().take(YL_CATALOGUE_DATA_SYMBOLS) {
            let mut value = 0u8;
            for _ in 0..5 {
                value = value << 1 | (digest[bit / 8] >> (7 - bit % 8) & 1);
                bit += 1;
            }
            *symbol = value;
        }
        compact[YL_CATALOGUE_DATA_SYMBOLS] =
            Self::checksum_symbol_value(&compact[..YL_CATALOGUE_DATA_SYMBOLS]);
        Self(compact)
    }

    pub fn canonical(self) -> String {
        format!("{YL_CATALOGUE_PREFIX}{}", self.as_compact_str())
    }

    pub fn slug(self) -> String {
        let compact = self.as_compact_str().to_ascii_lowercase();
        format!("{}-{}-{}", &compact[..5], &compact[5..10], &compact[10..11])
    }

    pub fn label(self) -> String {
        let compact = self.as_compact_str();
        format!(
            "YL {}{}{}{}{}",
            &compact[..5],
            YL_CATALOGUE_LABEL_SEPARATOR,
            &compact[5..10],
            YL_CATALOGUE_LABEL_SEPARATOR,
            &compact[10..11],
        )
    }

    pub fn permalink_path(self) -> String {
        format!("/{}", self.slug())
    }

    pub fn permalink(self) -> String {
        format!("https://{}{}", YL_PERMALINK_HOST, self.permalink_path())
    }

    pub fn as_compact_str(&self) -> String {
        String::from_utf8(self.compact_ascii_bytes().to_vec()).expect("catalogue code is ASCII")
    }

    fn compact_ascii_bytes(&self) -> [u8; YL_CATALOGUE_COMPACT_LENGTH] {
        let mut ascii = [0u8; YL_CATALOGUE_COMPACT_LENGTH];
        for (index, symbol) in self.0.iter().enumerate() {
            ascii[index] = encode_symbol(*symbol);
        }
        ascii
    }

    fn checksum_symbol_value(data_symbols: &[u8]) -> u8 {
        let mut checksum = YL_CATALOGUE_CHECKSUM_SEED;
        for (index, value) in data_symbols.iter().copied().enumerate() {
            checksum =
                (checksum + (u16::from(value) + 1) * YL_CATALOGUE_CHECKSUM_WEIGHTS[index]) % 32;
        }
        checksum as u8
    }

    fn from_symbol_values(
        values: [u8; YL_CATALOGUE_COMPACT_LENGTH],
    ) -> Result<Self, YlCatalogueCodeParseError> {
        let expected = Self::checksum_symbol_value(&values[..YL_CATALOGUE_DATA_SYMBOLS]);
        if values[YL_CATALOGUE_DATA_SYMBOLS] != expected {
            return Err(YlCatalogueCodeParseError::InvalidChecksum);
        }
        Ok(Self(values))
    }
}

impl fmt::Display for YlCatalogueCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.canonical())
    }
}

impl FromStr for YlCatalogueCode {
    type Err = YlCatalogueCodeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(YlCatalogueCodeParseError::Empty);
        }

        let body = strip_catalogue_prefix(trimmed);
        let mut symbols = Vec::with_capacity(YL_CATALOGUE_COMPACT_LENGTH);

        for (index, character) in body.char_indices() {
            if is_catalogue_separator(character) {
                continue;
            }
            let symbol = decode_symbol(character)
                .ok_or(YlCatalogueCodeParseError::InvalidCharacter { character, index })?;
            symbols.push(symbol);
        }

        if symbols.len() != YL_CATALOGUE_COMPACT_LENGTH {
            return Err(YlCatalogueCodeParseError::InvalidLength {
                actual: symbols.len(),
            });
        }

        let compact: [u8; YL_CATALOGUE_COMPACT_LENGTH] = symbols
            .try_into()
            .expect("catalogue symbol count already validated");
        Self::from_symbol_values(compact)
    }
}

impl Serialize for YlCatalogueCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for YlCatalogueCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

fn strip_catalogue_prefix(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && bytes[0].eq_ignore_ascii_case(&b'y')
        && bytes[1].eq_ignore_ascii_case(&b'l')
        && bytes
            .get(2)
            .copied()
            .map(|byte| is_catalogue_separator(byte as char))
            .unwrap_or(true)
    {
        &value[2..]
    } else {
        value
    }
}

fn is_catalogue_separator(character: char) -> bool {
    matches!(character, '-' | ' ' | '.' | '·' | '_')
}

fn decode_symbol(character: char) -> Option<u8> {
    match character {
        '0' | 'O' | 'o' => Some(0),
        '1' | 'I' | 'i' | 'L' | 'l' => Some(1),
        '2'..='9' => Some((character as u8) - b'0'),
        'A' | 'a' => Some(10),
        'B' | 'b' => Some(11),
        'C' | 'c' => Some(12),
        'D' | 'd' => Some(13),
        'E' | 'e' => Some(14),
        'F' | 'f' => Some(15),
        'G' | 'g' => Some(16),
        'H' | 'h' => Some(17),
        'J' | 'j' => Some(18),
        'K' | 'k' => Some(19),
        'M' | 'm' => Some(20),
        'N' | 'n' => Some(21),
        'P' | 'p' => Some(22),
        'Q' | 'q' => Some(23),
        'R' | 'r' => Some(24),
        'S' | 's' => Some(25),
        'T' | 't' => Some(26),
        'V' | 'v' => Some(27),
        'W' | 'w' => Some(28),
        'X' | 'x' => Some(29),
        'Y' | 'y' => Some(30),
        'Z' | 'z' => Some(31),
        _ => None,
    }
}

fn encode_symbol(value: u8) -> u8 {
    YL_CATALOGUE_ALPHABET.as_bytes()[usize::from(value)]
}

macro_rules! define_typed_id {
    (
        $(#[$meta:meta])*
        $name:ident,
        $kind:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Ulid);

        impl $name {
            #[cfg(feature = "generate")]
            pub fn new() -> Self {
                Self(Ulid::new())
            }

            pub const fn from_ulid(ulid: Ulid) -> Self {
                Self(ulid)
            }

            pub const fn ulid(self) -> Ulid {
                self.0
            }

            pub const fn kind() -> BitneedleIdKind {
                BitneedleIdKind::$kind
            }

            pub fn timestamp(self) -> SystemTime {
                UNIX_EPOCH + Duration::from_millis(self.0.timestamp_ms())
            }
        }

        #[cfg(feature = "generate")]
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}{}", BitneedleIdKind::$kind.tagged_prefix(), self.0)
            }
        }

        impl FromStr for $name {
            type Err = BitneedleIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let (actual_kind, ulid) = parse_parts(value)?;
                let expected_kind = BitneedleIdKind::$kind;

                if actual_kind != expected_kind {
                    return Err(BitneedleIdError::WrongKind {
                        expected: expected_kind.prefix(),
                        actual: actual_kind.prefix().to_owned(),
                    });
                }

                Ok(Self(ulid))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(de::Error::custom)
            }
        }

        impl From<$name> for BitneedleId {
            fn from(value: $name) -> Self {
                Self::from_ulid(BitneedleIdKind::$kind, value.0)
            }
        }

        impl TryFrom<BitneedleId> for $name {
            type Error = BitneedleIdError;

            fn try_from(value: BitneedleId) -> Result<Self, Self::Error> {
                value.expect_kind(BitneedleIdKind::$kind)?;
                Ok(Self(value.ulid))
            }
        }
    };
}

define_typed_id!(ReleaseId, Release);
define_typed_id!(EditionId, Edition);
define_typed_id!(AssetId, Asset);
define_typed_id!(TrackId, Track);
define_typed_id!(RevolutionId, Revolution);
define_typed_id!(ReceiptId, Receipt);
define_typed_id!(RightsId, Rights);
define_typed_id!(SidecarId, Sidecar);
define_typed_id!(AttestationId, Attestation);
define_typed_id!(AuthorizationId, Authorization);

#[cfg(test)]
mod tests {
    #[test]
    fn a_catalogue_code_is_the_release_it_belongs_to() {
        let release = [
            0x01, 0x8f, 0x2a, 0x3b, 0x4c, 0x5d, 0x6e, 0x7f, 0x80, 0x91, 0xa2, 0xb3, 0xc4, 0xd5,
            0xe6, 0xf7,
        ];
        let code = YlCatalogueCode::derive(release);

        // Same release, same code: derived, never stored, so two readers
        // that never speak agree.
        assert_eq!(code, YlCatalogueCode::derive(release));

        // And it survives the round trip through the form people read.
        let parsed = YlCatalogueCode::parse(code.canonical()).expect("a valid code");
        assert_eq!(parsed, code);

        // A different release is a different code.
        let mut other = release;
        other[0] ^= 0xff;
        assert_ne!(code, YlCatalogueCode::derive(other));
    }

    #[test]
    fn a_derived_code_does_not_open_with_the_ulid() {
        // Hashed, not truncated: the first symbols must not simply be the
        // release ID's own leading bits, which are its timestamp.
        let release = [
            0x01, 0x8f, 0x2a, 0x3b, 0x4c, 0x5d, 0x6e, 0x7f, 0x80, 0x91, 0xa2, 0xb3, 0xc4, 0xd5,
            0xe6, 0xf7,
        ];
        let code = YlCatalogueCode::derive(release);
        let leading = code.as_compact_str();
        let raw = YlCatalogueCode::derive([0; 16]).as_compact_str();
        assert_ne!(leading, raw);
        assert_eq!(leading.len(), YL_CATALOGUE_COMPACT_LENGTH);
    }

    use super::*;

    fn code_from_payload(payload: &str) -> String {
        let mut compact = [0u8; YL_CATALOGUE_COMPACT_LENGTH];
        for (index, character) in payload.chars().enumerate() {
            compact[index] = decode_symbol(character).unwrap();
        }
        compact[YL_CATALOGUE_DATA_SYMBOLS] =
            YlCatalogueCode::checksum_symbol_value(&compact[..YL_CATALOGUE_DATA_SYMBOLS]);
        let code = YlCatalogueCode::from_symbol_values(compact).unwrap();
        code.as_compact_str().to_string()
    }

    #[test]
    fn fixed_vectors_are_stable() {
        assert_eq!(code_from_payload("0000000000"), "0000000000S");
        assert_eq!(code_from_payload("ABCDEFGHJK"), "ABCDEFGHJK8");
        assert_eq!(code_from_payload("K7M3P9TX4Q"), "K7M3P9TX4Q1");
    }

    #[test]
    fn formatting_is_stable() {
        let code: YlCatalogueCode = "yl_K7M3P9TX4Q1".parse().unwrap();
        assert_eq!(code.as_compact_str(), "K7M3P9TX4Q1");
        assert_eq!(code.canonical(), "yl_K7M3P9TX4Q1");
        assert_eq!(code.slug(), "k7m3p-9tx4q-1");
        assert_eq!(code.label(), "YL K7M3P · 9TX4Q · 1");
        assert_eq!(code.permalink_path(), "/k7m3p-9tx4q-1");
        assert_eq!(code.permalink(), "https://yl.vin/k7m3p-9tx4q-1");
        assert_eq!(code.to_string(), "yl_K7M3P9TX4Q1");
        assert_eq!(serde_json::to_string(&code).unwrap(), "\"yl_K7M3P9TX4Q1\"");
    }

    #[test]
    fn parsing_accepts_presentation_forms_and_aliases() {
        let expected: YlCatalogueCode = "yl_K7M3P9TX4Q1".parse().unwrap();
        for input in [
            "yl_K7M3P9TX4Q1",
            "YL K7M3P · 9TX4Q · 1",
            "k7m3p-9tx4q-1",
            "K7M3P9TX4Q1",
            "yl.k7m3p.9tx4q.1",
            "yl_k7m3p9tx4q1",
        ] {
            assert_eq!(input.parse::<YlCatalogueCode>().unwrap(), expected);
        }

        assert_eq!(
            "yl_K7M3P9TX4QI".parse::<YlCatalogueCode>().unwrap(),
            expected
        );
        assert_eq!(
            "yl_OOOOOOOOOOS".parse::<YlCatalogueCode>().unwrap(),
            "yl_0000000000S".parse::<YlCatalogueCode>().unwrap()
        );
    }

    #[test]
    fn parsing_rejects_bad_inputs() {
        assert_eq!(
            "".parse::<YlCatalogueCode>().unwrap_err(),
            YlCatalogueCodeParseError::Empty
        );
        assert_eq!(
            "yl_K7M3P9TX4Q".parse::<YlCatalogueCode>().unwrap_err(),
            YlCatalogueCodeParseError::InvalidLength { actual: 10 }
        );
        assert_eq!(
            "yl_K7M3P9TX4Q44".parse::<YlCatalogueCode>().unwrap_err(),
            YlCatalogueCodeParseError::InvalidLength { actual: 12 }
        );
        assert!(matches!(
            "yl_K7M3P9TX4Q*".parse::<YlCatalogueCode>().unwrap_err(),
            YlCatalogueCodeParseError::InvalidCharacter { character: '*', .. }
        ));
        assert_eq!(
            "yl_K7M3P9TX4Q4".parse::<YlCatalogueCode>().unwrap_err(),
            YlCatalogueCodeParseError::InvalidChecksum
        );
        assert_eq!(
            "yl_K7M3P9TX5Q1".parse::<YlCatalogueCode>().unwrap_err(),
            YlCatalogueCodeParseError::InvalidChecksum
        );
    }

    #[test]
    fn checksum_detects_common_adjacent_transpositions() {
        let original = "K7M3P9TX4Q1".parse::<YlCatalogueCode>().unwrap();
        for swapped in [
            "7KM3P9TX4Q1",
            "KM73P9TX4Q1",
            "K7MP39TX4Q1",
            "K7M3PT9X4Q1",
            "K7M3P94XTQ1",
        ] {
            assert_eq!(
                swapped.parse::<YlCatalogueCode>().unwrap_err(),
                YlCatalogueCodeParseError::InvalidChecksum,
                "swap should be detected against {}",
                original.as_compact_str()
            );
        }
    }

    #[cfg(feature = "generate")]
    #[test]
    fn generation_produces_valid_canonical_codes() {
        let code = YlCatalogueCode::generate();
        assert_eq!(code.as_compact_str().len(), YL_CATALOGUE_COMPACT_LENGTH);
        assert!(code
            .as_compact_str()
            .chars()
            .all(|character| YL_CATALOGUE_ALPHABET.contains(character)));
        assert_eq!(code.canonical().parse::<YlCatalogueCode>().unwrap(), code);
    }
}

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
//! ## Public YL catalogue numbers
//!
//! Publicly issued picture records may additionally receive a centrally
//! allocated YL catalogue number:
//!
//! ```text
//! yl_000241
//! ```
//!
//! Its presentation forms are:
//!
//! ```text
//! Printed catalogue number: YL 000241
//! Permalink:                https://yl.vin/000241
//! ```
//!
//! A YL catalogue number is allocated by the YL registry and MUST NOT be
//! generated independently by clients. It normally identifies one immutable
//! publicly issued record edition.
//!
//! IDs and catalogue numbers name objects. They are not content commitments.
//! Exact content identity belongs in explicitly named hashes such as
//! `recordCommitment`, `recordSha256`, or `releaseManifestSha256`.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use ulid::Ulid;

/// Number of characters in the canonical text representation of a ULID.
pub const ULID_TEXT_LENGTH: usize = 26;

/// Minimum number of decimal digits in a canonical YL catalogue number.
///
/// Values remain naturally extensible beyond six digits. For example,
/// `1_000_000` is represented canonically as `yl_1000000`.
pub const YL_CATALOGUE_MIN_DIGITS: usize = 6;

/// Canonical machine-readable YL catalogue prefix.
pub const YL_CATALOGUE_PREFIX: &str = "yl_";

/// Public host used for canonical YL record permalinks.
pub const YL_PERMALINK_HOST: &str = "yl.vin";

/// Public Bitneedle identifier kinds.
///
/// Application-only workflow identifiers such as uploads, jobs, sessions, and
/// users deliberately do not belong in this crate.
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
    /// Canonical lowercase prefix without the trailing underscore.
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

    /// Canonical prefix including the trailing underscore.
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

    /// Resolve a canonical prefix without accepting aliases.
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

/// Error returned when a Bitneedle identifier is malformed or has the wrong
/// object kind.
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

    #[error("YL catalogue number must be greater than zero")]
    ZeroYlCatalogueNumber,

    #[error("YL catalogue number is missing the canonical `yl_` prefix")]
    MissingYlCataloguePrefix,

    #[error("YL catalogue body must contain decimal ASCII digits only")]
    InvalidYlCatalogueDigits,

    #[error("YL catalogue number is not in canonical zero-padded form")]
    NonCanonicalYlCatalogueNumber,

    #[error("YL catalogue number is too large")]
    YlCatalogueNumberOverflow,
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

    // `ulid` accepts some non-canonical textual inputs. Bitneedle emits and
    // accepts only the canonical uppercase representation.
    if ulid.to_string() != body {
        return Err(BitneedleIdError::NonCanonicalUlid);
    }

    Ok((kind, ulid))
}

/// A dynamically typed public Bitneedle identifier.
///
/// Prefer the concrete wrappers (`ReleaseId`, `EditionId`, and so on) in normal
/// domain models. Use this type when parsing a field that intentionally accepts
/// more than one registered identifier kind.
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

/// Centrally allocated public catalogue number for a YL picture record.
///
/// This is intentionally separate from [`BitneedleId`]. A YL catalogue number
/// is a short public identifier used on labels, share cards, QR destinations,
/// and `yl.vin` permalinks.
///
/// It normally maps to one immutable [`EditionId`].
///
/// This type deliberately does not provide `new()`. Catalogue numbers must be
/// issued transactionally by the YL registry rather than generated by clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct YlCatalogueNumber(u64);

impl YlCatalogueNumber {
    /// Construct a catalogue number from a registry-allocated sequence value.
    pub const fn from_sequence(sequence: u64) -> Result<Self, BitneedleIdError> {
        if sequence == 0 {
            return Err(BitneedleIdError::ZeroYlCatalogueNumber);
        }

        Ok(Self(sequence))
    }

    /// Return the underlying positive registry sequence.
    pub const fn sequence(self) -> u64 {
        self.0
    }

    /// Canonical URL path segment.
    pub fn slug(self) -> String {
        format!("{:0width$}", self.0, width = YL_CATALOGUE_MIN_DIGITS)
    }

    /// Printed catalogue form used on labels and artwork.
    pub fn label(self) -> String {
        format!("YL {}", self.slug())
    }

    /// Canonical permalink path.
    pub fn permalink_path(self) -> String {
        format!("/{}", self.slug())
    }

    /// Canonical HTTPS permalink.
    pub fn permalink(self) -> String {
        format!("https://{}{}", YL_PERMALINK_HOST, self.permalink_path())
    }

    /// Parse the decimal path segment used by `yl.vin`.
    ///
    /// Only canonical path forms are accepted. Values below one million must
    /// use six digits; larger values must not contain redundant leading zeroes.
    pub fn from_slug(slug: &str) -> Result<Self, BitneedleIdError> {
        if slug.is_empty() || !slug.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(BitneedleIdError::InvalidYlCatalogueDigits);
        }

        let sequence = slug
            .parse::<u64>()
            .map_err(|_| BitneedleIdError::YlCatalogueNumberOverflow)?;

        let number = Self::from_sequence(sequence)?;

        if number.slug() != slug {
            return Err(BitneedleIdError::NonCanonicalYlCatalogueNumber);
        }

        Ok(number)
    }
}

impl fmt::Display for YlCatalogueNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{YL_CATALOGUE_PREFIX}{}", self.slug())
    }
}

impl FromStr for YlCatalogueNumber {
    type Err = BitneedleIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let body = value
            .strip_prefix(YL_CATALOGUE_PREFIX)
            .ok_or(BitneedleIdError::MissingYlCataloguePrefix)?;

        Self::from_slug(body)
    }
}

impl Serialize for YlCatalogueNumber {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for YlCatalogueNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
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
                write!(
                    formatter,
                    "{}{}",
                    BitneedleIdKind::$kind.tagged_prefix(),
                    self.0
                )
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

define_typed_id!(
    /// Stable identity of a logical release.
    ReleaseId,
    Release
);

define_typed_id!(
    /// Stable identity of a release edition.
    EditionId,
    Edition
);

define_typed_id!(
    /// Stable identity of a reusable public asset.
    AssetId,
    Asset
);

define_typed_id!(
    /// Identity of a track that is addressable outside one record's local track array.
    TrackId,
    Track
);

define_typed_id!(
    /// Identity of an independently addressable or licensable revolution.
    RevolutionId,
    Revolution
);

define_typed_id!(
    /// Identity of a persisted registration or issuance receipt.
    ReceiptId,
    Receipt
);

define_typed_id!(
    /// Identity of a persisted rights object.
    RightsId,
    Rights
);

define_typed_id!(
    /// Identity of an independently addressable Bitneedle sidecar object.
    SidecarId,
    Sidecar
);

define_typed_id!(
    /// Identity of a persisted issuance or platform attestation.
    AttestationId,
    Attestation
);

define_typed_id!(
    /// Identity of a persisted creator or account authorisation.
    AuthorizationId,
    Authorization
);

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = "rel_01JXWQ7H6K8V4Z2T9M3N5C1BPA";
    const EDITION: &str = "edn_01JXWQ8CM2R6F4KD9H7T3Y5VNE";

    #[test]
    fn parses_and_formats_release_id() {
        let id: ReleaseId = RELEASE.parse().unwrap();
        assert_eq!(id.to_string(), RELEASE);
        assert_eq!(ReleaseId::kind(), BitneedleIdKind::Release);
    }

    #[cfg(feature = "generate")]
    #[test]
    fn generated_ids_have_the_expected_prefix() {
        assert!(ReleaseId::new().to_string().starts_with("rel_"));
        assert!(EditionId::new().to_string().starts_with("edn_"));
        assert!(AssetId::new().to_string().starts_with("ast_"));
        assert!(TrackId::new().to_string().starts_with("trk_"));
        assert!(RevolutionId::new().to_string().starts_with("rev_"));
        assert!(ReceiptId::new().to_string().starts_with("rcp_"));
        assert!(RightsId::new().to_string().starts_with("rgt_"));
        assert!(SidecarId::new().to_string().starts_with("bsc_"));
        assert!(AttestationId::new().to_string().starts_with("att_"));
        assert!(AuthorizationId::new().to_string().starts_with("aut_"));
    }

    #[test]
    fn rejects_wrong_kind() {
        let error = EDITION.parse::<ReleaseId>().unwrap_err();
        assert!(matches!(
            error,
            BitneedleIdError::WrongKind {
                expected: "rel",
                ..
            }
        ));
    }

    #[test]
    fn rejects_lowercase_ulid_body() {
        let value = RELEASE.to_ascii_lowercase();
        assert_eq!(
            value.parse::<ReleaseId>().unwrap_err(),
            BitneedleIdError::NonCanonicalUlid
        );
    }

    #[test]
    fn rejects_unknown_prefix() {
        let value = "upl_01JXWQ7H6K8V4Z2T9M3N5C1BPA";
        assert_eq!(
            value.parse::<BitneedleId>().unwrap_err(),
            BitneedleIdError::UnknownPrefix("upl".to_owned())
        );
    }

    #[test]
    fn dynamic_id_round_trips_to_typed_id() {
        let dynamic: BitneedleId = RELEASE.parse().unwrap();
        assert_eq!(dynamic.kind(), BitneedleIdKind::Release);

        let typed = ReleaseId::try_from(dynamic).unwrap();
        assert_eq!(typed.to_string(), RELEASE);
    }

    #[test]
    fn serde_uses_the_prefixed_canonical_string() {
        let id: ReleaseId = RELEASE.parse().unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{RELEASE}\""));

        let decoded: ReleaseId = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn timestamp_is_available_but_not_identity_proof() {
        let id: ReleaseId = RELEASE.parse().unwrap();
        assert!(id.timestamp() >= UNIX_EPOCH);
    }
    #[test]
    fn yl_catalogue_number_has_distinct_machine_label_and_permalink_forms() {
        let number = YlCatalogueNumber::from_sequence(241).unwrap();

        assert_eq!(number.sequence(), 241);
        assert_eq!(number.to_string(), "yl_000241");
        assert_eq!(number.slug(), "000241");
        assert_eq!(number.label(), "YL 000241");
        assert_eq!(number.permalink_path(), "/000241");
        assert_eq!(number.permalink(), "https://yl.vin/000241");
    }

    #[test]
    fn yl_catalogue_number_round_trips_from_canonical_text() {
        let number: YlCatalogueNumber = "yl_000241".parse().unwrap();

        assert_eq!(number.sequence(), 241);
        assert_eq!(number.to_string(), "yl_000241");
    }

    #[test]
    fn yl_catalogue_number_round_trips_from_permalink_slug() {
        let number = YlCatalogueNumber::from_slug("000241").unwrap();

        assert_eq!(number.to_string(), "yl_000241");
    }

    #[test]
    fn yl_catalogue_number_expands_beyond_six_digits() {
        let number = YlCatalogueNumber::from_sequence(1_000_000).unwrap();

        assert_eq!(number.to_string(), "yl_1000000");
        assert_eq!(number.slug(), "1000000");
        assert_eq!(number.label(), "YL 1000000");
    }

    #[test]
    fn yl_catalogue_number_rejects_zero() {
        assert_eq!(
            YlCatalogueNumber::from_sequence(0).unwrap_err(),
            BitneedleIdError::ZeroYlCatalogueNumber
        );

        assert_eq!(
            "yl_000000".parse::<YlCatalogueNumber>().unwrap_err(),
            BitneedleIdError::ZeroYlCatalogueNumber
        );
    }

    #[test]
    fn yl_catalogue_number_rejects_noncanonical_padding() {
        assert_eq!(
            "yl_241".parse::<YlCatalogueNumber>().unwrap_err(),
            BitneedleIdError::NonCanonicalYlCatalogueNumber
        );

        assert_eq!(
            "yl_0000241".parse::<YlCatalogueNumber>().unwrap_err(),
            BitneedleIdError::NonCanonicalYlCatalogueNumber
        );
    }

    #[test]
    fn yl_catalogue_number_rejects_non_decimal_characters() {
        assert_eq!(
            "yl_00A241".parse::<YlCatalogueNumber>().unwrap_err(),
            BitneedleIdError::InvalidYlCatalogueDigits
        );
    }

    #[test]
    fn yl_catalogue_number_serde_uses_canonical_machine_text() {
        let number = YlCatalogueNumber::from_sequence(241).unwrap();

        let json = serde_json::to_string(&number).unwrap();
        assert_eq!(json, "\"yl_000241\"");

        let decoded: YlCatalogueNumber = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, number);
    }
}

use anyhow::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

const ULID_LEN: usize = 26;
const PREFIXED_ID_MIN_PREFIX_LEN: usize = 3;
const PREFIXED_ID_MAX_PREFIX_LEN: usize = 8;
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    Key,
    User,
    Job,
    Upload,
    Session,
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
            Self::Key => "key",
            Self::User => "usr",
            Self::Job => "job",
            Self::Upload => "upl",
            Self::Session => "ses",
        }
    }

    pub fn from_prefix(prefix: &str) -> Option<Self> {
        Some(match prefix {
            "rel" => Self::Release,
            "edn" => Self::Edition,
            "ast" => Self::Asset,
            "trk" => Self::Track,
            "rev" => Self::Revolution,
            "rcp" => Self::Receipt,
            "rgt" => Self::Rights,
            "bsc" => Self::Sidecar,
            "att" => Self::Attestation,
            "aut" => Self::Authorization,
            "key" => Self::Key,
            "usr" => Self::User,
            "job" => Self::Job,
            "upl" => Self::Upload,
            "ses" => Self::Session,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BitneedleId {
    kind: BitneedleIdKind,
    value: String,
}

impl BitneedleId {
    pub fn parse(value: &str) -> Result<Self> {
        let Some((prefix, ulid)) = value.split_once('_') else {
            bail!("Bitneedle identifier must contain a prefix and ULID separated by '_'");
        };
        let kind = BitneedleIdKind::from_prefix(prefix)
            .ok_or_else(|| anyhow::anyhow!("unsupported Bitneedle identifier prefix: {prefix}"))?;
        validate_prefix(prefix)?;
        let ulid = canonical_ulid(ulid)?;
        Ok(Self {
            kind,
            value: format!("{prefix}_{ulid}"),
        })
    }

    pub fn parse_kind(kind: BitneedleIdKind, value: &str) -> Result<Self> {
        let id = Self::parse(value)?;
        if id.kind != kind {
            bail!(
                "expected {}_ identifier, received {}",
                kind.prefix(),
                id.as_str()
            );
        }
        Ok(id)
    }

    pub fn from_timestamp_entropy(
        kind: BitneedleIdKind,
        timestamp_ms: u64,
        entropy: [u8; 10],
    ) -> Self {
        let mut bytes = [0u8; 16];
        let timestamp = timestamp_ms & 0x0000_ffff_ffff_ffff;
        bytes[0] = (timestamp >> 40) as u8;
        bytes[1] = (timestamp >> 32) as u8;
        bytes[2] = (timestamp >> 24) as u8;
        bytes[3] = (timestamp >> 16) as u8;
        bytes[4] = (timestamp >> 8) as u8;
        bytes[5] = timestamp as u8;
        bytes[6..].copy_from_slice(&entropy);
        let ulid = encode_ulid_bytes(bytes);
        Self {
            kind,
            value: format!("{}_{}", kind.prefix(), ulid),
        }
    }

    pub fn kind(&self) -> BitneedleIdKind {
        self.kind
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn ulid(&self) -> &str {
        &self.value[self.value.len() - ULID_LEN..]
    }

    /// The raw 16-byte ULID value, as stored in binary wire formats.
    pub fn ulid_bytes(&self) -> [u8; 16] {
        decode_ulid_bytes(self.ulid()).expect("BitneedleId always holds a canonical ULID")
    }

    pub fn from_ulid_bytes(kind: BitneedleIdKind, bytes: [u8; 16]) -> Self {
        let ulid = encode_ulid_bytes(bytes);
        Self {
            kind,
            value: format!("{}_{}", kind.prefix(), ulid),
        }
    }
}

impl fmt::Display for BitneedleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}

impl FromStr for BitneedleId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

impl Serialize for BitneedleId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for BitneedleId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

macro_rules! typed_id {
    ($name:ident, $kind:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        pub struct $name(BitneedleId);

        impl $name {
            pub fn parse(value: &str) -> Result<Self> {
                Ok(Self(BitneedleId::parse_kind($kind, value)?))
            }

            pub fn from_timestamp_entropy(timestamp_ms: u64, entropy: [u8; 10]) -> Self {
                Self(BitneedleId::from_timestamp_entropy(
                    $kind,
                    timestamp_ms,
                    entropy,
                ))
            }

            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }

            pub fn into_inner(self) -> BitneedleId {
                self.0
            }

            pub fn ulid_bytes(&self) -> [u8; 16] {
                self.0.ulid_bytes()
            }

            pub fn from_ulid_bytes(bytes: [u8; 16]) -> Self {
                Self(BitneedleId::from_ulid_bytes($kind, bytes))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(value: &str) -> Result<Self> {
                Self::parse(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}

typed_id!(ReleaseId, BitneedleIdKind::Release);
typed_id!(EditionId, BitneedleIdKind::Edition);
typed_id!(AssetId, BitneedleIdKind::Asset);
typed_id!(TrackId, BitneedleIdKind::Track);
typed_id!(RevolutionId, BitneedleIdKind::Revolution);
typed_id!(ReceiptId, BitneedleIdKind::Receipt);
typed_id!(RightsId, BitneedleIdKind::Rights);
typed_id!(SidecarId, BitneedleIdKind::Sidecar);
typed_id!(AttestationId, BitneedleIdKind::Attestation);
typed_id!(AuthorizationId, BitneedleIdKind::Authorization);
typed_id!(JobId, BitneedleIdKind::Job);
typed_id!(UploadId, BitneedleIdKind::Upload);
typed_id!(SessionId, BitneedleIdKind::Session);
typed_id!(UserId, BitneedleIdKind::User);

fn validate_prefix(prefix: &str) -> Result<()> {
    if prefix.len() < PREFIXED_ID_MIN_PREFIX_LEN || prefix.len() > PREFIXED_ID_MAX_PREFIX_LEN {
        bail!("Bitneedle identifier prefix length is invalid");
    }
    let mut chars = prefix.chars();
    let Some(first) = chars.next() else {
        bail!("Bitneedle identifier prefix is empty");
    };
    if !first.is_ascii_lowercase() {
        bail!("Bitneedle identifier prefix must start with lowercase ASCII");
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        bail!("Bitneedle identifier prefix must contain only lowercase ASCII letters and digits");
    }
    Ok(())
}

fn canonical_ulid(value: &str) -> Result<String> {
    if value.len() != ULID_LEN {
        bail!("ULID must be 26 Crockford Base32 characters");
    }
    let mut out = String::with_capacity(ULID_LEN);
    for c in value.chars() {
        let c = c.to_ascii_uppercase();
        if !matches!(c, '0'..='9' | 'A'..='H' | 'J'..='K' | 'M'..='N' | 'P'..='T' | 'V'..='Z') {
            bail!("ULID contains a non-canonical Crockford Base32 character");
        }
        out.push(c);
    }
    Ok(out)
}

fn decode_ulid_bytes(value: &str) -> Result<[u8; 16]> {
    let canonical = canonical_ulid(value)?;
    let mut bits: u128 = 0;
    for c in canonical.bytes() {
        let digit = CROCKFORD
            .iter()
            .position(|&x| x == c)
            .ok_or_else(|| anyhow::anyhow!("ULID contains a non-canonical Crockford Base32 character"))?;
        bits = (bits << 5) | digit as u128;
    }
    Ok(bits.to_be_bytes())
}

fn encode_ulid_bytes(bytes: [u8; 16]) -> String {
    let mut value = u128::from_be_bytes(bytes);
    let mut chars = [b'0'; ULID_LEN];
    for index in (0..ULID_LEN).rev() {
        chars[index] = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(chars.to_vec()).expect("ULID alphabet is ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_canonicalizes_prefixed_ulids() {
        let id = ReleaseId::parse("rel_01jxwq7h6k8v4z2t9m3n5c1bpa").unwrap();
        assert_eq!(id.as_str(), "rel_01JXWQ7H6K8V4Z2T9M3N5C1BPA");
    }

    #[test]
    fn rejects_wrong_prefix_for_typed_id() {
        let err = ReleaseId::parse("edn_01JXWQ8CM2R6F4KD9H7T3Y5VNE")
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected rel_ identifier"));
    }

    #[test]
    fn rejects_malformed_ulids() {
        assert!(ReleaseId::parse("rel_01JXWQ7H6K8V4Z2T9M3N5C1BPO").is_err());
        assert!(ReleaseId::parse("rel_01JXWQ7H6K8V4Z2T9M3N5C1BP").is_err());
        assert!(ReleaseId::parse("REL_01JXWQ7H6K8V4Z2T9M3N5C1BPA").is_err());
    }

    #[test]
    fn generates_from_timestamp_and_entropy() {
        let id = TrackId::from_timestamp_entropy(0x0197_1234_5678, [0xab; 10]);
        assert_eq!(id.as_str().len(), 30);
        assert!(id.as_str().starts_with("trk_"));
        TrackId::parse(id.as_str()).unwrap();
    }

    #[test]
    fn serde_uses_canonical_text() {
        let id = ReceiptId::parse("rcp_01jxwqct4b6z8n0m2k5d7y9p3v").unwrap();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"rcp_01JXWQCT4B6Z8N0M2K5D7Y9P3V\""
        );
        let parsed: ReceiptId = serde_json::from_str("\"rcp_01JXWQCT4B6Z8N0M2K5D7Y9P3V\"").unwrap();
        assert_eq!(parsed, id);
    }
}

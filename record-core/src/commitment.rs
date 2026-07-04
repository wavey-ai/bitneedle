//! Revolution and release commitment hashing for the final compact format.
//!
//! See `TODO-final-one-ecdc-per-revolution.md` sections 1.3 and 1.4 for the
//! normative preimage layouts. These functions hash the exact byte
//! sequences specified there; do not reorder or reshape the preimage.
//!
//! One ECDC payload entry is exactly one revolution: there is no separate
//! revolution table, and `revolution_index == payload_entry_index`.

use sha2::{Digest, Sha256};

const REVOLUTION_DOMAIN: &[u8] = b"BITNEEDLE-REVOLUTION-V1";
const RELEASE_DOMAIN: &[u8] = b"BITNEEDLE-RELEASE-V1";

/// SHA-256 over the canonical revolution commitment preimage (section 1.3):
/// domain || release_id || revolution_index (u32be) || SHA-256(ECDC entry bytes).
pub fn revolution_commitment(
    release_id: [u8; 16],
    revolution_index: u32,
    ecdc_entry_sha256: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(REVOLUTION_DOMAIN);
    hasher.update(release_id);
    hasher.update(revolution_index.to_be_bytes());
    hasher.update(ecdc_entry_sha256);
    hasher.finalize().into()
}

/// SHA-256 over the canonical release commitment preimage (section 1.4).
///
/// `revolution_commitments` must already be in ascending `revolution_index`
/// order; this function does not sort them.
pub fn release_commitment(
    release_id: [u8; 16],
    record_profile_code: u8,
    payload_encoding_code: u8,
    brs1_sha256: [u8; 32],
    revolution_commitments: &[[u8; 32]],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(RELEASE_DOMAIN);
    hasher.update(release_id);
    hasher.update([record_profile_code]);
    hasher.update([payload_encoding_code]);
    hasher.update(brs1_sha256);
    hasher.update((revolution_commitments.len() as u32).to_be_bytes());
    for commitment in revolution_commitments {
        hasher.update(commitment);
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_id() -> [u8; 16] {
        [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ]
    }

    fn ecdc_sha256() -> [u8; 32] {
        Sha256::digest(b"ecdc entry bytes").into()
    }

    #[test]
    fn revolution_commitment_matches_manual_preimage() {
        let release_id = release_id();
        let ecdc_sha256 = ecdc_sha256();
        let commitment = revolution_commitment(release_id, 3, ecdc_sha256);

        let mut preimage = Vec::new();
        preimage.extend_from_slice(REVOLUTION_DOMAIN);
        preimage.extend_from_slice(&release_id);
        preimage.extend_from_slice(&3u32.to_be_bytes());
        preimage.extend_from_slice(&ecdc_sha256);
        let expected: [u8; 32] = Sha256::digest(&preimage).into();

        assert_eq!(commitment, expected);
    }

    #[test]
    fn wrong_release_id_changes_commitment() {
        let ecdc_sha256 = ecdc_sha256();
        let a = revolution_commitment(release_id(), 3, ecdc_sha256);
        let mut other_release_id = release_id();
        other_release_id[0] ^= 0xff;
        let b = revolution_commitment(other_release_id, 3, ecdc_sha256);
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_revolution_index_changes_commitment() {
        let ecdc_sha256 = ecdc_sha256();
        let a = revolution_commitment(release_id(), 3, ecdc_sha256);
        let b = revolution_commitment(release_id(), 4, ecdc_sha256);
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_ecdc_bytes_change_commitment() {
        let a = revolution_commitment(release_id(), 3, ecdc_sha256());
        let mut other_ecdc_sha256 = ecdc_sha256();
        other_ecdc_sha256[0] ^= 0xff;
        let b = revolution_commitment(release_id(), 3, other_ecdc_sha256);
        assert_ne!(a, b);
    }

    #[test]
    fn release_commitment_matches_manual_preimage() {
        let release_id = release_id();
        let brs1_sha256: [u8; 32] = Sha256::digest(b"brs1 bytes").into();
        let revolutions = [
            revolution_commitment(release_id, 0, ecdc_sha256()),
            revolution_commitment(release_id, 1, ecdc_sha256()),
        ];

        let commitment = release_commitment(release_id, 1, 0, brs1_sha256, &revolutions);

        let mut preimage = Vec::new();
        preimage.extend_from_slice(RELEASE_DOMAIN);
        preimage.extend_from_slice(&release_id);
        preimage.push(1);
        preimage.push(0);
        preimage.extend_from_slice(&brs1_sha256);
        preimage.extend_from_slice(&(revolutions.len() as u32).to_be_bytes());
        for r in &revolutions {
            preimage.extend_from_slice(r);
        }
        let expected: [u8; 32] = Sha256::digest(&preimage).into();

        assert_eq!(commitment, expected);
    }

    #[test]
    fn release_commitment_changes_when_any_revolution_changes() {
        let release_id = release_id();
        let brs1_sha256: [u8; 32] = Sha256::digest(b"brs1 bytes").into();
        let r0 = revolution_commitment(release_id, 0, ecdc_sha256());
        let r1 = revolution_commitment(release_id, 1, ecdc_sha256());
        let r1_changed = revolution_commitment(
            release_id,
            1,
            Sha256::digest(b"different ecdc bytes").into(),
        );

        let a = release_commitment(release_id, 1, 0, brs1_sha256, &[r0, r1]);
        let b = release_commitment(release_id, 1, 0, brs1_sha256, &[r0, r1_changed]);
        assert_ne!(a, b);
    }

    #[test]
    fn release_commitment_changes_when_order_changes() {
        let release_id = release_id();
        let brs1_sha256: [u8; 32] = Sha256::digest(b"brs1 bytes").into();
        let r0 = revolution_commitment(release_id, 0, ecdc_sha256());
        let r1 = revolution_commitment(release_id, 1, Sha256::digest(b"other bytes").into());

        let forward = release_commitment(release_id, 1, 0, brs1_sha256, &[r0, r1]);
        let reversed = release_commitment(release_id, 1, 0, brs1_sha256, &[r1, r0]);

        assert_ne!(forward, reversed);
    }

    #[test]
    fn changing_brs1_bytes_changes_release_commitment() {
        let release_id = release_id();
        let revolutions = [revolution_commitment(release_id, 0, ecdc_sha256())];
        let brs1_a: [u8; 32] = Sha256::digest(b"brs1 bytes").into();
        let brs1_b: [u8; 32] = Sha256::digest(b"brs1 bytes, modified").into();

        let a = release_commitment(release_id, 1, 0, brs1_a, &revolutions);
        let b = release_commitment(release_id, 1, 0, brs1_b, &revolutions);
        assert_ne!(a, b);
    }

    #[test]
    fn changing_record_profile_changes_release_commitment() {
        let release_id = release_id();
        let brs1_sha256: [u8; 32] = Sha256::digest(b"brs1 bytes").into();
        let revolutions = [revolution_commitment(release_id, 0, ecdc_sha256())];

        let a = release_commitment(release_id, 0, 0, brs1_sha256, &revolutions);
        let b = release_commitment(release_id, 1, 0, brs1_sha256, &revolutions);
        assert_ne!(a, b);
    }

    #[test]
    fn changing_payload_encoding_changes_release_commitment() {
        let release_id = release_id();
        let brs1_sha256: [u8; 32] = Sha256::digest(b"brs1 bytes").into();
        let revolutions = [revolution_commitment(release_id, 0, ecdc_sha256())];

        let a = release_commitment(release_id, 1, 0, brs1_sha256, &revolutions);
        let b = release_commitment(release_id, 1, 1, brs1_sha256, &revolutions);
        assert_ne!(a, b);
    }
}

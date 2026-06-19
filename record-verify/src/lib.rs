//! Public trust and cryptographic verification for Bitneedle records.
//!
//! Structural BRS1 verification belongs to `record-core`.
//! Structural BRD1 verification belongs to `record-descriptor`.
//! This crate resolves trust and verifies the opaque binary
//! `SignedReleaseReference`.

use anyhow::Result;
use record_descriptor::SignedReleaseReference;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedReleaseStatus {
    Missing,
    Verified,
    Invalid,
    Unsupported,
    UntrustedKey,
}

pub trait SignedReleaseVerifier {
    fn verify(
        &mut self,
        manifest_hash_algorithm: u8,
        manifest_hash: &[u8],
        signature_algorithm: u8,
        key_id: &[u8],
        signature: &[u8],
    ) -> Result<()>;
}

impl<F> SignedReleaseVerifier for F
where
    F: FnMut(u8, &[u8], u8, &[u8], &[u8]) -> Result<()>,
{
    fn verify(
        &mut self,
        manifest_hash_algorithm: u8,
        manifest_hash: &[u8],
        signature_algorithm: u8,
        key_id: &[u8],
        signature: &[u8],
    ) -> Result<()> {
        self(
            manifest_hash_algorithm,
            manifest_hash,
            signature_algorithm,
            key_id,
            signature,
        )
    }
}

pub fn verify_signed_release(
    reference: Option<&SignedReleaseReference>,
    verifier: &mut impl SignedReleaseVerifier,
) -> SignedReleaseStatus {
    let Some(reference) = reference else {
        return SignedReleaseStatus::Missing;
    };

    if reference.validate().is_err() {
        return SignedReleaseStatus::Invalid;
    }

    match verifier.verify(
        reference.manifest_hash_algorithm,
        &reference.manifest_hash,
        reference.signature_algorithm,
        &reference.key_id,
        &reference.signature,
    ) {
        Ok(()) => SignedReleaseStatus::Verified,
        Err(_) => SignedReleaseStatus::Invalid,
    }
}

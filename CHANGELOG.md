# Changelog

All notable changes to the public Bitneedle format, decode, and verify crates
are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Wallet creator authorisation (EIP-191), per
  `docs/bitneedle-wallet-signing-architecture.md` using the hybrid model
  (structured JSON stored; deterministic text message signed):
  - `record-descriptor`: `CreatorAuthorizationPayload`, `WalletSigner`,
    `CreatorAuthorizationEnvelope`, the `CREATOR_AUTHORIZATION_MESSAGE_DOMAIN`
    constant, and `build_creator_authorization_message` (deterministic, byte-exact
    UTF-8). New segments 30/31 carry the envelope; `RecordDescriptor` gains
    `creator_authorization`. (Replaces the earlier flat `CreatorAuthorization`.)
  - `record-verify`: `verify_creator_authorization` now rebuilds the text message
    and checks the EIP-191 signature (not the JSON); adds
    `verify_creator_authorization_eip191` + `CreatorAuthorizationVerification`
    (built-in secp256k1 recovery). Removed the old JSON-preimage path/domain.
- Account-based creator authorisation (wallet-free), per
  `docs/bitneedle-account-based-creator-authorisation.md`:
  - `record-descriptor`: `CreatorAuthorizationMode` enum, `AccountAuthorization`
    struct, and an extended `IssuanceAttestation` (adds `authorizationMode`,
    `accountNamespace`, `accountId`, `accountAuthorizationHash`). New descriptor
    segments 26–29 carry the account authorisation and issuance attestation,
    parsed as typed JSON with duplicate-segment rejection. `RecordDescriptor`
    gains `account_authorization` and `issuance_attestation`.
  - `record-verify`: `account_authorization_hash` (§11), `verify_account_authorization`
    (hash match + ed25519 attestation signature), and `describe_creator_authorization`
    reporting (`account-authorized` is not reported as unsigned).
  - Account identity is sourced from Wavey Zeroth's opaque `sub` (`usr_…`,
    256-bit random, provider-independent), read under the `global` namespace.
- `record-verify` crate: public verification with canonical JSON hashing,
  identity reconstruction (release ID, record commitment, edition ID),
  domain-separated signing preimages, append-only registration-receipt chain
  validation, creator-authorisation and issuance-attestation verification,
  `CreatorAuthorizationStatus`/`RegistryStatus` states, and `ed25519` + `eip191`
  signature primitives behind a `SignatureVerifier` trait.
- `record-descriptor`: typed format structs `RegistrationReceiptSet`,
  `RegistrationReceipt`, `RegistrationTarget`, `RecordCommitment`,
  `CreatorAuthorization` (+ `CreatorAuthorizationSigner`), and
  `IssuanceAttestation`.

### Changed
- `record-descriptor`: `RecordDescriptor.chain_registration_receipt:
  Option<String>` is now `registration_receipts: Option<RegistrationReceiptSet>`;
  descriptor segments 22/25 are parsed as a typed registration-receipt set, and
  duplicate receipt segments are now rejected.

### Reference
- Implements the public-side changes from
  `docs/bitneedle-signing-editions-and-onchain-architecture.md` (§9, §11–§13,
  §20–§22).

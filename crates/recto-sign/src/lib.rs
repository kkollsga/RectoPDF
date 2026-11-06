//! Digital signatures: **a placeholder with the final shape.**
//!
//! Owner, 2026-11-06: "lets do a signature place holder for now. We can build it out properly later using the official
//! standards. So structure the code accordingly." Nothing here signs anything or checks anything yet. The seams below
//! follow the standards and `docs/research/2026-11-06-signing.md`, so the real work replaces [`PlaceholderSigner`]
//! without changing its callers.
//!
//! Standards this fills in later: ISO 32000-2 §12.8, ETSI EN 319 142 (PAdES), RFC 5652 (CMS), RFC 3161 (timestamps).
//!
//! Two rules hold now and later:
//! - A placeholder never produces something that looks like a signature.
//! - [`Status`] has no "valid" state. A viewer reports facts; a verdict needs real trust checks.

/// The signature levels of ETSI EN 319 142-1 that the real signer will offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Signature with a signing certificate.
    BaselineB,
    /// B plus a trusted timestamp from an RFC 3161 service.
    BaselineT,
    /// T plus validation data embedded in the file.
    BaselineLt,
    /// LT plus an archive timestamp.
    BaselineLta,
}

/// Holds a private key somewhere this app never sees it: Keychain, CNG, a token. It signs a digest and hands back bytes.
pub trait KeyProvider {
    /// A name for the person choosing a certificate.
    fn display_name(&self) -> &str;
    /// The DER-encoded signing certificate.
    fn certificate_der(&self) -> &[u8];
    /// Signs a digest. Implementations ask a person through the OS (Touch ID, a PIN), never through an agent.
    fn sign_digest(&self, digest: &[u8]) -> Result<Vec<u8>, SignError>;
}

/// What to sign and how it should appear.
#[derive(Clone, Debug)]
pub struct SignRequest {
    pub level: Level,
    /// The form field to sign into.
    pub field_name: Box<str>,
    /// Why the person is signing, as they typed it.
    pub reason: Option<Box<str>>,
}

/// The byte range a signature covers, written by the append writer and read back to compute the digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub first_start: u64,
    pub first_len: u64,
    pub second_start: u64,
    pub second_len: u64,
}

/// Why signing didn't happen. Wording is built here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignError {
    NotBuilt,
    Cancelled,
    KeyFailed,
}

impl SignError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotBuilt => "Digital signatures aren't available in this version.",
            Self::Cancelled => "Signing was cancelled.",
            Self::KeyFailed => "The certificate couldn't sign this file.",
        }
    }
}

/// Applies a signature. The real one appends a revision through the lopdf writer and never touches earlier bytes.
pub trait Signer {
    fn sign(&self, request: &SignRequest, key: &dyn KeyProvider) -> Result<ByteRange, SignError>;
}

/// Stands in until the standards-based signer exists. It always refuses, and never asks the key to sign.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlaceholderSigner;

impl Signer for PlaceholderSigner {
    fn sign(&self, _request: &SignRequest, _key: &dyn KeyProvider) -> Result<ByteRange, SignError> {
        Err(SignError::NotBuilt)
    }
}

/// What a viewer may say about a signature already in a file. There is deliberately no "valid" state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// A signature is present. Nothing about it has been checked.
    NotChecked,
}

/// One signature found in a file.
#[derive(Clone, Debug)]
pub struct SignatureFacts {
    /// The name on the certificate, from the file itself, so unverified.
    pub certificate_name: Option<Box<str>>,
    pub status: Status,
}

impl SignatureFacts {
    /// The sentence shown for this signature. It states what is known, and never vouches for it.
    pub fn summary(&self) -> &'static str {
        match self.status {
            Status::NotChecked => "Signature present, not checked",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CountingKey(std::cell::Cell<u32>);

    impl KeyProvider for CountingKey {
        fn display_name(&self) -> &str {
            "Test"
        }
        fn certificate_der(&self) -> &[u8] {
            &[]
        }
        fn sign_digest(&self, _digest: &[u8]) -> Result<Vec<u8>, SignError> {
            self.0.set(self.0.get() + 1);
            Ok(Vec::new())
        }
    }

    /// Failure: the placeholder reports success or uses the key, and someone believes a file was signed.
    #[test]
    fn placeholder_refuses_and_never_touches_the_key() {
        let key = CountingKey(std::cell::Cell::new(0));
        let request = SignRequest { level: Level::BaselineT, field_name: "Tenant".into(), reason: None };
        assert_eq!(PlaceholderSigner.sign(&request, &key), Err(SignError::NotBuilt));
        assert_eq!(key.0.get(), 0);
    }

    /// Failure: wording claims a check that never happened.
    #[test]
    fn a_found_signature_is_never_described_as_valid() {
        let facts = SignatureFacts { certificate_name: Some("Tor Haugen".into()), status: Status::NotChecked };
        let text = facts.summary().to_lowercase();
        assert!(text.contains("not checked"));
        assert!(!text.contains("valid") && !text.contains("verified") && !text.contains("trusted"));
    }
}

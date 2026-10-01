//! The core's identity, as sidevoice-core proves it (`control/devices.py`): one ECDSA P-256 key per machine; its
//! public half is `public_key` (SPKI DER, base64), its `fingerprint` the SHA-256 of that DER, base64url without
//! padding; `GET /api/device/identity?nonce=…` answers a signature over `sidevoice-node-identity:<nonce>` in IEEE
//! P1363 form (r ‖ s, 32 bytes each), base64url.
//!
//! The app pins the key when it pairs and makes the core sign a fresh nonce with it before it sends the device token
//! on a new launch (design §4.1: discovery is skipped for the local host because native checks this itself).

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use sha2::{Digest, Sha256};

pub const CONTEXT: &str = "sidevoice-node-identity:";

/// Either alphabet, padded or not: the core writes the key in standard base64 and signatures in base64url.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD].iter().find_map(|engine| engine.decode(text).ok())
}

pub fn base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The fingerprint of a public key given as the core sends it (SPKI DER, base64); `None` if it is not base64.
pub fn fingerprint(public_key: &str) -> Option<String> {
    Some(base64url(&Sha256::digest(decode_base64(public_key)?)))
}

/// `count` bytes from the OS's generator, base64url.
pub fn random(count: usize) -> String {
    let mut bytes = vec![0u8; count];
    getrandom::fill(&mut bytes).expect("the OS random generator is available");
    base64url(&bytes)
}

/// A nonce the core accepts (base64url of 16 to 64 bytes): 32 fresh bytes.
pub fn nonce() -> String {
    random(32)
}

/// Whether `signature` is `public_key`'s signature over [`CONTEXT`] + `nonce`. Anything malformed is `false`.
/// A P-256 public key as the core sends it: SPKI DER (the fixed P-256 prefix, then the uncompressed point), base64.
pub fn spki(key: &VerifyingKey) -> String {
    const PREFIX: [u8; 26] = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce,
        0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];
    let mut der = PREFIX.to_vec();
    der.extend_from_slice(key.to_encoded_point(false).as_bytes());
    STANDARD.encode(der)
}

pub fn verify(public_key: &str, nonce: &str, signature: &str) -> bool {
    let Some(der) = decode_base64(public_key) else { return false };
    let Ok(key) = VerifyingKey::from_public_key_der(&der) else { return false };
    let Some(raw) = decode_base64(signature) else { return false };
    let Ok(signature) = Signature::from_slice(&raw) else { return false };
    // Python's `cryptography` does not normalise s; (r, s) and (r, n - s) are the same signature.
    let signature = signature.normalize_s().unwrap_or(signature);
    key.verify(format!("{CONTEXT}{nonce}").as_bytes(), &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::SigningKey;

    fn key() -> (SigningKey, String) {
        let signing = SigningKey::from_slice(&[7u8; 32]).unwrap();
        let public_key = spki(signing.verifying_key());
        (signing, public_key)
    }

    #[test]
    fn a_signature_by_the_pinned_key_verifies_and_nothing_else_does() {
        let (signing, public_key) = key();
        let nonce = nonce();
        let signature: Signature = signing.sign(format!("{CONTEXT}{nonce}").as_bytes());
        let encoded = base64url(&signature.to_bytes());
        assert!(verify(&public_key, &nonce, &encoded));
        assert!(!verify(&public_key, &(nonce.clone() + "x"), &encoded), "another nonce");
        let other = SigningKey::from_slice(&[9u8; 32]).unwrap();
        let theirs: Signature = other.sign(format!("{CONTEXT}{nonce}").as_bytes());
        assert!(!verify(&public_key, &nonce, &base64url(&theirs.to_bytes())), "another key");
        assert!(!verify(&public_key, &nonce, "not base64!"));
        assert!(!verify("AAAA", &nonce, &encoded));
    }

    #[test]
    fn a_high_s_signature_verifies_too() {
        let (signing, public_key) = key();
        let nonce = nonce();
        let signature: Signature = signing.sign(format!("{CONTEXT}{nonce}").as_bytes());
        let (r, s) = signature.split_scalars();
        let high = Signature::from_scalars(r, -*s).unwrap();
        assert_ne!(high, signature);
        assert!(verify(&public_key, &nonce, &base64url(&high.to_bytes())));
    }

    #[test]
    fn fingerprint_is_sha256_of_the_der_base64url() {
        let (_, public_key) = key();
        let der = STANDARD.decode(&public_key).unwrap();
        assert_eq!(fingerprint(&public_key).unwrap(), URL_SAFE_NO_PAD.encode(Sha256::digest(der)));
        assert_eq!(nonce().len(), 43);
    }
}

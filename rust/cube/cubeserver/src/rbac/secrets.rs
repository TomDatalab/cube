//! Secrets the RBAC database keeps (a data source's password, say), sealed
//! with AES-256-GCM under `CUBE_RBAC_SECRET_KEY` (base64 of 32 bytes). A
//! sealed value is a 12-byte random nonce followed by the ciphertext. The
//! associated data a value is sealed with (the row it belongs to) must be
//! given again to open it, so a value copied onto another row does not.

use aes_gcm::aead::{Aead, Generate, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};

/// The variable that holds the key.
pub const SECRET_KEY_VAR: &str = "CUBE_RBAC_SECRET_KEY";

#[derive(Clone)]
pub struct SecretKey {
    bytes: [u8; 32],
}

/// The key never reaches a log.
impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

impl SecretKey {
    /// The key as `CUBE_RBAC_SECRET_KEY` holds it: base64 of 32 bytes.
    pub fn from_base64(s: &str) -> Result<SecretKey, String> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|_| format!("{SECRET_KEY_VAR} is not base64"))?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|b: Vec<u8>| {
            format!(
                "{SECRET_KEY_VAR} must be 32 bytes (base64), not {}; make one with: openssl rand -base64 32",
                b.len()
            )
        })?;
        Ok(SecretKey { bytes })
    }

    pub fn from_env() -> Result<SecretKey, String> {
        match std::env::var(SECRET_KEY_VAR) {
            Ok(value) if !value.trim().is_empty() => Self::from_base64(&value),
            _ => Err(format!(
                "{SECRET_KEY_VAR} is not set; it encrypts the data source passwords RBAC keeps \
                 (make one with: openssl rand -base64 32)"
            )),
        }
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.bytes).expect("a 32-byte key")
    }

    /// A fresh random nonce followed by the ciphertext of `plain`, bound to
    /// `aad`.
    pub fn seal(&self, plain: &[u8], aad: &[u8]) -> Vec<u8> {
        let nonce = Nonce::generate();
        let ciphertext = self
            .cipher()
            .encrypt(&nonce, Payload { msg: plain, aad })
            .expect("AES-GCM encryption of an in-memory buffer");
        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(&ciphertext);
        out
    }

    /// What [`SecretKey::seal`] sealed with the same `aad`; an error for
    /// another key's value, another `aad`'s or a damaged one.
    pub fn open(&self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
        if sealed.len() < NONCE_LEN {
            return Err("the sealed value is too short".into());
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce).map_err(|_| "bad nonce".to_string())?;
        self.cipher().decrypt(&nonce, Payload { msg: ciphertext, aad }).map_err(|_| {
            format!("cannot decrypt a stored secret: {SECRET_KEY_VAR} is not the key it was sealed with, or the value is damaged or not this row's")
        })
    }
}

const NONCE_LEN: usize = 12;

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn key(byte: u8) -> SecretKey {
        SecretKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([byte; 32])).unwrap()
    }

    #[test]
    fn seal_then_open_roundtrips() {
        let k = key(1);
        let plain = b"{\"CUBEJS_DB_PASS\":\"x\"}";
        assert_eq!(k.open(&k.seal(plain, b"row"), b"row").unwrap(), plain);
    }

    #[test]
    fn sealing_twice_differs() {
        let k = key(1);
        assert_ne!(k.seal(b"same", b"row"), k.seal(b"same", b"row"));
    }

    #[test]
    fn wrong_key_fails() {
        let sealed = key(1).seal(b"secret", b"row");
        assert!(key(2).open(&sealed, b"row").is_err());
    }

    #[test]
    fn another_rows_associated_data_fails() {
        let k = key(1);
        let sealed = k.seal(b"secret", b"row a");
        assert!(k.open(&sealed, b"row b").is_err());
        assert!(k.open(&sealed, b"").is_err());
    }

    #[test]
    fn tampered_fails() {
        let k = key(1);
        let mut sealed = k.seal(b"secret", b"row");
        *sealed.last_mut().unwrap() ^= 1;
        assert!(k.open(&sealed, b"row").is_err());
    }

    #[test]
    fn short_input_fails() {
        assert!(key(1).open(&[0; 5], b"row").is_err());
    }

    #[test]
    fn key_must_be_32_bytes() {
        let short = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        let err = SecretKey::from_base64(&short).unwrap_err();
        assert!(err.contains(SECRET_KEY_VAR), "{err}");
        assert!(SecretKey::from_base64("not base64!").unwrap_err().contains(SECRET_KEY_VAR));
    }
}

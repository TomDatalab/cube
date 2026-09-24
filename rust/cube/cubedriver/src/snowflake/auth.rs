//! Snowflake authentication for the SQL REST API.
//!
//! The REST API accepts two token types: a key-pair JWT (`KEYPAIR_JWT`) that
//! the client signs itself, and an OAuth access token (`OAUTH`). The JWT is
//! the interesting one: its issuer carries the SHA-256 fingerprint of the
//! *public* key, which has to be derived from the configured private key. That
//! derivation is done here with a tiny DER reader/writer instead of pulling in
//! an RSA crate (and definitely not OpenSSL).

use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{DriverError, Result};

/// Lifetime of a signed assertion (Snowflake allows up to an hour).
pub const JWT_LIFETIME_SECONDS: u64 = 3540;

/// How the driver authenticates against the SQL API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authentication {
    /// `SNOWFLAKE_JWT`: sign an assertion with the account's private key.
    KeyPair {
        /// PEM of the (unencrypted) private key.
        private_key: String,
    },
    /// `OAUTH`: a ready-made access token.
    OAuth { token: String },
}

impl Authentication {
    /// Value of the `X-Snowflake-Authorization-Token-Type` header.
    pub fn token_type(&self) -> &'static str {
        match self {
            Authentication::KeyPair { .. } => "KEYPAIR_JWT",
            Authentication::OAuth { .. } => "OAUTH",
        }
    }
}

/// Claims of a Snowflake key-pair assertion.
#[derive(Debug, Serialize)]
struct Claims {
    iss: String,
    sub: String,
    iat: u64,
    exp: u64,
}

/// Normalises an account identifier for the JWT subject: the account locator
/// without its region/cloud suffix, upper-cased.
pub fn normalise_account(account: &str) -> String {
    account.split('.').next().unwrap_or(account).to_uppercase()
}

/// `<ACCOUNT>.<USER>.SHA256:<fingerprint>` — the issuer Snowflake expects.
pub fn jwt_issuer(account: &str, user: &str, private_key_pem: &str) -> Result<String> {
    let fingerprint = public_key_fingerprint(private_key_pem)?;
    Ok(format!(
        "{}.{}.{fingerprint}",
        normalise_account(account),
        user.to_uppercase()
    ))
}

/// Signs a key-pair assertion valid from `now` (seconds since the epoch).
pub fn build_jwt(account: &str, user: &str, private_key_pem: &str, now: u64) -> Result<String> {
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).map_err(|e| {
        DriverError::Config(format!(
            "Invalid CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY: {e}. \
             Only unencrypted PKCS#1/PKCS#8 RSA keys are supported."
        ))
    })?;
    let claims = Claims {
        iss: jwt_issuer(account, user, private_key_pem)?,
        sub: format!("{}.{}", normalise_account(account), user.to_uppercase()),
        iat: now,
        exp: now + JWT_LIFETIME_SECONDS,
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &key,
    )
    .map_err(|e| DriverError::Config(format!("Unable to sign the Snowflake assertion: {e}")))
}

/// `SHA256:<base64>` of the DER `SubjectPublicKeyInfo` of the key, which is
/// what `snowsql` and the official connectors compute.
pub fn public_key_fingerprint(private_key_pem: &str) -> Result<String> {
    let spki = public_key_spki(private_key_pem)?;
    let digest = Sha256::digest(&spki);
    Ok(format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    ))
}

/// Derives the DER `SubjectPublicKeyInfo` of an RSA private key in PEM form.
pub fn public_key_spki(private_key_pem: &str) -> Result<Vec<u8>> {
    let (label, der) = decode_pem(private_key_pem)?;
    let pkcs1 = match label.as_str() {
        "RSA PRIVATE KEY" => der,
        "PRIVATE KEY" => pkcs8_private_key(&der)?,
        "ENCRYPTED PRIVATE KEY" => {
            return Err(DriverError::Config(
                "Encrypted Snowflake private keys are not supported yet: decrypt the key \
                 (openssl pkcs8 -in key.p8 -out key.pem) and set CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY \
                 to the result."
                    .to_string(),
            ))
        }
        other => {
            return Err(DriverError::Config(format!(
                "Unsupported private key format: \"{other}\"."
            )))
        }
    };

    // RSAPrivateKey ::= SEQUENCE { version, modulus, publicExponent, ... }
    let body = expect_tag(&pkcs1, 0x30, "RSAPrivateKey")?;
    let mut pos = 0;
    let _version = read_tlv(body, &mut pos)?;
    let modulus = read_tlv(body, &mut pos)?;
    let exponent = read_tlv(body, &mut pos)?;
    if modulus.0 != 0x02 || exponent.0 != 0x02 {
        return Err(DriverError::Config(
            "Unexpected RSA private key structure.".to_string(),
        ));
    }

    // RSAPublicKey ::= SEQUENCE { modulus INTEGER, publicExponent INTEGER }
    let mut public_key = Vec::new();
    public_key.extend(der_tlv(0x02, modulus.1));
    public_key.extend(der_tlv(0x02, exponent.1));
    let public_key = der_tlv(0x30, &public_key);

    // AlgorithmIdentifier ::= SEQUENCE { OID rsaEncryption, NULL }
    const RSA_OID: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    let mut algorithm = der_tlv(0x06, RSA_OID);
    algorithm.extend([0x05, 0x00]);
    let algorithm = der_tlv(0x30, &algorithm);

    // BIT STRING with no unused bits.
    let mut bit_string = vec![0x00];
    bit_string.extend(public_key);
    let bit_string = der_tlv(0x03, &bit_string);

    let mut spki = algorithm;
    spki.extend(bit_string);
    Ok(der_tlv(0x30, &spki))
}

/// PrivateKeyInfo ::= SEQUENCE { version, AlgorithmIdentifier, OCTET STRING }.
fn pkcs8_private_key(der: &[u8]) -> Result<Vec<u8>> {
    let body = expect_tag(der, 0x30, "PrivateKeyInfo")?;
    let mut pos = 0;
    let _version = read_tlv(body, &mut pos)?;
    let _algorithm = read_tlv(body, &mut pos)?;
    let (tag, content) = read_tlv(body, &mut pos)?;
    if tag != 0x04 {
        return Err(DriverError::Config(
            "Unexpected PKCS#8 private key structure.".to_string(),
        ));
    }
    Ok(content.to_vec())
}

/// Splits a PEM document into its label and DER bytes.
fn decode_pem(pem: &str) -> Result<(String, Vec<u8>)> {
    let mut label = None;
    let mut base64_body = String::new();
    for line in pem.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("-----BEGIN ") {
            label = rest.strip_suffix("-----").map(|l| l.to_string());
        } else if line.starts_with("-----END ") {
            break;
        } else if !line.is_empty() {
            base64_body.push_str(line);
        }
    }
    let label = label.ok_or_else(|| {
        DriverError::Config("The Snowflake private key is not in PEM format.".to_string())
    })?;
    let der = base64::engine::general_purpose::STANDARD
        .decode(base64_body)
        .map_err(|e| {
            DriverError::Config(format!("The Snowflake private key is not valid PEM: {e}"))
        })?;
    Ok((label, der))
}

/// Reads one DER TLV at `pos` and advances it.
fn read_tlv<'a>(input: &'a [u8], pos: &mut usize) -> Result<(u8, &'a [u8])> {
    let malformed =
        || DriverError::Config("Malformed DER data in the Snowflake private key.".to_string());
    let tag = *input.get(*pos).ok_or_else(malformed)?;
    let first = *input.get(*pos + 1).ok_or_else(malformed)? as usize;
    let (len, header) = if first < 0x80 {
        (first, 2)
    } else {
        let bytes = first & 0x7f;
        if bytes == 0 || bytes > 4 {
            return Err(malformed());
        }
        let mut len = 0usize;
        for i in 0..bytes {
            len = (len << 8) | *input.get(*pos + 2 + i).ok_or_else(malformed)? as usize;
        }
        (len, 2 + bytes)
    };
    let start = *pos + header;
    let end = start.checked_add(len).ok_or_else(malformed)?;
    if end > input.len() {
        return Err(malformed());
    }
    *pos = end;
    Ok((tag, &input[start..end]))
}

/// Reads a single TLV and checks its tag.
fn expect_tag<'a>(input: &'a [u8], tag: u8, what: &str) -> Result<&'a [u8]> {
    let mut pos = 0;
    let (actual, content) = read_tlv(input, &mut pos)?;
    if actual != tag {
        return Err(DriverError::Config(format!("Unexpected {what} structure.")));
    }
    Ok(content)
}

/// Encodes one DER TLV.
fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let start = bytes
            .iter()
            .position(|b| *b != 0)
            .unwrap_or(bytes.len() - 1);
        let significant = &bytes[start..];
        out.push(0x80 | significant.len() as u8);
        out.extend(significant);
    }
    out.extend(content);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    const TEST_KEY: &str = include_str!("../../test/fixtures/rsa_test_key.pem");

    #[test]
    fn fingerprint_matches_openssl() {
        // openssl rsa -in key.pem -pubout -outform DER | openssl dgst -sha256 -binary | base64
        assert_eq!(
            public_key_fingerprint(TEST_KEY).unwrap(),
            "SHA256:eOZw2sxeA2PYr0sYOEkjfwwIbzzNxoUQDRLscL8CXaY="
        );
    }

    #[test]
    fn issuer_and_claims() {
        let issuer = jwt_issuer("myorg-account1.eu-central-1.aws", "cube", TEST_KEY).unwrap();
        assert!(issuer.starts_with("MYORG-ACCOUNT1.CUBE.SHA256:"));

        let jwt = build_jwt("acc", "cube", TEST_KEY, 1_700_000_000).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(jwt.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["sub"], "ACC.CUBE");
        assert_eq!(claims["iat"], 1_700_000_000u64);
        assert_eq!(claims["exp"], 1_700_000_000u64 + JWT_LIFETIME_SECONDS);
        assert!(claims["iss"].as_str().unwrap().contains(".SHA256:"));
    }

    #[test]
    fn encrypted_keys_are_rejected_clearly() {
        let pem =
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----";
        let err = public_key_spki(pem).unwrap_err();
        assert!(err.to_string().contains("Encrypted Snowflake private keys"));

        let err = public_key_spki("nonsense").unwrap_err();
        assert!(err.to_string().contains("not in PEM format"));
    }

    #[test]
    fn der_length_encoding() {
        assert_eq!(der_tlv(0x04, &[1, 2, 3]), vec![0x04, 0x03, 1, 2, 3]);
        let long = vec![7u8; 300];
        let encoded = der_tlv(0x04, &long);
        assert_eq!(&encoded[..4], &[0x04, 0x82, 0x01, 0x2c]);
        assert_eq!(encoded.len(), 304);
    }

    #[test]
    fn token_types() {
        assert_eq!(
            Authentication::KeyPair {
                private_key: String::new()
            }
            .token_type(),
            "KEYPAIR_JWT"
        );
        assert_eq!(
            Authentication::OAuth {
                token: String::new()
            }
            .token_type(),
            "OAUTH"
        );
    }
}

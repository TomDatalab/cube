//! Google service-account authentication for the BigQuery REST API.
//!
//! The Node driver hands the credentials to `@google-cloud/bigquery`, which
//! runs the standard two-legged OAuth flow: sign a JWT with the service
//! account's RSA key and exchange it for an access token. That flow is short
//! enough to implement directly (`jsonwebtoken` + `reqwest`), which keeps the
//! Google SDK — and its transitive C dependencies — out of the build.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::{DriverError, Result};

/// OAuth scopes requested by the Node driver.
pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/bigquery",
    "https://www.googleapis.com/auth/drive",
];

/// Default token endpoint when the key file does not name one.
pub const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// A Google service-account key (the JSON of `CUBEJS_DB_BQ_KEY_FILE` /
/// `CUBEJS_DB_BQ_CREDENTIALS`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ServiceAccount {
    #[serde(default)]
    pub project_id: Option<String>,
    pub client_email: String,
    pub private_key: String,
    #[serde(default)]
    pub private_key_id: Option<String>,
    #[serde(default)]
    pub token_uri: Option<String>,
}

impl ServiceAccount {
    /// Parses the JSON of a service-account key file.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| {
            DriverError::Config(format!("Invalid BigQuery service account credentials: {e}"))
        })
    }

    /// Parses the base64 encoded JSON of `CUBEJS_DB_BQ_CREDENTIALS`.
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|e| {
                DriverError::Config(format!(
                    "CUBEJS_DB_BQ_CREDENTIALS is not valid base64 data: {e}"
                ))
            })?;
        let json = String::from_utf8(decoded).map_err(|e| {
            DriverError::Config(format!("CUBEJS_DB_BQ_CREDENTIALS is not valid UTF-8: {e}"))
        })?;
        Self::from_json(&json)
    }

    /// Reads a service-account key file from disk.
    pub fn from_key_file(path: &str) -> Result<Self> {
        let json = std::fs::read_to_string(path).map_err(|e| {
            DriverError::Config(format!(
                "Unable to read the BigQuery key file \"{path}\": {e}"
            ))
        })?;
        Self::from_json(&json)
    }

    /// Endpoint the signed assertion is exchanged at.
    pub fn token_uri(&self) -> &str {
        self.token_uri.as_deref().unwrap_or(DEFAULT_TOKEN_URI)
    }
}

/// Claims of the self-signed assertion (`urn:ietf:params:oauth:grant-type:jwt-bearer`).
#[derive(Debug, Serialize)]
struct Claims<'a> {
    iss: &'a str,
    scope: String,
    aud: &'a str,
    exp: u64,
    iat: u64,
}

/// The `/token` response.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// Cached access token.
#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    /// Seconds since the Unix epoch at which the token must be refreshed.
    expires_at: u64,
}

/// Signs assertions for `account` and caches the resulting access token.
#[derive(Debug)]
pub struct TokenProvider {
    account: ServiceAccount,
    client: reqwest::Client,
    cached: Mutex<Option<CachedToken>>,
}

impl TokenProvider {
    pub fn new(account: ServiceAccount, client: reqwest::Client) -> Self {
        Self {
            account,
            client,
            cached: Mutex::new(None),
        }
    }

    /// The service account this provider signs for.
    pub fn account(&self) -> &ServiceAccount {
        &self.account
    }

    /// A valid access token, refreshed when the cached one is about to expire.
    pub async fn access_token(&self) -> Result<String> {
        let now = unix_time();
        let mut cached = self.cached.lock().await;
        if let Some(token) = cached.as_ref() {
            if token.expires_at > now {
                return Ok(token.token.clone());
            }
        }

        let assertion = self.build_assertion(now)?;
        let response = self
            .client
            .post(self.account.token_uri())
            .form(&[
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string(),
                ),
                ("assertion", assertion),
            ])
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: "bigquery".to_string(),
                message: format!("Unable to request a Google access token: {e}"),
            })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(DriverError::Config(format!(
                "Unable to authenticate the BigQuery service account ({status}): {}",
                body.trim()
            )));
        }

        let token: TokenResponse = serde_json::from_str(&body).map_err(|e| {
            DriverError::Config(format!("Unexpected Google token response: {e}: {body}"))
        })?;
        // Refresh a minute early so an in-flight request never races the expiry.
        let expires_in = token.expires_in.unwrap_or(3600).saturating_sub(60);
        *cached = Some(CachedToken {
            token: token.access_token.clone(),
            expires_at: now + expires_in,
        });
        Ok(token.access_token)
    }

    /// Builds the RS256 assertion for `now` (seconds since the epoch).
    fn build_assertion(&self, now: u64) -> Result<String> {
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(self.account.private_key.as_bytes())
            .map_err(|e| {
                DriverError::Config(format!(
                    "Invalid BigQuery service account private key: {e}. \
                     Only unencrypted PKCS#1/PKCS#8 RSA keys are supported."
                ))
            })?;
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = self.account.private_key_id.clone();
        let claims = Claims {
            iss: &self.account.client_email,
            scope: SCOPES.join(" "),
            aud: self.account.token_uri(),
            exp: now + 3600,
            iat: now,
        };
        jsonwebtoken::encode(&header, &claims, &key)
            .map_err(|e| DriverError::Config(format!("Unable to sign the Google assertion: {e}")))
    }
}

/// Seconds since the Unix epoch.
pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// A throwaway 2048-bit RSA key, generated for the unit tests only.
    pub const TEST_KEY: &str = include_str!("../../test/fixtures/rsa_test_key.pem");

    fn account() -> ServiceAccount {
        ServiceAccount {
            project_id: Some("my-project".into()),
            client_email: "cube@my-project.iam.gserviceaccount.com".into(),
            private_key: TEST_KEY.to_string(),
            private_key_id: Some("kid-1".into()),
            token_uri: None,
        }
    }

    #[test]
    fn parses_base64_credentials() {
        let json = serde_json::to_string(&account()).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        let parsed = ServiceAccount::from_base64(&encoded).unwrap();
        assert_eq!(
            parsed.client_email,
            "cube@my-project.iam.gserviceaccount.com"
        );
        assert_eq!(parsed.project_id.as_deref(), Some("my-project"));
        assert_eq!(parsed.token_uri(), DEFAULT_TOKEN_URI);

        let err = ServiceAccount::from_base64("not base64!!").unwrap_err();
        assert!(err.to_string().contains("not valid base64"));
    }

    #[test]
    fn signs_an_assertion() {
        let provider = TokenProvider::new(account(), reqwest::Client::new());
        let jwt = provider.build_assertion(1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[0])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(header["alg"], "RS256");
        assert_eq!(header["kid"], "kid-1");
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[1])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["iss"], "cube@my-project.iam.gserviceaccount.com");
        assert_eq!(claims["aud"], DEFAULT_TOKEN_URI);
        assert_eq!(
            claims["scope"],
            "https://www.googleapis.com/auth/bigquery https://www.googleapis.com/auth/drive"
        );
        assert_eq!(claims["iat"], 1_700_000_000u64);
        assert_eq!(claims["exp"], 1_700_003_600u64);
    }

    #[test]
    fn rejects_an_invalid_private_key() {
        let mut account = account();
        account.private_key = "-----BEGIN PRIVATE KEY-----\nnope\n-----END PRIVATE KEY-----".into();
        let provider = TokenProvider::new(account, reqwest::Client::new());
        let err = provider.build_assertion(0).unwrap_err();
        assert!(err.to_string().contains("private key"));
    }
}

//! JWK set fetching and caching (port of `jwk.ts`).
//!
//! The Node.js implementation memoizes the JWK set per URL, expires it after
//! `Cache-Control: max-age` (or `jwkDefaultExpire`, or 5 minutes), refreshes
//! expired entries in the background (keeping the stale value if the refresh
//! fails) and force-refetches when an unknown `kid` shows up more than
//! `jwkRefetchWindow` after the last fetch (key rotation).
//!
//! This port refreshes expired entries lazily on access instead of on a
//! background timer, which keeps the crate runtime-agnostic; the observable
//! behaviour (stale value kept on failure, rotation window) is the same.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk};
use jsonwebtoken::DecodingKey;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::{AuthConfig, DEFAULT_JWK_EXPIRE};
use crate::error::JwkError;

/// Key family a verification key belongs to; used to pick the algorithms
/// the key can be used with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFamily {
    Hmac,
    Rsa,
    Ec,
    Ed,
}

/// A verification key together with its family.
#[derive(Clone)]
pub struct VerificationKey {
    pub family: KeyFamily,
    pub key: DecodingKey,
}

impl std::fmt::Debug for VerificationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerificationKey")
            .field("family", &self.family)
            .finish_non_exhaustive()
    }
}

impl VerificationKey {
    pub fn from_secret(secret: &[u8]) -> Self {
        Self {
            family: KeyFamily::Hmac,
            key: DecodingKey::from_secret(secret),
        }
    }

    /// Converts a JWK into a decoding key (`jwk-to-pem` in Node.js).
    pub fn from_jwk(jwk: &Jwk) -> Result<Self, jsonwebtoken::errors::Error> {
        let family = match &jwk.algorithm {
            AlgorithmParameters::RSA(_) => KeyFamily::Rsa,
            AlgorithmParameters::EllipticCurve(_) => KeyFamily::Ec,
            AlgorithmParameters::OctetKeyPair(_) => KeyFamily::Ed,
            AlgorithmParameters::OctetKey(_) => KeyFamily::Hmac,
        };
        Ok(Self {
            family,
            key: DecodingKey::from_jwk(jwk)?,
        })
    }

    /// Parses a PEM encoded public key (RSA, EC or Ed25519).
    pub fn from_pem(pem: &[u8]) -> Result<Self, jsonwebtoken::errors::Error> {
        DecodingKey::from_rsa_pem(pem)
            .map(|key| Self {
                family: KeyFamily::Rsa,
                key,
            })
            .or_else(|_| {
                DecodingKey::from_ec_pem(pem).map(|key| Self {
                    family: KeyFamily::Ec,
                    key,
                })
            })
            .or_else(|_| {
                DecodingKey::from_ed_pem(pem).map(|key| Self {
                    family: KeyFamily::Ed,
                    key,
                })
            })
    }
}

/// Raw JWK set response.
#[derive(Debug, Clone)]
pub struct JwkResponse {
    /// Parsed JSON body (expected to be `{ "keys": [...] }`).
    pub body: Value,
    /// Value of the `Cache-Control` response header, if any.
    pub cache_control: Option<String>,
}

/// Fetches a JWK set document. Injected into [`JwkCache`] so that tests (and
/// alternative transports) do not need real HTTP.
#[async_trait]
pub trait JwkFetcher: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<JwkResponse, JwkError>;
}

/// [`JwkFetcher`] backed by `reqwest` (rustls). Retries `retry` times like
/// `asyncRetry` in Node.js.
pub struct ReqwestJwkFetcher {
    client: reqwest::Client,
    retry: u32,
}

impl ReqwestJwkFetcher {
    pub fn new(retry: u32) -> Self {
        Self {
            client: reqwest::Client::new(),
            retry: retry.max(1),
        }
    }

    async fn fetch_once(&self, url: &str) -> Result<JwkResponse, JwkError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| JwkError::Fetch(e.to_string()))?;
        let cache_control = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        let body: Value = response
            .json()
            .await
            .map_err(|e| JwkError::Fetch(e.to_string()))?;
        Ok(JwkResponse {
            body,
            cache_control,
        })
    }
}

#[async_trait]
impl JwkFetcher for ReqwestJwkFetcher {
    async fn fetch(&self, url: &str) -> Result<JwkResponse, JwkError> {
        let mut last_error = None;
        for _ in 0..self.retry {
            match self.fetch_once(url).await {
                Ok(response) => return Ok(response),
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error.unwrap_or_else(|| JwkError::Fetch("no attempts made".to_string())))
    }
}

/// `max-age` from a `Cache-Control` header (`parseCacheControl` in jwk.ts).
pub fn parse_cache_control_max_age(header: &str) -> Option<u64> {
    header.split(',').find_map(|directive| {
        let mut parts = directive.trim().splitn(2, '=');
        let key = parts.next()?.trim();
        if !key.eq_ignore_ascii_case("max-age") {
            return None;
        }
        let value = parts.next()?.trim().trim_matches('"');
        // parseInt semantics: leading digits only
        let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    })
}

struct CacheEntry {
    keys: Arc<HashMap<String, VerificationKey>>,
    done_at: Instant,
    expires_at: Instant,
}

/// Per-URL cache of JWK sets keyed by `kid`.
pub struct JwkCache {
    fetcher: Arc<dyn JwkFetcher>,
    default_expire: Duration,
    refetch_window: Duration,
    entries: Mutex<HashMap<String, CacheEntry>>,
}

impl JwkCache {
    pub fn new(config: &AuthConfig, fetcher: Arc<dyn JwkFetcher>) -> Self {
        Self {
            fetcher,
            default_expire: config.jwk_default_expire.unwrap_or(DEFAULT_JWK_EXPIRE),
            refetch_window: config.jwk_refetch_window,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn parse_response(&self, response: JwkResponse) -> Result<CacheEntry, JwkError> {
        let keys = response
            .body
            .get("keys")
            .and_then(Value::as_array)
            .ok_or(JwkError::NoKeys)?;

        let mut result = HashMap::with_capacity(keys.len());
        for jwk in keys {
            let kid = jwk
                .get("kid")
                .and_then(Value::as_str)
                .filter(|kid| !kid.is_empty())
                .ok_or(JwkError::NoKid)?
                .to_string();
            let parsed: Jwk =
                serde_json::from_value(jwk.clone()).map_err(|e| JwkError::InvalidJwk {
                    kid: kid.clone(),
                    reason: e.to_string(),
                })?;
            let key = VerificationKey::from_jwk(&parsed).map_err(|e| JwkError::InvalidJwk {
                kid: kid.clone(),
                reason: e.to_string(),
            })?;
            result.insert(kid, key);
        }

        let lifetime = response
            .cache_control
            .as_deref()
            .and_then(parse_cache_control_max_age)
            .filter(|max_age| *max_age > 0)
            .map(Duration::from_secs)
            .unwrap_or(self.default_expire);

        let now = Instant::now();
        Ok(CacheEntry {
            keys: Arc::new(result),
            done_at: now,
            expires_at: now + lifetime,
        })
    }

    async fn fetch_entry(&self, url: &str) -> Result<CacheEntry, JwkError> {
        let response = self.fetcher.fetch(url).await?;
        self.parse_response(response)
    }

    /// Fetches (and caches) the JWK set for `url`, refreshing it when the
    /// cached copy expired. A failed refresh keeps the stale copy
    /// (`onBackgroundException` in Node.js).
    pub async fn fetch(&self, url: &str) -> Result<(), JwkError> {
        let mut entries = self.entries.lock().await;
        match entries.get(url) {
            Some(entry) if entry.expires_at > Instant::now() => Ok(()),
            Some(_) => {
                match self.fetch_entry(url).await {
                    Ok(entry) => {
                        entries.insert(url.to_string(), entry);
                    }
                    Err(e) => log::warn!("JWKs Background Fetching Error: {e}"),
                }
                Ok(())
            }
            None => {
                let entry = self.fetch_entry(url).await?;
                entries.insert(url.to_string(), entry);
                Ok(())
            }
        }
    }

    /// Forces a refetch of `url`.
    pub async fn force_fetch(&self, url: &str) -> Result<(), JwkError> {
        let entry = self.fetch_entry(url).await?;
        self.entries.lock().await.insert(url.to_string(), entry);
        Ok(())
    }

    /// `getJWKbyKid`: looks `kid` up in the cached set for `url`, force
    /// refetching once when the set is older than the refetch window (key
    /// rotation). Returns `None` when the kid is unknown.
    pub async fn get_key_by_kid(
        &self,
        url: &str,
        kid: &str,
    ) -> Result<Option<VerificationKey>, JwkError> {
        self.fetch(url).await?;

        let (keys, done_at) = {
            let entries = self.entries.lock().await;
            let entry = entries.get(url).ok_or(JwkError::NoKeys)?;
            (Arc::clone(&entry.keys), entry.done_at)
        };

        if let Some(key) = keys.get(kid) {
            return Ok(Some(key.clone()));
        }

        // The kid is unknown: it may be a rotated key, or a bogus token.
        // Only refetch when the cached set is older than the refetch window.
        if done_at.elapsed() > self.refetch_window {
            self.force_fetch(url).await?;
            let entries = self.entries.lock().await;
            if let Some(key) = entries.get(url).and_then(|entry| entry.keys.get(kid)) {
                return Ok(Some(key.clone()));
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_control_max_age() {
        assert_eq!(parse_cache_control_max_age("max-age=300"), Some(300));
        assert_eq!(
            parse_cache_control_max_age("public, max-age=3600, must-revalidate"),
            Some(3600)
        );
        assert_eq!(parse_cache_control_max_age("Max-Age=\"42\""), Some(42));
        assert_eq!(parse_cache_control_max_age("no-cache"), None);
        assert_eq!(parse_cache_control_max_age("max-age"), None);
        assert_eq!(parse_cache_control_max_age("max-age=abc"), None);
    }
}

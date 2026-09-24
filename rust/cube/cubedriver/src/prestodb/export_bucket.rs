//! Export bucket listing and URL signing for the Presto / Trino unload
//! (`extractUnloadedFilesFromS3` / `extractFilesFromGCS` of `BaseDriver`).
//!
//! The Node driver uses the AWS and Google SDKs. Both operations are small —
//! list the objects under a prefix, then sign one-hour `GET` URLs — so they
//! are implemented here on `reqwest` with AWS Signature V4 (HMAC-SHA256 over
//! `sha2`) and Google V4 signing (RSA-SHA256 through `jsonwebtoken`'s `ring`
//! backend), which keeps the SDKs out of the build.
//!
//! Credentials: static keys (`CUBEJS_DB_EXPORT_BUCKET_AWS_KEY` / `_SECRET`,
//! falling back to `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` /
//! `AWS_SESSION_TOKEN`) and service-account JSON
//! (`CUBEJS_DB_EXPORT_GCS_CREDENTIALS`, falling back to a service-account
//! file named by `GOOGLE_APPLICATION_CREDENTIALS`). The SDKs' other
//! providers (IRSA web identity, instance profiles, GCE metadata, workload
//! identity federation) are refused with [`DriverError::NotImplemented`].

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bigquery::auth::ServiceAccount;
use crate::error::{DriverError, Result};

/// Validity of the signed URLs (one hour, as in Node).
pub const SIGNED_URL_EXPIRES_SECS: u64 = 3600;
/// Scope of the GCS access token used for listing.
pub const GCS_READ_SCOPE: &str = "https://www.googleapis.com/auth/devstorage.read_only";
/// SHA-256 of the empty payload.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// HMAC-SHA256 (RFC 2104).
pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(data);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

/// RFC 3986 percent-encoding of everything but the unreserved characters
/// (and `/` when `keep_slash`), as both AWS and Google canonical forms need.
pub(crate) fn uri_encode(input: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn canonical_query(params: &[(String, String)]) -> String {
    let mut encoded: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    encoded.sort();
    encoded
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

// ---------------------------------------------------------------------------
// S3
// ---------------------------------------------------------------------------

/// Static AWS credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

/// Everything needed to talk to one S3 bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Location {
    pub bucket: String,
    pub region: String,
    pub credentials: AwsCredentials,
    /// Custom endpoint (`AWS_ENDPOINT_URL_S3` / `AWS_ENDPOINT_URL`, MinIO, …);
    /// requests are then path-style.
    pub endpoint: Option<String>,
}

impl S3Location {
    /// Resolves the S3 settings like `normalizeS3ClientConfig` + the SDK
    /// defaults: empty static keys and a blank region fall back to the
    /// standard `AWS_*` variables.
    pub fn resolve(
        bucket: &str,
        access_key_id: Option<&str>,
        secret_access_key: Option<&str>,
        region: Option<&str>,
        endpoint: Option<&str>,
    ) -> Result<Self> {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let credentials = match (
            access_key_id.filter(|v| !v.is_empty()),
            secret_access_key.filter(|v| !v.is_empty()),
        ) {
            (Some(key), Some(secret)) => AwsCredentials {
                access_key_id: key.to_string(),
                secret_access_key: secret.to_string(),
                session_token: None,
            },
            _ => match (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY")) {
                (Some(key), Some(secret)) => AwsCredentials {
                    access_key_id: key,
                    secret_access_key: secret,
                    session_token: env("AWS_SESSION_TOKEN"),
                },
                _ => {
                    return Err(DriverError::NotImplemented(
                        "S3 export bucket: no static credentials. Set CUBEJS_DB_EXPORT_BUCKET_AWS_KEY and \
                         CUBEJS_DB_EXPORT_BUCKET_AWS_SECRET (or AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY); \
                         the AWS default credential chain (web identity / IRSA, instance profiles, SSO) \
                         is not supported by the Rust driver."
                            .to_string(),
                    ))
                }
            },
        };
        let region = region
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .or_else(|| env("AWS_REGION"))
            .or_else(|| env("AWS_DEFAULT_REGION"))
            .ok_or_else(|| {
                DriverError::Config(
                    "S3 export bucket: the region is unknown. Set CUBEJS_DB_EXPORT_BUCKET_AWS_REGION \
                     (or AWS_REGION)."
                        .to_string(),
                )
            })?;
        let endpoint = endpoint
            .map(str::to_string)
            .or_else(|| env("AWS_ENDPOINT_URL_S3"))
            .or_else(|| env("AWS_ENDPOINT_URL"))
            .map(|e| e.trim_end_matches('/').to_string());
        Ok(Self {
            // "different driver configurations use different formats for the
            // bucket - some expect only names, some - full url-like names"
            bucket: strip_scheme(bucket).trim_end_matches('/').to_string(),
            region,
            credentials,
            endpoint,
        })
    }

    /// `(scheme://host[:port], path prefix)` of the bucket.
    fn base(&self) -> (String, String) {
        match &self.endpoint {
            Some(endpoint) => (endpoint.clone(), format!("/{}", self.bucket)),
            None if self.bucket.contains('.') => (
                format!("https://s3.{}.amazonaws.com", self.region),
                format!("/{}", self.bucket),
            ),
            None => (
                format!("https://{}.s3.{}.amazonaws.com", self.bucket, self.region),
                String::new(),
            ),
        }
    }

    fn host(origin: &str) -> String {
        origin
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(origin)
            .to_string()
    }

    fn scope(&self, date: &str) -> String {
        format!("{date}/{}/s3/aws4_request", self.region)
    }

    fn signature(&self, now: DateTime<Utc>, canonical_request: &str) -> String {
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{}\n{}",
            self.scope(&date),
            sha256_hex(canonical_request.as_bytes())
        );
        let k_date = hmac_sha256(
            format!("AWS4{}", self.credentials.secret_access_key).as_bytes(),
            date.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, b"s3");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()))
    }

    /// A presigned `GET` URL of `key` valid for `expires` seconds
    /// (`getSignedUrl(GetObjectCommand, { expiresIn: 3600 })`).
    pub fn presign_get(&self, key: &str, now: DateTime<Utc>, expires: u64) -> String {
        let (origin, prefix) = self.base();
        let path = format!("{prefix}/{}", uri_encode(key, true));
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let mut params = vec![
            (
                "X-Amz-Algorithm".to_string(),
                "AWS4-HMAC-SHA256".to_string(),
            ),
            (
                "X-Amz-Credential".to_string(),
                format!("{}/{}", self.credentials.access_key_id, self.scope(&date)),
            ),
            ("X-Amz-Date".to_string(), amz_date),
            ("X-Amz-Expires".to_string(), expires.to_string()),
            ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
        ];
        if let Some(token) = &self.credentials.session_token {
            params.push(("X-Amz-Security-Token".to_string(), token.clone()));
        }
        let query = canonical_query(&params);
        let canonical_request = format!(
            "GET\n{path}\n{query}\nhost:{}\n\nhost\nUNSIGNED-PAYLOAD",
            Self::host(&origin)
        );
        let signature = self.signature(now, &canonical_request);
        format!("{origin}{path}?{query}&X-Amz-Signature={signature}")
    }

    /// A signed `ListObjectsV2` request.
    fn list_request(
        &self,
        http: &reqwest::Client,
        prefix: &str,
        continuation: Option<&str>,
        now: DateTime<Utc>,
    ) -> reqwest::RequestBuilder {
        let (origin, bucket_path) = self.base();
        let path = if bucket_path.is_empty() {
            "/".to_string()
        } else {
            bucket_path
        };
        let mut params = vec![
            ("list-type".to_string(), "2".to_string()),
            ("prefix".to_string(), prefix.to_string()),
        ];
        if let Some(token) = continuation {
            params.push(("continuation-token".to_string(), token.to_string()));
        }
        let query = canonical_query(&params);
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let host = Self::host(&origin);

        let mut headers = vec![
            ("host".to_string(), host),
            ("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string()),
            ("x-amz-date".to_string(), amz_date),
        ];
        if let Some(token) = &self.credentials.session_token {
            headers.push(("x-amz-security-token".to_string(), token.clone()));
        }
        headers.sort();
        let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
        let signed_headers = headers
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_request =
            format!("GET\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{EMPTY_SHA256}");
        let signature = self.signature(now, &canonical_request);
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={signed_headers}, Signature={signature}",
            self.credentials.access_key_id,
            self.scope(&date)
        );

        let mut request = http
            .get(format!("{origin}{path}?{query}"))
            .header("Authorization", authorization);
        for (k, v) in headers {
            if k != "host" {
                request = request.header(k, v);
            }
        }
        request
    }

    /// Lists every key under `prefix` (following continuation tokens).
    pub async fn list(&self, http: &reqwest::Client, prefix: &str) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let response = self
                .list_request(http, prefix, continuation.as_deref(), Utc::now())
                .send()
                .await
                .map_err(|e| {
                    DriverError::Query(format!("Unable to list the S3 export bucket: {e}"))
                })?;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(DriverError::Query(format!(
                    "Unable to retrieve list of files from S3 storage after unloading ({status}): {}",
                    xml_tag(&body, "Message").unwrap_or_else(|| body.trim().to_string())
                )));
            }
            keys.extend(parse_list_keys(&body));
            let truncated = xml_tag(&body, "IsTruncated").is_some_and(|v| v == "true");
            continuation = xml_tag(&body, "NextContinuationToken");
            if !truncated || continuation.is_none() {
                return Ok(keys);
            }
        }
    }
}

/// `bucketName.replace(/^[a-zA-Z]+:\/\//, '')`.
pub fn strip_scheme(bucket: &str) -> &str {
    if let Some(pos) = bucket.find("://") {
        if pos > 0 && bucket[..pos].chars().all(|c| c.is_ascii_alphabetic()) {
            return &bucket[pos + 3..];
        }
    }
    bucket
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn xml_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(xml_unescape(&body[start..end]))
}

/// `<Contents><Key>…</Key>…</Contents>` of a `ListObjectsV2` response.
pub fn parse_list_keys(body: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<Contents>") {
        let after = &rest[start + "<Contents>".len()..];
        let end = after.find("</Contents>").unwrap_or(after.len());
        if let Some(key) = xml_tag(&after[..end], "Key") {
            keys.push(key);
        }
        rest = &after[end..];
    }
    keys
}

// ---------------------------------------------------------------------------
// GCS
// ---------------------------------------------------------------------------

/// Resolves the GCS service account: `CUBEJS_DB_EXPORT_GCS_CREDENTIALS`
/// (already decoded JSON), else `GOOGLE_APPLICATION_CREDENTIALS`.
pub fn resolve_gcs_account(credentials: Option<&str>) -> Result<ServiceAccount> {
    // `hasGCSCredentials`: an empty value or `{}` counts as absent.
    let explicit = credentials
        .map(str::trim)
        .filter(|c| !c.is_empty() && *c != "{}");
    let json = match explicit {
        Some(json) => json.to_string(),
        None => match std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .filter(|p| !p.is_empty())
        {
            Some(path) => std::fs::read_to_string(&path).map_err(|e| {
                DriverError::Config(format!(
                    "Unable to read GOOGLE_APPLICATION_CREDENTIALS \"{path}\": {e}"
                ))
            })?,
            None => {
                return Err(DriverError::NotImplemented(
                    "GCS export bucket: no credentials. Set CUBEJS_DB_EXPORT_GCS_CREDENTIALS \
                     (base64 service-account JSON) or GOOGLE_APPLICATION_CREDENTIALS; Application \
                     Default Credentials from the metadata server are not supported by the Rust driver."
                        .to_string(),
                ))
            }
        },
    };
    #[derive(Deserialize)]
    struct Kind {
        #[serde(rename = "type", default)]
        type_: Option<String>,
    }
    if let Ok(Kind { type_: Some(kind) }) = serde_json::from_str::<Kind>(&json) {
        if kind != "service_account" {
            return Err(DriverError::NotImplemented(format!(
                "GCS export bucket: \"{kind}\" credentials are not supported by the Rust driver; \
                 use a service-account key"
            )));
        }
    }
    serde_json::from_str(&json)
        .map_err(|e| DriverError::Config(format!("Invalid GCS export bucket credentials: {e}")))
}

fn rsa_key(account: &ServiceAccount) -> Result<jsonwebtoken::EncodingKey> {
    jsonwebtoken::EncodingKey::from_rsa_pem(account.private_key.as_bytes()).map_err(|e| {
        DriverError::Config(format!(
            "Invalid GCS service account private key: {e}. \
             Only unencrypted PKCS#1/PKCS#8 RSA keys are supported."
        ))
    })
}

/// RSA-SHA256 signature of `message`, hex encoded.
fn rsa_sha256_hex(account: &ServiceAccount, message: &[u8]) -> Result<String> {
    let key = rsa_key(account)?;
    let signature = jsonwebtoken::crypto::sign(message, &key, jsonwebtoken::Algorithm::RS256)
        .map_err(|e| DriverError::Config(format!("Unable to sign the GCS URL: {e}")))?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|e| DriverError::Config(format!("Unable to sign the GCS URL: {e}")))?;
    Ok(hex(&raw))
}

/// A V4 signed `GET` URL of `object` in `bucket`
/// (`file.getSignedUrl({ action: 'read', expires: now + 1h })`).
pub fn gcs_signed_url(
    account: &ServiceAccount,
    bucket: &str,
    object: &str,
    now: DateTime<Utc>,
    expires: u64,
) -> Result<String> {
    let host = "storage.googleapis.com";
    let path = format!(
        "/{}/{}",
        uri_encode(bucket, false),
        uri_encode(object, true)
    );
    let datetime = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let scope = format!("{date}/auto/storage/goog4_request");
    let params = vec![
        (
            "X-Goog-Algorithm".to_string(),
            "GOOG4-RSA-SHA256".to_string(),
        ),
        (
            "X-Goog-Credential".to_string(),
            format!("{}/{scope}", account.client_email),
        ),
        ("X-Goog-Date".to_string(), datetime.clone()),
        ("X-Goog-Expires".to_string(), expires.to_string()),
        ("X-Goog-SignedHeaders".to_string(), "host".to_string()),
    ];
    let query = canonical_query(&params);
    let canonical_request = format!("GET\n{path}\n{query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD");
    let string_to_sign = format!(
        "GOOG4-RSA-SHA256\n{datetime}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let signature = rsa_sha256_hex(account, string_to_sign.as_bytes())?;
    Ok(format!(
        "https://{host}{path}?{query}&X-Goog-Signature={signature}"
    ))
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    exp: u64,
    iat: u64,
}

/// Exchanges a self-signed assertion for a `devstorage.read_only` token.
async fn gcs_access_token(http: &reqwest::Client, account: &ServiceAccount) -> Result<String> {
    let now = crate::bigquery::auth::unix_time();
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = account.private_key_id.clone();
    let claims = Claims {
        iss: &account.client_email,
        scope: GCS_READ_SCOPE,
        aud: account.token_uri(),
        exp: now + 3600,
        iat: now,
    };
    let assertion = jsonwebtoken::encode(&header, &claims, &rsa_key(account)?)
        .map_err(|e| DriverError::Config(format!("Unable to sign the Google assertion: {e}")))?;
    let response = http
        .post(account.token_uri())
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ])
        .send()
        .await
        .map_err(|e| DriverError::Query(format!("Unable to request a Google access token: {e}")))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(DriverError::Config(format!(
            "Unable to authenticate the GCS service account ({status}): {}",
            body.trim()
        )));
    }
    #[derive(Deserialize)]
    struct Token {
        access_token: String,
    }
    let token: Token = serde_json::from_str(&body)
        .map_err(|e| DriverError::Config(format!("Unexpected Google token response: {e}")))?;
    Ok(token.access_token)
}

/// Lists the objects under `prefix` and signs a URL for each
/// (`extractFilesFromGCS`).
pub async fn extract_files_from_gcs(
    http: &reqwest::Client,
    account: &ServiceAccount,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Listing {
        #[serde(default)]
        items: Vec<Item>,
        #[serde(default)]
        next_page_token: Option<String>,
    }
    #[derive(Deserialize)]
    struct Item {
        name: String,
    }

    let token = gcs_access_token(http, account).await?;
    let mut names = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut request = http
            .get(format!(
                "https://storage.googleapis.com/storage/v1/b/{}/o",
                uri_encode(bucket, false)
            ))
            .bearer_auth(&token)
            .query(&[("prefix", prefix), ("fields", "items(name),nextPageToken")]);
        if let Some(t) = &page_token {
            request = request.query(&[("pageToken", t.as_str())]);
        }
        let response = request.send().await.map_err(|e| {
            DriverError::Query(format!("Unable to list the GCS export bucket: {e}"))
        })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(DriverError::Query(format!(
                "Unable to list the GCS export bucket ({status}): {}",
                body.trim()
            )));
        }
        let listing: Listing = serde_json::from_str(&body)
            .map_err(|e| DriverError::Query(format!("Unexpected GCS listing: {e}")))?;
        names.extend(listing.items.into_iter().map(|i| i.name));
        match listing.next_page_token {
            Some(t) => page_token = Some(t),
            None => break,
        }
    }

    if names.is_empty() {
        return Err(DriverError::Query(
            "No CSV files were obtained from the bucket".to_string(),
        ));
    }
    let now = Utc::now();
    names
        .iter()
        .map(|name| gcs_signed_url(account, bucket, name, now, SIGNED_URL_EXPIRES_SECS))
        .collect()
}

/// Lists the objects under `prefix` and presigns a URL for each
/// (`extractUnloadedFilesFromS3`).
pub async fn extract_unloaded_files_from_s3(
    http: &reqwest::Client,
    location: &S3Location,
    prefix: &str,
) -> Result<Vec<String>> {
    let keys = location.list(http, prefix).await?;
    let now = Utc::now();
    Ok(keys
        .iter()
        .map(|key| location.presign_get(key, now, SIGNED_URL_EXPIRES_SECS))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn hmac_matches_rfc4231() {
        // RFC 4231, test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block size.
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn presigned_url_matches_the_aws_example() {
        // "Example: A presigned URL" of the AWS SigV4 query-string documentation.
        let location = S3Location {
            bucket: "examplebucket".into(),
            region: "us-east-1".into(),
            credentials: AwsCredentials {
                access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
                secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
                session_token: None,
            },
            endpoint: Some("https://examplebucket.s3.amazonaws.com".into()),
        };
        // The documented example is virtual-hosted; emulate it with an
        // endpoint and no bucket path.
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let canonical = format!(
            "GET\n/test.txt\n{}\nhost:examplebucket.s3.amazonaws.com\n\nhost\nUNSIGNED-PAYLOAD",
            canonical_query(&[
                ("X-Amz-Algorithm".into(), "AWS4-HMAC-SHA256".into()),
                (
                    "X-Amz-Credential".into(),
                    "AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request".into()
                ),
                ("X-Amz-Date".into(), "20130524T000000Z".into()),
                ("X-Amz-Expires".into(), "86400".into()),
                ("X-Amz-SignedHeaders".into(), "host".into()),
            ])
        );
        assert_eq!(
            location.signature(now, &canonical),
            "aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );

        // And `presign_get` builds the same canonical request (path-style
        // for a custom endpoint).
        let url = location.presign_get("dir/a b.csv", now, 3600);
        assert!(url.starts_with(
            "https://examplebucket.s3.amazonaws.com/examplebucket/dir/a%20b.csv?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20130524T000000Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host&X-Amz-Signature="
        ));
    }

    #[test]
    fn s3_endpoints() {
        let mut location = S3Location {
            bucket: "bucket".into(),
            region: "eu-west-1".into(),
            credentials: AwsCredentials {
                access_key_id: "k".into(),
                secret_access_key: "s".into(),
                session_token: Some("t".into()),
            },
            endpoint: None,
        };
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let url = location.presign_get("a/b.csv", now, 3600);
        assert!(url.starts_with("https://bucket.s3.eu-west-1.amazonaws.com/a/b.csv?"));
        assert!(url.contains("X-Amz-Security-Token=t"));
        location.bucket = "my.bucket".into();
        let url = location.presign_get("a/b.csv", now, 3600);
        assert!(url.starts_with("https://s3.eu-west-1.amazonaws.com/my.bucket/a/b.csv?"));
    }

    #[test]
    fn bucket_scheme_is_stripped() {
        // `/^[a-zA-Z]+:\/\//` has no digits: `s3://` is kept, as in Node.
        assert_eq!(strip_scheme("s3://bucket"), "s3://bucket");
        assert_eq!(strip_scheme("gs://bucket/x"), "bucket/x");
        assert_eq!(strip_scheme("https://bucket"), "bucket");
        assert_eq!(strip_scheme("bucket"), "bucket");
        assert_eq!(strip_scheme("s3a1://bucket"), "s3a1://bucket");
    }

    #[test]
    fn list_keys_are_parsed() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult><Name>b</Name><Prefix>s/t/</Prefix><IsTruncated>false</IsTruncated>
<Contents><Key>s/t/20240101_0001</Key><Size>10</Size></Contents>
<Contents><Key>s/t/a&amp;b</Key><Size>3</Size></Contents></ListBucketResult>"#;
        assert_eq!(parse_list_keys(body), vec!["s/t/20240101_0001", "s/t/a&b"]);
        assert_eq!(xml_tag(body, "IsTruncated").as_deref(), Some("false"));
    }

    #[test]
    fn gcs_signed_url_shape() {
        let account = ServiceAccount {
            project_id: Some("p".into()),
            client_email: "sa@p.iam.gserviceaccount.com".into(),
            private_key: include_str!("../../test/fixtures/rsa_test_key.pem").into(),
            private_key_id: None,
            token_uri: None,
        };
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let url = gcs_signed_url(&account, "bucket", "schema/table/0001", now, 3600).unwrap();
        assert!(url.starts_with(
            "https://storage.googleapis.com/bucket/schema/table/0001?X-Goog-Algorithm=GOOG4-RSA-SHA256&X-Goog-Credential=sa%40p.iam.gserviceaccount.com%2F20240101%2Fauto%2Fstorage%2Fgoog4_request&X-Goog-Date=20240101T000000Z&X-Goog-Expires=3600&X-Goog-SignedHeaders=host&X-Goog-Signature="
        ));
        let signature = url.rsplit('=').next().unwrap();
        // 2048-bit RSA → 256 bytes → 512 hex digits.
        assert_eq!(signature.len(), 512);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn gcs_credentials_resolution() {
        let err = resolve_gcs_account(Some(r#"{"type":"external_account"}"#)).unwrap_err();
        assert!(matches!(err, DriverError::NotImplemented(_)));
        let account = resolve_gcs_account(Some(
            r#"{"type":"service_account","client_email":"a@b","private_key":"k"}"#,
        ))
        .unwrap();
        assert_eq!(account.client_email, "a@b");
    }
}

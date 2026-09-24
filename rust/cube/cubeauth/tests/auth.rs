//! Behavioural tests mirroring `cubejs-api-gateway/test/auth.test.ts` and
//! `test/permissions.test.ts` (minus the Express / native gateway plumbing).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cubeauth::{
    assert_api_scope, context_to_api_scopes, default_scopes, extract_token, AuthConfig, AuthError,
    AuthResult, Authenticator, JwkError, JwkFetcher, JwkResponse, TokenError,
};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};

const SECRET: &str = "secret";

// Test-only RSA key pair (2048 bit) and its JWK modulus.
const RSA_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCnnNlvQ2x01hgg
XGAXCFbO09ecZOE9R8dFwDMph4BNd8g3BHvqW4BsPQJTL6ggx1BuiT8db8GEI174
CsmYw9keQkPJQEYqyCe9UKmHTDo4KHKxGYkp7b/BN48KujzzgQORxMdET80eSLiw
HKWGa2hZ2dUlAOtwpW+79TacQuvU2uNgdP7++Y1+8hHLyEhK5p484kfY9HJHdZZS
isS1jva23+7ulgpDDJUP+cvb0gYQEaXF2vDXgxHCAjuJaCfGhGV+/hfKVjiZKalJ
EClN1juTB7+aTovIdYb9HySIPo1nkvNSzT0vX1wKFvBX3hLlu/gmV00EiDo7bJLc
7uzo9UXtAgMBAAECggEAMFAfQpl6KSFPGgDWd70hHIPmib9wRzQp5dqVRLq4ilvk
+6rQtwhB97EMOwMpIK2i6wGnjioY6ygw5ylg26ZULosmM9vRfeJsxf56pzObMnXC
PXchWNMdaynDEvIEwKGm8Dz6vR+NfdWzWpwfQCQ0k3WdIQnnU3R0RQbVA2yswpEP
gX4ZQTXN5oV5lYhUfTC0X8zUUevkgyt3dl+T+CK1nEmGHzlSibnx2/K6syW0eXxB
W3unBqM6gsOwXrCc2pztYStO1K6UUKhKB24p++VHk04QDEe2z1R7Z5ffAel1KCT9
E7N41aCcw9LgkiZvmqRWbLo+ccthcLnfcszA2y6HdwKBgQDi0yyiGEd3UKg/DZd8
nIXW4TuJcftBeCEIaXOLXL/fI5V9ZfcwSP37DFswztbML6UHe5uonGZFRr09T2ii
iqW4aVyuvx57i8bupK0mJ6cqGOYil9Xyn0rVm9tLfbLSAY78CYKepcoHXrCmk5jR
taDXex6FStCWSEc7qO1rECGWWwKBgQC9K/PL4RrQNx559VPEBi1sYoM3VASycEq6
PZXmxeKFot+fbiJhrUsOErVRAx+sBVDfiCiMhW1SSrzkAyQHQpun5k4dQW7QH4sr
/jnIO2HOedTPgXfU1eMYexx/tDivjYfQp4QxjXkxIX6ipwhPDp7LvSl9zoWOfvEc
FMXng2sXVwKBgQCdmd0JQ5VkccZ1CRyYmKjmBNk5RtktRCqvjZWa33bxs+fKmW6H
PjA6nvs9jnnwpaok6N3e6cylleEnGGW7ilpbJ9oeEO09KoGujv0/5Y1g0qwUnSsq
yUNV6FUWvt/gyvRuaq03TjpxpHlZRHSKQYjgL8ulEbactNvJuDY+jZbIwQKBgGe8
DGrGvA9lyl6SeybJRGtk8hOLDTBUh4Xtc6Ai737cu8gPeucZQkkrVSZhkiKgn6KU
Zbf5CuPPfBmE52Lb0cOWdUtxsDSMt6KePE1i0tWI1XwcwPuDdo7cI9qbl2IdOFbh
JYqOy+B3P5wuAE5p9AZBatlEQNTNI6aEdano1PbtAoGBAM6ghAuDQfQeq1A7aIZP
ycrXckDCK/LxWoMit2Z6gEzY5KZ4XGL1BGC9vjeDcPz+rtgBsUqIu9uUfZSdiKed
ANepV4LnaFeMdLHvur+awKsnsJZrcTqhKgA87MfFaX02dbAn/QyncyBfan0GKK+j
I7ruxeIKHxrdj1oANrkd2cUo
-----END PRIVATE KEY-----
";
const RSA_N: &str = "p5zZb0NsdNYYIFxgFwhWztPXnGThPUfHRcAzKYeATXfINwR76luAbD0CUy-oIMdQbok_HW_BhCNe-ArJmMPZHkJDyUBGKsgnvVCph0w6OChysRmJKe2_wTePCro884EDkcTHRE_NHki4sBylhmtoWdnVJQDrcKVvu_U2nELr1NrjYHT-_vmNfvIRy8hISuaePOJH2PRyR3WWUorEtY72tt_u7pYKQwyVD_nL29IGEBGlxdrw14MRwgI7iWgnxoRlfv4XylY4mSmpSRApTdY7kwe_mk6LyHWG_R8kiD6NZ5LzUs09L19cChbwV94S5bv4JldNBIg6O2yS3O7s6PVF7Q";
const RSA_E: &str = "AQAB";

// Test-only P-256 key pair (PKCS#8) and its JWK coordinates.
const EC_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgQPff5z34rpIPhbAE
RS707DkXtlnyIz/q5ItZksjbzAShRANCAAQv0Nq3riDIMUpUL3rRW2VE6foCyhP1
R7LSxbTbKTp6hJqr4UU4jj4m9O/7kitRlQGEgAeDImhhBv9+FLYzAGyW
-----END PRIVATE KEY-----
";
const EC_X: &str = "L9Dat64gyDFKVC960VtlROn6AsoT9Uey0sW02yk6eoQ";
const EC_Y: &str = "mqvhRTiOPib07_uSK1GVAYSAB4MiaGEG_34UtjMAbJY";

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// `generateAuthToken` from test/utils.ts: adds `iat` and `exp` (+10000 days).
fn claims_with_exp(payload: Value, exp_offset_secs: i64) -> Value {
    let mut claims = payload;
    let now = now_secs();
    claims["iat"] = json!(now);
    claims["exp"] = json!(now + exp_offset_secs);
    claims
}

fn sign_hs(payload: Value, secret: &str, alg: Algorithm) -> String {
    encode(
        &Header::new(alg),
        &payload,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

fn generate_auth_token(payload: Value, secret: &str) -> String {
    sign_hs(
        claims_with_exp(payload, 10_000 * 24 * 3600),
        secret,
        Algorithm::HS256,
    )
}

fn sign_rs256(payload: Value, kid: Option<&str>) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(|k| k.to_string());
    encode(
        &header,
        &claims_with_exp(payload, 3600),
        &EncodingKey::from_rsa_pem(RSA_PRIVATE_PEM.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn sign_es256(payload: Value, kid: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        &claims_with_exp(payload, 3600),
        &EncodingKey::from_ec_pem(EC_PRIVATE_PEM.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn rsa_jwk(kid: &str) -> Value {
    json!({ "kty": "RSA", "kid": kid, "use": "sig", "alg": "RS256", "n": RSA_N, "e": RSA_E })
}

fn ec_jwk(kid: &str) -> Value {
    json!({ "kty": "EC", "kid": kid, "crv": "P-256", "x": EC_X, "y": EC_Y })
}

fn config() -> AuthConfig {
    AuthConfig::builder()
        .api_secret(SECRET)
        .enforce_security_checks(true)
        .build()
}

fn expect_security_context(ctx: &Value) {
    assert_eq!(ctx["uid"], json!(5));
    assert!(ctx.get("iat").is_some());
    assert!(ctx.get("exp").is_some());
}

fn expect_invalid_token(result: Result<AuthResult, AuthError>) -> TokenError {
    match result {
        Err(AuthError::InvalidToken(cause)) => cause,
        other => panic!("expected Invalid token, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// header handling
// ---------------------------------------------------------------------------

#[test]
fn extract_token_strips_schema() {
    assert_eq!(extract_token("abc"), "abc");
    assert_eq!(extract_token("Bearer abc"), "abc");
    assert_eq!(extract_token("Authorization: abc"), "abc");
    assert_eq!(extract_token("Bearer abc def"), "abc");
    assert_eq!(extract_token(""), "");
}

#[tokio::test]
async fn default_authorization() {
    let auth = Authenticator::new(config());
    let token = generate_auth_token(json!({ "uid": 5 }), SECRET);

    for header in [
        token.clone(),
        format!("Authorization: {token}"),
        format!("Bearer {token}"),
    ] {
        let result = auth.authenticate(Some(&header)).await.unwrap();
        expect_security_context(&result.security_context);
        assert!(!result.signed_with_playground_auth_secret);
    }
}

#[tokio::test]
async fn default_authorization_wrong_secret() {
    let auth = Authenticator::new(config());

    let bad_token = "SUPER_LARGE_BAD_TOKEN_WHICH_IS_NOT_A_TOKEN";
    let err = auth.authenticate(Some(bad_token)).await.unwrap_err();
    assert_eq!(err.to_string(), "Invalid token");
    assert_eq!(err.status_code(), 403);
    assert!(matches!(err.cause(), Some(TokenError::Malformed)));

    let wrong_secret = generate_auth_token(json!({ "uid": 5 }), "bad");
    let cause = expect_invalid_token(auth.authenticate(Some(&wrong_secret)).await);
    assert!(matches!(cause, TokenError::InvalidSignature), "{cause:?}");
}

#[tokio::test]
async fn default_authorization_missing_auth_header() {
    let auth = Authenticator::new(config());

    for header in [None, Some(""), Some("Bearer ")] {
        let err = auth.authenticate(header).await.unwrap_err();
        assert!(matches!(err, AuthError::AuthorizationHeaderMissing));
        assert_eq!(err.to_string(), "Authorization header isn't set");
        assert_eq!(err.status_code(), 403);
    }
}

#[tokio::test]
async fn expired_and_not_yet_active_tokens() {
    let auth = Authenticator::new(config());

    let expired = sign_hs(
        claims_with_exp(json!({ "uid": 5 }), -1),
        SECRET,
        Algorithm::HS256,
    );
    let cause = expect_invalid_token(auth.authenticate(Some(&expired)).await);
    assert!(matches!(cause, TokenError::Expired), "{cause:?}");
    assert_eq!(cause.to_string(), "jwt expired");

    let not_active = sign_hs(
        json!({ "uid": 5, "nbf": now_secs() + 3600 }),
        SECRET,
        Algorithm::HS256,
    );
    let cause = expect_invalid_token(auth.authenticate(Some(&not_active)).await);
    assert!(matches!(cause, TokenError::NotBefore), "{cause:?}");

    // `exp` is optional, like in the Node.js jsonwebtoken library.
    let no_exp = sign_hs(json!({ "uid": 5 }), SECRET, Algorithm::HS256);
    let result = auth.authenticate(Some(&no_exp)).await.unwrap();
    assert_eq!(result.security_context, json!({ "uid": 5 }));

    let bad_exp = sign_hs(json!({ "uid": 5, "exp": "soon" }), SECRET, Algorithm::HS256);
    let cause = expect_invalid_token(auth.authenticate(Some(&bad_exp)).await);
    assert!(matches!(cause, TokenError::InvalidExp), "{cause:?}");
}

#[tokio::test]
async fn security_checks_not_enforced() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .enforce_security_checks(false)
            .build(),
    );

    let empty = AuthResult {
        security_context: Value::Null,
        signed_with_playground_auth_secret: false,
    };
    assert_eq!(auth.authenticate(None).await.unwrap(), empty);
    let bad = generate_auth_token(json!({ "uid": 5 }), "bad");
    assert_eq!(auth.authenticate(Some(&bad)).await.unwrap(), empty);

    let good = generate_auth_token(json!({ "uid": 5 }), SECRET);
    expect_security_context(
        &auth
            .authenticate(Some(&good))
            .await
            .unwrap()
            .security_context,
    );
}

// ---------------------------------------------------------------------------
// playground secret
// ---------------------------------------------------------------------------

#[tokio::test]
async fn playground_auth_token() {
    let playground_auth_secret = "playgroundSecret";
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .playground_auth_secret(playground_auth_secret)
            .build(),
    );

    let token = generate_auth_token(json!({ "uid": 5 }), SECRET);
    let playground_token = generate_auth_token(json!({ "uid": 5 }), playground_auth_secret);
    let bad_token = generate_auth_token(json!({ "uid": 5 }), "bad");

    let result = auth
        .authenticate(Some(&format!("Authorization: {token}")))
        .await
        .unwrap();
    expect_security_context(&result.security_context);
    assert!(!result.signed_with_playground_auth_secret);

    let result = auth
        .authenticate(Some(&format!("Authorization: {playground_token}")))
        .await
        .unwrap();
    expect_security_context(&result.security_context);

    // The *main* error is reported when both paths fail.
    let cause = expect_invalid_token(auth.authenticate(Some(&bad_token)).await);
    assert!(matches!(cause, TokenError::InvalidSignature), "{cause:?}");
}

#[tokio::test]
async fn playground_secret_only_accepts_hs256() {
    let playground_auth_secret = "playgroundSecret";
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .playground_auth_secret(playground_auth_secret)
            .build(),
    );

    let hs512 = sign_hs(
        claims_with_exp(json!({ "uid": 5 }), 3600),
        playground_auth_secret,
        Algorithm::HS512,
    );
    assert!(auth.authenticate(Some(&hs512)).await.is_err());

    let hs512_main = sign_hs(
        claims_with_exp(json!({ "uid": 5 }), 3600),
        SECRET,
        Algorithm::HS512,
    );
    assert!(auth.authenticate(Some(&hs512_main)).await.is_ok());
}

#[tokio::test]
async fn authenticate_system_uses_only_the_playground_secret() {
    let playground_auth_secret = "playgroundSecret";
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .playground_auth_secret(playground_auth_secret)
            .build(),
    );

    let playground_token = generate_auth_token(json!({ "uid": 5 }), playground_auth_secret);
    let result = auth
        .authenticate_system(Some(&playground_token))
        .await
        .unwrap();
    expect_security_context(&result.security_context);

    let api_token = generate_auth_token(json!({ "uid": 5 }), SECRET);
    assert!(auth.authenticate_system(Some(&api_token)).await.is_err());
    assert!(matches!(
        auth.authenticate_system(None).await,
        Err(AuthError::AuthorizationHeaderMissing)
    ));
}

mod signed_with_playground_auth_secret_requires_the_dev_token_scope {
    use super::*;

    const PLAYGROUND_AUTH_SECRET: &str = "playgroundSecret";

    async fn flag_for(payload: Value) -> bool {
        let auth = Authenticator::new(
            AuthConfig::builder()
                .api_secret(SECRET)
                .playground_auth_secret(PLAYGROUND_AUTH_SECRET)
                .build(),
        );
        let token = generate_auth_token(payload, PLAYGROUND_AUTH_SECRET);
        auth.authenticate(Some(&format!("Authorization: {token}")))
            .await
            .unwrap()
            .signed_with_playground_auth_secret
    }

    #[tokio::test]
    async fn is_false_for_a_playground_signed_token_with_no_scope_at_all() {
        assert!(!flag_for(json!({ "uid": 5 })).await);
    }

    #[tokio::test]
    async fn is_false_for_a_playground_signed_token_scoped_to_something_else() {
        assert!(!flag_for(json!({ "uid": 5, "scope": ["sql-runner", "agents-config"] })).await);
    }

    #[tokio::test]
    async fn is_true_for_a_playground_signed_token_carrying_the_dev_token_scope() {
        assert!(flag_for(json!({ "uid": 5, "scope": ["dev-token"] })).await);
        assert!(flag_for(json!({ "uid": 5, "scope": ["sql-runner", "dev-token"] })).await);
    }

    #[tokio::test]
    async fn is_false_when_scope_is_not_an_array_of_scope_names() {
        assert!(!flag_for(json!({ "uid": 5, "scope": "dev-token" })).await);
        assert!(!flag_for(json!({ "uid": 5, "scope": { "dev-token": true } })).await);
    }

    #[tokio::test]
    async fn is_false_for_a_token_signed_with_the_main_api_secret_scope_or_not() {
        let auth = Authenticator::new(
            AuthConfig::builder()
                .api_secret(SECRET)
                .playground_auth_secret(PLAYGROUND_AUTH_SECRET)
                .build(),
        );
        let token = generate_auth_token(json!({ "uid": 5, "scope": ["dev-token"] }), SECRET);
        let result = auth.authenticate(Some(&token)).await.unwrap();
        assert!(!result.signed_with_playground_auth_secret);
    }
}

// ---------------------------------------------------------------------------
// security context extraction: legacy `u` claim and claims namespace
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_authorization_with_jwt_token_and_security_context_in_u() {
    let auth = Authenticator::new(config());
    let token = generate_auth_token(json!({ "u": { "uid": 5 } }), SECRET);

    // `req.securityContext` keeps the raw payload ...
    let result = auth.authenticate(Some(&token)).await.unwrap();
    let raw = &result.security_context;
    assert_eq!(raw["u"], json!({ "uid": 5 }));
    assert!(raw.get("iat").is_some());
    assert!(raw.get("exp").is_some());

    // ... and the extractor moves `u` to the root (`coerceForSqlQuery`).
    let extracted = auth.extract_security_context(raw);
    assert_eq!(extracted["uid"], json!(5));
    assert!(extracted.get("u").is_none());
    assert_eq!(extracted["exp"], raw["exp"]);
    assert_eq!(extracted["iat"], raw["iat"]);
    // The original is not changed.
    assert_eq!(raw["u"], json!({ "uid": 5 }));
}

#[test]
fn extract_security_context_handles_non_objects() {
    let auth = Authenticator::new(config());
    assert_eq!(auth.extract_security_context(&Value::Null), json!({}));
    assert_eq!(
        auth.extract_security_context(&json!("AAABBBCCC")),
        json!({})
    );
    assert_eq!(
        auth.extract_security_context(&json!({ "uid": 5, "u": null })),
        json!({ "uid": 5, "u": null })
    );
    assert_eq!(
        auth.extract_security_context(
            &json!({ "exp": 2475858836_i64, "iat": 1611858836, "u": { "uid": 5 } })
        ),
        json!({ "exp": 2475858836_i64, "iat": 1611858836, "uid": 5 })
    );
}

#[tokio::test]
async fn claims_namespace() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .jwt_claims_namespace("https://example.com/cube")
            .build(),
    );

    let token = generate_auth_token(
        json!({ "uid": 5, "https://example.com/cube": { "tenant": "acme" } }),
        SECRET,
    );
    let result = auth.authenticate(Some(&token)).await.unwrap();
    assert_eq!(result.security_context["uid"], json!(5));
    assert_eq!(
        auth.extract_security_context(&result.security_context),
        json!({ "tenant": "acme" })
    );

    // Missing namespace claim → empty context, even if `u` is present.
    let token = generate_auth_token(json!({ "uid": 5, "u": { "uid": 7 } }), SECRET);
    let result = auth.authenticate(Some(&token)).await.unwrap();
    assert_eq!(
        auth.extract_security_context(&result.security_context),
        json!({})
    );
}

// ---------------------------------------------------------------------------
// apiSecrets rotation list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_secrets_accepts_tokens_signed_by_any_secret_in_the_list() {
    let api_secrets = ["outgoing-secret", "current-secret", "incoming-secret"];
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .api_secrets(api_secrets)
            .build(),
    );

    for secret in api_secrets {
        let token = generate_auth_token(json!({ "uid": 5 }), secret);
        let result = auth
            .authenticate(Some(&format!("Authorization: {token}")))
            .await
            .unwrap();
        expect_security_context(&result.security_context);
    }
}

#[tokio::test]
async fn api_secrets_rejects_tokens_not_signed_by_any_secret_in_the_list() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .api_secrets(["a", "b", "c"])
            .build(),
    );

    let bad_token = generate_auth_token(json!({ "uid": 5 }), "not-in-list");
    let cause = expect_invalid_token(auth.authenticate(Some(&bad_token)).await);
    assert!(matches!(cause, TokenError::InvalidSignature), "{cause:?}");
}

#[tokio::test]
async fn api_secrets_takes_precedence_over_api_secret_when_both_are_configured() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .api_secrets(["only-this-one"])
            .build(),
    );

    let old_singular_token = generate_auth_token(json!({ "uid": 5 }), SECRET);
    assert!(auth.authenticate(Some(&old_singular_token)).await.is_err());

    let listed_token = generate_auth_token(json!({ "uid": 5 }), "only-this-one");
    assert!(auth.authenticate(Some(&listed_token)).await.is_ok());
}

#[tokio::test]
async fn api_secrets_empty_list_falls_back_to_singular_api_secret() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .api_secrets(Vec::<String>::new())
            .build(),
    );

    let token = generate_auth_token(json!({ "uid": 5 }), SECRET);
    assert!(auth.authenticate(Some(&token)).await.is_ok());
}

#[tokio::test]
async fn api_secrets_expired_token_signed_by_a_listed_secret_is_rejected() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .api_secrets(["s1", "s2", "s3"])
            .build(),
    );

    // Signed by the *second* secret: the failure on `s1` is a signature
    // failure, the real cause (expiry) must still be surfaced.
    let expired_token = sign_hs(
        claims_with_exp(json!({ "uid": 5 }), -1),
        "s2",
        Algorithm::HS256,
    );
    let cause = expect_invalid_token(auth.authenticate(Some(&expired_token)).await);
    assert!(matches!(cause, TokenError::Expired), "{cause:?}");
}

#[tokio::test]
async fn api_secrets_coexists_with_playground_auth_secret() {
    let playground_auth_secret = "playgroundSecret";
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .api_secrets(["outgoing", "current"])
            .playground_auth_secret(playground_auth_secret)
            .build(),
    );

    // A token signed by the playground secret is accepted via the system path.
    let playground_token = generate_auth_token(json!({ "uid": 5 }), playground_auth_secret);
    assert!(auth.authenticate(Some(&playground_token)).await.is_ok());

    // A token signed by any listed secret is accepted via the main path.
    for secret in ["outgoing", "current"] {
        let api_token = generate_auth_token(json!({ "uid": 5 }), secret);
        assert!(auth.authenticate(Some(&api_token)).await.is_ok());
    }

    // The singular apiSecret is shadowed by apiSecrets and is not a playground
    // secret either, so a token signed with it is rejected by both paths.
    let shadowed_singular_token = generate_auth_token(json!({ "uid": 5 }), SECRET);
    assert!(auth
        .authenticate(Some(&shadowed_singular_token))
        .await
        .is_err());
}

#[tokio::test]
async fn no_secret_configured() {
    let auth = Authenticator::new(AuthConfig::builder().build());
    let token = generate_auth_token(json!({ "uid": 5 }), SECRET);
    let cause = expect_invalid_token(auth.authenticate(Some(&token)).await);
    assert!(matches!(cause, TokenError::NoSecret), "{cause:?}");
    assert_eq!(cause.to_string(), "secret or public key must be provided");
}

// ---------------------------------------------------------------------------
// jwt.key / algorithms / audience / issuer / subject
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jwt_key_wins_over_api_secrets_and_api_secret() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .api_secrets(["rotation"])
            .jwt_key("the-jwt-key")
            .build(),
    );

    let token = generate_auth_token(json!({ "uid": 5 }), "the-jwt-key");
    assert!(auth.authenticate(Some(&token)).await.is_ok());
    for secret in [SECRET, "rotation"] {
        let token = generate_auth_token(json!({ "uid": 5 }), secret);
        assert!(auth.authenticate(Some(&token)).await.is_err());
    }
}

#[tokio::test]
async fn jwt_key_can_be_a_pem_public_key() {
    let public_pem = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        rsa_public_key_base64()
    );
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .jwt_key(public_pem)
            .build(),
    );

    let token = sign_rs256(json!({ "uid": 5 }), None);
    let result = auth.authenticate(Some(&token)).await.unwrap();
    expect_security_context(&result.security_context);

    // HS256 is not among the default algorithms for a public key.
    let hs_token = generate_auth_token(json!({ "uid": 5 }), SECRET);
    let cause = expect_invalid_token(auth.authenticate(Some(&hs_token)).await);
    assert!(matches!(cause, TokenError::InvalidAlgorithm), "{cause:?}");
}

/// SubjectPublicKeyInfo of the test RSA key, derived from its JWK parts.
fn rsa_public_key_base64() -> String {
    use base64::Engine;

    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(RSA_N)
        .unwrap();
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(RSA_E)
        .unwrap();

    fn der_len(len: usize) -> Vec<u8> {
        if len < 0x80 {
            vec![len as u8]
        } else if len < 0x100 {
            vec![0x81, len as u8]
        } else {
            vec![0x82, (len >> 8) as u8, len as u8]
        }
    }
    fn der_integer(bytes: &[u8]) -> Vec<u8> {
        let mut content = Vec::new();
        if bytes[0] & 0x80 != 0 {
            content.push(0);
        }
        content.extend_from_slice(bytes);
        let mut out = vec![0x02];
        out.extend(der_len(content.len()));
        out.extend(content);
        out
    }
    fn der_sequence(content: Vec<u8>) -> Vec<u8> {
        let mut out = vec![0x30];
        out.extend(der_len(content.len()));
        out.extend(content);
        out
    }

    let mut rsa_key = der_integer(&n);
    rsa_key.extend(der_integer(&e));
    let rsa_key = der_sequence(rsa_key);

    let mut bit_string = vec![0x03];
    bit_string.extend(der_len(rsa_key.len() + 1));
    bit_string.push(0);
    bit_string.extend(rsa_key);

    // AlgorithmIdentifier: rsaEncryption OID + NULL
    let algorithm = der_sequence(vec![
        0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
    ]);
    let mut spki = algorithm;
    spki.extend(bit_string);
    let spki = der_sequence(spki);

    base64::engine::general_purpose::STANDARD.encode(spki)
}

#[tokio::test]
async fn jwt_algorithms_restricts_accepted_algorithms() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .jwt_algorithms(["HS512"])
            .build(),
    );

    let hs256 = generate_auth_token(json!({ "uid": 5 }), SECRET);
    let cause = expect_invalid_token(auth.authenticate(Some(&hs256)).await);
    assert!(matches!(cause, TokenError::InvalidAlgorithm), "{cause:?}");

    let hs512 = sign_hs(
        claims_with_exp(json!({ "uid": 5 }), 3600),
        SECRET,
        Algorithm::HS512,
    );
    assert!(auth.authenticate(Some(&hs512)).await.is_ok());
}

#[tokio::test]
async fn audience_issuer_subject_checks() {
    let auth = Authenticator::new(
        AuthConfig::builder()
            .api_secret(SECRET)
            .jwt_audience("cube-api")
            .jwt_issuer(["https://issuer.one", "https://issuer.two"])
            .jwt_subject("user-1")
            .build(),
    );

    let good = json!({ "uid": 5, "aud": "cube-api", "iss": "https://issuer.two", "sub": "user-1" });
    let token = generate_auth_token(good.clone(), SECRET);
    assert!(auth.authenticate(Some(&token)).await.is_ok());

    let aud_array = json!({ "uid": 5, "aud": ["other", "cube-api"], "iss": "https://issuer.one", "sub": "user-1" });
    let token = generate_auth_token(aud_array, SECRET);
    assert!(auth.authenticate(Some(&token)).await.is_ok());

    let mut wrong_aud = good.clone();
    wrong_aud["aud"] = json!("someone-else");
    let cause = expect_invalid_token(
        auth.authenticate(Some(&generate_auth_token(wrong_aud, SECRET)))
            .await,
    );
    assert_eq!(
        cause.to_string(),
        "jwt audience invalid. expected: cube-api"
    );

    let mut no_aud = good.clone();
    no_aud.as_object_mut().unwrap().remove("aud");
    let cause = expect_invalid_token(
        auth.authenticate(Some(&generate_auth_token(no_aud, SECRET)))
            .await,
    );
    assert!(
        matches!(cause, TokenError::InvalidAudience { .. }),
        "{cause:?}"
    );

    let mut wrong_iss = good.clone();
    wrong_iss["iss"] = json!("https://issuer.three");
    let cause = expect_invalid_token(
        auth.authenticate(Some(&generate_auth_token(wrong_iss, SECRET)))
            .await,
    );
    assert_eq!(
        cause.to_string(),
        "jwt issuer invalid. expected: https://issuer.one,https://issuer.two"
    );

    let mut wrong_sub = good;
    wrong_sub["sub"] = json!("user-2");
    let cause = expect_invalid_token(
        auth.authenticate(Some(&generate_auth_token(wrong_sub, SECRET)))
            .await,
    );
    assert_eq!(cause.to_string(), "jwt subject invalid. expected: user-1");
}

#[tokio::test]
async fn audience_claim_is_ignored_when_no_audience_configured() {
    let auth = Authenticator::new(config());
    let token = generate_auth_token(json!({ "uid": 5, "aud": "whatever" }), SECRET);
    assert!(auth.authenticate(Some(&token)).await.is_ok());
}

// ---------------------------------------------------------------------------
// JWK
// ---------------------------------------------------------------------------

/// In-memory [`JwkFetcher`]: serves whatever `keys` currently holds and
/// counts the fetches.
struct MockJwkFetcher {
    keys: Mutex<Value>,
    cache_control: Option<String>,
    fetches: AtomicUsize,
    fail: Mutex<bool>,
}

impl MockJwkFetcher {
    fn new(keys: Value) -> Arc<Self> {
        Arc::new(Self {
            keys: Mutex::new(keys),
            cache_control: None,
            fetches: AtomicUsize::new(0),
            fail: Mutex::new(false),
        })
    }

    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }

    fn set_keys(&self, keys: Value) {
        *self.keys.lock().unwrap() = keys;
    }

    fn set_fail(&self, fail: bool) {
        *self.fail.lock().unwrap() = fail;
    }
}

#[async_trait]
impl JwkFetcher for MockJwkFetcher {
    async fn fetch(&self, _url: &str) -> Result<JwkResponse, JwkError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        if *self.fail.lock().unwrap() {
            return Err(JwkError::Fetch("connection refused".to_string()));
        }
        Ok(JwkResponse {
            body: self.keys.lock().unwrap().clone(),
            cache_control: self.cache_control.clone(),
        })
    }
}

const JWK_URL: &str = "https://issuer.example.com/.well-known/jwks.json";

fn jwk_config() -> AuthConfig {
    AuthConfig::builder()
        .api_secret(SECRET)
        .jwk_url(JWK_URL)
        .build()
}

#[tokio::test]
async fn jwk_rs256_and_es256_tokens() {
    let fetcher = MockJwkFetcher::new(json!({ "keys": [rsa_jwk("rsa-1"), ec_jwk("ec-1")] }));
    let auth = Authenticator::with_jwk_fetcher(jwk_config(), fetcher.clone());

    let rs = sign_rs256(json!({ "uid": 5 }), Some("rsa-1"));
    let result = auth
        .authenticate(Some(&format!("Bearer {rs}")))
        .await
        .unwrap();
    expect_security_context(&result.security_context);
    assert!(!result.signed_with_playground_auth_secret);

    let es = sign_es256(json!({ "uid": 5 }), "ec-1");
    expect_security_context(&auth.authenticate(Some(&es)).await.unwrap().security_context);

    // Cached: a single fetch served both tokens.
    assert_eq!(fetcher.fetches(), 1);

    // A wrong key for the kid fails the signature.
    let mismatched = sign_es256(json!({ "uid": 5 }), "rsa-1");
    let cause = expect_invalid_token(auth.authenticate(Some(&mismatched)).await);
    assert!(
        matches!(
            cause,
            TokenError::InvalidAlgorithm | TokenError::InvalidSignature
        ),
        "{cause:?}"
    );

    // The api secret is not consulted when a JWK URL is configured.
    let hs = generate_auth_token(json!({ "uid": 5 }), SECRET);
    let cause = expect_invalid_token(auth.authenticate(Some(&hs)).await);
    assert!(matches!(cause, TokenError::NoKid), "{cause:?}");
    assert_eq!(cause.to_string(), "JWT without kid inside headers");
}

#[tokio::test]
async fn jwk_header_errors() {
    let fetcher = MockJwkFetcher::new(json!({ "keys": [rsa_jwk("rsa-1")] }));
    let auth = Authenticator::with_jwk_fetcher(jwk_config(), fetcher.clone());

    let err = auth.authenticate(Some("not-a-jwt")).await.unwrap_err();
    assert_eq!(err.to_string(), "Invalid token");
    assert!(matches!(err.cause(), Some(TokenError::UnableToDecode)));
    assert_eq!(err.cause().unwrap().to_string(), "Unable to decode JWT key");

    let no_kid = sign_rs256(json!({ "uid": 5 }), None);
    let cause = expect_invalid_token(auth.authenticate(Some(&no_kid)).await);
    assert!(matches!(cause, TokenError::NoKid), "{cause:?}");

    // Header errors are detected before any fetch.
    assert_eq!(fetcher.fetches(), 0);

    let unknown_kid = sign_rs256(json!({ "uid": 5 }), Some("rsa-2"));
    let cause = expect_invalid_token(auth.authenticate(Some(&unknown_kid)).await);
    assert!(matches!(cause, TokenError::JwkNotFound { .. }), "{cause:?}");
    assert_eq!(
        cause.to_string(),
        "Unable to verify, JWK with kid: \"rsa-2\" not found"
    );
    // Freshly fetched: within the refetch window no second fetch happens.
    assert_eq!(fetcher.fetches(), 1);
}

#[tokio::test]
async fn jwk_key_rotation_refetches_after_the_refetch_window() {
    let fetcher = MockJwkFetcher::new(json!({ "keys": [ec_jwk("ec-1")] }));
    let config = AuthConfig::builder()
        .api_secret(SECRET)
        .jwk_url(JWK_URL)
        .jwk_refetch_window(Duration::ZERO)
        .build();
    let auth = Authenticator::with_jwk_fetcher(config, fetcher.clone());

    let es = sign_es256(json!({ "uid": 5 }), "ec-1");
    assert!(auth.authenticate(Some(&es)).await.is_ok());
    assert_eq!(fetcher.fetches(), 1);

    // Rotated: the new kid is unknown to the cache → force refetch.
    let rs = sign_rs256(json!({ "uid": 5 }), Some("rsa-1"));
    let cause = expect_invalid_token(auth.authenticate(Some(&rs)).await);
    assert!(matches!(cause, TokenError::JwkNotFound { .. }), "{cause:?}");
    assert_eq!(fetcher.fetches(), 2);

    fetcher.set_keys(json!({ "keys": [rsa_jwk("rsa-1")] }));
    assert!(auth.authenticate(Some(&rs)).await.is_ok());
    assert_eq!(fetcher.fetches(), 3);

    // A failing forced refetch surfaces as an invalid token.
    fetcher.set_fail(true);
    let cause = expect_invalid_token(auth.authenticate(Some(&es)).await);
    assert!(matches!(cause, TokenError::JwkFetch(_)), "{cause:?}");
}

#[tokio::test]
async fn jwk_cache_expiry_keeps_stale_keys_when_refresh_fails() {
    let fetcher = MockJwkFetcher::new(json!({ "keys": [rsa_jwk("rsa-1")] }));
    let config = AuthConfig::builder()
        .api_secret(SECRET)
        .jwk_url(JWK_URL)
        .jwk_default_expire(Duration::ZERO)
        .build();
    let auth = Authenticator::with_jwk_fetcher(config, fetcher.clone());

    auth.prefetch_jwks().await;
    assert_eq!(fetcher.fetches(), 1);

    let rs = sign_rs256(json!({ "uid": 5 }), Some("rsa-1"));
    // Expired immediately → refreshed on access.
    assert!(auth.authenticate(Some(&rs)).await.is_ok());
    assert_eq!(fetcher.fetches(), 2);

    // Refresh failure keeps the stale set (background exception in Node.js).
    fetcher.set_fail(true);
    assert!(auth.authenticate(Some(&rs)).await.is_ok());
    assert_eq!(fetcher.fetches(), 3);
}

#[tokio::test]
async fn jwk_fetch_errors() {
    let fetcher = MockJwkFetcher::new(json!({ "nokeys": [] }));
    let auth = Authenticator::with_jwk_fetcher(jwk_config(), fetcher.clone());
    let rs = sign_rs256(json!({ "uid": 5 }), Some("rsa-1"));

    let cause = expect_invalid_token(auth.authenticate(Some(&rs)).await);
    assert_eq!(
        cause.to_string(),
        "Unable to find keys inside response from JWK_URL"
    );

    fetcher.set_keys(json!({ "keys": [{ "kty": "RSA", "n": RSA_N, "e": RSA_E }] }));
    let cause = expect_invalid_token(auth.authenticate(Some(&rs)).await);
    assert_eq!(cause.to_string(), "Unable to find kid inside JWK");

    fetcher.set_fail(true);
    let cause = expect_invalid_token(auth.authenticate(Some(&rs)).await);
    assert!(
        matches!(cause, TokenError::JwkFetch(JwkError::Fetch(_))),
        "{cause:?}"
    );

    // Nothing was cached by the failed attempts.
    fetcher.set_fail(false);
    fetcher.set_keys(json!({ "keys": [rsa_jwk("rsa-1")] }));
    assert!(auth.authenticate(Some(&rs)).await.is_ok());
}

#[tokio::test]
async fn jwk_with_playground_fallback() {
    let fetcher = MockJwkFetcher::new(json!({ "keys": [rsa_jwk("rsa-1")] }));
    let config = AuthConfig::builder()
        .jwk_url(JWK_URL)
        .playground_auth_secret("playgroundSecret")
        .build();
    let auth = Authenticator::with_jwk_fetcher(config, fetcher);

    let playground = generate_auth_token(
        json!({ "uid": 5, "scope": ["dev-token"] }),
        "playgroundSecret",
    );
    let result = auth.authenticate(Some(&playground)).await.unwrap();
    expect_security_context(&result.security_context);
    assert!(result.signed_with_playground_auth_secret);
}

// ---------------------------------------------------------------------------
// API scopes (permissions.test.ts)
// ---------------------------------------------------------------------------

#[test]
fn default_api_scopes() {
    let config = AuthConfig::builder().build();
    assert_eq!(default_scopes(&config), ["graphql", "meta", "data", "sql"]);

    let config = AuthConfig::builder()
        .default_api_scopes(["meta", "data"])
        .build();
    assert_eq!(default_scopes(&config), ["meta", "data"]);
}

#[test]
fn cubejs_default_api_scopes_empty_denies_everything() {
    let config = AuthConfig::from_env_with(|name| match name {
        "CUBEJS_DEFAULT_API_SCOPES" => Some("".to_string()),
        _ => None,
    })
    .unwrap();
    let scopes = context_to_api_scopes(&json!({}), &default_scopes(&config));
    assert!(scopes.is_empty());

    for scope in ["graphql", "meta", "data", "sql", "jobs"] {
        let err = assert_api_scope(&scopes, scope).unwrap_err();
        assert_eq!(err.to_string(), format!("API scope is missing: {scope}"));
        assert_eq!(err.status_code(), 403);
        assert!(matches!(err, AuthError::ApiScopeMissing(_)));
    }
}

#[test]
fn scopes_declined() {
    let scopes: Vec<String> = ["meta", "data", "jobs"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        assert_api_scope(&scopes, "graphql")
            .unwrap_err()
            .to_string(),
        "API scope is missing: graphql"
    );
    assert!(assert_api_scope(&scopes, "meta").is_ok());
    assert!(assert_api_scope(&scopes, "data").is_ok());
    assert!(assert_api_scope(&scopes, "jobs").is_ok());
    assert_eq!(
        assert_api_scope(&scopes, "sql").unwrap_err().to_string(),
        "API scope is missing: sql"
    );
}

#[test]
fn default_context_to_api_scopes_ignores_the_context() {
    let defaults = default_scopes(&AuthConfig::builder().build());
    assert_eq!(
        context_to_api_scopes(&json!({ "uid": 5, "scope": ["jobs"] }), &defaults),
        defaults
    );
    // `jobs` is not granted by default.
    assert!(assert_api_scope(&defaults, "jobs").is_err());
}

// ---------------------------------------------------------------------------
// configuration from the environment
// ---------------------------------------------------------------------------

fn env(vars: &[(&str, &str)]) -> HashMap<String, String> {
    vars.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn from_env_defaults() {
    let config = AuthConfig::from_env_with(|_| None).unwrap();
    assert_eq!(config.api_secret, None);
    assert!(config.api_secrets.is_empty());
    assert_eq!(config.jwt_key, None);
    assert_eq!(config.jwk_url, None);
    assert_eq!(config.jwt_algorithms, None);
    assert_eq!(config.jwt_audience, None);
    assert_eq!(config.jwt_issuer, None);
    assert_eq!(config.jwt_subject, None);
    assert_eq!(config.jwt_claims_namespace, None);
    assert_eq!(config.playground_auth_secret, None);
    assert_eq!(config.default_api_scopes, None);
    assert!(!config.dev_mode);
    assert!(!config.enforce_security_checks);
    assert_eq!(config.jwk_retry, 3);
    assert_eq!(config.jwk_default_expire, None);
    assert_eq!(config.jwk_refetch_window, Duration::from_secs(60));
    assert!(config.candidate_secrets().is_empty());
}

#[test]
fn from_env_reads_cubejs_variables() {
    let vars = env(&[
        ("CUBEJS_API_SECRET", "single"),
        ("CUBEJS_API_SECRETS", " a, b ,,a,c "),
        ("CUBEJS_JWT_KEY", "jwt-key"),
        ("CUBEJS_JWK_URL", "https://example.com/jwks"),
        ("CUBEJS_JWT_ALGS", "RS256,ES256"),
        ("CUBEJS_JWT_AUDIENCE", "aud"),
        ("CUBEJS_JWT_ISSUER", "iss1,iss2"),
        ("CUBEJS_JWT_SUBJECT", "sub"),
        ("CUBEJS_JWT_CLAIMS_NAMESPACE", "ns"),
        ("CUBEJS_PLAYGROUND_AUTH_SECRET", "pg"),
        ("CUBEJS_DEFAULT_API_SCOPES", "meta,data"),
        ("CUBEJS_DEV_MODE", "true"),
        ("NODE_ENV", "production"),
    ]);
    let config = AuthConfig::from_env_with(|name| vars.get(name).cloned()).unwrap();

    assert_eq!(config.api_secret.as_deref(), Some("single"));
    assert_eq!(config.api_secrets, ["a", "b", "c"]);
    assert_eq!(config.jwt_key.as_deref(), Some("jwt-key"));
    assert_eq!(config.jwk_url.as_deref(), Some("https://example.com/jwks"));
    assert_eq!(
        config.jwt_algorithms,
        Some(vec!["RS256".to_string(), "ES256".to_string()])
    );
    assert_eq!(config.jwt_audience.as_deref(), Some("aud"));
    assert_eq!(
        config.jwt_issuer,
        Some(vec!["iss1".to_string(), "iss2".to_string()])
    );
    assert_eq!(config.jwt_subject.as_deref(), Some("sub"));
    assert_eq!(config.jwt_claims_namespace.as_deref(), Some("ns"));
    assert_eq!(config.playground_auth_secret.as_deref(), Some("pg"));
    assert_eq!(
        config.default_api_scopes,
        Some(vec!["meta".to_string(), "data".to_string()])
    );
    assert!(config.dev_mode);
    assert!(config.enforce_security_checks);

    // jwt_key > api_secrets > api_secret
    assert_eq!(config.candidate_secrets(), ["jwt-key"]);
}

#[test]
fn from_env_secret_precedence() {
    let vars = env(&[
        ("CUBEJS_API_SECRET", "single"),
        ("CUBEJS_API_SECRETS", "a,b"),
    ]);
    let config = AuthConfig::from_env_with(|name| vars.get(name).cloned()).unwrap();
    assert_eq!(config.candidate_secrets(), ["a", "b"]);

    // An empty rotation list falls back to the singular secret.
    let vars = env(&[
        ("CUBEJS_API_SECRET", "single"),
        ("CUBEJS_API_SECRETS", " , "),
    ]);
    let config = AuthConfig::from_env_with(|name| vars.get(name).cloned()).unwrap();
    assert!(config.api_secrets.is_empty());
    assert_eq!(config.candidate_secrets(), ["single"]);

    // Empty strings count as unset.
    let vars = env(&[("CUBEJS_API_SECRET", ""), ("CUBEJS_JWK_URL", "")]);
    let config = AuthConfig::from_env_with(|name| vars.get(name).cloned()).unwrap();
    assert_eq!(config.api_secret, None);
    assert_eq!(config.jwk_url, None);
}

#[test]
fn from_env_rejects_invalid_dev_mode() {
    let vars = env(&[("CUBEJS_DEV_MODE", "yes")]);
    let err = AuthConfig::from_env_with(|name| vars.get(name).cloned()).unwrap_err();
    assert!(err.to_string().contains("CUBEJS_DEV_MODE"));

    let vars = env(&[("CUBEJS_DEV_MODE", "false")]);
    assert!(
        !AuthConfig::from_env_with(|name| vars.get(name).cloned())
            .unwrap()
            .dev_mode
    );
}

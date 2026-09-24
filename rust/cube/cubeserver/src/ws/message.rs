//! The WebSocket message protocol, a port of
//! `packages/cubejs-api-gateway/src/ws/message-schema.ts`.
//!
//! The Zod schemas there are `strict()`, so unknown fields are rejected and
//! the error text names the offending path.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `z.union([z.string().max(16), z.int()]).transform(String)`
pub fn parse_message_id(value: &Value) -> Result<String, String> {
    match value {
        Value::String(s) if s.chars().count() <= 16 => Ok(s.clone()),
        Value::String(_) => {
            Err("messageId: Too big: expected string to have <=16 characters".into())
        }
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(n.to_string()),
        Value::Number(_) => Err("messageId: Invalid input: expected int".into()),
        _ => Err("messageId: Invalid input: expected string or int".into()),
    }
}

/// A parsed client message.
#[derive(Debug, Clone, PartialEq)]
pub enum WsMessage {
    /// `{ "authorization": "<token>" }`
    Auth { authorization: String },
    /// `{ "unsubscribe": "<messageId>" }`
    Unsubscribe { message_id: String },
    /// A method call.
    Method(MethodMessage),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodMessage {
    pub method: Method,
    pub message_id: String,
    pub request_id: Option<String>,
    pub params: MethodParams,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    Load,
    Sql,
    DryRun,
    Meta,
    Subscribe,
    Unsubscribe,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Load => "load",
            Method::Sql => "sql",
            Method::DryRun => "dry-run",
            Method::Meta => "meta",
            Method::Subscribe => "subscribe",
            Method::Unsubscribe => "unsubscribe",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "load" => Method::Load,
            "sql" => Method::Sql,
            "dry-run" => Method::DryRun,
            "meta" => Method::Meta,
            "subscribe" => Method::Subscribe,
            "unsubscribe" => Method::Unsubscribe,
            _ => return None,
        })
    }

    /// The parameter names each method reads, as `methodParams` lists them.
    fn allowed_params(self) -> &'static [&'static str] {
        match self {
            Method::Load | Method::Subscribe => &["query", "queryType", "cache"],
            Method::Sql | Method::DryRun => &["query"],
            Method::Meta => &["onlyViews"],
            Method::Unsubscribe => &[],
        }
    }
}

/// The parameters a method carries, already filtered to the allowed names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MethodParams {
    pub query: Option<Value>,
    pub query_type: Option<String>,
    /// `cache` on the wire, `cacheMode` once collected.
    pub cache_mode: Option<String>,
    pub only_views: Option<bool>,
}

fn object_of<'a>(
    value: &'a Value,
    what: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what}: Invalid input: expected object"))
}

fn reject_unknown(
    object: &serde_json::Map<String, Value>,
    allowed: &[&str],
    prefix: &str,
) -> Result<(), String> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(if prefix.is_empty() {
                format!("Unrecognized key: \"{key}\"")
            } else {
                format!("{prefix}: Unrecognized key: \"{key}\"")
            });
        }
    }
    Ok(())
}

/// Parses and validates one client message.
///
/// The error text is what the Node.js server puts in the `error` field, and
/// the caller pairs it with the matching "Invalid ... format" title.
pub fn parse_message(value: &Value) -> Result<WsMessage, MessageError> {
    let object = object_of(value, "message").map_err(MessageError::format)?;

    if object.contains_key("authorization") {
        reject_unknown(object, &["authorization"], "").map_err(MessageError::auth)?;
        let authorization = object["authorization"]
            .as_str()
            .ok_or_else(|| MessageError::auth("authorization: Invalid input: expected string"))?;

        return Ok(WsMessage::Auth {
            authorization: authorization.to_string(),
        });
    }

    if object.contains_key("unsubscribe") {
        reject_unknown(object, &["unsubscribe"], "").map_err(MessageError::unsubscribe)?;
        let message_id = parse_message_id(&object["unsubscribe"])
            .map_err(|e| MessageError::unsubscribe(e.replace("messageId", "unsubscribe")))?;

        return Ok(WsMessage::Unsubscribe { message_id });
    }

    reject_unknown(object, &["method", "messageId", "requestId", "params"], "")
        .map_err(MessageError::format)?;

    let method_value = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| MessageError::format("method: Invalid input"))?;
    let method = Method::parse(method_value)
        .ok_or_else(|| MessageError::format(format!("method: Invalid option: {method_value}")))?;

    let message_id = object
        .get("messageId")
        .ok_or_else(|| MessageError::format("messageId: Invalid input"))
        .and_then(|v| parse_message_id(v).map_err(MessageError::format))?;

    let request_id = match object.get("requestId") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.chars().count() <= 64 => Some(s.clone()),
        Some(Value::String(_)) => {
            return Err(MessageError::format(
                "requestId: Too big: expected string to have <=64 characters",
            ))
        }
        Some(_) => {
            return Err(MessageError::format(
                "requestId: Invalid input: expected string",
            ))
        }
    };

    let params = match object.get("params") {
        None | Some(Value::Null) => MethodParams::default(),
        Some(value) => {
            let params = object_of(value, "params").map_err(MessageError::format)?;
            reject_unknown(params, method.allowed_params(), "params")
                .map_err(MessageError::format)?;

            // `query` is required for every method that declares it.
            if method.allowed_params().contains(&"query") && !params.contains_key("query") {
                return Err(MessageError::format("params.query: Invalid input"));
            }

            MethodParams {
                query: params.get("query").cloned(),
                query_type: params
                    .get("queryType")
                    .and_then(Value::as_str)
                    .map(String::from),
                cache_mode: params
                    .get("cache")
                    .and_then(Value::as_str)
                    .map(String::from),
                only_views: params.get("onlyViews").and_then(Value::as_bool),
            }
        }
    };

    // A method that requires `query` cannot omit `params` entirely.
    if method.allowed_params().contains(&"query") && params.query.is_none() {
        return Err(MessageError::format("params: Invalid input"));
    }

    Ok(WsMessage::Method(MethodMessage {
        method,
        message_id,
        request_id,
        params,
    }))
}

/// A validation failure, carrying the title the Node.js server uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageError {
    pub title: &'static str,
    pub detail: String,
}

impl MessageError {
    fn format(detail: impl Into<String>) -> Self {
        Self {
            title: "Invalid message format",
            detail: detail.into(),
        }
    }

    fn auth(detail: impl Into<String>) -> Self {
        Self {
            title: "Invalid authorization message format",
            detail: detail.into(),
        }
    }

    fn unsubscribe(detail: impl Into<String>) -> Self {
        Self {
            title: "Invalid unsubscribe message format",
            detail: detail.into(),
        }
    }

    pub fn invalid_json(detail: impl Into<String>) -> Self {
        Self {
            title: "Invalid JSON payload",
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_an_auth_message() {
        assert_eq!(
            parse_message(&json!({ "authorization": "token" })).unwrap(),
            WsMessage::Auth {
                authorization: "token".to_string()
            }
        );
    }

    #[test]
    fn rejects_unknown_keys_strictly() {
        let err = parse_message(&json!({ "authorization": "t", "extra": 1 })).unwrap_err();
        assert_eq!(err.title, "Invalid authorization message format");
        assert!(err.detail.contains("extra"), "{}", err.detail);
    }

    #[test]
    fn message_ids_accept_strings_and_integers() {
        assert_eq!(parse_message_id(&json!("abc")).unwrap(), "abc");
        assert_eq!(parse_message_id(&json!(42)).unwrap(), "42");
        assert!(parse_message_id(&json!("x".repeat(17))).is_err());
        assert!(parse_message_id(&json!(1.5)).is_err());
    }

    #[test]
    fn parses_a_load_message_and_keeps_only_allowed_params() {
        let message = parse_message(&json!({
            "method": "load",
            "messageId": 1,
            "requestId": "req",
            "params": { "query": { "measures": ["a.b"] }, "queryType": "multi", "cache": "no-cache" }
        }))
        .unwrap();

        let WsMessage::Method(method) = message else {
            panic!("expected a method message");
        };
        assert_eq!(method.method, Method::Load);
        assert_eq!(method.message_id, "1");
        assert_eq!(method.request_id.as_deref(), Some("req"));
        assert_eq!(method.params.query_type.as_deref(), Some("multi"));
        // `cache` becomes `cacheMode` once collected.
        assert_eq!(method.params.cache_mode.as_deref(), Some("no-cache"));
    }

    #[test]
    fn rejects_params_a_method_does_not_declare() {
        let err = parse_message(&json!({
            "method": "sql",
            "messageId": "1",
            "params": { "query": {}, "cache": "no-cache" }
        }))
        .unwrap_err();
        assert_eq!(err.title, "Invalid message format");
        assert!(err.detail.contains("cache"), "{}", err.detail);
    }

    #[test]
    fn meta_takes_only_views_and_no_query() {
        let message = parse_message(&json!({
            "method": "meta",
            "messageId": "1",
            "params": { "onlyViews": true }
        }))
        .unwrap();
        let WsMessage::Method(method) = message else {
            panic!("expected a method message");
        };
        assert_eq!(method.params.only_views, Some(true));
        assert!(method.params.query.is_none());
    }

    #[test]
    fn a_query_method_requires_a_query() {
        for message in [
            json!({ "method": "load", "messageId": "1" }),
            json!({ "method": "load", "messageId": "1", "params": {} }),
        ] {
            assert!(parse_message(&message).is_err(), "{message}");
        }
    }

    #[test]
    fn parses_unsubscribe() {
        assert_eq!(
            parse_message(&json!({ "unsubscribe": 7 })).unwrap(),
            WsMessage::Unsubscribe {
                message_id: "7".to_string()
            }
        );
    }

    #[test]
    fn rejects_an_unknown_method() {
        let err = parse_message(&json!({ "method": "nope", "messageId": "1" })).unwrap_err();
        assert!(err.detail.contains("nope"), "{}", err.detail);
    }
}

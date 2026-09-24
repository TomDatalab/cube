//! Databricks JDBC URL handling: port of `helpers.ts`
//! (`extractAndRemoveUidPwdFromJdbcUrl`, `parseDatabricksJdbcUrl`).
//!
//! The Rust driver never loads the JDBC jar, but `CUBEJS_DB_DATABRICKS_URL`
//! keeps its JDBC form so existing deployments work unchanged. The URL is
//! only mined for what the REST API needs: the workspace host, the SQL
//! warehouse id (from `httpPath`), the credentials (`UID`/`PWD`) and the
//! session defaults (`ConnCatalog`, `ConnSchema` or the `/schema` path).

use crate::error::{DriverError, Result};

/// The prefix of every Databricks JDBC URL.
pub const JDBC_PREFIX: &str = "jdbc:databricks://";
/// The deprecated Spark prefix, rewritten to [`JDBC_PREFIX`].
pub const SPARK_PREFIX: &str = "jdbc:spark://";

/// `ParsedConnectionProperties` plus the session defaults.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedJdbcUrl {
    /// Workspace host name (`adb-123.4.azuredatabricks.net`).
    pub host: String,
    /// Port, when present (the REST API always uses HTTPS on 443).
    pub port: Option<u16>,
    /// SQL warehouse id extracted from `httpPath`.
    pub warehouse_id: String,
    /// The raw `httpPath` parameter.
    pub http_path: String,
    /// `ConnCatalog` URL parameter (session catalog of the JDBC connection).
    pub catalog: Option<String>,
    /// `ConnSchema` URL parameter, or the path segment after the port.
    pub schema: Option<String>,
}

/// Replaces a `jdbc:spark://` prefix with `jdbc:databricks://`.
/// Returns the URL and whether the deprecated protocol was used.
pub fn normalize_spark_protocol(url: &str) -> (String, bool) {
    if url.contains(SPARK_PREFIX) {
        (url.replacen(SPARK_PREFIX, JDBC_PREFIX, 1), true)
    } else {
        (url.to_string(), false)
    }
}

/// Case-insensitive search of `KEY=` returning the byte range of the value.
fn find_param(url: &str, key: &str) -> Option<(usize, usize, usize)> {
    let lower = url.to_ascii_lowercase();
    let needle = format!("{}=", key.to_ascii_lowercase());
    let start = lower.find(&needle)?;
    let value_start = start + needle.len();
    let value_end = url[value_start..]
        .find(';')
        .map(|i| value_start + i)
        .unwrap_or(url.len());
    Some((start, value_start, value_end))
}

/// Removes the first `;?KEY=[^;]*` occurrence (case-insensitive), like the
/// JS `replace(/;?KEY=[^;]*/i, '')`.
fn remove_param(url: &str, key: &str) -> String {
    match find_param(url, key) {
        Some((start, _, end)) => {
            let start = if start > 0 && url.as_bytes()[start - 1] == b';' {
                start - 1
            } else {
                start
            };
            format!("{}{}", &url[..start], &url[end..])
        }
        None => url.to_string(),
    }
}

/// Port of `extractAndRemoveUidPwdFromJdbcUrl`: returns `(uid, pwd, cleaned_url)`.
/// `uid` defaults to `token` and `pwd` to an empty string.
pub fn extract_and_remove_uid_pwd(jdbc_url: &str) -> (String, String, String) {
    let value = |key: &str| {
        find_param(jdbc_url, key)
            .map(|(_, s, e)| jdbc_url[s..e].to_string())
            .filter(|v| !v.is_empty())
    };
    let uid = value("UID").unwrap_or_else(|| "token".to_string());
    let pwd = value("PWD").unwrap_or_default();
    let cleaned = remove_param(
        &remove_param(&remove_param(jdbc_url, "UID"), "PWD"),
        "AuthMech",
    );
    (uid, pwd, cleaned)
}

/// Port of `parseDatabricksJdbcUrl`, extended with the session defaults.
///
/// Like the Node.js driver it requires `httpPath` to point at a SQL
/// warehouse (`/sql/1.0/warehouses/<id>`); the legacy `/sql/1.0/endpoints/<id>`
/// form is accepted as well. An all-purpose cluster path cannot be used: the
/// Statement Execution API only runs on SQL warehouses.
pub fn parse_jdbc_url(jdbc_url: &str) -> Result<ParsedJdbcUrl> {
    let without_prefix = jdbc_url.strip_prefix(JDBC_PREFIX).ok_or_else(|| {
        DriverError::Config(format!(
            "CUBEJS_DB_DATABRICKS_URL must start with {JDBC_PREFIX}, got \"{}\"",
            redact(jdbc_url)
        ))
    })?;

    let mut parts = without_prefix.split(';');
    let host_port_and_path = parts.next().unwrap_or_default();
    let (host_port, path) = match host_port_and_path.split_once('/') {
        Some((hp, p)) => (hp, Some(p)),
        None => (host_port_and_path, None),
    };
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()),
        None => (host_port, None),
    };
    if host.is_empty() {
        return Err(DriverError::Config(
            "Missing host in CUBEJS_DB_DATABRICKS_URL".to_string(),
        ));
    }

    let mut http_path = None;
    let mut catalog = None;
    let mut schema = path
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    for param in parts {
        // JS: `const [key, value] = param.split('=')`.
        let mut kv = param.splitn(3, '=');
        let (Some(key), Some(value)) = (kv.next(), kv.next()) else {
            continue;
        };
        if key.is_empty() || value.is_empty() {
            continue;
        }
        match key.to_ascii_lowercase().as_str() {
            "httppath" => http_path = Some(value.to_string()),
            "conncatalog" => catalog = Some(value.to_string()),
            "connschema" => schema = Some(value.to_string()),
            _ => {}
        }
    }

    let http_path =
        http_path.ok_or_else(|| DriverError::Config("Missing httpPath in JDBC URL".to_string()))?;
    let warehouse_id = extract_warehouse_id(&http_path).ok_or_else(|| {
        DriverError::Config(format!(
            "Could not extract warehouseId from httpPath \"{http_path}\". The Rust Databricks \
             driver uses the SQL Statement Execution API, which needs a SQL warehouse \
             (httpPath=/sql/1.0/warehouses/<id>); all-purpose clusters are not supported."
        ))
    })?;

    Ok(ParsedJdbcUrl {
        host: host.to_string(),
        port,
        warehouse_id,
        http_path,
        catalog,
        schema,
    })
}

/// `/warehouses/([a-zA-Z0-9]+)` (or `/endpoints/...`).
fn extract_warehouse_id(http_path: &str) -> Option<String> {
    for marker in ["/warehouses/", "/endpoints/"] {
        if let Some(idx) = http_path.find(marker) {
            let id: String = http_path[idx + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
}

/// The URL with any `PWD=` value masked, for error messages.
pub fn redact(url: &str) -> String {
    match find_param(url, "PWD") {
        Some((_, s, e)) => format!("{}***{}", &url[..s], &url[e..]),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "jdbc:databricks://adb-123456789.10.azuredatabricks.net:443/default;transportMode=http;ssl=1;httpPath=/sql/1.0/warehouses/abc123def;AuthMech=3;UID=token;PWD=dapi123";

    #[test]
    fn extracts_and_removes_credentials() {
        let (uid, pwd, cleaned) = extract_and_remove_uid_pwd(URL);
        assert_eq!(uid, "token");
        assert_eq!(pwd, "dapi123");
        assert_eq!(
            cleaned,
            "jdbc:databricks://adb-123456789.10.azuredatabricks.net:443/default;transportMode=http;ssl=1;httpPath=/sql/1.0/warehouses/abc123def"
        );

        let (uid, pwd, cleaned) =
            extract_and_remove_uid_pwd("jdbc:databricks://h:443;httpPath=/sql/1.0/warehouses/x");
        assert_eq!(uid, "token");
        assert_eq!(pwd, "");
        assert_eq!(
            cleaned,
            "jdbc:databricks://h:443;httpPath=/sql/1.0/warehouses/x"
        );

        // case-insensitive, like the JS `/i` regexes
        let (uid, pwd, _) = extract_and_remove_uid_pwd("jdbc:databricks://h;uid=me;pwd=secret");
        assert_eq!(uid, "me");
        assert_eq!(pwd, "secret");
    }

    #[test]
    fn parses_host_and_warehouse() {
        let parsed = parse_jdbc_url(URL).unwrap();
        assert_eq!(parsed.host, "adb-123456789.10.azuredatabricks.net");
        assert_eq!(parsed.port, Some(443));
        assert_eq!(parsed.warehouse_id, "abc123def");
        assert_eq!(parsed.schema.as_deref(), Some("default"));
        assert_eq!(parsed.catalog, None);

        let parsed = parse_jdbc_url(
            "jdbc:databricks://dbc-1.cloud.databricks.com:443;httpPath=/sql/1.0/endpoints/e1;ConnCatalog=main;ConnSchema=sales",
        )
        .unwrap();
        assert_eq!(parsed.warehouse_id, "e1");
        assert_eq!(parsed.catalog.as_deref(), Some("main"));
        assert_eq!(parsed.schema.as_deref(), Some("sales"));
    }

    #[test]
    fn reports_missing_http_path_and_clusters() {
        let err = parse_jdbc_url("jdbc:databricks://adb-123456789.10.azuredatabricks.net:443")
            .unwrap_err();
        assert_eq!(err.to_string(), "Missing httpPath in JDBC URL");

        let err =
            parse_jdbc_url("jdbc:databricks://h:443;httpPath=sql/protocolv1/o/123/0123-456789-abc")
                .unwrap_err();
        assert!(err.to_string().contains("Could not extract warehouseId"));
        assert!(err.to_string().contains("all-purpose clusters"));

        let err = parse_jdbc_url("https://h").unwrap_err();
        assert!(err
            .to_string()
            .contains("must start with jdbc:databricks://"));
    }

    #[test]
    fn spark_protocol_is_rewritten() {
        let (url, warn) =
            normalize_spark_protocol("jdbc:spark://h:443;httpPath=/sql/1.0/warehouses/w");
        assert!(warn);
        assert_eq!(
            url,
            "jdbc:databricks://h:443;httpPath=/sql/1.0/warehouses/w"
        );
        let (_, warn) = normalize_spark_protocol(URL);
        assert!(!warn);
    }

    #[test]
    fn redacts_password() {
        assert!(!redact(URL).contains("dapi123"));
        assert!(redact(URL).contains("PWD=***"));
    }
}

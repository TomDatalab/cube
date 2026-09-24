//! Environment-driven limits used by query normalization
//! (`@cubejs-backend/shared` `env.ts`).

use std::env;

use crate::error::QueryError;
use crate::timezone::canonical_timezone;

/// `CUBEJS_DB_QUERY_LIMIT`
pub const DEFAULT_DB_QUERY_LIMIT: u64 = 50_000;
/// `CUBEJS_DB_QUERY_DEFAULT_LIMIT`
pub const DEFAULT_DB_QUERY_DEFAULT_LIMIT: u64 = 10_000;
/// `CUBEJS_DEFAULT_TIMEZONE`
pub const DEFAULT_TIMEZONE: &str = "UTC";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryConfig {
    /// Hard cap: a bigger explicit `limit` is rejected.
    pub db_query_limit: u64,
    /// Applied when the query carries no `limit`.
    pub db_query_default_limit: u64,
    /// Used when the query carries no `timezone`.
    pub default_timezone: String,
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            db_query_limit: DEFAULT_DB_QUERY_LIMIT,
            db_query_default_limit: DEFAULT_DB_QUERY_DEFAULT_LIMIT,
            default_timezone: DEFAULT_TIMEZONE.to_string(),
        }
    }
}

impl QueryConfig {
    pub fn from_env() -> Result<Self, QueryError> {
        Self::from_env_with(|key| env::var(key).ok())
    }

    pub fn from_env_with(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, QueryError> {
        let defaults = Self::default();

        let int = |key: &str, default: u64| -> Result<u64, QueryError> {
            match lookup(key) {
                Some(value) => value.trim().parse::<u64>().map_err(|_| {
                    QueryError::internal(format!(
                        "Value \"{}\" is not valid for {}. Should be an integer.",
                        value, key
                    ))
                }),
                None => Ok(default),
            }
        };

        // env.ts: `(CUBEJS_DEFAULT_TIMEZONE || '').trim() || 'UTC'`, then
        // `canonicalTimezone`, which throws for an unknown name.
        let timezone_value = lookup("CUBEJS_DEFAULT_TIMEZONE")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| defaults.default_timezone.clone());
        let default_timezone = canonical_timezone(&timezone_value).ok_or_else(|| {
            QueryError::internal(format!(
                "Value \"{}\" is not valid for CUBEJS_DEFAULT_TIMEZONE. Should be a correct time zone.",
                timezone_value
            ))
        })?;

        Ok(Self {
            db_query_limit: int("CUBEJS_DB_QUERY_LIMIT", defaults.db_query_limit)?,
            db_query_default_limit: int(
                "CUBEJS_DB_QUERY_DEFAULT_LIMIT",
                defaults.db_query_default_limit,
            )?,
            default_timezone,
        })
    }

    /// `getEnv('dbQueryDefaultLimit') <= getEnv('dbQueryLimit') ? default : limit`
    pub fn effective_default_limit(&self) -> u64 {
        if self.db_query_default_limit <= self.db_query_limit {
            self.db_query_default_limit
        } else {
            self.db_query_limit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config_from(pairs: &[(&str, &str)]) -> Result<QueryConfig, QueryError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        QueryConfig::from_env_with(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults_match_node() {
        let config = config_from(&[]).unwrap();
        assert_eq!(config.db_query_limit, 50_000);
        assert_eq!(config.db_query_default_limit, 10_000);
        assert_eq!(config.default_timezone, "UTC");
        assert_eq!(config.effective_default_limit(), 10_000);
    }

    #[test]
    fn reads_env() {
        let config = config_from(&[
            ("CUBEJS_DB_QUERY_LIMIT", "100"),
            ("CUBEJS_DB_QUERY_DEFAULT_LIMIT", "200"),
            ("CUBEJS_DEFAULT_TIMEZONE", "america/sao_paulo"),
        ])
        .unwrap();
        assert_eq!(config.db_query_limit, 100);
        // The default limit is capped by the hard limit.
        assert_eq!(config.effective_default_limit(), 100);
        assert_eq!(config.default_timezone, "America/Sao_Paulo");
    }

    #[test]
    fn rejects_unknown_timezone() {
        let err = config_from(&[("CUBEJS_DEFAULT_TIMEZONE", "Mars/Olympus")]).unwrap_err();
        assert!(err.message().contains("CUBEJS_DEFAULT_TIMEZONE"));
    }
}

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::env::{parse_bool, parse_list, parse_u64, Env};
use crate::error::{ConfigError, Origin, Result};

/// One background-refresh context. Replaces the `scheduledRefreshContexts`
/// callback, which could return an arbitrary, dynamically computed list.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RefreshContextSpec {
    #[serde(default)]
    pub security_context: Value,
}

/// The `scheduled_refresh` block.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledRefreshSpec {
    pub enabled: Option<bool>,
    /// Refresh interval in seconds.
    pub interval: Option<u64>,
    pub timezones: Option<Vec<String>>,
    pub concurrency: Option<u32>,
    pub batch_size: Option<u32>,
    /// Static list of security contexts to refresh for. With multi-tenancy,
    /// this must enumerate the tenants: nothing can discover them at runtime.
    #[serde(default)]
    pub contexts: Vec<RefreshContextSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledRefresh {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub timezones: Vec<String>,
    pub concurrency: Option<u32>,
    pub batch_size: u32,
    pub contexts: Vec<Value>,
}

pub const DEFAULT_REFRESH_INTERVAL_SECONDS: u64 = 30;

impl ScheduledRefreshSpec {
    pub(crate) fn resolve(&self, env: &Env, file: &Path) -> Result<ScheduledRefresh> {
        let enabled = match env
            .get("CUBEJS_REFRESH_WORKER")
            .or_else(|| env.get("CUBEJS_SCHEDULED_REFRESH"))
        {
            Some(raw) => parse_bool(raw, Origin::env("CUBEJS_REFRESH_WORKER"))?,
            None => self.enabled.unwrap_or(false),
        };

        let interval_seconds = match env.get("CUBEJS_SCHEDULED_REFRESH_TIMER") {
            Some(raw) => parse_u64(raw, Origin::env("CUBEJS_SCHEDULED_REFRESH_TIMER"))?,
            None => self.interval.unwrap_or(DEFAULT_REFRESH_INTERVAL_SECONDS),
        };
        if interval_seconds == 0 {
            return Err(ConfigError::invalid(
                Origin::file(file, "scheduled_refresh.interval"),
                "0",
                "must be at least 1 second; use `enabled: false` to turn refreshes off",
            ));
        }

        let (timezones, tz_origin) = match env.get("CUBEJS_SCHEDULED_REFRESH_TIMEZONES") {
            Some(raw) => (
                parse_list(raw),
                Origin::env("CUBEJS_SCHEDULED_REFRESH_TIMEZONES"),
            ),
            None => (
                self.timezones.clone().unwrap_or_else(|| vec!["UTC".into()]),
                Origin::file(file, "scheduled_refresh.timezones"),
            ),
        };
        for timezone in &timezones {
            if !is_plausible_timezone(timezone) {
                return Err(ConfigError::invalid(
                    tz_origin.clone(),
                    timezone,
                    "must be an IANA time zone name, e.g. UTC or America/Los_Angeles",
                ));
            }
        }
        let timezones = if timezones.is_empty() {
            vec!["UTC".to_string()]
        } else {
            timezones
        };

        let concurrency = match env.get("CUBEJS_SCHEDULED_REFRESH_QUERIES_PER_APP_ID") {
            Some(raw) => Some(positive(
                raw,
                Origin::env("CUBEJS_SCHEDULED_REFRESH_QUERIES_PER_APP_ID"),
            )?),
            None => self.concurrency,
        };
        if concurrency == Some(0) {
            return Err(ConfigError::invalid(
                Origin::file(file, "scheduled_refresh.concurrency"),
                "0",
                "must be at least 1",
            ));
        }

        let batch_size = match env.get("CUBEJS_SCHEDULED_REFRESH_BATCH_SIZE") {
            Some(raw) => positive(raw, Origin::env("CUBEJS_SCHEDULED_REFRESH_BATCH_SIZE"))?,
            None => self.batch_size.unwrap_or(1),
        };
        if batch_size == 0 {
            return Err(ConfigError::invalid(
                Origin::file(file, "scheduled_refresh.batch_size"),
                "0",
                "must be at least 1",
            ));
        }

        let contexts = if self.contexts.is_empty() {
            vec![Value::Null]
        } else {
            self.contexts
                .iter()
                .map(|c| c.security_context.clone())
                .collect()
        };

        Ok(ScheduledRefresh {
            enabled,
            interval_seconds,
            timezones,
            concurrency,
            batch_size,
            contexts,
        })
    }
}

fn positive(raw: &str, origin: Origin) -> Result<u32> {
    let parsed = parse_u64(raw, origin.clone())?;
    if parsed == 0 || parsed > u64::from(u32::MAX) {
        return Err(ConfigError::invalid(origin, raw, "must be at least 1"));
    }
    Ok(parsed as u32)
}

/// Shape check only: `UTC`, `Area/Location`, `Area/Sub/Location`. The real
/// tz database lookup happens in the time-zone layer of the query engine.
fn is_plausible_timezone(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.split('/').all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+')
    })
}

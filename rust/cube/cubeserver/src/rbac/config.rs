//! RBAC settings, read from the environment (`CUBE_RBAC=spicedb` turns it on).

use std::time::Duration;

/// Default validity of a login token (`CUBE_RBAC_TOKEN_TTL`).
pub const DEFAULT_TOKEN_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// How long a failed login waits before answering, to slow down guessing.
pub const DEFAULT_FAILURE_DELAY: Duration = Duration::from_secs(1);

/// How long a login waits for a free hashing slot before answering 503.
pub const DEFAULT_HASH_QUEUE_WAIT: Duration = Duration::from_secs(2);

/// Password hashes run at once: each argon2 run holds ~19 MiB, so an
/// unbounded login flood could exhaust memory. Two per CPU.
pub fn default_max_concurrent_hashes() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        * 2
}

#[derive(Clone)]
pub struct RbacConfig {
    pub spicedb_endpoint: String,
    pub preshared_key: String,
    pub db_url: String,
    /// `CUBE_RBAC_ADMINS`: user ids made platform admins at every start.
    pub admins: Vec<String>,
    /// `CUBE_RBAC_ADMIN_PASSWORD`: the password of a listed admin that does
    /// not exist yet.
    pub admin_password: Option<String>,
    pub token_ttl: Duration,
    /// Wait before answering a failed login or password check. Always
    /// [`DEFAULT_FAILURE_DELAY`] outside tests.
    pub failure_delay: Duration,
    /// Password hashes and verifications run at once (see
    /// [`default_max_concurrent_hashes`]).
    pub max_concurrent_hashes: usize,
    /// Wait for a hashing slot before answering 503.
    pub hash_queue_wait: Duration,
}

/// Secrets stay out of logs and panics.
impl std::fmt::Debug for RbacConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RbacConfig")
            .field("spicedb_endpoint", &self.spicedb_endpoint)
            .field("admins", &self.admins)
            .field("token_ttl", &self.token_ttl)
            .field("failure_delay", &self.failure_delay)
            .field("max_concurrent_hashes", &self.max_concurrent_hashes)
            .field("hash_queue_wait", &self.hash_queue_wait)
            .finish_non_exhaustive()
    }
}

impl RbacConfig {
    /// `None` when `CUBE_RBAC` is unset (or empty); an error when it names
    /// anything but `spicedb` or a required variable is missing, so a typo
    /// never starts the server without RBAC.
    pub fn from_env() -> Result<Option<Self>, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`RbacConfig::from_env`] against any variable lookup.
    pub fn from_lookup<F>(lookup: F) -> Result<Option<Self>, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let var = |name: &str| lookup(name).filter(|v| !v.trim().is_empty());

        match var("CUBE_RBAC") {
            None => return Ok(None),
            Some(mode) if mode.trim() == "spicedb" => {}
            Some(other) => {
                return Err(format!(
                    "CUBE_RBAC={other} is not supported; the only value is `spicedb`"
                ))
            }
        }

        let required = |name: &str| {
            var(name).ok_or_else(|| format!("CUBE_RBAC=spicedb needs {name}, which is not set"))
        };
        let spicedb_endpoint = required("SPICEDB_ENDPOINT")?;
        let preshared_key = required("SPICEDB_PRESHARED_KEY")?;
        let db_url = required("CUBE_APP_DB_URL")?;

        let admins: Vec<String> = var("CUBE_RBAC_ADMINS")
            .map(|list| {
                list.split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for id in &admins {
            super::identity::validate_user_id(id)
                .map_err(|e| format!("CUBE_RBAC_ADMINS: `{id}`: {}", e.body.error))?;
        }

        let token_ttl = match var("CUBE_RBAC_TOKEN_TTL") {
            Some(value) => {
                parse_duration(&value).map_err(|e| format!("CUBE_RBAC_TOKEN_TTL: {e}"))?
            }
            None => DEFAULT_TOKEN_TTL,
        };

        Ok(Some(Self {
            spicedb_endpoint,
            preshared_key,
            db_url,
            admins,
            admin_password: var("CUBE_RBAC_ADMIN_PASSWORD"),
            token_ttl,
            failure_delay: DEFAULT_FAILURE_DELAY,
            max_concurrent_hashes: default_max_concurrent_hashes(),
            hash_queue_wait: DEFAULT_HASH_QUEUE_WAIT,
        }))
    }
}

/// Parses `"3600"` (seconds), `"90s"`, `"30m"`, `"12h"` or `"30d"`.
pub fn parse_duration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    let (number, unit) = match value.find(|c: char| !c.is_ascii_digit()) {
        Some(at) => value.split_at(at),
        None => (value, "s"),
    };
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => {
            return Err(format!(
                "`{value}` is not a duration like 3600, 30m, 12h or 30d"
            ))
        }
    };
    let number: u64 = number
        .parse()
        .map_err(|_| format!("`{value}` is not a duration like 3600, 30m, 12h or 30d"))?;
    let seconds = number
        .checked_mul(multiplier)
        .filter(|s| *s > 0)
        .ok_or_else(|| format!("`{value}` is not a positive duration"))?;
    Ok(Duration::from_secs(seconds))
}

/// RBAC assumes one data model and global connection names, so a `tenants:`
/// block in `cube.yml` cannot be combined with it.
pub fn check_rbac_compatible(config: &cubeconfig::CubeConfig) -> Result<(), String> {
    if config.has_tenants() {
        return Err(
            "CUBE_RBAC=spicedb cannot be used with a `tenants:` block in cube.yml: \
             RBAC assumes a single data model. Remove `tenants:` or unset CUBE_RBAC."
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    const FULL: &[(&str, &str)] = &[
        ("CUBE_RBAC", "spicedb"),
        ("SPICEDB_ENDPOINT", "http://127.0.0.1:18443"),
        ("SPICEDB_PRESHARED_KEY", "key"),
        ("CUBE_APP_DB_URL", "postgres://x/cube_app"),
    ];

    #[test]
    fn off_unless_asked_for() {
        assert!(RbacConfig::from_lookup(lookup(&[])).unwrap().is_none());
        assert!(RbacConfig::from_lookup(lookup(&[("CUBE_RBAC", "")]))
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_typo_does_not_silently_turn_it_off() {
        assert!(RbacConfig::from_lookup(lookup(&[("CUBE_RBAC", "spicdb")])).is_err());
    }

    #[test]
    fn every_connection_setting_is_required() {
        for missing in [
            "SPICEDB_ENDPOINT",
            "SPICEDB_PRESHARED_KEY",
            "CUBE_APP_DB_URL",
        ] {
            let vars: Vec<_> = FULL
                .iter()
                .copied()
                .filter(|(k, _)| *k != missing)
                .collect();
            let err = RbacConfig::from_lookup(lookup(&vars)).unwrap_err();
            assert!(err.contains(missing), "{err}");
        }
    }

    #[test]
    fn defaults_and_lists() {
        let config = RbacConfig::from_lookup(lookup(FULL)).unwrap().unwrap();
        assert_eq!(config.token_ttl, DEFAULT_TOKEN_TTL);
        assert_eq!(config.failure_delay, DEFAULT_FAILURE_DELAY);
        assert!(config.admins.is_empty());
        assert!(config.admin_password.is_none());

        let mut vars = FULL.to_vec();
        vars.push(("CUBE_RBAC_ADMINS", " tom, ,ana "));
        vars.push(("CUBE_RBAC_TOKEN_TTL", "2h"));
        vars.push(("CUBE_RBAC_ADMIN_PASSWORD", "a-long-password"));
        let config = RbacConfig::from_lookup(lookup(&vars)).unwrap().unwrap();
        assert_eq!(config.admins, vec!["tom", "ana"]);
        assert_eq!(config.token_ttl, Duration::from_secs(7200));
        // The key and password never show in debug output.
        let debug = format!("{config:?}");
        assert!(!debug.contains("a-long-password") && !debug.contains("\"key\""));
    }

    #[test]
    fn admin_ids_follow_the_user_id_rule() {
        for bad in ["Tom", "tom.datalab", "*", "a b"] {
            let mut vars = FULL.to_vec();
            let list = format!("ann,{bad}");
            vars.push(("CUBE_RBAC_ADMINS", &list));
            let err = RbacConfig::from_lookup(lookup(&vars)).unwrap_err();
            assert!(err.contains("CUBE_RBAC_ADMINS"), "{err}");
        }
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("3600").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(
            parse_duration("30d").unwrap(),
            Duration::from_secs(2_592_000)
        );
        for bad in ["", "0", "12x", "h", "-1h", "1.5h"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }
}

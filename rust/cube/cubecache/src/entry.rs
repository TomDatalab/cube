//! The cached query result and the decisions `QueryCache` makes about it.
//!
//! Port of `QueryCache.decideCacheAction` / `QueryCache.isMemoryEntryUsable`
//! (`QO/QueryCache.ts:1039-1091`) and the `CacheEntry` record they operate on.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `QueryCache.IN_MEMORY_CACHE_DISABLE_PERIOD` — 5 minutes (`QO/QueryCache.ts:203`).
pub const IN_MEMORY_CACHE_DISABLE_PERIOD_MS: i64 = 5 * 60 * 1000;

/// What a cache driver stores for a query result (`QO/QueryCache.ts:150-155`).
///
/// Field order matches the JavaScript object literal so that a serialized entry is byte
/// identical to the one Node writes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Epoch milliseconds of the last renewal. `0` stands for the absent `time` of the
    /// Node code, which counts as expired.
    pub time: i64,
    pub result: Value,
    #[serde(
        rename = "renewalKey",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub renewal_key: Option<String>,
    #[serde(rename = "requestId", default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl CacheEntry {
    pub fn new(time: i64, result: Value) -> Self {
        Self {
            time,
            result,
            renewal_key: None,
            request_id: None,
        }
    }

    #[must_use]
    pub fn with_renewal_key(mut self, renewal_key: Option<String>) -> Self {
        self.renewal_key = renewal_key;
        self
    }

    #[must_use]
    pub fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    /// `(new Date()).getTime() - entry.time`
    pub fn renewed_ago(&self, now_ms: i64) -> i64 {
        now_ms - self.time
    }
}

/// `QueryCache.CacheAction` (`QO/QueryCache.ts:157-162`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheAction {
    /// Hand the cached result back untouched.
    ServeCached,
    /// Hand the cached result back and renew it in the background because the caller is
    /// the request which wrote it (continue-wait polling).
    RefreshSameRequest,
    /// Hand the cached result back and renew it in the background.
    RefreshBackground,
    /// Block on a fresh fetch.
    WaitForRenew,
}

/// The subset of `CacheQueryResultOptions` the decision reads.
#[derive(Clone, Debug, Default)]
pub struct CacheActionOptions {
    pub renewal_threshold: Option<u64>,
    pub request_id: Option<String>,
    pub wait_for_renew: bool,
    pub renew_cycle: bool,
}

/// `requestId` without its `-span-N` suffix
/// (`packages/cubejs-backend-shared/src/request-id.ts`).
pub fn extract_request_uuid(request_id: &str) -> &str {
    match request_id.rfind("-span-") {
        Some(idx) => &request_id[..idx],
        None => request_id,
    }
}

/// JavaScript truthiness for the optional strings of the decision table: an empty string
/// is falsy there, so it has to behave like an absent value here too.
fn truthy(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

/// Port of `QueryCache.decideCacheAction` (`QO/QueryCache.ts:1039-1069`).
pub fn decide_cache_action(
    entry: &CacheEntry,
    renewed_ago_ms: i64,
    options: &CacheActionOptions,
    renewal_key: Option<&str>,
) -> CacheAction {
    let renewal_threshold = options.renewal_threshold.unwrap_or(0);
    let renewal_key = truthy(renewal_key);

    let is_expired = renewal_threshold == 0
        || entry.time == 0
        || renewed_ago_ms > renewal_threshold as i64 * 1000;
    let is_key_mismatch = renewal_key.is_some() && entry.renewal_key.as_deref() != renewal_key;

    if !is_expired && !is_key_mismatch {
        return CacheAction::ServeCached;
    }

    let is_same_request = match (
        truthy(options.request_id.as_deref()),
        truthy(entry.request_id.as_deref()),
    ) {
        (Some(requested), Some(cached)) => {
            extract_request_uuid(cached) == extract_request_uuid(requested)
        }
        _ => false,
    };

    // A client polling through continue-wait re-enters with the same requestId, so rejecting
    // the result it just wrote would restart the fetch on every poll and never converge while
    // the refreshKey keeps moving. Background renew opts out: fresh data is all it exists for.
    if is_same_request && !options.renew_cycle {
        return CacheAction::RefreshSameRequest;
    }

    // Without a refreshKey there is nothing to refresh against, so an elapsed threshold
    // alone never triggers a fetch.
    if renewal_key.is_none() {
        return CacheAction::ServeCached;
    }

    if options.wait_for_renew {
        CacheAction::WaitForRenew
    } else {
        CacheAction::RefreshBackground
    }
}

/// Port of `QueryCache.isMemoryEntryUsable` (`QO/QueryCache.ts:1071-1091`).
pub fn is_memory_entry_usable(
    entry: &CacheEntry,
    renewed_ago_ms: i64,
    expiration_secs: u64,
    renewal_threshold: Option<u64>,
    renewal_key: Option<&str>,
) -> bool {
    if renewed_ago_ms > expiration_secs as i64 * 1000
        || renewed_ago_ms > IN_MEMORY_CACHE_DISABLE_PERIOD_MS
    {
        return false;
    }

    let renewal_key = match truthy(renewal_key) {
        Some(renewal_key) => renewal_key,
        None => return true,
    };

    // Near expiry an in-memory entry races with refreshes carrying a different refreshKey value.
    let renewal_threshold = renewal_threshold.unwrap_or(0);

    renewal_threshold > 0
        && entry.time != 0
        && renewed_ago_ms + IN_MEMORY_CACHE_DISABLE_PERIOD_MS <= renewal_threshold as i64 * 1000
        && entry.renewal_key.as_deref() == Some(renewal_key)
}

/// Port of `QueryCache.storeInMemoryCache`'s guard (`QO/QueryCache.ts:1215-1221`): an entry
/// is only worth keeping in memory while it stays usable for the whole disable period.
pub fn should_store_in_memory(
    renewed_ago_ms: i64,
    use_in_memory: bool,
    renewal_threshold: Option<u64>,
) -> bool {
    let renewal_threshold = renewal_threshold.unwrap_or(0);

    use_in_memory
        && renewal_threshold > 0
        && renewed_ago_ms + IN_MEMORY_CACHE_DISABLE_PERIOD_MS <= renewal_threshold as i64 * 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(time: i64, renewal_key: Option<&str>, request_id: Option<&str>) -> CacheEntry {
        CacheEntry {
            time,
            result: Value::Null,
            renewal_key: renewal_key.map(str::to_string),
            request_id: request_id.map(str::to_string),
        }
    }

    fn options(
        renewal_threshold: Option<u64>,
        request_id: Option<&str>,
        wait_for_renew: bool,
        renew_cycle: bool,
    ) -> CacheActionOptions {
        CacheActionOptions {
            renewal_threshold,
            request_id: request_id.map(str::to_string),
            wait_for_renew,
            renew_cycle,
        }
    }

    /// name, entry, renewedAgo, options, renewalKey, expected
    type DecideCase = (
        &'static str,
        CacheEntry,
        i64,
        CacheActionOptions,
        Option<&'static str>,
        CacheAction,
    );

    /// name, entry, renewedAgo, expiration, renewalThreshold, renewalKey, expected
    type MemoryCase = (
        &'static str,
        CacheEntry,
        i64,
        u64,
        Option<u64>,
        Option<&'static str>,
        bool,
    );

    const THRESHOLD: Option<u64> = Some(600);
    const NOT_EXPIRED: i64 = 100 * 1000;
    const EXPIRED: i64 = 700 * 1000;

    /// Ported one for one from `test/unit/QueryCache.test.ts`,
    /// `describe('QueryCache.decideCacheAction')`.
    #[test]
    fn decide_cache_action_table() {
        let cases: Vec<DecideCase> = vec![
            (
                "not expired and renewal key matches: serves cache even for the same request",
                entry(1, Some("rk"), Some("req-1-span-1")),
                NOT_EXPIRED,
                options(THRESHOLD, Some("req-1-span-2"), true, false),
                Some("rk"),
                CacheAction::ServeCached,
            ),
            (
                "expired with renewal key and waitForRenew: blocks on fetch",
                entry(1, Some("rk"), Some("req-1")),
                EXPIRED,
                options(THRESHOLD, Some("req-2"), true, false),
                Some("rk"),
                CacheAction::WaitForRenew,
            ),
            (
                "expired with renewal key without waitForRenew: refreshes in background",
                entry(1, Some("rk"), Some("req-1")),
                EXPIRED,
                options(THRESHOLD, Some("req-2"), false, false),
                Some("rk"),
                CacheAction::RefreshBackground,
            ),
            (
                "renewal key mismatch while not expired and waitForRenew: blocks on fetch",
                entry(1, Some("old"), Some("req-1")),
                NOT_EXPIRED,
                options(THRESHOLD, Some("req-2"), true, false),
                Some("new"),
                CacheAction::WaitForRenew,
            ),
            (
                "renewal key mismatch while not expired without waitForRenew: refreshes in background",
                entry(1, Some("old"), Some("req-1")),
                NOT_EXPIRED,
                options(THRESHOLD, Some("req-2"), false, false),
                Some("new"),
                CacheAction::RefreshBackground,
            ),
            (
                "same request (different span) and expired: serves stale, refreshes in background",
                entry(1, Some("rk"), Some("req-1-span-1")),
                EXPIRED,
                options(THRESHOLD, Some("req-1-span-7"), true, false),
                Some("rk"),
                CacheAction::RefreshSameRequest,
            ),
            (
                "same request and renewal key mismatch: serves stale, refreshes in background",
                entry(1, Some("old"), Some("req-1-span-1")),
                NOT_EXPIRED,
                options(THRESHOLD, Some("req-1-span-7"), true, false),
                Some("new"),
                CacheAction::RefreshSameRequest,
            ),
            (
                "renew cycle never serves stale: expired same request blocks on fetch",
                entry(1, Some("rk"), Some("req-1-span-1")),
                EXPIRED,
                options(THRESHOLD, Some("req-1-span-7"), true, true),
                Some("rk"),
                CacheAction::WaitForRenew,
            ),
            (
                "renew cycle never serves stale: key mismatch without waitForRenew refreshes in background",
                entry(1, Some("old"), Some("req-1-span-1")),
                NOT_EXPIRED,
                options(THRESHOLD, Some("req-1-span-7"), false, true),
                Some("new"),
                CacheAction::RefreshBackground,
            ),
            (
                "expired without a renewal key: keeps serving cache",
                entry(1, None, Some("req-1")),
                EXPIRED,
                options(THRESHOLD, Some("req-2"), true, false),
                None,
                CacheAction::ServeCached,
            ),
            (
                "expired without a renewal key but same request: refreshes in background",
                entry(1, None, Some("req-1-span-1")),
                EXPIRED,
                options(THRESHOLD, Some("req-1-span-7"), true, false),
                None,
                CacheAction::RefreshSameRequest,
            ),
            (
                "missing renewal threshold counts as expired",
                entry(1, Some("rk"), Some("req-1")),
                0,
                options(None, Some("req-2"), true, false),
                Some("rk"),
                CacheAction::WaitForRenew,
            ),
            (
                "entry without a timestamp counts as expired",
                entry(0, Some("rk"), Some("req-1")),
                0,
                options(THRESHOLD, Some("req-2"), false, false),
                Some("rk"),
                CacheAction::RefreshBackground,
            ),
            (
                "entry without a request id is never treated as the same request",
                entry(1, Some("rk"), None),
                EXPIRED,
                options(THRESHOLD, Some("req-1"), true, false),
                Some("rk"),
                CacheAction::WaitForRenew,
            ),
        ];

        for (name, entry, renewed_ago, options, renewal_key, expected) in cases {
            assert_eq!(
                decide_cache_action(&entry, renewed_ago, &options, renewal_key),
                expected,
                "{name}"
            );
        }
    }

    /// Ported one for one from `test/unit/QueryCache.test.ts`,
    /// `describe('QueryCache.isMemoryEntryUsable')`.
    #[test]
    fn is_memory_entry_usable_table() {
        const WINDOW: i64 = IN_MEMORY_CACHE_DISABLE_PERIOD_MS;
        const HOUR: u64 = 3600;

        let cases: Vec<MemoryCase> = vec![
            (
                "past expiration but still inside the in-memory window: unusable",
                entry(1, None, None),
                61 * 1000,
                60,
                None,
                None,
                false,
            ),
            (
                "past the in-memory window but nowhere near expiration: unusable",
                entry(1, None, None),
                WINDOW + 1,
                HOUR,
                None,
                None,
                false,
            ),
            (
                "exactly at the in-memory window: usable",
                entry(1, None, None),
                WINDOW,
                HOUR,
                None,
                None,
                true,
            ),
            (
                "exactly at expiration: usable",
                entry(1, None, None),
                60 * 1000,
                60,
                None,
                None,
                true,
            ),
            (
                "without a renewal key the renewal checks are skipped",
                entry(0, None, None),
                1000,
                HOUR,
                None,
                None,
                true,
            ),
            (
                "renewal key without a renewal threshold: unusable",
                entry(1, Some("rk"), None),
                1000,
                HOUR,
                None,
                Some("rk"),
                false,
            ),
            (
                "renewal key with an entry that has no timestamp: unusable",
                entry(0, Some("rk"), None),
                1000,
                HOUR,
                Some(HOUR),
                Some("rk"),
                false,
            ),
            (
                "threshold below twice the window, entry inside the last window before expiry: unusable",
                entry(1, Some("rk"), None),
                100 * 1000,
                HOUR,
                Some(300),
                Some("rk"),
                false,
            ),
            (
                "threshold below twice the window, entry exactly at the last window before expiry: usable",
                entry(1, Some("rk"), None),
                100 * 1000,
                HOUR,
                Some(400),
                Some("rk"),
                true,
            ),
            (
                "renewal key mismatch: unusable",
                entry(1, Some("old"), None),
                1000,
                HOUR,
                Some(HOUR),
                Some("new"),
                false,
            ),
            (
                "every check satisfied: usable",
                entry(1, Some("rk"), None),
                1000,
                HOUR,
                Some(HOUR),
                Some("rk"),
                true,
            ),
        ];

        for (name, entry, renewed_ago, expiration, renewal_threshold, renewal_key, expected) in
            cases
        {
            assert_eq!(
                is_memory_entry_usable(
                    &entry,
                    renewed_ago,
                    expiration,
                    renewal_threshold,
                    renewal_key
                ),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn extract_request_uuid_strips_the_span_suffix() {
        assert_eq!(extract_request_uuid("req-1-span-7"), "req-1");
        assert_eq!(extract_request_uuid("req-1-span-7-span-2"), "req-1-span-7");
        assert_eq!(extract_request_uuid("req-1"), "req-1");
        assert_eq!(extract_request_uuid(""), "");
    }

    #[test]
    fn should_store_in_memory_follows_the_disable_period() {
        assert!(!should_store_in_memory(0, false, Some(3600)));
        assert!(!should_store_in_memory(0, true, None));
        assert!(should_store_in_memory(0, true, Some(3600)));
        // renewedAgo + 5 min has to stay inside the threshold
        assert!(should_store_in_memory(100 * 1000, true, Some(400)));
        assert!(!should_store_in_memory(100 * 1000, true, Some(300)));
    }

    #[test]
    fn cache_entry_serializes_like_the_node_literal() {
        let entry = CacheEntry::new(1700000000000, serde_json::json!([{ "a": 1 }]))
            .with_renewal_key(Some("rk".into()))
            .with_request_id(Some("req-1".into()));

        assert_eq!(
            serde_json::to_string(&entry).unwrap(),
            r#"{"time":1700000000000,"result":[{"a":1}],"renewalKey":"rk","requestId":"req-1"}"#
        );

        // absent optionals are omitted, exactly as `JSON.stringify` drops `undefined`
        let bare = CacheEntry::new(1, Value::Null);
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#"{"time":1,"result":null}"#
        );
        assert_eq!(
            serde_json::from_str::<CacheEntry>(r#"{"time":1,"result":null}"#).unwrap(),
            bare
        );
    }
}

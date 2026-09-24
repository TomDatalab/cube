//! Port of `canonicalTimezone` from `@cubejs-backend/shared`.
//!
//! `moment.tz.zone(name)` resolves IANA names case-insensitively and returns
//! the canonical spelling (link names keep their own spelling, e.g.
//! `US/Pacific`). `chrono-tz` ships the same database including backward
//! links, so a case-insensitive lookup over `TZ_VARIANTS` reproduces it.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono_tz::{Tz, TZ_VARIANTS};

static ZONES: LazyLock<HashMap<String, Tz>> = LazyLock::new(|| {
    TZ_VARIANTS
        .iter()
        .map(|tz| (tz.name().to_ascii_lowercase(), *tz))
        .collect()
});

/// Resolve an IANA time zone name case-insensitively.
pub fn find_timezone(value: &str) -> Option<Tz> {
    ZONES.get(&value.to_ascii_lowercase()).copied()
}

/// Resolve an IANA time zone name case-insensitively and return its canonical
/// spelling, or `None` when the name is unknown. Empty input yields `None`
/// (the Node.js version throws a `TypeError` for it; callers in this crate
/// never pass empty strings because an empty `timezone` falls back to the
/// default before validation).
pub fn canonical_timezone(value: &str) -> Option<String> {
    find_timezone(value).map(|tz| tz.name().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_case_insensitively() {
        assert_eq!(
            canonical_timezone("america/new_york").as_deref(),
            Some("America/New_York")
        );
        assert_eq!(
            canonical_timezone("AMERICA/NEW_YORK").as_deref(),
            Some("America/New_York")
        );
        assert_eq!(canonical_timezone("utc").as_deref(), Some("UTC"));
        assert_eq!(canonical_timezone("uTc").as_deref(), Some("UTC"));
        assert_eq!(
            canonical_timezone("Europe/Berlin").as_deref(),
            Some("Europe/Berlin")
        );
    }

    #[test]
    fn rejects_unknown_zones() {
        assert_eq!(canonical_timezone("Not/AZone"), None);
        assert_eq!(canonical_timezone("+05:00"), None);
        assert_eq!(canonical_timezone("foo/bar"), None);
        assert_eq!(canonical_timezone(""), None);
    }
}

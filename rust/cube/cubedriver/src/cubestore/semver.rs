//! Port of `packages/cubejs-backend-shared/src/semver.ts`.

struct VersionPart {
    num: i64,
    pre: String,
}

/// `parseInt(segment, 10) || 0` — JS parses the leading digits and yields `0`
/// for anything that does not start with a number.
fn parse_leading_int(s: &str) -> i64 {
    let s = s.trim_start();
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, s.strip_prefix('+').unwrap_or(s)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().map(|n| sign * n).unwrap_or(0)
}

fn parse_version_parts(v: &str) -> Vec<VersionPart> {
    v.split('.')
        .map(|segment| match segment.find('-') {
            None => VersionPart {
                num: parse_leading_int(segment),
                pre: String::new(),
            },
            Some(idx) => VersionPart {
                num: parse_leading_int(&segment[..idx]),
                pre: segment[idx + 1..].to_string(),
            },
        })
        .collect()
}

/// `isVersionGte`: `version >= min_version`. A missing version is never
/// good enough (`isVersionGte(null, x) === false`).
pub fn is_version_gte(version: Option<&str>, min_version: &str) -> bool {
    let Some(version) = version.filter(|v| !v.is_empty()) else {
        return false;
    };

    let parts = parse_version_parts(version);
    let min_parts = parse_version_parts(min_version);

    for i in 0..parts.len().max(min_parts.len()) {
        let (a_num, a_pre) = parts
            .get(i)
            .map(|p| (p.num, p.pre.as_str()))
            .unwrap_or((0, ""));
        let (b_num, b_pre) = min_parts
            .get(i)
            .map(|p| (p.num, p.pre.as_str()))
            .unwrap_or((0, ""));

        if a_num > b_num {
            return true;
        }
        if a_num < b_num {
            return false;
        }
        // Same numeric part: a pre-release is less than no pre-release.
        if !a_pre.is_empty() && b_pre.is_empty() {
            return false;
        }
        if a_pre.is_empty() && !b_pre.is_empty() {
            return true;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_version_gte(Some("1.6.38"), "1.6.38"));
        assert!(is_version_gte(Some("1.6.39"), "1.6.38"));
        assert!(is_version_gte(Some("1.7.0"), "1.6.38"));
        assert!(is_version_gte(Some("2.0.0"), "1.6.38"));
        assert!(!is_version_gte(Some("1.6.37"), "1.6.38"));
        assert!(!is_version_gte(Some("1.5.99"), "1.6.38"));
        assert!(!is_version_gte(Some("0.0.0"), "1.6.38"));
        assert!(!is_version_gte(None, "1.6.38"));
        assert!(!is_version_gte(Some(""), "1.6.38"));
        // more/less segments
        assert!(is_version_gte(Some("1.6.38.1"), "1.6.38"));
        assert!(is_version_gte(Some("1.7"), "1.6.38"));
        assert!(!is_version_gte(Some("1.6"), "1.6.38"));
        // pre-releases
        assert!(!is_version_gte(Some("1.6.38-alpha"), "1.6.38"));
        assert!(is_version_gte(Some("1.6.38"), "1.6.38-alpha"));
        // garbage parses as 0
        assert!(!is_version_gte(Some("dev"), "1.6.38"));
    }
}

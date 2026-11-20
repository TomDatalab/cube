//! Organization slugs: the fixed short identifier used as the organization's
//! folder name. Format `[a-z0-9][a-z0-9-]{0,62}`; values starting with `_`
//! or `.` (and so `_platform`) are reserved.

use unicode_normalization::UnicodeNormalization;

const MAX_LEN: usize = 63;

/// A slug from a display name: accents stripped, lower case, runs of other
/// characters collapsed into one `-`. Never empty (`org` as a fallback).
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in name.nfd().filter(|c| c.is_ascii() || !is_combining(*c)) {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    out.truncate(MAX_LEN);
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "org".into()
    } else {
        out
    }
}

fn is_combining(c: char) -> bool {
    ('\u{0300}'..='\u{036f}').contains(&c)
}

/// `Ok` when `s` is a valid, non-reserved slug.
pub fn validate_slug(s: &str) -> Result<(), String> {
    if s.starts_with('_') || s.starts_with('.') {
        return Err("slug values starting with '_' or '.' are reserved".into());
    }
    let ok = !s.is_empty()
        && s.len() <= MAX_LEN
        && s.bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && b == b'-'));
    if ok {
        Ok(())
    } else {
        Err("slug must match [a-z0-9][a-z0-9-]{0,62}".into())
    }
}

/// `base`, else `base-2`, `base-3`, ... the first one `taken` does not claim.
/// The result stays within the length limit.
pub fn unique_slug(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    for n in 2.. {
        let suffix = format!("-{n}");
        let mut stem = base.to_string();
        stem.truncate(MAX_LEN - suffix.len());
        while stem.ends_with('-') {
            stem.pop();
        }
        let candidate = format!("{stem}{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_names() {
        assert_eq!(slugify("Digital"), "digital");
        assert_eq!(slugify("ON"), "on");
        assert_eq!(slugify("Ação & Cia"), "acao-cia");
        assert_eq!(slugify("  Foo__Bar  "), "foo-bar");
        assert_eq!(slugify("!!!"), "org");
        let long = slugify(&format!("{} {}", "a".repeat(62), "b".repeat(38)));
        assert_eq!(long.len(), 62);
        let long = slugify(&"ab ".repeat(34));
        assert!(long.len() <= 63 && !long.ends_with('-'), "{long}");
        assert_eq!(slugify(&"a".repeat(100)).len(), 63);
    }

    #[test]
    fn validate() {
        for bad in ["_platform", "_x", ".x", "A", "", "-a"] {
            assert!(validate_slug(bad).is_err(), "{bad}");
        }
        for good in ["on", "digital-2"] {
            assert!(validate_slug(good).is_ok(), "{good}");
        }
        assert!(validate_slug(&"a".repeat(64)).is_err());
    }

    #[test]
    fn unique() {
        let taken = |s: &str| s == "digital" || s == "digital-2";
        assert_eq!(unique_slug("digital", &taken), "digital-3");
        assert_eq!(unique_slug("free", &taken), "free");
    }
}

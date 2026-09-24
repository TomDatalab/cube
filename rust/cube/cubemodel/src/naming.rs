//! Ports of the JS string helpers the Node.js schema compiler relies on.
//!
//! * [`camelize_lower`] mirrors `inflection.camelize(str, true)`
//! * [`underscore`] mirrors `inflection.underscore(str)`
//! * [`titleize`] mirrors `inflection.titleize(str)`
//! * [`camel_case_pascal`] mirrors `camelcase(str, { pascalCase: true })`
//! * [`member_title`] mirrors `CubeToMetaTransformer.titleize`

const NON_TITLECASED_WORDS: &[&str] = &[
    "and", "or", "nor", "a", "an", "the", "so", "but", "to", "of", "at", "by", "from", "into",
    "on", "onto", "off", "out", "in", "over", "with", "for",
];

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `inflection.camelize(str, true)`.
pub fn camelize_lower(input: &str) -> String {
    let segments: Vec<String> = input
        .split('/')
        .enumerate()
        .map(|(i, segment)| {
            segment
                .split('_')
                .enumerate()
                .map(|(k, part)| {
                    let part = if k != 0 {
                        part.to_lowercase()
                    } else {
                        part.to_string()
                    };
                    if i == 0 && k == 0 {
                        lower_first(&part)
                    } else {
                        upper_first(&part)
                    }
                })
                .collect::<String>()
        })
        .collect();

    segments.join("::")
}

/// `inflection.underscore(str)`.
pub fn underscore(input: &str) -> String {
    input
        .split("::")
        .map(|segment| {
            let mut out = String::with_capacity(segment.len() + 4);
            for c in segment.chars() {
                if c.is_ascii_uppercase() {
                    out.push('_');
                }
                out.push(c);
            }
            out.strip_prefix('_').map(str::to_string).unwrap_or(out)
        })
        .collect::<Vec<_>>()
        .join("/")
        .to_lowercase()
}

/// `inflection.capitalize(str)`.
fn capitalize(input: &str) -> String {
    upper_first(&input.to_lowercase())
}

/// `inflection.titleize(str)`.
pub fn titleize(input: &str) -> String {
    let lowered = input.to_lowercase().replace('_', " ");
    let joined = lowered
        .split(' ')
        .map(|word| {
            word.split('-')
                .map(|part| {
                    if NON_TITLECASED_WORDS.contains(&part.to_lowercase().as_str()) {
                        part.to_string()
                    } else {
                        capitalize(part)
                    }
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ");

    upper_first(&joined)
}

/// The `preserveCamelCase` pass of the `camelcase` npm package.
fn preserve_camel_case(input: &str) -> String {
    let mut chars: Vec<char> = input.chars().collect();
    let mut is_last_char_lower = false;
    let mut is_last_char_upper = false;
    let mut is_last_last_char_upper = false;

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let is_upper = c.is_ascii_alphabetic() && c.is_ascii_uppercase();
        let is_lower = c.is_ascii_alphabetic() && c.is_ascii_lowercase();

        if is_last_char_lower && is_upper {
            chars.insert(i, '-');
            is_last_char_lower = false;
            is_last_last_char_upper = is_last_char_upper;
            is_last_char_upper = true;
            i += 1;
        } else if is_last_char_upper && is_last_last_char_upper && is_lower {
            chars.insert(i - 1, '-');
            is_last_last_char_upper = is_last_char_upper;
            is_last_char_upper = false;
            is_last_char_lower = true;
        } else {
            is_last_char_lower =
                c.to_lowercase().eq(std::iter::once(c)) && !c.to_uppercase().eq(std::iter::once(c));
            is_last_last_char_upper = is_last_char_upper;
            is_last_char_upper =
                c.to_uppercase().eq(std::iter::once(c)) && !c.to_lowercase().eq(std::iter::once(c));
        }
        i += 1;
    }

    chars.into_iter().collect()
}

const CAMEL_SEPARATORS: &[char] = &['_', '.', '-', ' '];

/// `camelcase(str, { pascalCase: true })`.
pub fn camel_case_pascal(input: &str) -> String {
    let input = input.trim();

    if input.is_empty() {
        return String::new();
    }
    if input.chars().count() == 1 {
        return input.to_uppercase();
    }

    let has_upper_case = input != input.to_lowercase();
    let staged = if has_upper_case {
        preserve_camel_case(input)
    } else {
        input.to_string()
    };

    let staged = staged.trim_start_matches(CAMEL_SEPARATORS).to_lowercase();

    // .replace(/[_.\- ]+(\w|$)/g, (_, p1) => p1.toUpperCase())
    let mut out = String::with_capacity(staged.len());
    let chars: Vec<char> = staged.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if CAMEL_SEPARATORS.contains(&chars[i]) {
            while i < chars.len() && CAMEL_SEPARATORS.contains(&chars[i]) {
                i += 1;
            }
            if i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                out.extend(chars[i].to_uppercase());
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }

    // .replace(/\d+(\w|$)/g, m => m.toUpperCase())
    let chars: Vec<char> = out.chars().collect();
    let mut result = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            while i < chars.len() && chars[i].is_ascii_digit() {
                result.push(chars[i]);
                i += 1;
            }
            if i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                result.extend(chars[i].to_uppercase());
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    upper_first(&result)
}

/// `CubeToMetaTransformer.titleize`: titleize(underscore(pascalCase(name))) with
/// the `Id`/`Ids` -> `ID`/`IDs` acronym fix-up.
pub fn member_title(name: &str) -> String {
    let titleized = titleize(&underscore(&camel_case_pascal(name)));
    fix_id_acronym(&titleized)
}

fn fix_id_acronym(input: &str) -> String {
    // Equivalent of `.replace(/\bId(s?)\b/g, (_m, plural) => `ID${plural}`)`
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while i < chars.len() {
        let boundary_before = i == 0 || !is_word(chars[i - 1]);
        if boundary_before && chars[i] == 'I' && i + 1 < chars.len() && chars[i + 1] == 'd' {
            let plural = i + 2 < chars.len() && chars[i + 2] == 's';
            let end = if plural { i + 3 } else { i + 2 };
            let boundary_after = end >= chars.len() || !is_word(chars[end]);
            if boundary_after {
                out.push_str(if plural { "IDs" } else { "ID" });
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelize_matches_inflection() {
        assert_eq!(camelize_lower("primary_key"), "primaryKey");
        assert_eq!(camelize_lower("primaryKey"), "primaryKey");
        assert_eq!(camelize_lower("sql_table"), "sqlTable");
        assert_eq!(camelize_lower("join_path"), "joinPath");
        assert_eq!(camelize_lower("sql"), "sql");
        assert_eq!(camelize_lower("pre_aggregations"), "preAggregations");
        assert_eq!(camelize_lower("many_to_one"), "manyToOne");
    }

    #[test]
    fn titleize_matches_inflection() {
        assert_eq!(member_title("order_date"), "Order Date");
        assert_eq!(member_title("taxful_total_price"), "Taxful Total Price");
        assert_eq!(member_title("count"), "Count");
        assert_eq!(member_title("id"), "ID");
        assert_eq!(member_title("user_id"), "User ID");
        assert_eq!(member_title("userId"), "User ID");
        assert_eq!(
            member_title("KibanaSampleDataEcommerce"),
            "Kibana Sample Data Ecommerce"
        );
        assert_eq!(member_title("orders_hierarchy"), "Orders Hierarchy");
    }
}

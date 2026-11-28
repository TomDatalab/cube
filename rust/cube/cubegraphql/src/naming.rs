//! The small name helpers from `graphql.ts`: `safeName`, `objectName`,
//! `capitalize` and `unCapitalize`.

/// `name.split('.').slice(1).join('')` — `Orders.created_at` -> `created_at`.
///
/// Note that this deliberately does *not* camelize: a snake_case model keeps
/// snake_case GraphQL field names, which is what the `base-snake-case.gql`
/// fixtures rely on.
pub fn safe_name(name: &str) -> String {
    let mut parts = name.split('.');
    parts.next();
    parts.collect::<Vec<_>>().join("")
}

/// The GraphQL field a member is queried by: its name when that is already a
/// GraphQL name, else its words in lower camel case, accents stripped —
/// `Quantidade Total` -> `quantidadeTotal`, `Data de Fabricação` ->
/// `dataDeFabricacao`. A leading digit takes a `_`.
pub fn graphql_field(member: &str) -> String {
    if is_graphql_name(member) {
        return member.to_string();
    }
    use unicode_normalization::UnicodeNormalization;
    let folded: String = member
        .nfd()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .collect();
    let mut out = String::new();
    for (i, word) in folded
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .enumerate()
    {
        if i == 0 {
            out.push_str(&un_capitalize(word));
        } else {
            out.push_str(&capitalize(word));
        }
    }
    match out.chars().next() {
        None => "_".to_string(),
        Some(c) if c.is_ascii_digit() => format!("_{out}"),
        Some(_) => out,
    }
}

/// `/^[_A-Za-z][_0-9A-Za-z]*$/` — the GraphQL name grammar.
pub fn is_graphql_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {
            chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

/// `inflection.camelize(name, false)`.
///
/// The JS implementation lowercases the whole string first, so `OrdersView`
/// becomes `Ordersview` and `created_at` becomes `CreatedAt`. We reproduce that
/// exactly, including the `/` -> `::` path handling, so generated type names
/// match the Node.js gateway byte for byte.
pub fn object_name(name: &str) -> String {
    camelize(name, false)
}

/// `inflection.camelize`.
pub fn camelize(name: &str, low_first_letter: bool) -> String {
    let lowered = name.to_lowercase();
    let segments: Vec<&str> = lowered.split('/').collect();
    let segment_count = segments.len();

    let out: Vec<String> = segments
        .into_iter()
        .enumerate()
        .map(|(i, segment)| {
            let init_x = usize::from(low_first_letter && i + 1 == segment_count);
            segment
                .split('_')
                .enumerate()
                .map(|(x, part)| {
                    if x >= init_x {
                        capitalize(part)
                    } else {
                        part.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .collect();

    out.join("::")
}

/// `${name[0].toUpperCase()}${name.slice(1)}`.
pub fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `${name[0].toLowerCase()}${name.slice(1)}`.
pub fn un_capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphql_field_of_a_friendly_name() {
        assert_eq!(graphql_field("created_at"), "created_at");
        assert_eq!(graphql_field("Quantidade Total"), "quantidadeTotal");
        assert_eq!(graphql_field("Data de Fabricação"), "dataDeFabricacao");
        assert_eq!(graphql_field("1ª conexão"), "_1Conexao");
    }

    #[test]
    fn safe_name_strips_the_cube_prefix() {
        assert_eq!(safe_name("Orders.createdAt"), "createdAt");
        assert_eq!(safe_name("orders.created_at"), "created_at");
        assert_eq!(safe_name("Orders"), "");
    }

    #[test]
    fn object_name_matches_inflection_camelize() {
        assert_eq!(object_name("Orders"), "Orders");
        assert_eq!(object_name("orders"), "Orders");
        assert_eq!(object_name("orders_view"), "OrdersView");
        // inflection lowercases first, so an already camelCased name collapses.
        assert_eq!(object_name("OrdersView"), "Ordersview");
    }

    #[test]
    fn capitalization_helpers() {
        assert_eq!(capitalize("orders"), "Orders");
        assert_eq!(un_capitalize("Orders"), "orders");
        assert_eq!(capitalize(""), "");
        assert_eq!(un_capitalize(""), "");
    }
}

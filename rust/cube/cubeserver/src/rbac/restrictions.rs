//! Data restrictions of an organization role: columns it blocks and row
//! filters driven by user attributes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ApiError;

const MAX_ROW_FILTERS: usize = 50;
const MAX_BLOCKED_MEMBERS: usize = 500;
const MAX_ATTRIBUTES: usize = 50;
const MAX_VALUES: usize = 100;
const MAX_VALUE_LEN: usize = 200;
const MAX_NAME_LEN: usize = 64;

/// What a role hides from its holders. Empty means no restriction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Restrictions {
    #[serde(default)]
    pub row_filters: Vec<RowFilter>,
    #[serde(default)]
    pub blocked_members: Vec<String>,
}

/// Keeps only the rows where `member` matches the values from exactly one
/// source: the user's `attribute`, or the fixed `values`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowFilter {
    pub member: String,
    pub operator: RowOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RowOperator {
    Equals,
    NotEquals,
}

/// A user's attributes: name to values.
pub type Attributes = BTreeMap<String, Vec<String>>;

impl Restrictions {
    /// Validates (400 on failure) and normalizes: `blocked_members` come out
    /// without repeats, in order.
    pub fn validated(mut self) -> Result<Restrictions, ApiError> {
        if self.row_filters.len() > MAX_ROW_FILTERS {
            return Err(ApiError::bad_request(format!(
                "at most {MAX_ROW_FILTERS} row filters"
            )));
        }
        if self.blocked_members.len() > MAX_BLOCKED_MEMBERS {
            return Err(ApiError::bad_request(format!(
                "at most {MAX_BLOCKED_MEMBERS} blocked members"
            )));
        }
        for filter in &self.row_filters {
            validate_member(&filter.member)?;
            match (&filter.attribute, &filter.values) {
                (Some(attribute), None) => validate_attribute_name(attribute)?,
                (None, Some(values)) => {
                    if values.is_empty() {
                        return Err(ApiError::bad_request(
                            "a row filter needs at least one value",
                        ));
                    }
                    validate_values(values)?;
                }
                _ => {
                    return Err(ApiError::bad_request(
                        "a row filter takes exactly one of attribute and values",
                    ))
                }
            }
        }
        for member in &self.blocked_members {
            validate_member(member)?;
        }
        self.blocked_members.sort();
        self.blocked_members.dedup();
        Ok(self)
    }

    pub fn is_empty(&self) -> bool {
        self.row_filters.is_empty() && self.blocked_members.is_empty()
    }
}

/// `cube.member`, each part made of `[A-Za-z0-9_]`.
fn validate_member(member: &str) -> Result<(), ApiError> {
    let ok = member.split_once('.').is_some_and(|(cube, name)| {
        let part =
            |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        part(cube) && part(name)
    });
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "invalid member `{member}`: expected cube.member"
        )))
    }
}

fn validate_values(values: &[String]) -> Result<(), ApiError> {
    if values.len() > MAX_VALUES {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_VALUES} values"
        )));
    }
    if values.iter().any(|v| v.chars().count() > MAX_VALUE_LEN) {
        return Err(ApiError::bad_request(format!(
            "values have at most {MAX_VALUE_LEN} characters"
        )));
    }
    Ok(())
}

/// `a-z`, `0-9` and `_`, 1 to 64 characters.
pub fn validate_attribute_name(name: &str) -> Result<(), ApiError> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "invalid attribute name `{name}`: use a-z, 0-9 and _, up to {MAX_NAME_LEN} characters"
        )))
    }
}

/// Checks names, counts and value sizes. An empty value list is allowed.
pub fn validated_attributes(attributes: Attributes) -> Result<Attributes, ApiError> {
    if attributes.len() > MAX_ATTRIBUTES {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_ATTRIBUTES} attributes"
        )));
    }
    for (name, values) in &attributes {
        validate_attribute_name(name)?;
        validate_values(values)?;
    }
    Ok(attributes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn parse(v: Value) -> Result<Restrictions, serde_json::Error> {
        serde_json::from_value(v)
    }

    fn filter(member: &str, attribute: Option<&str>, values: Option<Vec<&str>>) -> RowFilter {
        RowFilter {
            member: member.into(),
            operator: RowOperator::Equals,
            attribute: attribute.map(Into::into),
            values: values.map(|v| v.into_iter().map(Into::into).collect()),
        }
    }

    fn with_filter(f: RowFilter) -> Restrictions {
        Restrictions {
            row_filters: vec![f],
            blocked_members: vec![],
        }
    }

    #[test]
    fn restrictions_accept_the_spec_example() {
        let r = parse(json!({
            "rowFilters": [
                { "member": "orders.region", "operator": "equals", "attribute": "region" },
                { "member": "orders.status", "operator": "notEquals", "values": ["internal"] }
            ],
            "blockedMembers": ["orders.margin", "customers.cpf"]
        }))
        .unwrap();
        assert_eq!(r.row_filters[1].operator, RowOperator::NotEquals);
        let r = r.validated().unwrap();
        assert_eq!(r.row_filters.len(), 2);
        assert!(!r.is_empty());
        assert!(Restrictions::default().is_empty());
    }

    #[test]
    fn a_filter_takes_exactly_one_value_source() {
        assert!(with_filter(filter("a.b", Some("x"), Some(vec!["1"])))
            .validated()
            .is_err());
        assert!(with_filter(filter("a.b", None, None)).validated().is_err());
        assert!(with_filter(filter("a.b", None, Some(vec![])))
            .validated()
            .is_err());
        assert!(with_filter(filter("a.b", Some("x"), None))
            .validated()
            .is_ok());
        assert!(with_filter(filter("a.b", None, Some(vec!["1"])))
            .validated()
            .is_ok());
    }

    #[test]
    fn members_must_be_cube_dot_member() {
        for bad in ["orders", "orders.", "a.b.c", "or ders.x"] {
            let blocked = Restrictions {
                row_filters: vec![],
                blocked_members: vec![bad.into()],
            };
            assert!(blocked.validated().is_err(), "{bad}");
            assert!(
                with_filter(filter(bad, Some("x"), None))
                    .validated()
                    .is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn unknown_operators_and_fields_are_refused() {
        assert!(parse(json!({"rowFilters": [
            {"member": "a.b", "operator": "gt", "attribute": "x"}
        ]}))
        .is_err());
        assert!(parse(json!({"rowFilters": [
            {"member": "a.b", "operator": "equals", "attribute": "x", "extra": 1}
        ]}))
        .is_err());
        assert!(parse(json!({"other": []})).is_err());
    }

    #[test]
    fn blocked_members_are_deduplicated() {
        let r = Restrictions {
            row_filters: vec![],
            blocked_members: vec!["b.y".into(), "a.x".into(), "b.y".into()],
        }
        .validated()
        .unwrap();
        assert_eq!(r.blocked_members, vec!["a.x", "b.y"]);
    }

    #[test]
    fn limits_are_enforced() {
        let many = Restrictions {
            row_filters: (0..51).map(|_| filter("a.b", Some("x"), None)).collect(),
            blocked_members: vec![],
        };
        assert!(many.validated().is_err());
        let ok = Restrictions {
            row_filters: (0..50).map(|_| filter("a.b", Some("x"), None)).collect(),
            blocked_members: (0..500).map(|i| format!("a.m{i}")).collect(),
        };
        assert!(ok.validated().is_ok());
        let blocked = Restrictions {
            row_filters: vec![],
            blocked_members: (0..501).map(|i| format!("a.m{i}")).collect(),
        };
        assert!(blocked.validated().is_err());
    }

    #[test]
    fn attribute_names_follow_the_identifier_rule() {
        assert!(validate_attribute_name("Region").is_err());
        assert!(validate_attribute_name("a-b").is_err());
        assert!(validate_attribute_name("").is_err());
        assert!(validate_attribute_name(&"a".repeat(65)).is_err());
        assert!(validate_attribute_name("cost_center").is_ok());
        assert!(validate_attribute_name(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn attribute_values_are_bounded() {
        let one = |values: Vec<String>| Attributes::from([("region".to_string(), values)]);
        assert!(validated_attributes(one((0..101).map(|i| i.to_string()).collect())).is_err());
        assert!(validated_attributes(one((0..100).map(|i| i.to_string()).collect())).is_ok());
        assert!(validated_attributes(one(vec!["x".repeat(201)])).is_err());
        assert!(validated_attributes(one(vec![])).is_ok());
        let many: Attributes = (0..51).map(|i| (format!("a{i}"), vec![])).collect();
        assert!(validated_attributes(many).is_err());
        assert!(validated_attributes(Attributes::from([("Bad".to_string(), vec![])])).is_err());
    }
}

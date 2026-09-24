//! Member expressions: the SQL API's push-down members.
//!
//! When cubesql pushes a projection into Cube it cannot always name a model
//! member, so it sends the expression itself. The gateway turns the JSON
//! cubesql emits into the shape the planner reads
//! (`gateway.ts:1657-1700` — `parseMemberExpression`), and this module
//! reproduces both ends of that in Rust, because the Rust backend has no
//! gateway JavaScript to do it.
//!
//! Two shapes are accepted:
//!
//! - the *parsed* shape the planner consumes —
//!   `{expression, cubeName, name, expressionName, definition}`, where
//!   `expression` is either the JS function body cubesql's `SqlFunction`
//!   becomes (`[...cubeParams, "return `<sql>`"]`) or a `PatchMeasure` object;
//! - the *input* shape cubesql itself emits — `{cubeName, alias, expr}` with
//!   `expr` tagged `SqlFunction` or `PatchMeasure`, which `parseMemberExpression`
//!   converts to the first.
//!
//! The SQL inside is a JS template literal (`${orders.status}`), which the
//! member-sql parser reads as an alias of its own `{…}` interpolation, so no
//! JavaScript is evaluated to get at it.

use crate::error::PlannerError;
use cubesqlplanner::cube_bridge::member_expression::{
    ExpressionStruct, MemberExpressionDefinition, MemberExpressionExpressionDef,
};
use cubesqlplanner::cube_bridge::member_sql::MemberSql;
use cubesqlplanner::cube_bridge::options_member::OptionsMember;
use cubesqlplanner::rust_model::{
    MockExpressionStruct, MockMemberExpressionDefinition, MockMemberSql, MockStructWithSqlMember,
};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::rc::Rc;

/// One entry of `measures` / `dimensions` / `segments`: a member name, or a
/// member expression.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum QueryMember {
    Name(String),
    Expression(Box<MemberExpression>),
}

/// Hand-written so a malformed member expression is reported by what is wrong
/// with it, instead of serde's "data did not match any variant".
impl<'de> Deserialize<'de> for QueryMember {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(name) => Ok(Self::Name(name)),
            value @ Value::Object(_) => Ok(Self::Expression(Box::new(
                MemberExpression::from_value(value).map_err(D::Error::custom)?,
            ))),
            other => Err(D::Error::custom(format!(
                "a member must be a member name or a member expression, got `{other}`"
            ))),
        }
    }
}

impl QueryMember {
    /// The name this member is addressed by in `order`, `filters` and
    /// `memberToAlias`.
    pub fn name(&self) -> String {
        match self {
            Self::Name(name) => name.clone(),
            Self::Expression(expression) => expression.member_name(),
        }
    }

    pub(crate) fn to_options_member(&self) -> Result<OptionsMember, PlannerError> {
        match self {
            Self::Name(name) => Ok(OptionsMember::MemberName(name.clone())),
            Self::Expression(expression) => expression.to_options_member(),
        }
    }
}

impl From<&str> for QueryMember {
    fn from(name: &str) -> Self {
        Self::Name(name.to_string())
    }
}

/// A member expression, in either of the two shapes described above.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MemberExpression {
    /// `{expression, cubeName, name, expressionName, definition}`.
    Parsed(ParsedMemberExpression),
    /// `{cubeName, alias, expr}` — what cubesql emits.
    Input(InputMemberExpression),
}

/// The shape `BaseQueryOptions` reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedMemberExpression {
    pub expression: Expression,
    pub cube_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub expression_name: Option<String>,
    /// The member's source text, kept for error messages and for
    /// `exportAnnotatedSql`.
    #[serde(default)]
    pub definition: Option<String>,
}

/// The shape cubesql emits (`compile/engine/df/wrapper.rs:95-108`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputMemberExpression {
    pub cube_name: String,
    pub alias: String,
    pub expr: InputExpression,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputExpression {
    SqlFunction(SqlFunction),
    PatchMeasure(InputPatchMeasure),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlFunction {
    #[serde(default)]
    pub cube_params: Vec<String>,
    pub sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputPatchMeasure {
    pub source_measure: String,
    #[serde(default)]
    pub replace_aggregation_type: Option<String>,
    #[serde(default)]
    pub add_filters: Vec<SqlFunction>,
}

/// The `expression` field of a parsed member expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Expression {
    /// `[...cubeParams, "return `<sql>`"]` — the JS `Function` argument list
    /// the gateway builds. Also accepts a bare SQL string, which is what the
    /// last element carries once the wrapper is stripped.
    Sql(SqlBody),
    /// The `PatchMeasure` struct.
    PatchMeasure(PatchMeasure),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SqlBody {
    /// `[...cubeParams, "return `<sql>`"]`.
    FunctionBody(Vec<String>),
    /// The SQL on its own.
    Sql(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchMeasure {
    /// Always `"PatchMeasure"`; kept so the tagged JSON round-trips.
    #[serde(rename = "type", default = "patch_measure_type")]
    pub expression_type: String,
    pub source_measure: String,
    #[serde(default)]
    pub replace_aggregation_type: Option<String>,
    #[serde(default)]
    pub add_filters: Vec<SqlBody>,
}

fn patch_measure_type() -> String {
    "PatchMeasure".to_string()
}

impl SqlBody {
    /// The SQL template, with the `return \`…\`` wrapper the gateway adds
    /// stripped. The leading elements of the function body are the cube
    /// parameters; the Rust member-sql compiler derives its dependencies from
    /// the template instead, so they are only used to check the shape.
    pub fn sql(&self) -> Result<String, PlannerError> {
        match self {
            Self::Sql(sql) => Ok(sql.clone()),
            Self::FunctionBody(parts) => {
                let body = parts.last().ok_or_else(|| {
                    PlannerError::query(
                        "A member expression's `expression` array must end with the function body"
                            .to_string(),
                    )
                })?;
                let trimmed = body.trim();
                let sql = trimmed
                    .strip_prefix("return `")
                    .and_then(|rest| rest.strip_suffix('`'))
                    .ok_or_else(|| {
                        PlannerError::query(format!(
                            "A member expression's function body must be a `return `…`` template \
                             literal, got `{body}`"
                        ))
                    })?;
                Ok(sql.to_string())
            }
        }
    }
}

impl<'de> Deserialize<'de> for MemberExpression {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_value(Value::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl MemberExpression {
    /// Picks the shape by the key that distinguishes them, so a malformed one
    /// is reported against that shape rather than against all of them.
    fn from_value(value: Value) -> Result<Self, String> {
        let Some(object) = value.as_object() else {
            return Err(format!(
                "a member expression must be an object, got `{value}`"
            ));
        };
        if object.contains_key("expr") {
            return serde_json::from_value(value)
                .map(Self::Input)
                .map_err(|e| format!("in a member expression: {e}"));
        }
        if object.contains_key("expression") {
            return serde_json::from_value(value)
                .map(Self::Parsed)
                .map_err(|e| format!("in a member expression: {e}"));
        }
        Err(format!(
            "a member expression needs an `expression` (the parsed shape) or an `expr` (the SQL \
             API shape); got the keys {}",
            object
                .keys()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    /// The name the planner reports this member under.
    pub fn member_name(&self) -> String {
        match self {
            Self::Parsed(parsed) => parsed
                .expression_name
                .clone()
                .or_else(|| parsed.name.clone())
                .unwrap_or_default(),
            Self::Input(input) => input.alias.clone(),
        }
    }

    /// Normalizes the cubesql input shape into the parsed one, exactly as
    /// `CubejsApiGateway.parseMemberExpression` does.
    pub fn into_parsed(self) -> ParsedMemberExpression {
        match self {
            Self::Parsed(parsed) => parsed,
            Self::Input(input) => {
                let definition = serde_json::to_string(&input).ok();
                let expression = match input.expr {
                    InputExpression::SqlFunction(function) => {
                        Expression::Sql(function.into_function_body())
                    }
                    InputExpression::PatchMeasure(patch) => {
                        Expression::PatchMeasure(PatchMeasure {
                            expression_type: patch_measure_type(),
                            source_measure: patch.source_measure,
                            replace_aggregation_type: patch.replace_aggregation_type,
                            add_filters: patch
                                .add_filters
                                .into_iter()
                                .map(SqlFunction::into_function_body)
                                .collect(),
                        })
                    }
                };
                ParsedMemberExpression {
                    expression,
                    cube_name: input.cube_name,
                    name: Some(input.alias.clone()),
                    expression_name: Some(input.alias),
                    definition,
                }
            }
        }
    }

    pub(crate) fn to_options_member(&self) -> Result<OptionsMember, PlannerError> {
        self.clone().into_parsed().to_options_member()
    }
}

impl SqlFunction {
    fn into_function_body(self) -> SqlBody {
        let mut parts = self.cube_params;
        parts.push(format!("return `{}`", self.sql));
        SqlBody::FunctionBody(parts)
    }
}

impl ParsedMemberExpression {
    pub(crate) fn to_options_member(&self) -> Result<OptionsMember, PlannerError> {
        Ok(OptionsMember::MemberExpression(self.to_definition()?))
    }

    fn to_definition(&self) -> Result<Rc<dyn MemberExpressionDefinition>, PlannerError> {
        let expression = match &self.expression {
            Expression::Sql(body) => {
                MemberExpressionExpressionDef::Sql(self.member_sql(body, "expression")?)
            }
            Expression::PatchMeasure(patch) => {
                if patch.expression_type != "PatchMeasure" {
                    return Err(PlannerError::unsupported(format!(
                        "Member expression `{}`: only `PatchMeasure` struct expressions are \
                         supported, got `{}`",
                        self.reported_name(),
                        patch.expression_type
                    )));
                }
                let mut add_filters = Vec::with_capacity(patch.add_filters.len());
                for (index, filter) in patch.add_filters.iter().enumerate() {
                    add_filters.push(Rc::new(
                        MockStructWithSqlMember::builder()
                            .sql(self.sql_of(filter, &format!("addFilters[{index}]"))?)
                            .build(),
                    ));
                }
                let expression_struct = MockExpressionStruct::builder()
                    .expression_type(patch.expression_type.clone())
                    .source_measure(Some(patch.source_measure.clone()))
                    .replace_aggregation_type(patch.replace_aggregation_type.clone())
                    .add_filters((!add_filters.is_empty()).then_some(add_filters))
                    .build();
                MemberExpressionExpressionDef::Struct(
                    Rc::new(expression_struct) as Rc<dyn ExpressionStruct>
                )
            }
        };

        Ok(Rc::new(
            MockMemberExpressionDefinition::builder()
                .expression_name(
                    self.expression_name
                        .clone()
                        .or_else(|| self.name.clone())
                        .or_else(|| Some(self.reported_name())),
                )
                .name(self.name.clone().or_else(|| self.expression_name.clone()))
                .cube_name(Some(self.cube_name.clone()))
                .definition(self.definition.clone())
                .expression(expression)
                .build(),
        ))
    }

    fn reported_name(&self) -> String {
        self.expression_name
            .clone()
            .or_else(|| self.name.clone())
            .unwrap_or_else(|| format!("<expression on {}>", self.cube_name))
    }

    fn sql_of(&self, body: &SqlBody, what: &str) -> Result<String, PlannerError> {
        body.sql().map_err(|e| {
            PlannerError::query(format!(
                "Member expression `{}` {what}: {}",
                self.reported_name(),
                e.message()
            ))
        })
    }

    fn member_sql(&self, body: &SqlBody, what: &str) -> Result<Rc<dyn MemberSql>, PlannerError> {
        let sql = self.sql_of(body, what)?;
        let parsed = MockMemberSql::new(&sql).map_err(|e| {
            PlannerError::unsupported(format!(
                "Member expression `{}` {what}: {}",
                self.reported_name(),
                e.message
            ))
        })?;
        Ok(Rc::new(parsed))
    }
}

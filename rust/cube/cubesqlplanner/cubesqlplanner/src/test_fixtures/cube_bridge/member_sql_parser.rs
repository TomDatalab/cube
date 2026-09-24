//! Pure-Rust replacement for the JS `MemberSqlTemplateCompiler`.
//!
//! Parses a data model member's `sql:` written in Cube's real YAML syntax and
//! produces the very same [`CompiledMemberTemplate`] the JS compiler hands to
//! `SqlCallBuilder`, so the planner is untouched.
//!
//! Supported inside `{ … }`:
//!
//! - `{CUBE.member}`, `{cube.member}`, `{member}`, `{CUBE}`, `{TABLE}` — member
//!   / cube references, recorded as `{arg:N}` symbol paths.
//! - `{FILTER_PARAMS.<cube>.<member>.filter(<column>)}` and
//!   `…​.time_shifts.<name>.filter(<column>)`, where `<column>` is a string
//!   literal or a `lambda a, b: f"…"` / `(a, b) => \`…\`` callback, recorded as
//!   `{fp:N}`.
//! - `{FILTER_GROUP(<binding>, <binding>, …)}` — recorded as `{fg:N}`.
//! - `{SECURITY_CONTEXT.<path>.filter('col')}`, `.requiredFilter('col')`,
//!   `.unsafeValue()` and bare `{SECURITY_CONTEXT.<path>}`, resolved against
//!   the request's security context at compile time into `{sv:N}` values.
//! - `{SQL_UTILS.convertTz(<expr>)}` / `{SQL_UTILS.urlEncode(<expr>)}`,
//!   resolved through the dialect at compile time.
//!
//! `{{` / `}}` are literal braces, as in the Python f-strings the YAML compiler
//! transpiles. `${…}` is accepted as an alias of `{…}` so JS-style template
//! literals nested in a callback read the same way.
//!
//! Parsing happens once, when the model is loaded; the security context and the
//! dialect are only known per request, so references to them stay symbolic in
//! the parsed form and are resolved by [`ParsedMemberSql::compile`].

use crate::cube_bridge::driver_tools::DriverTools;
use crate::cube_bridge::member_sql::{
    CompiledFilterParamsColumn, CompiledMemberTemplate, FilterGroupItem, FilterParamsColumn,
    FilterParamsItem, SqlTemplate, SqlTemplateArgs,
};
use crate::test_fixtures::cube_bridge::MockFilterParamsCallback;
use cubenativeutils::CubeError;
use serde_json::Value;
use std::rc::Rc;

/// Names that address the request's security context in a member `sql`.
const SECURITY_CONTEXT_NAMES: [&str; 3] =
    ["SECURITY_CONTEXT", "security_context", "securityContext"];

/// What a `SECURITY_CONTEXT` reference asks for. Resolved per request, so the
/// parsed form only records the path and the shape of the call.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SecurityRefKind {
    /// `.filter(column)` — renders `column = {sv:N}` / `column IN (…)` /
    /// `1 = 1` when the context carries no value.
    Filter { column: String },
    /// `.requiredFilter(column)` — as above, but a missing value is an error.
    RequiredFilter { column: String },
    /// `.unsafeValue()` — the raw value, interpolated as JS would.
    UnsafeValue,
    /// Bare `{SECURITY_CONTEXT.x}` — string coercion, comma separated.
    ToStringValue,
}

#[derive(Clone, Debug)]
struct SecurityRef {
    path: Vec<String>,
    kind: SecurityRefKind,
}

/// A `SQL_UTILS.<method>(<expr>)` call. `arg` is the already-parsed argument
/// template, so it may itself carry `{arg:N}` placeholders.
#[derive(Clone, Debug)]
struct SqlUtilsCall {
    method: String,
    arg: String,
}

/// A member `sql` parsed but not yet bound to a request.
#[derive(Clone, Debug)]
pub struct ParsedMemberSql {
    template: SqlTemplate,
    args: SqlTemplateArgs,
    args_names: Vec<String>,
    security_refs: Vec<SecurityRef>,
    sql_utils_calls: Vec<SqlUtilsCall>,
}

impl ParsedMemberSql {
    /// Parses one member `sql` string.
    pub fn parse(sql: &str) -> Result<Self, CubeError> {
        let mut sink = ParseSink::default();
        let template = sink.parse_template(sql)?;
        Ok(sink.finish(SqlTemplate::String(template)))
    }

    /// Parses a reference list — a pre-aggregation's `dimensions:` /
    /// `measures:`, or a `rollup_references` declaration. An element may be a
    /// bare member path (`orders.status`) or an interpolation (`{CUBE.status}`).
    pub fn parse_reference_list(members: &[String]) -> Result<Self, CubeError> {
        let mut sink = ParseSink::default();
        let mut elements = Vec::with_capacity(members.len());
        for member in members {
            if member.contains('{') {
                elements.push(sink.parse_template(member)?);
            } else {
                elements.push(sink.symbol_placeholder(split_member_path(member)?));
            }
        }
        Ok(sink.finish(SqlTemplate::StringVec(elements)))
    }

    /// Parses a single member reference, as a pre-aggregation's
    /// `time_dimension:` declares it.
    pub fn parse_reference(member: &str) -> Result<Self, CubeError> {
        if member.contains('{') {
            return Self::parse(member);
        }
        let mut sink = ParseSink::default();
        let placeholder = sink.symbol_placeholder(split_member_path(member)?);
        Ok(sink.finish(SqlTemplate::String(placeholder)))
    }

    pub fn args_names(&self) -> &Vec<String> {
        &self.args_names
    }

    /// Whether anything in this template needs the request to resolve.
    pub fn is_request_dependent(&self) -> bool {
        !self.security_refs.is_empty() || !self.sql_utils_calls.is_empty()
    }

    /// Binds the parsed template to one request: resolves `SECURITY_CONTEXT`
    /// references against `security_context` and `SQL_UTILS` calls against the
    /// dialect.
    pub fn compile(
        &self,
        security_context: &Value,
        driver_tools: Option<&dyn DriverTools>,
    ) -> Result<CompiledMemberTemplate, CubeError> {
        let mut args = self.args.clone();
        let template = match &self.template {
            SqlTemplate::String(s) => {
                SqlTemplate::String(self.resolve(s, security_context, driver_tools, &mut args)?)
            }
            SqlTemplate::StringVec(items) => SqlTemplate::StringVec(
                items
                    .iter()
                    .map(|s| self.resolve(s, security_context, driver_tools, &mut args))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };
        Ok(CompiledMemberTemplate { template, args })
    }

    /// Single left-to-right pass replacing the request-dependent placeholders
    /// this parser emits (`{sec:N}`, `{utils:N}`). Everything the planner
    /// understands is left alone, and so is any text the pass produces.
    fn resolve(
        &self,
        template: &str,
        security_context: &Value,
        driver_tools: Option<&dyn DriverTools>,
        args: &mut SqlTemplateArgs,
    ) -> Result<String, CubeError> {
        let chars: Vec<char> = template.chars().collect();
        let mut out = String::with_capacity(template.len());
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '{' {
                out.push(chars[i]);
                i += 1;
                continue;
            }
            let Some(close) = chars[i..].iter().position(|c| *c == '}').map(|p| p + i) else {
                out.push(chars[i]);
                i += 1;
                continue;
            };
            let body: String = chars[i + 1..close].iter().collect();
            match body.split_once(':') {
                Some(("sec", idx)) => {
                    let idx: usize = idx.parse().map_err(|_| {
                        CubeError::internal(format!("Bad security placeholder `{}`", body))
                    })?;
                    let reference = self.security_refs.get(idx).ok_or_else(|| {
                        CubeError::internal(format!("Security placeholder {} out of bounds", idx))
                    })?;
                    out.push_str(&resolve_security_ref(reference, security_context, args)?);
                }
                Some(("utils", idx)) => {
                    let idx: usize = idx.parse().map_err(|_| {
                        CubeError::internal(format!("Bad SQL_UTILS placeholder `{}`", body))
                    })?;
                    let call = self.sql_utils_calls.get(idx).ok_or_else(|| {
                        CubeError::internal(format!("SQL_UTILS placeholder {} out of bounds", idx))
                    })?;
                    out.push_str(&resolve_sql_utils_call(call, driver_tools)?);
                }
                _ => {
                    out.push('{');
                    out.push_str(&body);
                    out.push('}');
                }
            }
            i = close + 1;
        }
        Ok(out)
    }
}

// ---- security context ------------------------------------------------------

/// The shapes `.filter()` coerces a security-context value to. Mirrors
/// `coerceFilterValue` in `MemberSqlTemplateCompiler.js`: falsy scalars
/// collapse to "no value".
enum FilterValueShape {
    None,
    Scalar(String),
    Vector(Vec<String>),
}

fn scalar_to_string(value: &Value) -> Result<String, CubeError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(format_json_number(n.as_f64().unwrap_or(0.0))),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(CubeError::user(
            "Invalid param for security context".to_string(),
        )),
    }
}

fn format_json_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

fn coerce_filter_value(value: Option<&Value>) -> Result<FilterValueShape, CubeError> {
    let Some(value) = value else {
        return Ok(FilterValueShape::None);
    };
    match value {
        Value::Null => Ok(FilterValueShape::None),
        Value::Array(items) => Ok(FilterValueShape::Vector(
            items
                .iter()
                .map(scalar_to_string)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::String(s) => Ok(if s.is_empty() {
            FilterValueShape::None
        } else {
            FilterValueShape::Scalar(s.clone())
        }),
        Value::Number(n) => {
            let n = n.as_f64().unwrap_or(0.0);
            Ok(if n == 0.0 || n.is_nan() {
                FilterValueShape::None
            } else {
                FilterValueShape::Scalar(format_json_number(n))
            })
        }
        Value::Bool(b) => Ok(if *b {
            FilterValueShape::Scalar("true".to_string())
        } else {
            FilterValueShape::None
        }),
        Value::Object(_) => Err(CubeError::user(
            "Invalid param for security context".to_string(),
        )),
    }
}

fn lookup_security_value<'a>(context: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = context;
    for part in path {
        match current {
            Value::Object(map) => current = map.get(part)?,
            _ => return None,
        }
    }
    Some(current)
}

fn resolve_security_ref(
    reference: &SecurityRef,
    context: &Value,
    args: &mut SqlTemplateArgs,
) -> Result<String, CubeError> {
    let value = lookup_security_value(context, &reference.path);
    match &reference.kind {
        SecurityRefKind::Filter { column } | SecurityRefKind::RequiredFilter { column } => {
            let required = matches!(reference.kind, SecurityRefKind::RequiredFilter { .. });
            match coerce_filter_value(value)? {
                FilterValueShape::Scalar(v) => {
                    let index = args.insert_security_context_value(v);
                    Ok(format!("{} = {{sv:{}}}", column, index))
                }
                FilterValueShape::Vector(values) => {
                    if values.is_empty() {
                        return Ok("1 = 0".to_string());
                    }
                    let placeholders = values
                        .into_iter()
                        .map(|v| format!("{{sv:{}}}", args.insert_security_context_value(v)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Ok(format!("{} IN ({})", column, placeholders))
                }
                FilterValueShape::None => {
                    if required {
                        Err(CubeError::user(format!(
                            "Filter for {} is required",
                            column
                        )))
                    } else {
                        Ok("1 = 1".to_string())
                    }
                }
            }
        }
        SecurityRefKind::UnsafeValue => match value {
            None => Ok("undefined".to_string()),
            Some(Value::Null) => Ok("null".to_string()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(Value::Number(n)) => Ok(format_json_number(n.as_f64().unwrap_or(0.0))),
            Some(Value::Bool(b)) => Ok(b.to_string()),
            Some(Value::Array(items)) => Ok(items
                .iter()
                .map(|item| scalar_to_string(item).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(",")),
            Some(Value::Object(_)) => Ok("[object Object]".to_string()),
        },
        SecurityRefKind::ToStringValue => {
            let values = match value {
                None | Some(Value::Null) => return Ok(String::new()),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(scalar_to_string)
                    .collect::<Result<Vec<_>, _>>()?,
                Some(scalar) => vec![scalar_to_string(scalar)?],
            };
            Ok(values
                .into_iter()
                .map(|v| format!("{{sv:{}}}", args.insert_security_context_value(v)))
                .collect::<Vec<_>>()
                .join(","))
        }
    }
}

// ---- SQL_UTILS -------------------------------------------------------------

fn resolve_sql_utils_call(
    call: &SqlUtilsCall,
    driver_tools: Option<&dyn DriverTools>,
) -> Result<String, CubeError> {
    match call.method.as_str() {
        "convertTz" | "convert_tz" => {
            let driver_tools = driver_tools.ok_or_else(|| {
                CubeError::internal(
                    "SQL_UTILS.convertTz needs a dialect to compile against".to_string(),
                )
            })?;
            driver_tools.convert_tz(call.arg.clone())
        }
        "urlEncode" | "url_encode" => Ok(url_encode(&call.arg)),
        other => Err(CubeError::user(format!(
            "SQL_UTILS.{} is not supported by the Rust member sql compiler",
            other
        ))),
    }
}

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

// ---- parsing ---------------------------------------------------------------

#[derive(Default)]
struct ParseSink {
    args: SqlTemplateArgs,
    args_names: Vec<String>,
    security_refs: Vec<SecurityRef>,
    sql_utils_calls: Vec<SqlUtilsCall>,
    /// Names of the filter values a `FILTER_PARAMS` column callback takes,
    /// while its body is being parsed. An interpolation naming one of them
    /// becomes `{fpv:N}` instead of a member reference.
    value_params: Vec<String>,
}

impl ParseSink {
    fn finish(self, template: SqlTemplate) -> ParsedMemberSql {
        ParsedMemberSql {
            template,
            args: self.args,
            args_names: self.args_names,
            security_refs: self.security_refs,
            sql_utils_calls: self.sql_utils_calls,
        }
    }

    fn record_name(&mut self, name: &str) {
        if !self.args_names.iter().any(|n| n == name) {
            self.args_names.push(name.to_string());
        }
    }

    fn symbol_placeholder(&mut self, path: Vec<String>) -> String {
        self.record_name(&path[0]);
        format!("{{arg:{}}}", self.args.insert_symbol_path(path))
    }

    /// Scans a template, turning every `{ … }` interpolation into the
    /// placeholder that stands for it.
    fn parse_template(&mut self, src: &str) -> Result<String, CubeError> {
        let chars: Vec<char> = src.chars().collect();
        let mut out = String::with_capacity(src.len());
        let mut i = 0;
        while i < chars.len() {
            let ch = chars[i];
            if let Some((end, _)) = string_literal_at(&chars, i) {
                // A quoted run inside the SQL itself: copied through verbatim so
                // a brace it carries is never read as an interpolation.
                out.extend(&chars[i..end]);
                i = end;
                continue;
            }
            if ch == '$' && chars.get(i + 1) == Some(&'{') {
                i += 1;
                continue;
            }
            if ch == '{' {
                if chars.get(i + 1) == Some(&'{') {
                    out.push('{');
                    i += 2;
                    continue;
                }
                let close = find_closing_brace(&chars, i)?;
                let inner: String = chars[i + 1..close].iter().collect();
                out.push_str(&self.parse_interpolation(&inner)?);
                i = close + 1;
                continue;
            }
            if ch == '}' && chars.get(i + 1) == Some(&'}') {
                out.push('}');
                i += 2;
                continue;
            }
            out.push(ch);
            i += 1;
        }
        Ok(out)
    }

    fn parse_interpolation(&mut self, inner: &str) -> Result<String, CubeError> {
        let trimmed = inner.trim();
        if trimmed.is_empty() {
            return Err(CubeError::user(
                "Empty `{}` interpolation in member sql".to_string(),
            ));
        }

        // A filter value of the column callback currently being parsed.
        if let Some(index) = self.value_params.iter().position(|p| p == trimmed) {
            return Ok(format!("{{fpv:{}}}", index));
        }

        // Legacy fixture syntax, kept so the planner's own tests keep their
        // compact way of declaring bindings.
        if let Some(value) = trimmed.strip_prefix("SECURITY_VALUE:") {
            let index = self.args.insert_security_context_value(value.to_string());
            return Ok(format!("{{sv:{}}}", index));
        }
        if trimmed.starts_with("FILTER_PARAMS_COLUMN:") || trimmed.starts_with("FILTER_PARAMS:") {
            let item = self.parse_legacy_filter_params_item(trimmed)?;
            return Ok(format!("{{fp:{}}}", self.args.insert_filter_params(item)));
        }
        if let Some(body) = trimmed.strip_prefix("FILTER_GROUP|") {
            let filter_params = body
                .split('|')
                .map(|spec| self.parse_legacy_filter_params_item(spec.trim()))
                .collect::<Result<Vec<_>, _>>()?;
            let index = self
                .args
                .insert_filter_group(FilterGroupItem { filter_params });
            return Ok(format!("{{fg:{}}}", index));
        }

        let chars: Vec<char> = trimmed.chars().collect();
        let head = read_ident(&chars, 0);

        if head == "FILTER_GROUP" {
            self.record_name("FILTER_GROUP");
            let args_src = read_call_args(&chars, head.len(), "FILTER_GROUP")?;
            let filter_params = split_top_level(&args_src, ',')
                .into_iter()
                .map(|arg| self.parse_filter_params_binding(arg.trim()))
                .collect::<Result<Vec<_>, _>>()?;
            if filter_params.is_empty() {
                return Err(CubeError::user(
                    "FILTER_GROUP expects FILTER_PARAMS args to be passed.".to_string(),
                ));
            }
            let index = self
                .args
                .insert_filter_group(FilterGroupItem { filter_params });
            return Ok(format!("{{fg:{}}}", index));
        }

        if head == "FILTER_PARAMS" {
            let item = self.parse_filter_params_binding(trimmed)?;
            return Ok(format!("{{fp:{}}}", self.args.insert_filter_params(item)));
        }

        if SECURITY_CONTEXT_NAMES.contains(&head.as_str()) {
            return self.parse_security_context(&head, trimmed);
        }

        if head == "SQL_UTILS" {
            return self.parse_sql_utils(trimmed);
        }

        // Plain member / cube reference.
        let path = split_member_path(trimmed)?;
        Ok(self.symbol_placeholder(path))
    }

    // `FILTER_PARAMS.<cube>.<member>[.time_shifts.<name>].filter(<column>)`
    fn parse_filter_params_binding(&mut self, src: &str) -> Result<FilterParamsItem, CubeError> {
        let chars: Vec<char> = src.chars().collect();
        let mut pos = 0;
        let head = read_ident(&chars, pos);
        if head != "FILTER_PARAMS" {
            return Err(CubeError::user(format!(
                "FILTER_GROUP expects FILTER_PARAMS args to be passed, got `{}`",
                src
            )));
        }
        self.record_name("FILTER_PARAMS");
        pos += head.len();

        let mut segments = Vec::new();
        while chars.get(pos) == Some(&'.') {
            let ident = read_ident(&chars, pos + 1);
            if ident.is_empty() {
                break;
            }
            pos += 1 + ident.len();
            if ident == "filter" {
                let column_src = read_call_args(&chars, pos, "FILTER_PARAMS…filter")?;
                let (cube_name, name, time_shift_name) = filter_params_target(&segments, src)?;
                let column = self.parse_filter_params_column(&column_src)?;
                return Ok(FilterParamsItem {
                    cube_name,
                    name,
                    time_shift_name,
                    column,
                });
            }
            segments.push(ident);
        }

        Err(CubeError::user(format!(
            "FILTER_PARAMS must be written as \
             `FILTER_PARAMS.<cube>.<member>[.time_shifts.<name>].filter(<column>)`, got `{}`",
            src
        )))
    }

    fn parse_filter_params_column(&mut self, src: &str) -> Result<FilterParamsColumn, CubeError> {
        let trimmed = src.trim();
        let chars: Vec<char> = trimmed.chars().collect();

        // A plain column name / SQL fragment.
        if let Some((end, content)) = string_literal_at(&chars, 0) {
            if end == chars.len() {
                return Ok(FilterParamsColumn::String(content));
            }
        }

        let (params, body) = parse_callback(trimmed)?;
        if params
            .iter()
            .any(|p| p.starts_with("...") || p.starts_with('*'))
        {
            return Err(CubeError::user(format!(
                "A FILTER_PARAMS column callback taking a rest parameter is not supported by \
                 the Rust member sql compiler: `{}`",
                trimmed
            )));
        }

        let mut column_sink = ParseSink {
            value_params: params.clone(),
            ..ParseSink::default()
        };
        let template = column_sink.parse_template(&body)?;
        if !column_sink.security_refs.is_empty() || !column_sink.sql_utils_calls.is_empty() {
            return Err(CubeError::user(
                "SECURITY_CONTEXT / SQL_UTILS inside a FILTER_PARAMS column callback is not \
                 supported by the Rust member sql compiler"
                    .to_string(),
            ));
        }
        for name in &column_sink.args_names {
            self.record_name(name);
        }

        Ok(FilterParamsColumn::Compiled(Rc::new(
            CompiledFilterParamsColumn {
                template: SqlTemplate::String(template),
                args: column_sink.args,
                value_params_count: params.len(),
            },
        )))
    }

    fn parse_security_context(&mut self, head: &str, src: &str) -> Result<String, CubeError> {
        self.record_name(head);
        let chars: Vec<char> = src.chars().collect();
        let mut pos = head.len();
        let mut path = Vec::new();

        while chars.get(pos) == Some(&'.') {
            let ident = read_ident(&chars, pos + 1);
            if ident.is_empty() {
                break;
            }
            pos += 1 + ident.len();
            let kind = match ident.as_str() {
                "filter" => {
                    let column = read_call_args(&chars, pos, "SECURITY_CONTEXT…filter")?;
                    SecurityRefKind::Filter {
                        column: expect_string_literal(&column, "SECURITY_CONTEXT filter column")?,
                    }
                }
                "requiredFilter" => {
                    let column = read_call_args(&chars, pos, "SECURITY_CONTEXT…requiredFilter")?;
                    SecurityRefKind::RequiredFilter {
                        column: expect_string_literal(&column, "SECURITY_CONTEXT filter column")?,
                    }
                }
                "unsafeValue" => {
                    read_call_args(&chars, pos, "SECURITY_CONTEXT…unsafeValue")?;
                    SecurityRefKind::UnsafeValue
                }
                _ => {
                    path.push(ident);
                    continue;
                }
            };
            self.security_refs.push(SecurityRef { path, kind });
            return Ok(format!("{{sec:{}}}", self.security_refs.len() - 1));
        }

        if pos != chars.len() {
            return Err(CubeError::user(format!(
                "Unsupported SECURITY_CONTEXT expression `{}`. Supported forms are \
                 `.filter('col')`, `.requiredFilter('col')` and `.unsafeValue()`",
                src
            )));
        }
        self.security_refs.push(SecurityRef {
            path,
            kind: SecurityRefKind::ToStringValue,
        });
        Ok(format!("{{sec:{}}}", self.security_refs.len() - 1))
    }

    fn parse_sql_utils(&mut self, src: &str) -> Result<String, CubeError> {
        self.record_name("SQL_UTILS");
        let chars: Vec<char> = src.chars().collect();
        let mut pos = "SQL_UTILS".len();
        if chars.get(pos) != Some(&'.') {
            return Err(CubeError::user(format!(
                "SQL_UTILS must be called as `SQL_UTILS.<method>(<expr>)`, got `{}`",
                src
            )));
        }
        let method = read_ident(&chars, pos + 1);
        pos += 1 + method.len();
        let arg_src = read_call_args(&chars, pos, "SQL_UTILS")?;
        // The argument is written as a quoted SQL fragment; its interpolations
        // are the enclosing member's, so they are recorded here.
        let arg_body = match string_literal_at(&arg_src.trim().chars().collect::<Vec<_>>(), 0) {
            Some((end, content)) if end == arg_src.trim().chars().count() => content,
            _ => arg_src.trim().to_string(),
        };
        let arg = self.parse_template(&arg_body)?;
        self.sql_utils_calls.push(SqlUtilsCall { method, arg });
        Ok(format!("{{utils:{}}}", self.sql_utils_calls.len() - 1))
    }

    // ---- legacy fixture syntax --------------------------------------------

    fn parse_legacy_filter_params_item(
        &mut self,
        spec: &str,
    ) -> Result<FilterParamsItem, CubeError> {
        if let Some(body) = spec.strip_prefix("FILTER_PARAMS_COLUMN:") {
            let (cube_name, name, time_shift_name, column) = parse_legacy_body(body)?;
            Ok(FilterParamsItem {
                cube_name,
                name,
                time_shift_name,
                column: FilterParamsColumn::String(column),
            })
        } else if let Some(body) = spec.strip_prefix("FILTER_PARAMS:") {
            let (cube_name, name, time_shift_name, column) = parse_legacy_body(body)?;
            let column = self.parse_legacy_column_references(&column)?;
            Ok(FilterParamsItem {
                cube_name,
                name,
                time_shift_name,
                column: FilterParamsColumn::Callback(Rc::new(MockFilterParamsCallback::new(
                    column,
                ))),
            })
        } else {
            Err(CubeError::user(format!(
                "FILTER_PARAMS binding must start with `FILTER_PARAMS:` or \
                 `FILTER_PARAMS_COLUMN:`: {}",
                spec
            )))
        }
    }

    fn parse_legacy_column_references(&mut self, column: &str) -> Result<String, CubeError> {
        let mut result = String::new();
        let mut rest = column;

        while let Some(open) = rest.find('[') {
            result.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let close = after.find(']').ok_or_else(|| {
                CubeError::user(format!("Unclosed member reference in column: {}", column))
            })?;
            let path = split_member_path(&after[..close])?;
            result.push_str(&self.symbol_placeholder(path));
            rest = &after[close + 1..];
        }
        result.push_str(rest);
        Ok(result)
    }
}

fn parse_legacy_body(body: &str) -> Result<(String, String, Option<String>, String), CubeError> {
    let (member, column) = body.split_once(':').ok_or_else(|| {
        CubeError::user(format!(
            "FILTER_PARAMS needs a `<cube>.<member>:<column>` body: {}",
            body
        ))
    })?;
    if column.is_empty() || column.contains('{') {
        return Err(CubeError::user(format!(
            "FILTER_PARAMS column must be non-empty and reference members as `[path]`: {}",
            column
        )));
    }
    let (member, time_shift_name) = match member.split_once('@') {
        Some((member, shift)) if !shift.is_empty() => (member, Some(shift.to_string())),
        Some(_) => {
            return Err(CubeError::user(format!(
                "FILTER_PARAMS time shift name must be non-empty: {}",
                member
            )))
        }
        None => (member, None),
    };
    let parts = member.split('.').collect::<Vec<_>>();
    if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
        return Err(CubeError::user(format!(
            "FILTER_PARAMS member must be `<cube>.<member>`: {}",
            member
        )));
    }
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        time_shift_name,
        column.to_string(),
    ))
}

// ---- lexing helpers --------------------------------------------------------

fn is_ident_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

fn read_ident(chars: &[char], start: usize) -> String {
    chars[start.min(chars.len())..]
        .iter()
        .take_while(|c| is_ident_char(**c))
        .collect()
}

/// Reads a `( … )` argument list starting at `start`, returning its raw inner
/// text. Strings and nested brackets are skipped.
fn read_call_args(chars: &[char], start: usize, what: &str) -> Result<String, CubeError> {
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    if chars.get(i) != Some(&'(') {
        return Err(CubeError::user(format!(
            "{} must be called: expected `(` in `{}`",
            what,
            chars.iter().collect::<String>()
        )));
    }
    let open = i;
    let mut depth = 0usize;
    while i < chars.len() {
        if let Some((end, _)) = string_literal_at(chars, i) {
            i = end;
            continue;
        }
        match chars[i] {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    if chars[i] != ')' {
                        break;
                    }
                    if i + 1 != chars.len() {
                        // Trailing text after the call (e.g. a JS ternary) is
                        // not a shape this compiler understands.
                        return Err(CubeError::user(format!(
                            "Unsupported expression after `{}(…)` in `{}`",
                            what,
                            chars.iter().collect::<String>()
                        )));
                    }
                    return Ok(chars[open + 1..i].iter().collect());
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(CubeError::user(format!(
        "Unbalanced parentheses in `{}`",
        chars.iter().collect::<String>()
    )))
}

/// Index of the `}` matching the `{` at `open`.
fn find_closing_brace(chars: &[char], open: usize) -> Result<usize, CubeError> {
    let mut depth = 0usize;
    let mut i = open;
    while i < chars.len() {
        if i > open {
            if let Some((end, _)) = string_literal_at(chars, i) {
                i = end;
                continue;
            }
        }
        match chars[i] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(CubeError::user(format!(
        "Unclosed brace in template: {}",
        chars.iter().collect::<String>()
    )))
}

/// Reads the string literal starting at `start`, if there is one. Supports
/// `'…'`, `"…"`, `` `…` `` and their triple-quoted forms.
fn string_literal_at(chars: &[char], start: usize) -> Option<(usize, String)> {
    let quote = *chars.get(start)?;
    if quote != '\'' && quote != '"' && quote != '`' {
        return None;
    }
    let triple = chars.get(start + 1) == Some(&quote) && chars.get(start + 2) == Some(&quote);
    let mut i = start + if triple { 3 } else { 1 };
    let mut content = String::new();
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' {
            if let Some(next) = chars.get(i + 1) {
                content.push(match next {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => *other,
                });
                i += 2;
                continue;
            }
        }
        if ch == quote {
            if !triple {
                return Some((i + 1, content));
            }
            if chars.get(i + 1) == Some(&quote) && chars.get(i + 2) == Some(&quote) {
                return Some((i + 3, content));
            }
        }
        content.push(ch);
        i += 1;
    }
    None
}

/// Splits on a separator that is not nested in brackets or a string.
fn split_top_level(src: &str, separator: char) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some((end, _)) = string_literal_at(&chars, i) {
            current.extend(&chars[i..end]);
            i = end;
            continue;
        }
        let ch = chars[i];
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if ch == separator && depth == 0 {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
        i += 1;
    }
    if !current.trim().is_empty() || !parts.is_empty() {
        parts.push(current);
    }
    parts.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

fn split_member_path(path: &str) -> Result<Vec<String>, CubeError> {
    let parts: Vec<String> = path
        .trim()
        .split('.')
        .map(|s| s.trim().to_string())
        .collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(is_ident_char))
    {
        return Err(CubeError::user(format!(
            "Unsupported expression in member sql: `{}`. Only member references, FILTER_PARAMS, \
             FILTER_GROUP, SECURITY_CONTEXT and SQL_UTILS are supported",
            path
        )));
    }
    Ok(parts)
}

fn expect_string_literal(src: &str, what: &str) -> Result<String, CubeError> {
    let trimmed = src.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    match string_literal_at(&chars, 0) {
        Some((end, content)) if end == chars.len() => Ok(content),
        _ => Err(CubeError::user(format!(
            "{} must be a quoted string, got `{}`",
            what, trimmed
        ))),
    }
}

/// Splits `FILTER_PARAMS.<cube>.<member>[.time_shifts.<name>]` into its parts.
fn filter_params_target(
    segments: &[String],
    src: &str,
) -> Result<(String, String, Option<String>), CubeError> {
    match segments {
        [cube, member] => Ok((cube.clone(), member.clone(), None)),
        [cube, member, shifts, shift] if shifts == "time_shifts" || shifts == "timeShifts" => {
            Ok((cube.clone(), member.clone(), Some(shift.clone())))
        }
        _ => Err(CubeError::user(format!(
            "FILTER_PARAMS must name `<cube>.<member>` optionally followed by \
             `.time_shifts.<name>`, got `{}`",
            src
        ))),
    }
}

/// Parses a column callback — `lambda a, b: f"…"` or `(a, b) => \`…\`` — into
/// its parameter names and the raw body template.
fn parse_callback(src: &str) -> Result<(Vec<String>, String), CubeError> {
    let trimmed = src.trim();

    if let Some(rest) = trimmed.strip_prefix("lambda") {
        if !rest.starts_with(|c: char| c.is_whitespace() || c == ':') {
            return Err(unsupported_column(trimmed));
        }
        let (params_src, body) = rest.split_once(':').ok_or_else(|| {
            CubeError::user(format!(
                "A `lambda` FILTER_PARAMS column needs a `:` before its body: `{}`",
                trimmed
            ))
        })?;
        let params = split_top_level(params_src, ',')
            .into_iter()
            .map(|p| p.trim().to_string())
            .collect();
        return Ok((params, strip_f_string(body)?));
    }

    // JS arrow function.
    if let Some((params_src, body)) = split_arrow(trimmed) {
        let params_src = params_src.trim();
        let params_src = params_src
            .strip_prefix('(')
            .and_then(|p| p.strip_suffix(')'))
            .unwrap_or(params_src);
        let params = split_top_level(params_src, ',')
            .into_iter()
            .map(|p| p.trim().to_string())
            .collect();
        return Ok((params, strip_f_string(body)?));
    }

    Err(unsupported_column(trimmed))
}

fn unsupported_column(src: &str) -> CubeError {
    CubeError::user(format!(
        "Unsupported FILTER_PARAMS column `{}`. Write a quoted column name, a \
         `lambda a, b: f\"…\"` or an `(a, b) => `…`` callback",
        src
    ))
}

fn split_arrow(src: &str) -> Option<(&str, &str)> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut depth = 0i32;
    while i < chars.len() {
        if let Some((end, _)) = string_literal_at(&chars, i) {
            i = end;
            continue;
        }
        match chars[i] {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '=' if depth == 0 && chars.get(i + 1) == Some(&'>') => {
                let byte_index = src
                    .char_indices()
                    .nth(i)
                    .map(|(b, _)| b)
                    .unwrap_or(src.len());
                let (head, tail) = src.split_at(byte_index);
                return Some((head, &tail[2..]));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Unwraps a callback body: an f-string (`f"…"`), a plain quoted string or a
/// bare expression.
fn strip_f_string(body: &str) -> Result<String, CubeError> {
    let trimmed = body.trim();
    let without_prefix = trimmed
        .strip_prefix("f")
        .filter(|rest| rest.starts_with(['\'', '"', '`']))
        .unwrap_or(trimmed);
    let chars: Vec<char> = without_prefix.chars().collect();
    match string_literal_at(&chars, 0) {
        Some((end, content)) if end == chars.len() => Ok(content),
        _ => Ok(without_prefix.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(sql: &str) -> CompiledMemberTemplate {
        ParsedMemberSql::parse(sql)
            .unwrap()
            .compile(&Value::Null, None)
            .unwrap()
    }

    fn template_of(compiled: &CompiledMemberTemplate) -> String {
        match &compiled.template {
            SqlTemplate::String(s) => s.clone(),
            SqlTemplate::StringVec(v) => v.join("|"),
        }
    }

    #[test]
    fn parses_member_references() {
        let compiled = compile("{CUBE.amount} / {other.count}");
        assert_eq!(template_of(&compiled), "{arg:0} / {arg:1}");
        assert_eq!(compiled.args.symbol_paths[0], vec!["CUBE", "amount"]);
        assert_eq!(compiled.args.symbol_paths[1], vec!["other", "count"]);
    }

    #[test]
    fn parses_cube_and_table_constants() {
        let compiled = compile("{CUBE}.city || {TABLE}.zip");
        assert_eq!(template_of(&compiled), "{arg:0}.city || {arg:1}.zip");
        assert_eq!(compiled.args.symbol_paths[0], vec!["CUBE"]);
        assert_eq!(compiled.args.symbol_paths[1], vec!["TABLE"]);
    }

    #[test]
    fn escaped_braces_become_literal() {
        let compiled = compile("{{literal}} {CUBE.field}");
        assert_eq!(template_of(&compiled), "{literal} {arg:0}");
    }

    #[test]
    fn filter_params_string_column() {
        let parsed =
            ParsedMemberSql::parse("SELECT * FROM t WHERE {FILTER_PARAMS.orders.day.filter('d')}")
                .unwrap();
        let compiled = parsed.compile(&Value::Null, None).unwrap();
        assert_eq!(template_of(&compiled), "SELECT * FROM t WHERE {fp:0}");
        let item = &compiled.args.filter_params[0];
        assert_eq!(item.cube_name, "orders");
        assert_eq!(item.name, "day");
        assert_eq!(item.time_shift_name, None);
        match &item.column {
            FilterParamsColumn::String(s) => assert_eq!(s, "d"),
            _ => panic!("expected a string column"),
        }
        assert_eq!(parsed.args_names(), &vec!["FILTER_PARAMS".to_string()]);
    }

    #[test]
    fn filter_params_time_shift() {
        let compiled = compile("{FILTER_PARAMS.cal.report_d.time_shifts.prev_fy.filter('day_d')}");
        let item = &compiled.args.filter_params[0];
        assert_eq!(item.time_shift_name.as_deref(), Some("prev_fy"));
    }

    #[test]
    fn filter_params_lambda_column() {
        let compiled =
            compile("{FILTER_PARAMS.events.date.filter(lambda x, y: f\"d >= {x} AND d <= {y}\")}");
        match &compiled.args.filter_params[0].column {
            FilterParamsColumn::Compiled(c) => {
                assert_eq!(c.value_params_count, 2);
                assert_eq!(
                    c.template,
                    SqlTemplate::String("d >= {fpv:0} AND d <= {fpv:1}".to_string())
                );
            }
            _ => panic!("expected a compiled column"),
        }
    }

    #[test]
    fn filter_params_arrow_column_with_member_reference() {
        let compiled = compile(
            "{FILTER_PARAMS.events.date.filter((from, to) => `{CUBE.d} >= ${from} AND {CUBE.d} <= ${to}`)}",
        );
        match &compiled.args.filter_params[0].column {
            FilterParamsColumn::Compiled(c) => {
                assert_eq!(
                    c.template,
                    SqlTemplate::String("{arg:0} >= {fpv:0} AND {arg:0} <= {fpv:1}".to_string())
                );
                assert_eq!(c.args.symbol_paths[0], vec!["CUBE", "d"]);
            }
            _ => panic!("expected a compiled column"),
        }
    }

    #[test]
    fn filter_group_collects_bindings() {
        let compiled = compile(
            "WHERE {FILTER_GROUP(FILTER_PARAMS.a.x.filter('x'), FILTER_PARAMS.a.y.filter('y'))}",
        );
        assert_eq!(template_of(&compiled), "WHERE {fg:0}");
        assert_eq!(compiled.args.filter_groups[0].filter_params.len(), 2);
    }

    #[test]
    fn security_context_filter_with_scalar() {
        let parsed =
            ParsedMemberSql::parse("WHERE {SECURITY_CONTEXT.tenant.filter('tenant_id')}").unwrap();
        let context = serde_json::json!({ "tenant": "acme" });
        let compiled = parsed.compile(&context, None).unwrap();
        assert_eq!(template_of(&compiled), "WHERE tenant_id = {sv:0}");
        assert_eq!(compiled.args.security_context.values, vec!["acme"]);
    }

    #[test]
    fn security_context_filter_with_list_and_absent_value() {
        let parsed = ParsedMemberSql::parse("WHERE {SECURITY_CONTEXT.ids.filter('id')}").unwrap();

        let compiled = parsed
            .compile(&serde_json::json!({ "ids": ["a", "b"] }), None)
            .unwrap();
        assert_eq!(template_of(&compiled), "WHERE id IN ({sv:0}, {sv:1})");

        let compiled = parsed.compile(&serde_json::json!({}), None).unwrap();
        assert_eq!(template_of(&compiled), "WHERE 1 = 1");

        let compiled = parsed
            .compile(&serde_json::json!({ "ids": [] }), None)
            .unwrap();
        assert_eq!(template_of(&compiled), "WHERE 1 = 0");
    }

    #[test]
    fn security_context_required_filter_errors_without_value() {
        let parsed =
            ParsedMemberSql::parse("WHERE {SECURITY_CONTEXT.tenant.requiredFilter('t')}").unwrap();
        let err = match parsed.compile(&serde_json::json!({}), None) {
            Err(err) => err,
            Ok(_) => panic!("expected a required-filter error"),
        };
        assert!(err.message.contains("Filter for t is required"), "{}", err);
    }

    #[test]
    fn security_context_unsafe_value_is_inlined() {
        let parsed =
            ParsedMemberSql::parse("{SECURITY_CONTEXT.role.unsafeValue()}_suffix").unwrap();
        let compiled = parsed
            .compile(&serde_json::json!({ "role": "admin" }), None)
            .unwrap();
        assert_eq!(template_of(&compiled), "admin_suffix");
    }

    #[test]
    fn security_values_are_deduplicated() {
        let parsed = ParsedMemberSql::parse(
            "{SECURITY_CONTEXT.t.filter('a')} OR {SECURITY_CONTEXT.t.filter('b')}",
        )
        .unwrap();
        let compiled = parsed
            .compile(&serde_json::json!({ "t": "x" }), None)
            .unwrap();
        assert_eq!(template_of(&compiled), "a = {sv:0} OR b = {sv:0}");
        assert_eq!(compiled.args.security_context.values.len(), 1);
    }

    #[test]
    fn nested_security_context_path() {
        let parsed =
            ParsedMemberSql::parse("{SECURITY_CONTEXT.user.team.filter('team_id')}").unwrap();
        let compiled = parsed
            .compile(&serde_json::json!({ "user": { "team": 7 } }), None)
            .unwrap();
        assert_eq!(template_of(&compiled), "team_id = {sv:0}");
        assert_eq!(compiled.args.security_context.values, vec!["7"]);
    }

    #[test]
    fn reference_lists_mix_paths_and_interpolations() {
        let parsed = ParsedMemberSql::parse_reference_list(&[
            "orders.status".to_string(),
            "{CUBE.count}".to_string(),
        ])
        .unwrap();
        let compiled = parsed.compile(&Value::Null, None).unwrap();
        assert_eq!(
            compiled.template,
            SqlTemplate::StringVec(vec!["{arg:0}".to_string(), "{arg:1}".to_string()])
        );
        assert_eq!(compiled.args.symbol_paths[0], vec!["orders", "status"]);
        assert_eq!(compiled.args.symbol_paths[1], vec!["CUBE", "count"]);
    }

    #[test]
    fn quoted_sql_literals_are_not_interpolated() {
        let compiled = compile("CASE WHEN {CUBE.x} THEN '{not a ref}' END");
        assert_eq!(
            template_of(&compiled),
            "CASE WHEN {arg:0} THEN '{not a ref}' END"
        );
    }

    #[test]
    fn rejects_unclosed_brace() {
        let err = ParsedMemberSql::parse("{CUBE.field").unwrap_err();
        assert!(err.message.contains("Unclosed brace"), "{}", err);
    }

    #[test]
    fn rejects_arbitrary_javascript() {
        let err = ParsedMemberSql::parse("{SECURITY_CONTEXT.x.unsafeValue() === 'a' ? 1 : 2}")
            .unwrap_err();
        assert!(err.message.contains("Unsupported"), "{}", err);
    }

    #[test]
    fn rejects_rest_parameter_columns() {
        let err =
            ParsedMemberSql::parse("{FILTER_PARAMS.a.b.filter((...values) => `x IN (${values})`)}")
                .unwrap_err();
        assert!(err.message.contains("rest parameter"), "{}", err);
    }

    #[test]
    fn legacy_fixture_syntax_still_parses() {
        let compiled = compile("WHERE {FILTER_PARAMS_COLUMN:orders.day:created_at}");
        assert_eq!(template_of(&compiled), "WHERE {fp:0}");
        match &compiled.args.filter_params[0].column {
            FilterParamsColumn::String(s) => assert_eq!(s, "created_at"),
            _ => panic!("expected a string column"),
        }
    }
}

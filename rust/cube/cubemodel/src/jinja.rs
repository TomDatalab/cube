//! Jinja rendering for YAML data model files.
//!
//! The engine setup mirrors `packages/cubejs-backend-native/src/template/entry.rs`
//! (same `minijinja` 1.x major, JSON auto-escaping, the `env_var` global) but is
//! self-contained so this crate does not depend on the Neon-based native crate.
//!
//! Python template functions and filters declared in `cube.py` are **not**
//! supported: they require an embedded CPython interpreter, which is out of
//! scope for a Node.js-free backend.

use minijinja::{AutoEscape, Environment, Error as MjError, ErrorKind, Value};
use serde_json::Value as JsonValue;

/// Builds the minijinja environment used for data model templates.
pub fn build_environment(debug: bool) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_debug(debug);
    env.add_function(
        "env_var",
        |var_name: String, var_default: Option<String>| -> Result<Value, MjError> {
            if let Ok(value) = std::env::var(&var_name) {
                return Ok(Value::from(value));
            }
            if let Some(var_default) = var_default {
                return Ok(Value::from(var_default));
            }
            Err(MjError::new(
                ErrorKind::InvalidOperation,
                format!("unknown env variable {var_name}"),
            ))
        },
    );
    env.set_auto_escape_callback(|_name: &str| AutoEscape::Json);
    env
}

/// The context every data model template is rendered with.
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    /// `COMPILE_CONTEXT` — the multi-tenant compile context.
    pub compile_context: JsonValue,
    /// Additional top-level variables.
    pub variables: JsonValue,
}

impl TemplateContext {
    fn to_json(&self) -> JsonValue {
        let mut map = serde_json::Map::new();
        if let JsonValue::Object(vars) = &self.variables {
            for (k, v) in vars {
                map.insert(k.clone(), v.clone());
            }
        }
        map.insert("COMPILE_CONTEXT".to_string(), self.compile_context.clone());
        JsonValue::Object(map)
    }
}

/// True when the source looks like it contains Jinja syntax and therefore has to
/// be rendered before being parsed as YAML.
pub fn has_jinja_syntax(source: &str) -> bool {
    source.contains("{{") || source.contains("{%") || source.contains("{#")
}

/// Renders a single template.
pub fn render(
    env: &Environment<'static>,
    source: &str,
    ctx: &TemplateContext,
) -> Result<String, MjError> {
    env.render_str(source, ctx.to_json())
}

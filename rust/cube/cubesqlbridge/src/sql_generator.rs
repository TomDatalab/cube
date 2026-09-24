//! The [`SqlGenerator`] the SQL API renders push-down SQL through.
//!
//! Node hands cubesql a JS `BaseQuery` subclass wrapped in `NodeSqlGenerator`;
//! this is the same template set taken straight from [`cubeplanner`]'s
//! dialects, with no JS object behind it.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cubeplanner::Dialect;
use cubesql::transport::{SqlGenerator, SqlTemplates};
use cubesql::CubeError;
use minijinja::Environment;

/// The flattened `"<group>/<name>" -> Jinja template` map of one dialect.
///
/// A name the dialect deliberately unset is simply absent, which is how a
/// dialect says "I have no template for this".
pub fn dialect_templates(dialect: Dialect) -> HashMap<String, String> {
    dialect.templates().templates_map().clone()
}

/// `shouldReuseParams`: whether one bound value may back several placeholders.
///
/// It can only be true when the placeholder names which parameter it is, so it
/// is read off the dialect's own `params/param` template rather than kept as a
/// second list that could disagree with it.
pub fn should_reuse_params(templates: &HashMap<String, String>) -> bool {
    templates
        .get("params/param")
        .is_some_and(|template| template.contains("param_index"))
}

/// A [`SqlGenerator`] over one [`Dialect`].
#[derive(Debug)]
pub struct RustSqlGenerator {
    dialect: Dialect,
    templates: Arc<SqlTemplates>,
    jinja: Environment<'static>,
}

impl RustSqlGenerator {
    pub fn new(dialect: Dialect) -> Result<Self, CubeError> {
        let raw = dialect_templates(dialect);

        let mut jinja = Environment::new();
        for (name, template) in raw.iter() {
            jinja
                .add_template_owned(name.to_string(), template.to_string())
                .map_err(|e| {
                    CubeError::internal(format!("Error parsing template {name} '{template}': {e}"))
                })?;
        }

        let reuse_params = should_reuse_params(&raw);

        Ok(Self {
            dialect,
            templates: Arc::new(SqlTemplates::new(raw, reuse_params)?),
            jinja,
        })
    }

    pub fn dialect(&self) -> Dialect {
        self.dialect
    }
}

#[async_trait]
impl SqlGenerator for RustSqlGenerator {
    fn get_sql_templates(&self) -> Arc<SqlTemplates> {
        self.templates.clone()
    }

    /// Renders one named template. The Node bridge leaves this unimplemented
    /// because cubesql renders through [`SqlTemplates`] instead; it is
    /// implemented here so a caller that does reach for it gets SQL rather
    /// than a panic.
    async fn call_template(
        &self,
        name: String,
        params: HashMap<String, String>,
    ) -> Result<String, CubeError> {
        self.jinja
            .get_template(&name)
            .map_err(|e| CubeError::internal(format!("Error getting {name} template: {e}")))?
            .render(minijinja::Value::from_serialize(&params))
            .map_err(|e| CubeError::internal(format!("Error rendering {name} template: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialect_produces_a_usable_template_set() {
        for dialect in Dialect::ALL {
            let generator = RustSqlGenerator::new(dialect)
                .unwrap_or_else(|e| panic!("{dialect} templates should compile: {e}"));
            let templates = generator.get_sql_templates();

            assert!(
                templates.contains_template("statements/select"),
                "{dialect} should render a SELECT"
            );
            assert!(
                templates.contains_template("params/param"),
                "{dialect} should render parameters"
            );
            assert_eq!(generator.dialect(), dialect);
        }
    }

    #[test]
    fn parameter_reuse_follows_the_dialect() {
        // Postgres numbers its placeholders, so one value can back several.
        assert!(should_reuse_params(&dialect_templates(Dialect::Postgres)));
        // CubeStore and MySQL render positional `?`, so each value is bound
        // once, in order.
        assert!(!should_reuse_params(&dialect_templates(Dialect::CubeStore)));
        assert!(!should_reuse_params(&dialect_templates(Dialect::MySql)));
    }

    #[test]
    fn dialect_deltas_reach_the_template_map() {
        let postgres = dialect_templates(Dialect::Postgres);
        let cubestore = dialect_templates(Dialect::CubeStore);

        assert_eq!(
            postgres.get("params/param").map(String::as_str),
            Some("${{ param_index + 1 }}")
        );
        assert_eq!(cubestore.get("params/param").map(String::as_str), Some("?"));
        assert_ne!(
            postgres.get("statements/time_series_select"),
            cubestore.get("statements/time_series_select")
        );
    }

    #[tokio::test]
    async fn call_template_renders_with_its_parameters() {
        let generator = RustSqlGenerator::new(Dialect::Postgres).expect("templates compile");

        let rendered = generator
            .call_template(
                "expressions/binary".to_string(),
                HashMap::from([
                    ("left".to_string(), "a".to_string()),
                    ("op".to_string(), "=".to_string()),
                    ("right".to_string(), "b".to_string()),
                ]),
            )
            .await
            .expect("the template should render");

        assert!(
            rendered.contains('a') && rendered.contains('b'),
            "{rendered}"
        );
    }

    #[tokio::test]
    async fn call_template_names_a_template_it_does_not_have() {
        let generator = RustSqlGenerator::new(Dialect::Postgres).expect("templates compile");

        let error = generator
            .call_template("nope/nothing".to_string(), HashMap::new())
            .await
            .expect_err("an unknown template should fail");
        assert!(error.message.contains("nope/nothing"), "{}", error.message);
    }
}

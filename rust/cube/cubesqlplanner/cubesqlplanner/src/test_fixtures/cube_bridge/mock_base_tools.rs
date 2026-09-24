use crate::cube_bridge::base_query_options::FilterValue;
use crate::cube_bridge::base_tools::BaseTools;
use crate::cube_bridge::driver_tools::DriverTools;
use crate::cube_bridge::join_definition::JoinDefinition;
use crate::cube_bridge::join_hints::JoinHintItem;
use crate::cube_bridge::member_sql::{CompiledMemberTemplate, MemberSql};
use crate::cube_bridge::pre_aggregation_obj::PreAggregationObj;
use crate::cube_bridge::security_context::SecurityContext;
use crate::cube_bridge::sql_templates_render::SqlTemplatesRender;
use crate::cube_bridge::sql_utils::SqlUtils;
use crate::planner::sql_templates::PlanSqlTemplates;
use crate::test_fixtures::cube_bridge::{
    MockDriverTools, MockJoinGraph, MockMemberSql, MockSqlTemplatesRender, MockSqlUtils,
};
use cubenativeutils::CubeError;
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;
use typed_builder::TypedBuilder;

/// Mock implementation of BaseTools for testing
///
/// This mock provides implementations for driver_tools, sql_templates,
/// security_context_for_rust, and sql_utils_for_rust.
/// Methods that depend on the orchestrator return a `CubeError` instead.
///
/// ```
#[derive(Clone, TypedBuilder)]
pub struct MockBaseTools {
    #[builder(default = Rc::new(MockDriverTools::new()) as Rc<dyn DriverTools>)]
    driver_tools: Rc<dyn DriverTools>,

    /// Driver tools returned for `driver_tools(external: true)` — the
    /// external pre-aggregations dialect (CubeStore in production).
    #[builder(default)]
    external_driver_tools: Option<Rc<dyn DriverTools>>,

    #[builder(default = Rc::new(MockSqlTemplatesRender::default_templates()))]
    sql_templates: Rc<MockSqlTemplatesRender>,

    #[builder(default = Rc::new(MockSqlUtils))]
    sql_utils: Rc<MockSqlUtils>,

    #[builder(default = Rc::new(MockJoinGraph::new()))]
    join_graph: Rc<MockJoinGraph>,

    /// Map of cube_name -> Vec<member_name> for all_cube_members
    #[builder(default = HashMap::new())]
    cube_members: HashMap<String, Vec<String>>,

    /// The request's security context, as JSON. Member sql that reads
    /// `SECURITY_CONTEXT` is resolved against it while the planner compiles
    /// the model, the way the JS compiler resolves it.
    #[builder(default = Value::Null)]
    security_context_json: Value,
}

impl MockBaseTools {
    pub fn set_external_driver_tools(&mut self, tools: Rc<dyn DriverTools>) {
        self.external_driver_tools = Some(tools);
    }

    pub fn set_security_context_json(&mut self, security_context: Value) {
        self.security_context_json = security_context;
    }
}

impl Default for MockBaseTools {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl BaseTools for MockBaseTools {
    fn as_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn driver_tools(&self, external: bool) -> Result<Rc<dyn DriverTools>, CubeError> {
        if external {
            if let Some(tools) = &self.external_driver_tools {
                return Ok(tools.clone());
            }
        }
        Ok(self.driver_tools.clone())
    }

    fn sql_templates(&self) -> Result<Rc<dyn SqlTemplatesRender>, CubeError> {
        Ok(self.sql_templates.clone())
    }

    fn sql_utils_for_rust(&self) -> Result<Rc<dyn SqlUtils>, CubeError> {
        Ok(self.sql_utils.clone())
    }

    fn get_allocated_params(&self) -> Result<Vec<FilterValue>, CubeError> {
        Ok(vec![])
    }

    fn all_cube_members(&self, path: String) -> Result<Vec<String>, CubeError> {
        Ok(self
            .cube_members
            .get(&path)
            .cloned()
            .unwrap_or_else(Vec::new))
    }

    /// Port of `BaseQuery.intervalAndMinimalTimeUnit` + `diffTimeUnitForInterval`
    /// (`packages/cubejs-schema-compiler/src/adapter/BaseQuery.js:2182,4176`).
    /// Reachable from rolling windows, so it must never panic.
    fn interval_and_minimal_time_unit(&self, interval: String) -> Result<Vec<String>, CubeError> {
        let lower = interval.to_lowercase();
        let minimal_time_unit = if lower.contains("second") {
            "second"
        } else if lower.contains("minute") {
            "minute"
        } else if lower.contains("hour") {
            "hour"
        // A week is diffed in days, like the JS implementation.
        } else if lower.contains("day") || lower.contains("week") {
            "day"
        // A quarter is diffed in months, like the JS implementation.
        } else if lower.contains("month") || lower.contains("quarter") {
            "month"
        } else {
            "year"
        };

        Ok(vec![interval, minimal_time_unit.to_string()])
    }

    /// Pre-aggregation objects are built by the orchestrator, which the Rust
    /// model does not own yet. Returns an error instead of panicking so a
    /// query that reaches this path fails the request, not the process.
    fn get_pre_aggregation_by_name(
        &self,
        cube_name: String,
        name: String,
    ) -> Result<Rc<dyn PreAggregationObj>, CubeError> {
        Err(CubeError::internal(format!(
            "Pre-aggregation objects are not available in the Rust model yet: {}.{}",
            cube_name, name
        )))
    }

    fn pre_aggregation_table_name(
        &self,
        cube_name: String,
        name: String,
    ) -> Result<String, CubeError> {
        let key = format!("{}.{}", cube_name, name);
        Ok(PlanSqlTemplates::alias_name(&key))
    }

    fn join_tree_for_hints(
        &self,
        hints: Vec<JoinHintItem>,
    ) -> Result<Rc<dyn JoinDefinition>, CubeError> {
        let result = self.join_graph.build_join(hints)?;
        Ok(result as Rc<dyn JoinDefinition>)
    }

    fn compile_member_sql(
        &self,
        member_sql: Rc<dyn MemberSql>,
        _security_context: Rc<dyn SecurityContext>,
        _arg_names: Vec<String>,
    ) -> Result<CompiledMemberTemplate, CubeError> {
        let mock = member_sql
            .as_any()
            .downcast::<MockMemberSql>()
            .map_err(|_| CubeError::internal("MockBaseTools expects MockMemberSql".to_string()))?;
        mock.compile(
            &self.security_context_json,
            Some(self.driver_tools.as_ref()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BaseQuery.diffTimeUnitForInterval` — the minimal unit an interval is
    /// diffed in. A week is diffed in days and a quarter in months.
    #[test]
    fn interval_and_minimal_time_unit_matches_base_query() {
        let tools = MockBaseTools::default();

        for (interval, expected) in [
            ("30 second", "second"),
            ("5 minutes", "minute"),
            ("2 HOURS", "hour"),
            ("7 day", "day"),
            ("1 week", "day"),
            ("3 month", "month"),
            ("1 quarter", "month"),
            ("2 year", "year"),
            // Anything unrecognized falls through to year, like the `else`.
            ("unparseable", "year"),
        ] {
            let result = tools
                .interval_and_minimal_time_unit(interval.to_string())
                .expect("never fails");
            assert_eq!(
                result,
                vec![interval.to_string(), expected.to_string()],
                "{interval}"
            );
        }
    }

    /// Reaching a pre-aggregation object must fail the request, not abort the
    /// process.
    #[test]
    fn get_pre_aggregation_by_name_errors_instead_of_panicking() {
        let tools = MockBaseTools::default();
        // `PreAggregationObj` is not `Debug`, so unwrap the Result by hand.
        let err = match tools.get_pre_aggregation_by_name("orders".to_string(), "main".to_string())
        {
            Ok(_) => panic!("pre-aggregation objects are not available in the Rust model yet"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("orders.main"), "{err}");
    }
}

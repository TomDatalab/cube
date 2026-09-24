//! The `cube_bridge` implementations the planner talks to, bound to one
//! request.

use crate::dialect::Dialect;
use crate::error::PlannerError;
use crate::model::Model;
use crate::options::PlanOptions;
use cubesqlplanner::cube_bridge::base_tools::BaseTools;
use cubesqlplanner::cube_bridge::evaluator::CubeEvaluator;
use cubesqlplanner::cube_bridge::join_graph::JoinGraph;
use cubesqlplanner::cube_bridge::security_context::SecurityContext;
use cubesqlplanner::rust_model::{
    MockBaseTools, MockCubeEvaluator, MockJoinGraph, MockSecurityContext,
};
use std::rc::Rc;

/// Everything the planner reaches the data model through: the `CubeEvaluator`
/// that answers member lookups, the `BaseTools` that compile member SQL and
/// resolve dialect behaviour, and the `JoinGraph` that finds join paths.
///
/// One `Evaluator` belongs to one request: it carries that request's security
/// context and timezone, because both are baked into the SQL that member `sql`
/// declarations compile to.
pub struct Evaluator {
    cube_evaluator: Rc<MockCubeEvaluator>,
    base_tools: Rc<MockBaseTools>,
    join_graph: Rc<MockJoinGraph>,
    security_context: Rc<dyn SecurityContext>,
}

impl Evaluator {
    /// Binds `model` to one request's dialect, timezone and security context.
    pub fn new(model: &Model, options: &PlanOptions, timezone: &str) -> Result<Self, PlannerError> {
        let schema = model.schema();

        let mut base_tools =
            schema.create_base_tools_with_dyn_driver(options.dialect.driver_tools(timezone))?;
        base_tools.set_security_context_json(options.security_context.clone());
        if let Some(external) = external_dialect(options) {
            base_tools.set_external_driver_tools(external.driver_tools(timezone));
        }

        Ok(Self {
            cube_evaluator: schema.clone().create_evaluator(),
            base_tools: Rc::new(base_tools),
            join_graph: Rc::new(schema.create_join_graph()?),
            security_context: Rc::new(MockSecurityContext),
        })
    }

    pub fn cube_evaluator(&self) -> Rc<dyn CubeEvaluator> {
        self.cube_evaluator.clone()
    }

    pub fn base_tools(&self) -> Rc<dyn BaseTools> {
        self.base_tools.clone()
    }

    pub fn join_graph(&self) -> Rc<dyn JoinGraph> {
        self.join_graph.clone()
    }

    pub fn security_context(&self) -> Rc<dyn SecurityContext> {
        self.security_context.clone()
    }
}

/// The dialect external pre-aggregations are read through, if any. A query
/// fully covered by external rollups renders in it instead of the source
/// dialect.
fn external_dialect(options: &PlanOptions) -> Option<Dialect> {
    options
        .external_dialect
        .filter(|external| *external != options.dialect)
}

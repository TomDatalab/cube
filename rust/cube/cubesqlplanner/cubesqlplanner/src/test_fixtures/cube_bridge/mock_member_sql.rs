use crate::cube_bridge::driver_tools::DriverTools;
use crate::cube_bridge::member_sql::{
    CompiledMemberTemplate, MemberSql, SqlTemplate, SqlTemplateArgs,
};
use crate::test_fixtures::cube_bridge::member_sql_parser::ParsedMemberSql;
use cubenativeutils::CubeError;
use serde_json::Value;
use std::any::Any;
use std::rc::Rc;

/// Test helper: extract the compiled `(template, args)` from a member sql,
/// as it compiles with an empty security context.
pub fn mock_compiled(sql: Rc<dyn MemberSql>) -> (SqlTemplate, SqlTemplateArgs) {
    let c = sql
        .as_any()
        .downcast::<MockMemberSql>()
        .expect("expected MockMemberSql")
        .compiled()
        .expect("member sql should compile");
    (c.template, c.args)
}

/// A member's `sql:` as written in the data model, parsed once by
/// [`ParsedMemberSql`]. The security context and the dialect are per request,
/// so binding them is left to [`MockMemberSql::compile`], which `BaseTools`
/// calls while the planner walks the model.
#[derive(Debug)]
pub struct MockMemberSql {
    parsed: ParsedMemberSql,
}

impl MockMemberSql {
    /// Parses one member sql, e.g. `"{CUBE.field} / {other_cube.field}"`.
    pub fn new(template: impl Into<String>) -> Result<Self, CubeError> {
        Ok(Self {
            parsed: ParsedMemberSql::parse(&template.into())?,
        })
    }

    /// Pre-aggregation single reference: `"orders.created_at"`.
    pub fn pre_agg_single_ref(member_path: impl Into<String>) -> Result<Self, CubeError> {
        Ok(Self {
            parsed: ParsedMemberSql::parse_reference(&member_path.into())?,
        })
    }

    /// Pre-aggregation reference list: `["orders.status", "line_items.id"]`.
    pub fn pre_agg_array_refs(member_paths: Vec<impl Into<String>>) -> Result<Rc<Self>, CubeError> {
        let paths: Vec<String> = member_paths.into_iter().map(|p| p.into()).collect();
        if paths.is_empty() {
            return Err(CubeError::user(
                "Pre-aggregation array references cannot be empty".to_string(),
            ));
        }
        Self::pre_agg_array_templates(paths)
    }

    /// Pre-aggregation reference list whose elements may interpolate the cube
    /// itself: `["{CUBE}.count", "{CUBE.status}", "city"]`.
    pub fn pre_agg_array_templates(members: Vec<String>) -> Result<Rc<Self>, CubeError> {
        Ok(Rc::new(Self {
            parsed: ParsedMemberSql::parse_reference_list(&members)?,
        }))
    }

    pub fn parsed(&self) -> &ParsedMemberSql {
        &self.parsed
    }

    /// Compiles with no security context and no dialect — enough for every
    /// member sql that references neither.
    pub fn compiled(&self) -> Result<CompiledMemberTemplate, CubeError> {
        self.compile(&Value::Null, None)
    }

    /// Compiles for one request.
    pub fn compile(
        &self,
        security_context: &Value,
        driver_tools: Option<&dyn DriverTools>,
    ) -> Result<CompiledMemberTemplate, CubeError> {
        self.parsed.compile(security_context, driver_tools)
    }
}

impl MemberSql for MockMemberSql {
    fn args_names(&self) -> &Vec<String> {
        self.parsed.args_names()
    }

    fn as_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(sql: &MockMemberSql) -> (SqlTemplate, SqlTemplateArgs, Vec<String>) {
        let compiled = sql.compiled().unwrap();
        (compiled.template, compiled.args, sql.args_names().clone())
    }

    #[test]
    fn test_simple_path() {
        let (template, args, names) = parts(&MockMemberSql::new("{CUBE.field}").unwrap());

        assert_eq!(template, SqlTemplate::String("{arg:0}".to_string()));
        assert_eq!(args.symbol_paths, vec![vec!["CUBE", "field"]]);
        assert_eq!(names, vec!["CUBE"]);
    }

    #[test]
    fn test_multiple_paths() {
        let (template, args, names) =
            parts(&MockMemberSql::new("{CUBE.field} / {other_cube.field}").unwrap());

        assert_eq!(
            template,
            SqlTemplate::String("{arg:0} / {arg:1}".to_string())
        );
        assert_eq!(args.symbol_paths[0], vec!["CUBE", "field"]);
        assert_eq!(args.symbol_paths[1], vec!["other_cube", "field"]);
        assert_eq!(names, vec!["CUBE", "other_cube"]);
    }

    #[test]
    fn test_nested_path() {
        let (template, args, names) =
            parts(&MockMemberSql::new("{other_cube.cube2.field}").unwrap());

        assert_eq!(template, SqlTemplate::String("{arg:0}".to_string()));
        assert_eq!(args.symbol_paths[0], vec!["other_cube", "cube2", "field"]);
        assert_eq!(names, vec!["other_cube"]);
    }

    #[test]
    fn test_complex_expression() {
        let (template, args, names) = parts(
            &MockMemberSql::new("{CUBE.field} / {other_cube.cube2.field} + {revenue}").unwrap(),
        );

        assert_eq!(
            template,
            SqlTemplate::String("{arg:0} / {arg:1} + {arg:2}".to_string())
        );
        assert_eq!(args.symbol_paths.len(), 3);
        assert_eq!(args.symbol_paths[2], vec!["revenue"]);
        assert_eq!(names, vec!["CUBE", "other_cube", "revenue"]);
    }

    #[test]
    fn test_duplicate_paths() {
        let (template, args, _) =
            parts(&MockMemberSql::new("{CUBE.field} + {CUBE.field}").unwrap());

        assert_eq!(
            template,
            SqlTemplate::String("{arg:0} + {arg:0}".to_string())
        );
        assert_eq!(args.symbol_paths.len(), 1);
    }

    #[test]
    fn test_same_top_level_different_paths() {
        let (template, args, names) =
            parts(&MockMemberSql::new("{CUBE.field1} + {CUBE.field2}").unwrap());

        assert_eq!(
            template,
            SqlTemplate::String("{arg:0} + {arg:1}".to_string())
        );
        assert_eq!(args.symbol_paths.len(), 2);
        assert_eq!(names, vec!["CUBE"]);
    }

    #[test]
    fn test_with_text() {
        let (template, args, _) = parts(&MockMemberSql::new("SUM({CUBE.amount}) * 100").unwrap());

        assert_eq!(
            template,
            SqlTemplate::String("SUM({arg:0}) * 100".to_string())
        );
        assert_eq!(args.symbol_paths[0], vec!["CUBE", "amount"]);
    }

    #[test]
    fn test_escaped_braces() {
        // `{{` / `}}` are the f-string escapes for literal braces, as in the
        // YAML the JS compiler transpiles.
        let (template, args, _) = parts(&MockMemberSql::new("{{literal}} {CUBE.field}").unwrap());

        assert_eq!(
            template,
            SqlTemplate::String("{literal} {arg:0}".to_string())
        );
        assert_eq!(args.symbol_paths[0], vec!["CUBE", "field"]);
    }

    #[test]
    fn test_unclosed_brace_error() {
        let result = MockMemberSql::new("{CUBE.field");

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Unclosed brace"));
    }

    #[test]
    fn test_empty_path_error() {
        let result = MockMemberSql::new("{}");

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Empty"));
    }

    #[test]
    fn test_compile_template_sql() {
        let mock = Rc::new(MockMemberSql::new("{CUBE.field} / {other.field}").unwrap());
        let (template, args) = mock_compiled(mock);

        assert_eq!(
            template,
            SqlTemplate::String("{arg:0} / {arg:1}".to_string())
        );
        assert_eq!(args.symbol_paths[0], vec!["CUBE", "field"]);
        assert_eq!(args.symbol_paths[1], vec!["other", "field"]);
    }

    #[test]
    fn test_pre_agg_single_ref() {
        let (template, args, names) =
            parts(&MockMemberSql::pre_agg_single_ref("orders.created_at").unwrap());

        assert_eq!(template, SqlTemplate::String("{arg:0}".to_string()));
        assert_eq!(args.symbol_paths[0], vec!["orders", "created_at"]);
        assert_eq!(names, vec!["orders"]);
    }

    #[test]
    fn test_pre_agg_single_ref_with_joins() {
        let (template, args, names) =
            parts(&MockMemberSql::pre_agg_single_ref("users.orders.created_at").unwrap());

        assert_eq!(template, SqlTemplate::String("{arg:0}".to_string()));
        assert_eq!(args.symbol_paths[0], vec!["users", "orders", "created_at"]);
        assert_eq!(names, vec!["users"]);
    }

    #[test]
    fn test_pre_agg_array_refs_simple() {
        let mock =
            MockMemberSql::pre_agg_array_refs(vec!["orders.status", "orders.amount"]).unwrap();
        let (template, args, names) = parts(&mock);

        assert_eq!(
            template,
            SqlTemplate::StringVec(vec!["{arg:0}".to_string(), "{arg:1}".to_string()])
        );
        assert_eq!(args.symbol_paths[0], vec!["orders", "status"]);
        assert_eq!(args.symbol_paths[1], vec!["orders", "amount"]);
        assert_eq!(names, vec!["orders"]);
    }

    #[test]
    fn test_pre_agg_array_refs_multiple_cubes() {
        let mock = MockMemberSql::pre_agg_array_refs(vec![
            "orders.status",
            "line_items.product_id",
            "orders.amount",
        ])
        .unwrap();
        let (template, args, names) = parts(&mock);

        assert_eq!(
            template,
            SqlTemplate::StringVec(vec![
                "{arg:0}".to_string(),
                "{arg:1}".to_string(),
                "{arg:2}".to_string()
            ])
        );
        assert_eq!(args.symbol_paths[1], vec!["line_items", "product_id"]);
        assert_eq!(names, vec!["orders", "line_items"]);
    }

    #[test]
    fn test_pre_agg_array_refs_with_joins() {
        let mock =
            MockMemberSql::pre_agg_array_refs(vec!["visitors.aaa.dim_1", "visitors.bbb.dim2"])
                .unwrap();
        let (_, args, names) = parts(&mock);

        assert_eq!(args.symbol_paths[0], vec!["visitors", "aaa", "dim_1"]);
        assert_eq!(args.symbol_paths[1], vec!["visitors", "bbb", "dim2"]);
        assert_eq!(names, vec!["visitors"]);
    }

    #[test]
    fn test_pre_agg_array_refs_empty_error() {
        let empty_vec: Vec<String> = vec![];
        let result = MockMemberSql::pre_agg_array_refs(empty_vec);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("cannot be empty"));
    }

    #[test]
    fn test_pre_agg_array_refs_compile_to_string_vec() {
        let mock =
            MockMemberSql::pre_agg_array_refs(vec!["orders.status", "line_items.product_id"])
                .unwrap();

        let (template, args) = mock_compiled(mock);

        assert_eq!(
            template,
            SqlTemplate::StringVec(vec!["{arg:0}".to_string(), "{arg:1}".to_string()])
        );
        assert_eq!(args.symbol_paths[0], vec!["orders", "status"]);
        assert_eq!(args.symbol_paths[1], vec!["line_items", "product_id"]);
    }
}

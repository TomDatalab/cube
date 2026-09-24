//! SQL text helpers shared by the drivers (port of the query builders in
//! `BaseDriver.ts`). They are plain functions so that they can be unit tested
//! and reused by trait default implementations.

use crate::types::{Column, GenericType, SchemaTable};

/// Schemas excluded from introspection by every driver.
pub const SYSTEM_SCHEMAS: &str =
    "'pg_catalog', 'information_schema', 'mysql', 'performance_schema', 'sys', 'INFORMATION_SCHEMA'";

/// `"identifier"` quoting used by `BaseDriver.quoteIdentifier`.
pub fn quote_identifier(identifier: &str) -> String {
    format!("\"{identifier}\"")
}

/// `BaseDriver.informationSchemaQuery`.
pub fn information_schema_query(quote: &dyn Fn(&str) -> String) -> String {
    format!(
        "
      SELECT columns.column_name as {},
             columns.table_name as {},
             columns.table_schema as {},
             columns.data_type as {}
      FROM information_schema.columns
      WHERE columns.table_schema NOT IN ({SYSTEM_SCHEMAS})
   ",
        quote("column_name"),
        quote("table_name"),
        quote("table_schema"),
        quote("data_type"),
    )
}

/// `BaseDriver.getSchemasQuery`.
pub fn get_schemas_query(quote: &dyn Fn(&str) -> String) -> String {
    format!(
        "
      SELECT table_schema as {}
      FROM information_schema.tables
      WHERE table_schema NOT IN ({SYSTEM_SCHEMAS})
      GROUP BY table_schema
    ",
        quote("schema_name"),
    )
}

/// `BaseDriver.getTablesForSpecificSchemasQuery`.
pub fn get_tables_for_specific_schemas_query(
    quote: &dyn Fn(&str) -> String,
    schemas_placeholders: &str,
) -> String {
    format!(
        "
      SELECT table_schema as {},
            table_name as {}
      FROM information_schema.tables as columns
      WHERE table_schema IN ({schemas_placeholders})
    ",
        quote("schema_name"),
        quote("table_name"),
    )
}

/// `BaseDriver.getColumnsForSpecificTablesQuery`.
pub fn get_columns_for_specific_tables_query(
    quote: &dyn Fn(&str) -> String,
    condition_string: &str,
) -> String {
    format!(
        "
      SELECT columns.column_name as {},
             columns.table_name as {},
             columns.table_schema as {},
             columns.data_type as {}
      FROM information_schema.columns as columns
      WHERE {condition_string}
    ",
        quote("column_name"),
        quote("table_name"),
        quote("schema_name"),
        quote("data_type"),
    )
}

/// Builds the `(schema = $1 AND table IN ($2, $3)) OR (...)` condition of
/// `getColumnsForSpecificTables`, returning the condition and its parameters.
pub fn columns_for_specific_tables_condition(
    tables: &[SchemaTable],
    param: &dyn Fn(usize) -> String,
    schema_column: &str,
    table_column: &str,
) -> (String, Vec<String>) {
    // Group by schema, preserving first-seen order (JS object key order).
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for t in tables {
        match grouped.iter_mut().find(|(s, _)| *s == t.schema_name) {
            Some((_, names)) => names.push(t.table_name.clone()),
            None => grouped.push((t.schema_name.clone(), vec![t.table_name.clone()])),
        }
    }

    let mut conditions = Vec::new();
    let mut parameters: Vec<String> = Vec::new();
    for (schema, table_names) in grouped {
        let schema_placeholder = param(parameters.len());
        parameters.push(schema);
        let table_placeholders = table_names
            .iter()
            .enumerate()
            .map(|(idx, _)| param(parameters.len() + idx))
            .collect::<Vec<_>>()
            .join(", ");
        parameters.extend(table_names);
        conditions.push(format!(
            "({schema_column} = {schema_placeholder} AND {table_column} IN ({table_placeholders}))"
        ));
    }

    (conditions.join(" OR "), parameters)
}

/// `BaseDriver.tableColumnTypes` query.
pub fn table_column_types_query(
    quote: &dyn Fn(&str) -> String,
    param: &dyn Fn(usize) -> String,
    fetch_by_ordinal_position: bool,
    with_precision: bool,
) -> String {
    let precision = if with_precision {
        format!(
            ",
             columns.numeric_precision as {},
             columns.numeric_scale as {}",
            quote("numeric_precision"),
            quote("numeric_scale")
        )
    } else {
        String::new()
    };
    let order = if fetch_by_ordinal_position {
        "ORDER BY columns.ordinal_position"
    } else {
        ""
    };
    format!(
        "SELECT columns.column_name as {},
             columns.table_name as {},
             columns.table_schema as {},
             columns.data_type  as {}{precision}
      FROM information_schema.columns
      WHERE table_name = {} AND table_schema = {}
      {order}",
        quote("column_name"),
        quote("table_name"),
        quote("table_schema"),
        quote("data_type"),
        param(0),
        param(1),
    )
}

/// `BaseDriver.createTableSql`.
pub fn create_table_sql(
    quoted_table_name: &str,
    columns: &[Column],
    quote: &dyn Fn(&str) -> String,
    from_generic_type: &dyn Fn(&GenericType) -> String,
) -> String {
    let column_names = columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), from_generic_type(&c.type_)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("CREATE TABLE {quoted_table_name} ({column_names})")
}

/// `BaseDriver.wrapQueryWithLimit`.
pub fn wrap_query_with_limit(query: &str, limit: u64) -> String {
    format!("SELECT * FROM ({query}) AS t LIMIT {limit}")
}

/// `INSERT INTO t (cols) VALUES (params)` used by the generic row upload.
pub fn insert_row_sql(
    table: &str,
    columns: &[Column],
    quote: &dyn Fn(&str) -> String,
    param: &dyn Fn(usize) -> String,
) -> String {
    format!(
        "INSERT INTO {table}
          ({})
          VALUES ({})",
        columns
            .iter()
            .map(|c| quote(&c.name))
            .collect::<Vec<_>>()
            .join(", "),
        columns
            .iter()
            .enumerate()
            .map(|(i, _)| param(i))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg_param(i: usize) -> String {
        format!("${}", i + 1)
    }

    #[test]
    fn create_table() {
        let sql = create_table_sql(
            "test.t",
            &[Column::new("id", "bigint"), Column::new("name", "text")],
            &quote_identifier,
            &|t| t.to_string(),
        );
        assert_eq!(sql, r#"CREATE TABLE test.t ("id" bigint, "name" text)"#);
    }

    #[test]
    fn wrap_limit() {
        assert_eq!(
            wrap_query_with_limit("SELECT 1", 10),
            "SELECT * FROM (SELECT 1) AS t LIMIT 10"
        );
    }

    #[test]
    fn columns_condition_groups_by_schema() {
        let tables = vec![
            SchemaTable {
                schema_name: "public".into(),
                table_name: "a".into(),
            },
            SchemaTable {
                schema_name: "other".into(),
                table_name: "b".into(),
            },
            SchemaTable {
                schema_name: "public".into(),
                table_name: "c".into(),
            },
        ];
        let (cond, params) = columns_for_specific_tables_condition(
            &tables,
            &pg_param,
            "columns.table_schema",
            "columns.table_name",
        );
        assert_eq!(
            cond,
            "(columns.table_schema = $1 AND columns.table_name IN ($2, $3)) OR (columns.table_schema = $4 AND columns.table_name IN ($5))"
        );
        assert_eq!(params, vec!["public", "a", "c", "other", "b"]);
    }

    #[test]
    fn table_column_types_sql() {
        let sql = table_column_types_query(&quote_identifier, &pg_param, true, true);
        assert!(sql.contains("columns.numeric_precision as \"numeric_precision\""));
        assert!(sql.contains("WHERE table_name = $1 AND table_schema = $2"));
        assert!(sql
            .trim_end()
            .ends_with("ORDER BY columns.ordinal_position"));
        let sql = table_column_types_query(&quote_identifier, &pg_param, false, false);
        assert!(!sql.contains("numeric_precision"));
        assert!(!sql.contains("ORDER BY"));
    }

    #[test]
    fn information_schema_queries_alias_columns() {
        let q = information_schema_query(&quote_identifier);
        assert!(q.contains("columns.table_schema as \"table_schema\""));
        assert!(q.contains(SYSTEM_SCHEMAS));
        let q = get_schemas_query(&quote_identifier);
        assert!(q.contains("GROUP BY table_schema"));
        let q = get_tables_for_specific_schemas_query(&quote_identifier, "$1, $2");
        assert!(q.contains("WHERE table_schema IN ($1, $2)"));
        let q = get_columns_for_specific_tables_query(&quote_identifier, "1 = 1");
        assert!(q.contains("columns.table_schema as \"schema_name\""));
        assert!(q.contains("WHERE 1 = 1"));
    }

    #[test]
    fn insert_row() {
        let sql = insert_row_sql(
            "t",
            &[Column::new("a", "int"), Column::new("b", "text")],
            &quote_identifier,
            &pg_param,
        );
        assert!(sql.starts_with("INSERT INTO t"));
        assert!(sql.contains("(\"a\", \"b\")"));
        assert!(sql.contains("VALUES ($1, $2)"));
    }
}

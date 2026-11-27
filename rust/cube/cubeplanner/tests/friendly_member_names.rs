//! Members named with friendly names, spaces included ("Quantidade Total"):
//! queries, references in SQL, joins and views.

use cubeplanner::{plan, Dialect, Model, PlanOptions, PlannerQuery};
use serde_json::json;

const YAML: &str = r#"
cubes:
  - name: bossa
    sql_table: dfos.fibercable
    joins:
      - name: conexao
        relationship: one_to_many
        sql: "{CUBE.Id Fibra} = {conexao.Cabo}"
    dimensions:
      - name: Id Fibra
        sql: id
        type: string
        primary_key: true
      - name: Fabricante do Cabo
        sql: manufacturer
        type: string
      - name: Data de Fabricação
        sql: manufacturing_date
        type: time
      - name: Fibras
        sql: fiber_count
        type: number
    measures:
      - name: Quantidade Total
        type: count
      - name: Total de Fibras
        sql: "{CUBE}.fiber_count"
        type: sum
      - name: Fibras por Cabo
        sql: "{Total de Fibras} / NULLIF({bossa.Quantidade Total}, 0)"
        type: number
    segments:
      - name: Só Corning
        sql: "{CUBE.Fabricante do Cabo} = 'Corning'"
  - name: conexao
    sql_table: dfos.fiberconnection
    dimensions:
      - name: Id Conexao
        sql: id
        type: string
        primary_key: true
      - name: Cabo
        sql: id
        type: string
      - name: Fabricante
        sql: "{bossa.Fabricante do Cabo}"
        type: string
    measures:
      - name: Perda Total
        sql: insertion_loss_db
        type: sum
views:
  - name: visao
    cubes:
      - join_path: bossa
        includes: "*"
      - join_path: bossa.conexao
        prefix: true
        includes:
          - Perda Total
"#;

fn model() -> Model {
    Model::from_yaml_str(YAML).unwrap_or_else(|e| panic!("model: {e}"))
}

fn sql(q: serde_json::Value) -> String {
    let q = PlannerQuery::from_value(q).unwrap_or_else(|e| panic!("query: {e}"));
    let opts = PlanOptions::postgres().with_dialect(Dialect::for_db_type("clickhouse").unwrap());
    plan(&model(), &q, &opts).unwrap_or_else(|e| panic!("plan: {e}")).sql
}

#[test]
fn a_loads() {
    model();
}

fn sql_of(q: serde_json::Value) -> String {
    sql(q)
}

#[test]
fn queries_by_friendly_names() {
    let s = sql_of(json!({"measures": ["bossa.Quantidade Total"], "dimensions": ["bossa.Fabricante do Cabo"],
                          "order": [{"id": "bossa.Quantidade Total", "desc": true}]}));
    assert!(s.contains("count(`bossa`.id) `bossa___quantidade total`"), "{s}");
    assert!(s.contains("`bossa`.manufacturer `bossa___fabricante do cabo`"), "{s}");
}

#[test]
fn time_dimension_filter_and_segment() {
    let s = sql_of(json!({"measures": ["bossa.Total de Fibras"],
                          "timeDimensions": [{"dimension": "bossa.Data de Fabricação", "granularity": "month",
                                              "dateRange": ["2025-01-01", "2025-12-31"]}],
                          "filters": [{"member": "bossa.Fabricante do Cabo", "operator": "equals", "values": ["AFL"]}],
                          "segments": ["bossa.Só Corning"]}));
    assert!(s.contains("`bossa___data de fabricação_month`"), "{s}");
    assert!(s.contains("(`bossa`.manufacturer = 'Corning')"), "{s}");
}

#[test]
fn references_with_spaces() {
    let s = sql_of(json!({"measures": ["bossa.Fibras por Cabo"]}));
    assert!(s.contains("sum(`bossa`.fiber_count) / NULLIF(count(`bossa`.id), 0)"), "{s}");
    let s = sql_of(json!({"dimensions": ["conexao.Fabricante"]}));
    assert!(s.contains("`bossa`.manufacturer"), "{s}");
}

#[test]
fn join_on_friendly_names() {
    let s = sql_of(json!({"measures": ["conexao.Perda Total", "bossa.Quantidade Total"],
                          "dimensions": ["bossa.Fabricante do Cabo"]}));
    assert!(s.contains("ON `bossa`.id = `conexao`.id"), "{s}");
}

#[test]
fn views_include_friendly_names() {
    let s = sql_of(json!({"measures": ["visao.Quantidade Total", "visao.conexao_Perda Total"],
                          "dimensions": ["visao.Fabricante do Cabo"]}));
    assert!(s.contains("`visao___quantidade total`"), "{s}");
    assert!(s.contains("insertion_loss_db"), "{s}");
}

//! Models: tables on a canvas, linked by their columns, built into ONE cube
//! whose `sql` JOINs them. Pure: [`validate`] checks the shape, [`build`]
//! checks it against the connection's columns and writes the cube YAML.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value as Yaml};
use uuid::Uuid;

use crate::handlers::scaffold::{cube_type, quote, snake};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Definition {
    pub data_source: String,
    pub tables: Vec<ModelTable>,
    pub primary: Option<String>,
    pub primary_key: Option<String>,
    pub joins: Vec<ModelJoin>,
}

/// A table on the canvas. The same table dragged twice gets two aliases.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelTable {
    pub alias: String,
    pub schema: String,
    pub table: String,
    pub x: f64,
    pub y: f64,
}

/// A link between two columns: `relationship` is one_to_one, one_to_many or
/// many_to_one, read from `from` to `to`; `join` is left or inner.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelJoin {
    pub from: End,
    pub to: End,
    pub relationship: String,
    pub join: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct End {
    pub alias: String,
    pub column: String,
}

const RELATIONSHIPS: [&str; 3] = ["one_to_one", "one_to_many", "many_to_one"];
const JOINS: [&str; 2] = ["left", "inner"];

/// The shape alone, valid while drafting: unique non-empty aliases, a
/// `primary` among them, links between known aliases with known values, and
/// a tree (no second path between two tables) that reaches every table from
/// `primary` once it is chosen. A model with no tables is a valid draft.
pub fn validate(def: &Definition) -> Result<(), String> {
    let mut aliases = HashSet::new();
    for table in &def.tables {
        if table.alias.trim().is_empty() {
            return Err("Toda tabela precisa de um alias".into());
        }
        if !aliases.insert(table.alias.as_str()) {
            return Err(format!("O alias {} se repete", table.alias));
        }
    }
    if let Some(primary) = &def.primary {
        if !aliases.contains(primary.as_str()) {
            return Err(format!("A tabela principal {primary} não está no model"));
        }
    }

    // Union-find: a link between two tables already connected closes a cycle.
    let mut parent: HashMap<&str, &str> = aliases.iter().map(|a| (*a, *a)).collect();
    fn root<'a>(parent: &HashMap<&'a str, &'a str>, mut a: &'a str) -> &'a str {
        while parent[a] != a {
            a = parent[a];
        }
        a
    }
    for join in &def.joins {
        for end in [&join.from, &join.to] {
            if !aliases.contains(end.alias.as_str()) {
                return Err(format!(
                    "A ligação usa a tabela {}, que não está no model",
                    end.alias
                ));
            }
        }
        if !RELATIONSHIPS.contains(&join.relationship.as_str()) {
            return Err(format!("Relacionamento inválido: {}", join.relationship));
        }
        if !JOINS.contains(&join.join.as_str()) {
            return Err(format!("JOIN inválido: {}", join.join));
        }
        let (a, b) = (
            root(&parent, &join.from.alias),
            root(&parent, &join.to.alias),
        );
        if a == b {
            return Err(format!(
                "{} e {} já estão ligadas",
                join.from.alias, join.to.alias
            ));
        }
        parent.insert(a, b);
    }

    if let Some(primary) = &def.primary {
        let reached = root(&parent, primary);
        if let Some(table) = def
            .tables
            .iter()
            .find(|t| root(&parent, &t.alias) != reached)
        {
            return Err(format!(
                "A tabela {} não está ligada à principal",
                table.alias
            ));
        }
    }
    Ok(())
}

/// A join oriented from the primary: `from` is already in the tree.
struct Step<'a> {
    from: &'a End,
    to: &'a End,
    relationship: &'a str,
    join: &'a str,
}

/// The cube YAML for `def`, checked against the connection's `columns`
/// (keyed by plain schema name), preceded by the header line.
pub fn build(
    name: &str,
    model_id: Uuid,
    cube_name: &str,
    def: &Definition,
    columns: &cubedriver::DatabaseStructure,
    db_type: &str,
) -> Result<String, String> {
    validate(def)?;
    let primary = def.primary.as_deref().ok_or("Escolha a tabela principal")?;
    let primary_key = def
        .primary_key
        .as_deref()
        .ok_or("Escolha a chave primária")?;

    let mut tables = HashMap::new();
    for table in &def.tables {
        let cols = columns
            .get(&table.schema)
            .and_then(|t| t.get(&table.table))
            .ok_or_else(|| {
                format!(
                    "A tabela {}.{} não existe na conexão {}",
                    table.schema, table.table, def.data_source
                )
            })?;
        tables.insert(table.alias.as_str(), (table, cols));
    }
    let column_of = |end: &End| {
        tables[end.alias.as_str()]
            .1
            .iter()
            .find(|c| c.name == end.column)
            .ok_or_else(|| format!("A coluna {}.{} não existe", end.alias, end.column))
    };
    for join in &def.joins {
        column_of(&join.from)?;
        column_of(&join.to)?;
    }
    let key = column_of(&End {
        alias: primary.into(),
        column: primary_key.into(),
    })?;
    if cube_type(&key.type_).is_none() {
        return Err(format!(
            "A chave primária {primary_key} não tem tipo no Cube"
        ));
    }

    // Breadth-first from the primary; validate made the links a tree.
    let mut order = vec![primary];
    let mut visited = HashSet::from([primary]);
    let mut steps = Vec::new();
    let mut queue = VecDeque::from([primary]);
    while let Some(alias) = queue.pop_front() {
        for join in &def.joins {
            let step = if join.from.alias == alias && !visited.contains(join.to.alias.as_str()) {
                Step {
                    from: &join.from,
                    to: &join.to,
                    relationship: &join.relationship,
                    join: &join.join,
                }
            } else if join.to.alias == alias && !visited.contains(join.from.alias.as_str()) {
                Step {
                    from: &join.to,
                    to: &join.from,
                    relationship: match join.relationship.as_str() {
                        "one_to_many" => "many_to_one",
                        "many_to_one" => "one_to_many",
                        other => other,
                    },
                    join: &join.join,
                }
            } else {
                continue;
            };
            visited.insert(step.to.alias.as_str());
            queue.push_back(step.to.alias.as_str());
            order.push(step.to.alias.as_str());
            steps.push(step);
        }
    }
    let multiplies = steps.iter().any(|s| s.relationship == "one_to_many");

    let q = |ident: &str| quote(db_type, ident);
    let mut members = BTreeSet::from(["count".to_string()]);
    let mut select = Vec::new();
    let mut dimensions = Vec::new();
    let mut key_sql = String::new();
    for alias in &order {
        let (_, cols) = tables[alias];
        for column in cols {
            let Some(cube_type) = cube_type(&column.type_) else {
                continue;
            };
            let base = format!("{}_{}", snake(alias), snake(&column.name));
            let mut member = base.clone();
            let mut n = 2;
            while !members.insert(member.clone()) {
                member = format!("{base}_{n}");
                n += 1;
            }
            select.push(format!(
                "{}.{} AS {}",
                q(alias),
                q(&column.name),
                q(&member)
            ));

            let sql = format!("{{CUBE}}.{}", q(&member));
            let mut dimension = Mapping::new();
            dimension.insert("name".into(), member.into());
            dimension.insert("sql".into(), sql.clone().into());
            dimension.insert("type".into(), cube_type.into());
            if *alias == primary && column.name == primary_key {
                dimension.insert("primary_key".into(), true.into());
                key_sql = sql;
            }
            dimensions.push(Yaml::Mapping(dimension));
        }
    }

    let from = |alias: &str| {
        let (table, _) = tables[alias];
        format!("{}.{} AS {}", q(&table.schema), q(&table.table), q(alias))
    };
    let mut sql = vec![
        format!("SELECT {}", select.join(", ")),
        format!("FROM {}", from(primary)),
    ];
    for step in &steps {
        sql.push(format!(
            "{} JOIN {} ON {}.{} = {}.{}",
            step.join.to_ascii_uppercase(),
            from(&step.to.alias),
            q(&step.from.alias),
            q(&step.from.column),
            q(&step.to.alias),
            q(&step.to.column),
        ));
    }

    let mut cube = Mapping::new();
    cube.insert("name".into(), cube_name.into());
    if def.data_source != "default" {
        cube.insert("data_source".into(), def.data_source.as_str().into());
    }
    cube.insert("sql".into(), sql.join("\n").into());
    cube.insert("dimensions".into(), Yaml::Sequence(dimensions));
    let mut count = Mapping::new();
    count.insert("name".into(), "count".into());
    if multiplies {
        // A one_to_many repeats the primary's rows: count its key, once.
        count.insert("type".into(), "count_distinct".into());
        count.insert("sql".into(), key_sql.into());
    } else {
        count.insert("type".into(), "count".into());
    }
    cube.insert(
        "measures".into(),
        Yaml::Sequence(vec![Yaml::Mapping(count)]),
    );

    let mut doc = Mapping::new();
    doc.insert("cubes".into(), Yaml::Sequence(vec![Yaml::Mapping(cube)]));
    let yaml = serde_yaml::to_string(&doc).map_err(|e| e.to_string())?;
    Ok(format!(
        "# Built from model {name} ({model_id}): Build cube rewrites this file\n{yaml}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubedriver::SchemaColumn;

    fn col(name: &str, type_: &str) -> SchemaColumn {
        SchemaColumn {
            name: name.into(),
            type_: type_.into(),
            attributes: vec![],
            foreign_keys: vec![],
        }
    }

    fn columns() -> cubedriver::DatabaseStructure {
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(
            "orders".to_string(),
            vec![
                col("id", "int"),
                col("customer_id", "int"),
                col("Order ID", "text"),
                col("order_id", "int"),
            ],
        );
        tables.insert(
            "customers".to_string(),
            vec![col("id", "int"), col("region", "text")],
        );
        let mut db = cubedriver::DatabaseStructure::new();
        db.insert("public".to_string(), tables);
        db
    }

    fn table(alias: &str, table: &str) -> ModelTable {
        ModelTable {
            alias: alias.into(),
            schema: "public".into(),
            table: table.into(),
            x: 0.0,
            y: 0.0,
        }
    }

    fn join(from: (&str, &str), to: (&str, &str), relationship: &str, kind: &str) -> ModelJoin {
        ModelJoin {
            from: End {
                alias: from.0.into(),
                column: from.1.into(),
            },
            to: End {
                alias: to.0.into(),
                column: to.1.into(),
            },
            relationship: relationship.into(),
            join: kind.into(),
        }
    }

    /// orders (primary, pk id) many_to_one customers, left.
    fn orders_customers() -> Definition {
        Definition {
            data_source: "default".into(),
            tables: vec![table("orders", "orders"), table("customers", "customers")],
            primary: Some("orders".into()),
            primary_key: Some("id".into()),
            joins: vec![join(
                ("orders", "customer_id"),
                ("customers", "id"),
                "many_to_one",
                "left",
            )],
        }
    }

    fn run(def: &Definition) -> Result<String, String> {
        build("Sales", Uuid::nil(), "sales", def, &columns(), "postgres")
    }

    fn cube(yaml: &str) -> Yaml {
        let doc: Yaml = serde_yaml::from_str(yaml).unwrap();
        doc["cubes"][0].clone()
    }

    fn member<'a>(cube: &'a Yaml, kind: &str, name: &str) -> &'a Yaml {
        cube[kind]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|m| m["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("no {kind} {name} in {cube:?}"))
    }

    fn sql(cube: &Yaml) -> String {
        cube["sql"].as_str().unwrap().to_string()
    }

    #[test]
    fn builds_a_left_join_cube() {
        let yaml = run(&orders_customers()).unwrap();
        assert!(yaml.starts_with(&format!(
            "# Built from model Sales ({}): Build cube rewrites this file\n",
            Uuid::nil()
        )));
        let cube = cube(&yaml);
        assert_eq!(cube["name"].as_str(), Some("sales"));
        let sql = sql(&cube);
        assert!(
            sql.contains(r#"FROM "public"."orders" AS "orders""#),
            "{sql}"
        );
        assert!(
            sql.contains(
                r#"LEFT JOIN "public"."customers" AS "customers" ON "orders"."customer_id" = "customers"."id""#
            ),
            "{sql}"
        );
        assert!(
            sql.contains(r#""customers"."region" AS "customers_region""#),
            "{sql}"
        );
        let region = member(&cube, "dimensions", "customers_region");
        assert_eq!(region["type"].as_str(), Some("string"));
        assert_eq!(region["sql"].as_str(), Some(r#"{CUBE}."customers_region""#));
        let id = member(&cube, "dimensions", "orders_id");
        assert_eq!(id["primary_key"].as_bool(), Some(true));
        assert_eq!(id["type"].as_str(), Some("number"));
        assert_eq!(
            member(&cube, "dimensions", "customers_id")["primary_key"].as_bool(),
            None
        );
        let count = member(&cube, "measures", "count");
        assert_eq!(count["type"].as_str(), Some("count"));
        assert!(count["sql"].is_null());
    }

    #[test]
    fn a_one_to_many_join_counts_distinct() {
        let def = Definition {
            primary: Some("customers".into()),
            joins: vec![join(
                ("customers", "id"),
                ("orders", "customer_id"),
                "one_to_many",
                "left",
            )],
            ..orders_customers()
        };
        let cube = cube(&run(&def).unwrap());
        assert!(sql(&cube).contains(r#"FROM "public"."customers" AS "customers""#));
        let count = member(&cube, "measures", "count");
        assert_eq!(count["type"].as_str(), Some("count_distinct"));
        assert_eq!(count["sql"].as_str(), Some(r#"{CUBE}."customers_id""#));
    }

    #[test]
    fn a_backwards_join_is_flipped() {
        let def = Definition {
            joins: vec![join(
                ("customers", "id"),
                ("orders", "customer_id"),
                "one_to_many",
                "left",
            )],
            ..orders_customers()
        };
        let cube = cube(&run(&def).unwrap());
        let sql = sql(&cube);
        assert!(
            sql.contains(r#"FROM "public"."orders" AS "orders""#),
            "{sql}"
        );
        assert!(
            sql.contains(
                r#"LEFT JOIN "public"."customers" AS "customers" ON "orders"."customer_id" = "customers"."id""#
            ),
            "{sql}"
        );
        assert_eq!(
            member(&cube, "measures", "count")["type"].as_str(),
            Some("count")
        );
    }

    #[test]
    fn an_inner_join_is_inner() {
        let mut def = orders_customers();
        def.joins[0].join = "inner".into();
        let sql = sql(&cube(&run(&def).unwrap()));
        assert!(
            sql.contains(r#"INNER JOIN "public"."customers" AS "customers""#),
            "{sql}"
        );
    }

    #[test]
    fn colliding_names_get_a_suffix() {
        let cube = cube(&run(&orders_customers()).unwrap());
        let sql = sql(&cube);
        assert!(
            sql.contains(r#""orders"."Order ID" AS "orders_order_id""#),
            "{sql}"
        );
        assert!(
            sql.contains(r#""orders"."order_id" AS "orders_order_id_2""#),
            "{sql}"
        );
        assert_eq!(
            member(&cube, "dimensions", "orders_order_id")["type"].as_str(),
            Some("string")
        );
        assert_eq!(
            member(&cube, "dimensions", "orders_order_id_2")["type"].as_str(),
            Some("number")
        );
    }

    #[test]
    fn the_same_table_twice_uses_both_aliases() {
        let def = Definition {
            tables: vec![
                table("orders", "orders"),
                table("customers", "customers"),
                table("customers_2", "customers"),
            ],
            joins: vec![
                join(
                    ("orders", "customer_id"),
                    ("customers", "id"),
                    "many_to_one",
                    "left",
                ),
                join(
                    ("orders", "order_id"),
                    ("customers_2", "id"),
                    "many_to_one",
                    "left",
                ),
            ],
            ..orders_customers()
        };
        let cube = cube(&run(&def).unwrap());
        let sql = sql(&cube);
        assert!(
            sql.contains(
                r#"LEFT JOIN "public"."customers" AS "customers_2" ON "orders"."order_id" = "customers_2"."id""#
            ),
            "{sql}"
        );
        member(&cube, "dimensions", "customers_region");
        member(&cube, "dimensions", "customers_2_region");
    }

    #[test]
    fn validate_refuses_bad_shapes() {
        let base = orders_customers();
        assert_eq!(validate(&base), Ok(()));

        let mut unlinked = base.clone();
        unlinked.tables.push(table("x", "customers"));
        assert_eq!(
            validate(&unlinked),
            Err("A tabela x não está ligada à principal".into())
        );

        let mut twice = base.clone();
        twice.joins.push(join(
            ("customers", "id"),
            ("orders", "id"),
            "one_to_one",
            "left",
        ));
        assert_eq!(
            validate(&twice),
            Err("customers e orders já estão ligadas".into())
        );

        let mut cycle = base.clone();
        cycle.tables.push(table("c2", "customers"));
        cycle.joins.push(join(
            ("orders", "order_id"),
            ("c2", "id"),
            "many_to_one",
            "left",
        ));
        cycle.joins.push(join(
            ("c2", "id"),
            ("customers", "id"),
            "one_to_one",
            "left",
        ));
        assert_eq!(
            validate(&cycle),
            Err("c2 e customers já estão ligadas".into())
        );

        let mut relationship = base.clone();
        relationship.joins[0].relationship = "many_to_many".into();
        assert!(validate(&relationship).is_err());

        let mut kind = base.clone();
        kind.joins[0].join = "full".into();
        assert!(validate(&kind).is_err());

        let mut duplicate = base.clone();
        duplicate.tables.push(table("orders", "customers"));
        assert!(validate(&duplicate).is_err());

        let mut empty_alias = base.clone();
        empty_alias.tables[1].alias = "".into();
        assert!(validate(&empty_alias).is_err());

        let mut unknown_primary = base.clone();
        unknown_primary.primary = Some("nope".into());
        assert!(validate(&unknown_primary).is_err());

        let mut unknown_alias = base.clone();
        unknown_alias.joins[0].to.alias = "nope".into();
        assert!(validate(&unknown_alias).is_err());
    }

    #[test]
    fn an_empty_model_is_a_valid_draft() {
        let def = Definition {
            data_source: "default".into(),
            tables: vec![],
            primary: None,
            primary_key: None,
            joins: vec![],
        };
        assert_eq!(validate(&def), Ok(()));
    }

    #[test]
    fn build_requires_primary_and_key_and_existing_columns() {
        let mut no_primary = orders_customers();
        no_primary.primary = None;
        assert_eq!(run(&no_primary), Err("Escolha a tabela principal".into()));

        let mut no_key = orders_customers();
        no_key.primary_key = None;
        assert_eq!(run(&no_key), Err("Escolha a chave primária".into()));

        let mut no_column = orders_customers();
        no_column.joins[0].to.column = "nope".into();
        assert_eq!(
            run(&no_column),
            Err("A coluna customers.nope não existe".into())
        );

        let mut no_key_column = orders_customers();
        no_key_column.primary_key = Some("nope".into());
        assert_eq!(
            run(&no_key_column),
            Err("A coluna orders.nope não existe".into())
        );

        let mut no_table = orders_customers();
        no_table.tables[1].table = "gone".into();
        assert_eq!(
            run(&no_table),
            Err("A tabela public.gone não existe na conexão default".into())
        );
    }

    #[test]
    fn an_untyped_primary_key_is_refused() {
        let mut db = columns();
        db.get_mut("public")
            .unwrap()
            .get_mut("orders")
            .unwrap()
            .push(col("tags", "text[]"));
        let mut def = orders_customers();
        def.primary_key = Some("tags".into());
        assert_eq!(
            build("Sales", Uuid::nil(), "sales", &def, &db, "postgres"),
            Err("A chave primária tags não tem tipo no Cube".into())
        );
    }

    #[test]
    fn the_data_source_line() {
        let yaml = run(&orders_customers()).unwrap();
        assert!(!yaml.contains("data_source"), "{yaml}");

        let def = Definition {
            data_source: "pg".into(),
            ..orders_customers()
        };
        let yaml = run(&def).unwrap();
        assert!(yaml.contains("data_source: pg"), "{yaml}");
    }
}

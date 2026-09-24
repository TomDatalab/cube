//! The expectations the Node dialect tests pin, ported to the Rust dialects:
//! `packages/cubejs-schema-compiler/test/unit/*-query.test.ts`,
//! `dialect-intervals.test.ts`, and the driver packages' `*Query.test.ts`.

use cubeplanner::{
    plan, Dialect, DriverTools, Model, PlanOptions, PlannedSql, PlannerError, PlannerQuery,
};
use serde_json::json;
use std::rc::Rc;

const MODEL: &str = r#"
cubes:
  - name: visitors
    sql_table: visitors

    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true

      - name: name
        sql: name
        type: string

      - name: is_active
        sql: is_active
        type: boolean

      - name: amount
        sql: amount
        type: number

      - name: created_at
        sql: created_at
        type: time
        granularities:
          - name: ten_seconds
            interval: 10 seconds

    measures:
      - name: count
        type: count
"#;

fn plan_in(dialect: Dialect, query: serde_json::Value) -> Result<PlannedSql, PlannerError> {
    let model = Model::from_yaml_str(MODEL).unwrap();
    let query = PlannerQuery::from_value(query).unwrap();
    plan(
        &model,
        &query,
        &PlanOptions::postgres().with_dialect(dialect),
    )
}

fn sql_in(dialect: Dialect, query: serde_json::Value) -> String {
    plan_in(dialect, query)
        .unwrap_or_else(|e| panic!("{dialect}: {e}"))
        .sql
}

fn tools(dialect: Dialect) -> Rc<dyn DriverTools> {
    dialect.driver_tools("UTC")
}

fn sub(dialect: Dialect, interval: &str) -> String {
    tools(dialect)
        .subtract_interval("x".to_string(), interval.to_string())
        .unwrap_or_else(|e| panic!("{dialect} {interval}: {}", e.message))
}

fn add(dialect: Dialect, interval: &str) -> String {
    tools(dialect)
        .add_interval("x".to_string(), interval.to_string())
        .unwrap_or_else(|e| panic!("{dialect} {interval}: {}", e.message))
}

fn assert_unsupported<T: std::fmt::Debug>(
    result: Result<T, impl Into<PlannerError>>,
    dialect: Dialect,
    needle: &str,
) {
    let err: PlannerError = result.expect_err("should be refused").into();
    assert!(matches!(err, PlannerError::Unsupported(_)), "{err:?}");
    assert!(
        err.message()
            .contains(&format!("Unsupported by the {} dialect", dialect.as_str())),
        "{err}"
    );
    assert!(err.message().contains(needle), "{err}");
}

fn equals_filter(member: &str, values: serde_json::Value) -> serde_json::Value {
    json!({
        "measures": ["visitors.count"],
        "filters": [{ "member": member, "operator": "equals", "values": values }]
    })
}

// --- Parameters ---------------------------------------------------------------

/// `PrestodbFilter.castParameter` / `FireboltFilter` / `DremioFilter` /
/// `PinotFilter` / `DuckDBFilter`: a boolean or number filter value is cast
/// where the dialect needs it, and left alone everywhere else.
#[test]
fn params_are_cast_the_way_each_filter_casts_them() {
    for (dialect, boolean, number) in [
        (Dialect::Presto, "CAST(? AS BOOLEAN)", "CAST(? AS DOUBLE)"),
        (Dialect::Trino, "CAST(? AS BOOLEAN)", "CAST(? AS DOUBLE)"),
        (Dialect::Firebolt, "CAST(? AS BOOLEAN)", "= ?)"),
        (Dialect::Dremio, "CAST(? AS BOOLEAN)", "CAST(? AS DOUBLE)"),
        (Dialect::Pinot, "CAST(? AS BOOLEAN)", "CAST(? AS DOUBLE)"),
        (Dialect::DuckDb, "= ?)", "CAST(? AS DOUBLE)"),
        // `BaseQuery` casts nothing; `::boolean` is Postgres-only syntax, and
        // Redshift reads `::numeric` as NUMERIC(18,0).
        (Dialect::MySql, "= ?)", "= ?)"),
        (Dialect::Snowflake, "= ?)", "= ?)"),
        (Dialect::Redshift, "= $1)", "= $1)"),
        (Dialect::Crate, "= $1)", "= $1)"),
    ] {
        let sql = sql_in(
            dialect,
            equals_filter("visitors.is_active", json!(["true"])),
        );
        assert!(sql.contains(boolean), "{dialect} boolean: {sql}");
        let sql = sql_in(dialect, equals_filter("visitors.amount", json!(["10.5"])));
        assert!(sql.contains(number), "{dialect} number: {sql}");
    }

    // prestodb-query.test.ts: 'bool param cast (PrestoQuery)'
    let sql = sql_in(
        Dialect::Presto,
        equals_filter("visitors.is_active", json!(["true"])),
    );
    assert!(
        sql.contains("\"visitors\".is_active = CAST(? AS BOOLEAN)"),
        "{sql}"
    );
    // FireboltQuery.test.ts / DremioQuery.test.ts: 'should cast BOOLEAN'
    for dialect in [Dialect::Firebolt, Dialect::Dremio] {
        let planned = plan_in(
            dialect,
            equals_filter("visitors.is_active", json!(["true"])),
        )
        .unwrap();
        assert!(
            planned
                .sql
                .contains("(\"visitors\".is_active = CAST(? AS BOOLEAN))"),
            "{dialect}: {}",
            planned.sql
        );
        assert_eq!(planned.param_strings(), vec![Some("true".to_string())]);
    }
}

fn contains_filter(value: &str) -> serde_json::Value {
    json!({
        "dimensions": ["visitors.name"],
        "filters": [{ "member": "visitors.name", "operator": "contains", "values": [value] }]
    })
}

/// prestodb-query.test.ts: 'escapes literal backslashes in LIKE filter
/// parameters', and DuckDBQueryTemplates.test.ts: a dialect without a default
/// LIKE escape character carries the ESCAPE clause the escaped value needs.
#[test]
fn like_filters_escape_and_say_so() {
    let planned = plan_in(Dialect::Presto, contains_filter("folder\\name_%")).unwrap();
    assert!(planned.sql.contains("ESCAPE '\\'"), "{}", planned.sql);
    assert_eq!(
        planned.param_strings(),
        vec![Some("folder\\\\name\\_\\%".to_string())]
    );

    let planned = plan_in(Dialect::DuckDb, contains_filter("%")).unwrap();
    assert_eq!(planned.param_strings(), vec![Some("\\%".to_string())]);
    assert!(
        planned.sql.contains("ILIKE '%' || ?|| '%' ESCAPE '\\'"),
        "{}",
        planned.sql
    );

    for dialect in [
        Dialect::Oracle,
        Dialect::Sqlite,
        Dialect::Druid,
        Dialect::Dremio,
    ] {
        let sql = sql_in(dialect, contains_filter("demo"));
        assert!(sql.contains("ESCAPE '\\'"), "{dialect}: {sql}");
        assert!(!sql.contains("ILIKE"), "{dialect} has no ILIKE: {sql}");
    }

    // druid-query.test.ts: 'druid query like test'
    let sql = sql_in(Dialect::Druid, contains_filter("demo"));
    assert!(
        sql.contains("LOWER(\"visitors\".name) LIKE CONCAT('%', LOWER(?), '%')"),
        "{sql}"
    );
    // HiveFilter.likeIgnoreCase: LIKE CONCAT, since Hive has no ILIKE.
    let sql = sql_in(Dialect::Hive, contains_filter("demo"));
    assert!(
        sql.contains("`visitors`.name LIKE CONCAT('%', ?, '%')"),
        "{sql}"
    );
    // QuestQuery.test.ts: 'test query like'
    let sql = sql_in(Dialect::QuestDb, contains_filter("demo"));
    assert!(sql.contains("ILIKE '%' || $1"), "{sql}");
}

// --- Time zones and time dimensions -------------------------------------------

fn by_day(timezone: &str) -> serde_json::Value {
    json!({
        "measures": ["visitors.count"],
        "timeDimensions": [{
            "dimension": "visitors.created_at",
            "granularity": "day",
            "dateRange": ["2024-02-01", "2024-02-02"]
        }],
        "timezone": timezone
    })
}

/// athena-query.test.ts and trino-presto-date-time-dimension.test.ts: Athena
/// and PrestoDB add the zone's offset by hand, Trino converts with AT TIME
/// ZONE, and both lift a DATE with COALESCE rather than a cast.
#[test]
fn presto_family_converts_time_zones_its_own_way() {
    let sql = sql_in(Dialect::Presto, by_day("Asia/Kolkata"));
    assert!(sql.contains("timezone_hour"), "{sql}");
    assert!(sql.contains("timezone_minute"), "{sql}");
    assert!(
        !sql.contains("AT TIME ZONE 'Asia/Kolkata') AS TIMESTAMP)"),
        "{sql}"
    );

    let sql = sql_in(Dialect::Trino, by_day("Asia/Kolkata"));
    assert!(!sql.contains("timezone_hour"), "{sql}");
    assert!(
        sql.contains(
            "CAST((COALESCE(\"visitors\".created_at, CAST(NULL AS TIMESTAMP)) AT TIME ZONE 'Asia/Kolkata') AS TIMESTAMP)"
        ),
        "{sql}"
    );
}

/// athena-query.test.ts: the custom granularity's origin is a plain
/// TIMESTAMP, so `date_add` does not return a zoned one.
#[test]
fn presto_date_bin_casts_its_origin_to_timestamp() {
    let sql = sql_in(
        Dialect::Presto,
        json!({
            "measures": ["visitors.count"],
            "timeDimensions": [{
                "dimension": "visitors.created_at",
                "granularity": "ten_seconds",
                "dateRange": ["2026-04-11", "2026-04-12"]
            }]
        }),
    );
    assert!(
        sql.contains("CAST(from_iso8601_timestamp('") && sql.contains("') AS TIMESTAMP)"),
        "{sql}"
    );
    assert!(sql.contains("date_add('second',"), "{sql}");
}

/// FireboltQuery.test.ts, DremioQuery.test.ts and druid-query.test.ts.
#[test]
fn driver_dialects_group_and_cast_time_the_way_their_tests_pin() {
    let sql = sql_in(Dialect::Firebolt, by_day("America/Los_Angeles"));
    assert!(
        sql.contains(
            "DATE_TRUNC('DAY', \"visitors\".created_at AT TIME ZONE 'America/Los_Angeles')"
        ),
        "{sql}"
    );
    assert!(
        sql.contains("(\"visitors\".created_at >= ?::timestamptz AND \"visitors\".created_at <= ?::timestamptz)"),
        "{sql}"
    );

    let sql = sql_in(Dialect::Dremio, by_day("America/Los_Angeles"));
    assert!(
        sql.contains(
            "DATE_TRUNC('day', CONVERT_TIMEZONE('America/Los_Angeles', \"visitors\".created_at))"
        ),
        "{sql}"
    );
    assert!(
        sql.contains("(\"visitors\".created_at >= TO_TIMESTAMP(?, 'YYYY-MM-DD\"T\"HH24:MI:SS.FFF') AND \"visitors\".created_at <= TO_TIMESTAMP(?, 'YYYY-MM-DD\"T\"HH24:MI:SS.FFF'))"),
        "{sql}"
    );

    let sql = sql_in(Dialect::Druid, by_day("Europe/Kiev"));
    assert!(
        sql.contains("CAST(TIME_FORMAT(\"visitors\".created_at, 'yyyy-MM-dd HH:mm:ss', 'Europe/Kiev') AS TIMESTAMP)"),
        "{sql}"
    );
}

/// oracle-query.test.ts
#[test]
fn oracle_renders_oracle_sql() {
    let planned = plan_in(
        Dialect::Oracle,
        json!({
            "measures": ["visitors.count"],
            "dimensions": ["visitors.name"],
            "timeDimensions": [{
                "dimension": "visitors.created_at",
                "granularity": "day",
                "dateRange": ["2024-02-01", "2024-02-02"]
            }],
            "limit": 100
        }),
    )
    .unwrap();
    let sql = &planned.sql;

    // 'uses FETCH NEXT syntax instead of LIMIT'
    assert!(sql.contains("FETCH NEXT 100 ROWS ONLY"), "{sql}");
    assert!(!sql.contains("LIMIT"), "{sql}");
    // 'does not use AS keyword in subquery aliases'
    assert!(!sql.contains(" AS \"visitors\""), "{sql}");
    assert!(sql.contains("FROM  visitors  \"visitors\""), "{sql}");
    // 'group by dimensions not indexes'
    let group_by = sql.split("GROUP BY").nth(1).unwrap();
    assert!(
        group_by.trim_start().starts_with("\"visitors\".name"),
        "{sql}"
    );
    assert!(
        sql.contains("TRUNC(\"visitors\".created_at, 'DD')"),
        "{sql}"
    );
    // 'generates TO_TIMESTAMP_TZ with millisecond precision for date range filters'
    assert!(
        sql.contains(
            "\"visitors\".created_at >= TO_TIMESTAMP_TZ(?, 'YYYY-MM-DD\"T\"HH24:MI:SS.FF\"Z\"')"
        ),
        "{sql}"
    );
    assert_eq!(
        planned.param_strings(),
        vec![
            Some("2024-02-01T00:00:00.000".to_string()),
            Some("2024-02-02T23:59:59.999".to_string()),
        ]
    );

    // 'generates TRUNC function for month granularity grouping'
    let sql = sql_in(
        Dialect::Oracle,
        json!({
            "measures": ["visitors.count"],
            "timeDimensions": [{ "dimension": "visitors.created_at", "granularity": "month" }]
        }),
    );
    assert!(
        sql.contains("TRUNC(\"visitors\".created_at, 'MM')"),
        "{sql}"
    );
    assert!(sql.contains("GROUP BY TRUNC("), "{sql}");
}

/// `OracleQuery` / `VerticaQuery` format models: minutes are `MI` (`mm` is
/// the month) and weeks start on Monday (`IW`).
#[test]
fn format_model_truncation_names_the_unit_it_means() {
    for dialect in [Dialect::Oracle, Dialect::Vertica] {
        let tools = tools(dialect);
        let minute = tools
            .time_grouped_column("minute".to_string(), "d".to_string())
            .unwrap();
        assert_eq!(minute, "TRUNC(d, 'MI')", "{dialect}");
        let week = tools
            .time_grouped_column("week".to_string(), "d".to_string())
            .unwrap();
        assert_eq!(week, "TRUNC(d, 'IW')", "{dialect}");
    }
}

/// A granularity a dialect has no expression for is refused by name.
#[test]
fn a_granularity_without_an_expression_is_refused() {
    for dialect in [Dialect::Hive, Dialect::QuestDb] {
        assert_unsupported(
            tools(dialect).time_grouped_column("quarter".to_string(), "d".to_string()),
            dialect,
            "`quarter` granularity",
        );
    }
}

/// QuestQuery.test.ts
#[test]
fn questdb_renders_questdb_sql() {
    // 'test equal filters'
    let sql = sql_in(
        Dialect::QuestDb,
        equals_filter("visitors.name", json!([""])),
    );
    assert!(sql.contains("WHERE (\"visitors\".name = $1)"), "{sql}");
    let sql = sql_in(
        Dialect::QuestDb,
        equals_filter("visitors.name", json!([null])),
    );
    assert!(sql.contains("WHERE (\"visitors\".name = NULL)"), "{sql}");

    // 'test non-positional group by'
    let sql = sql_in(Dialect::QuestDb, by_day("America/Los_Angeles"));
    assert!(
        sql.contains("GROUP BY timestamp_floor('d', to_timezone(\"visitors\".created_at, 'America/Los_Angeles'))"),
        "{sql}"
    );

    // `LIMIT lo, hi`
    let sql = sql_in(
        Dialect::QuestDb,
        json!({ "dimensions": ["visitors.name"], "limit": 10, "offset": 5 }),
    );
    assert!(sql.contains("LIMIT 5, 15"), "{sql}");
    assert!(!sql.contains("OFFSET"), "{sql}");

    // 'test having filter': QuestDB has no HAVING, and the planner cannot
    // rewrite the query around it the way `QuestQuery.baseHaving` does.
    let result = plan_in(
        Dialect::QuestDb,
        json!({
            "dimensions": ["visitors.name"],
            "measures": ["visitors.count"],
            "filters": [{ "member": "visitors.count", "operator": "gt", "values": ["42"] }]
        }),
    );
    assert_unsupported(result, Dialect::QuestDb, "no HAVING clause");
}

/// QuestQuery.test.ts: 'dateBin (custom granularities)'
#[test]
fn questdb_date_bin_shifts_its_origin_back_whole_strides() {
    let tools = tools(Dialect::QuestDb);
    let bin = |interval: &str, origin: &str| {
        tools.date_bin(interval.to_string(), "t".to_string(), origin.to_string())
    };

    assert_eq!(
        bin("6 months", "2024-01-01T00:00:00.000").unwrap(),
        "timestamp_floor('6M', t, dateadd('M', -12288, cast('2024-01-01T00:00:00.000' as timestamp)))"
    );
    assert_eq!(
        bin("2 months", "2024-01-01T00:00:00.000").unwrap(),
        "timestamp_floor('2M', t, dateadd('M', -12288, cast('2024-01-01T00:00:00.000' as timestamp)))"
    );
    assert_eq!(
        bin("1 quarter", "2024-01-01T00:00:00.000").unwrap(),
        "timestamp_floor('3M', t, dateadd('M', -12288, cast('2024-01-01T00:00:00.000' as timestamp)))"
    );
    assert_eq!(
        bin("2 years", "2024-01-01T00:00:00.000").unwrap(),
        "timestamp_floor('2y', t, dateadd('y', -1024, cast('2024-01-01T00:00:00.000' as timestamp)))"
    );
    assert_eq!(
        bin("6 months", "0900-06-15T00:00:00.000").unwrap(),
        "timestamp_floor('6M', t, cast('0900-06-15T00:00:00.000' as timestamp))"
    );

    assert_unsupported(
        bin("3 month 3 days 3 hours", "2024-01-01T00:00:00.000"),
        Dialect::QuestDb,
        "single unit",
    );
    let err = bin("6 months", "not-a-timestamp").unwrap_err();
    assert!(
        err.message.contains("unparseable origin"),
        "{}",
        err.message
    );
    assert_unsupported(
        bin("1 second", "2024-01-01T00:00:00.000"),
        Dialect::QuestDb,
        "32-bit range",
    );
}

/// PinotQueryTemplates.test.ts: Pinot expects LIMIT before OFFSET.
#[test]
fn pinot_limits_before_it_offsets() {
    let sql = sql_in(
        Dialect::Pinot,
        json!({ "dimensions": ["visitors.name"], "limit": 10, "offset": 5 }),
    );
    let limit = sql.find("LIMIT 10").expect(&sql);
    let offset = sql.find("OFFSET 5").expect(&sql);
    assert!(limit < offset, "{sql}");
}

/// mongobi-query.test.ts: 'convert_tz implementation', and the BI
/// Connector's lack of window functions and CTEs.
#[test]
fn mongobi_shifts_by_hand_and_refuses_what_the_connector_lacks() {
    let sql = sql_in(Dialect::MongoBi, by_day("America/Los_Angeles"));
    assert!(sql.contains("TIMESTAMPADD(HOUR, -"), "{sql}");
    assert!(!sql.contains("CONVERT_TZ"), "{sql}");

    assert_unsupported(
        Dialect::MongoBi.check_rendered_sql("SELECT rank() OVER (ORDER BY x) FROM t"),
        Dialect::MongoBi,
        "window functions",
    );
    let templates = Dialect::MongoBi.templates();
    let names: Vec<&str> = templates.template_names().collect();
    assert!(!names.contains(&"statements/generated_time_series_select"));
    assert!(!names.contains(&"functions/LAG"));
}

/// HiveQuery quotes with backticks and groups by expression.
#[test]
fn hive_quotes_with_backticks_and_groups_by_expression() {
    let sql = sql_in(
        Dialect::Hive,
        json!({ "measures": ["visitors.count"], "dimensions": ["visitors.name"] }),
    );
    assert!(sql.contains("`visitors`.name `visitors__name`"), "{sql}");
    assert!(sql.contains("GROUP BY `visitors`.name"), "{sql}");
}

// --- Interval arithmetic (dialect-intervals.test.ts) --------------------------

const SINGLE_SHAPES: [&str; 5] = ["3 month", "14 day", "6 hour", "30 minute", "15 second"];
const COMPOUND_SHAPES: [&str; 5] = [
    "3 month 14 day",
    "3 month 6 hour",
    "14 day 6 hour",
    "30 minute 15 second",
    "3 month 14 day 6 hour 30 minute 15 second",
];

/// 'keeps every component of an interval': whatever spelling a dialect picks,
/// a component that went in has to come out — or the dialect refuses the
/// interval by name. Dropping one silently moves rows to the wrong bucket.
#[test]
fn every_dialect_keeps_every_component_of_an_interval() {
    for dialect in Dialect::ALL {
        let tools = tools(dialect);
        for interval in SINGLE_SHAPES.iter().chain(COMPOUND_SHAPES.iter()) {
            let components: Vec<&str> = interval
                .split(' ')
                .filter(|w| w.chars().all(|c| c.is_ascii_digit()))
                .collect();
            for rendered in [
                tools.subtract_interval("x".to_string(), interval.to_string()),
                tools.add_interval("x".to_string(), interval.to_string()),
            ] {
                let rendered = match rendered {
                    Ok(sql) => sql,
                    Err(err) => {
                        let err = PlannerError::from(err);
                        assert!(
                            matches!(err, PlannerError::Unsupported(_)),
                            "{dialect} {interval}: {err:?}"
                        );
                        continue;
                    }
                };
                let numbers: Vec<String> = rendered
                    .split(|c: char| !c.is_ascii_digit())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                for component in &components {
                    assert!(
                        numbers.iter().any(|n| n == component),
                        "{dialect} lost {component} of `{interval}`: {rendered}"
                    );
                }
            }
        }
    }
}

/// The dialects that spell a compound interval by hand, pinned literally.
#[test]
fn compound_intervals_are_spelled_the_way_node_spells_them() {
    for (dialect, cases) in [
        (
            Dialect::Hive,
            vec![
                ("3 month", "(x - INTERVAL '3' month)"),
                (
                    "3 month 14 day",
                    "((x - INTERVAL '3' month) - INTERVAL '14' day)",
                ),
            ],
        ),
        (
            Dialect::Presto,
            vec![
                ("3 month", "x - interval '3' month"),
                (
                    "3 month 14 day",
                    "x - interval '3' month - interval '14' day",
                ),
            ],
        ),
        (
            Dialect::Sqlite,
            vec![
                ("3 month", "strftime('%Y-%m-%dT%H:%M:%f', x, '-3 month')"),
                (
                    "3 month 14 day",
                    "strftime('%Y-%m-%dT%H:%M:%f', x, '-3 month', '-14 day')",
                ),
            ],
        ),
        (
            Dialect::MySql,
            vec![
                ("3 month", "DATE_SUB(x, INTERVAL 3 MONTH)"),
                (
                    "3 month 14 day",
                    "DATE_SUB(DATE_SUB(x, INTERVAL 3 MONTH), INTERVAL 14 DAY)",
                ),
                ("14 day 6 hour", "DATE_SUB(x, INTERVAL '14 6' DAY_HOUR)"),
            ],
        ),
        (
            Dialect::MongoBi,
            vec![("3 month", "DATE_SUB(x, INTERVAL 3 MONTH)")],
        ),
        (
            Dialect::QuestDb,
            vec![
                ("3 month", "dateadd('M', -3, x)"),
                ("3 month 14 day", "dateadd('d', -14, dateadd('M', -3, x))"),
                ("1 quarter", "dateadd('M', -3, x)"),
            ],
        ),
        (
            Dialect::Druid,
            vec![
                ("7 day", "(x - INTERVAL 7 day)"),
                (
                    "3 month 14 day",
                    "((x - INTERVAL 3 month) - INTERVAL 14 day)",
                ),
            ],
        ),
        (
            Dialect::Redshift,
            vec![("3 month 14 day", "DATEADD(day, -14, DATEADD(month, -3, x))")],
        ),
    ] {
        for (interval, expected) in cases {
            assert_eq!(sub(dialect, interval), expected, "{dialect} {interval}");
        }
    }

    // Trino and Athena take their interval handling from Presto.
    assert_eq!(
        sub(Dialect::Trino, "3 month 14 day"),
        "x - interval '3' month - interval '14' day"
    );
    assert_eq!(Dialect::for_db_type("athena").unwrap(), Dialect::Presto);

    // QuestQuery.test.ts, druid-query.test.ts: the directions are opposite.
    assert_eq!(
        add(Dialect::QuestDb, "3 month 14 day"),
        "dateadd('d', 14, dateadd('M', 3, x))"
    );
    assert_eq!(add(Dialect::Druid, "7 day"), "(x + INTERVAL 7 day)");
}

/// oracle-query.test.ts: ADD_MONTHS for calendar units, NUMTODSINTERVAL for
/// the rest.
#[test]
fn oracle_interval_arithmetic() {
    let oracle = |f: fn(Dialect, &str) -> String, interval: &str| {
        f(Dialect::Oracle, interval).replacen('x', "my_date", 1)
    };
    for (interval, expected) in [
        ("1 year", "ADD_MONTHS(my_date, 12)"),
        ("3 years", "ADD_MONTHS(my_date, 36)"),
        ("1 month", "ADD_MONTHS(my_date, 1)"),
        ("1 quarter", "ADD_MONTHS(my_date, 3)"),
        ("4 quarters", "ADD_MONTHS(my_date, 12)"),
        ("7 days", "my_date + NUMTODSINTERVAL(7, 'DAY')"),
        ("24 hours", "my_date + NUMTODSINTERVAL(24, 'HOUR')"),
        ("30 minutes", "my_date + NUMTODSINTERVAL(30, 'MINUTE')"),
        ("45 seconds", "my_date + NUMTODSINTERVAL(45, 'SECOND')"),
        ("1 year 6 months", "ADD_MONTHS(my_date, 18)"),
        ("2 quarters 3 months", "ADD_MONTHS(my_date, 9)"),
        ("2 years 1 quarter 2 months", "ADD_MONTHS(my_date, 29)"),
        (
            "1 day 2 hours",
            "my_date + NUMTODSINTERVAL(1, 'DAY') + NUMTODSINTERVAL(2, 'HOUR')",
        ),
        (
            "1 year 2 days 3 hours",
            "ADD_MONTHS(my_date, 12) + NUMTODSINTERVAL(2, 'DAY') + NUMTODSINTERVAL(3, 'HOUR')",
        ),
        (
            "1 year 2 quarters 3 months 4 days 5 hours 6 minutes 7 seconds",
            "ADD_MONTHS(my_date, 21) + NUMTODSINTERVAL(4, 'DAY') + NUMTODSINTERVAL(5, 'HOUR') + NUMTODSINTERVAL(6, 'MINUTE') + NUMTODSINTERVAL(7, 'SECOND')",
        ),
        // The JS drops a week; it is seven days.
        ("2 weeks", "my_date + NUMTODSINTERVAL(14, 'DAY')"),
    ] {
        assert_eq!(oracle(add, interval), expected, "add {interval}");
    }
    for (interval, expected) in [
        ("1 year", "ADD_MONTHS(my_date, -12)"),
        ("1 quarter", "ADD_MONTHS(my_date, -3)"),
        ("1 year 6 months", "ADD_MONTHS(my_date, -18)"),
        (
            "1 hour 30 minutes 45 seconds",
            "my_date - NUMTODSINTERVAL(1, 'HOUR') - NUMTODSINTERVAL(30, 'MINUTE') - NUMTODSINTERVAL(45, 'SECOND')",
        ),
        (
            "1 year 2 days 3 hours",
            "ADD_MONTHS(my_date, -12) - NUMTODSINTERVAL(2, 'DAY') - NUMTODSINTERVAL(3, 'HOUR')",
        ),
    ] {
        assert_eq!(oracle(sub, interval), expected, "subtract {interval}");
    }

    let tools = tools(Dialect::Oracle);
    assert_eq!(
        tools
            .date_bin("3 months".to_string(), "t".to_string(), "2024-01-01T00:00:00.000".to_string())
            .unwrap(),
        "ADD_MONTHS(TO_TIMESTAMP('2024-01-01T00:00:00.000', 'YYYY-MM-DD\"T\"HH24:MI:SS.FF3'), FLOOR(MONTHS_BETWEEN(t, TO_TIMESTAMP('2024-01-01T00:00:00.000', 'YYYY-MM-DD\"T\"HH24:MI:SS.FF3')) / 3) * 3)"
    );
    assert_unsupported(
        tools.date_bin(
            "1 month 2 days".to_string(),
            "t".to_string(),
            "2024-01-01T00:00:00.000".to_string(),
        ),
        Dialect::Oracle,
        "Mixed month/second intervals".to_lowercase().as_str(),
    );
}

/// `PrestodbQuery.intervalString` spells one unit; the units a Presto
/// INTERVAL has no field for are converted, and a compound interval that
/// mixes calendar and fixed-length units is refused.
#[test]
fn presto_interval_literals() {
    let tools = tools(Dialect::Presto);
    let spell = |interval: &str| tools.interval_string(interval.to_string());
    assert_eq!(spell("1 day").unwrap(), "'1' day");
    assert_eq!(spell("2 week").unwrap(), "'14' day");
    assert_eq!(spell("1 quarter").unwrap(), "'3' month");
    assert_eq!(spell("1 year 6 month").unwrap(), "'18' month");
    assert_eq!(
        spell("1 day 6 hour").unwrap(),
        "'1 06:00:00.000' day to second"
    );
    assert_eq!(spell("1 millisecond").unwrap(), "'0.001' second");
    assert_unsupported(spell("1 month 1 day"), Dialect::Presto, "mixes calendar");

    // `PrestodbQuery.dateBin` takes one date part.
    assert_unsupported(
        tools.date_bin(
            "1 month 1 day".to_string(),
            "t".to_string(),
            "2024-01-01T00:00:00.000".to_string(),
        ),
        Dialect::Presto,
        "one date part",
    );
}

/// `RedshiftQuery.dateBin`: calendar intervals in whole months, fixed-length
/// ones through the Postgres spelling, and never both.
#[test]
fn redshift_date_bin() {
    let tools = tools(Dialect::Redshift);
    let bin = |interval: &str| {
        tools.date_bin(
            interval.to_string(),
            "t".to_string(),
            "2024-01-01T00:00:00.000".to_string(),
        )
    };
    let months = bin("1 year 1 quarter").unwrap();
    assert!(
        months
            .contains("DATEDIFF(month, '2024-01-01T00:00:00.000'::timestamp, t) / 15) * 15)::int"),
        "{months}"
    );
    let days = bin("1 week 2 days").unwrap();
    assert!(
        days.contains("EXTRACT(EPOCH FROM INTERVAL '1 week 2 days')"),
        "{days}"
    );
    assert_unsupported(
        bin("1 month 2 days"),
        Dialect::Redshift,
        "complex intervals",
    );
}

/// The ranged INTERVAL literals BigQuery and Databricks build are diffed in
/// their finest unit (`formatInterval`'s second element), not the range name.
#[test]
fn ranged_intervals_are_diffed_in_their_finest_unit() {
    let bigquery = tools(Dialect::BigQuery)
        .interval_and_minimal_time_unit("1 year 6 month".to_string())
        .unwrap();
    assert_eq!(
        bigquery,
        vec!["'1-6' YEAR TO MONTH".to_string(), "MONTH".to_string()]
    );

    let databricks = tools(Dialect::Databricks)
        .date_bin(
            "1 day 6 hour".to_string(),
            "t".to_string(),
            "2024-01-01T00:00:00.000".to_string(),
        )
        .unwrap();
    assert!(databricks.contains("date_diff(HOUR,"), "{databricks}");
    assert!(
        !databricks.contains("date_diff(DAY TO HOUR"),
        "{databricks}"
    );
}

/// ksqlDB's TIMESTAMPADD takes java TimeUnits; calendar arithmetic is refused
/// by name instead of rendering an interval literal ksqlDB cannot parse.
#[test]
fn ksql_interval_arithmetic() {
    assert_eq!(
        sub(Dialect::Ksql, "1 week 2 hour"),
        "TIMESTAMPSUB(HOURS, 2, TIMESTAMPSUB(DAYS, 7, x))"
    );
    assert_unsupported(
        tools(Dialect::Ksql).add_interval("x".to_string(), "1 month".to_string()),
        Dialect::Ksql,
        "`month` interval arithmetic",
    );
}

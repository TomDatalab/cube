//! The Jinja template set per dialect: `BaseQuery.sqlTemplates()` plus each
//! subclass's delta, exactly as the JS adapters layer them.
//!
//! The shared map in `cubesqlplanner` is `BaseQuery.sqlTemplates()` with the
//! Postgres deltas already folded in, because Postgres was the only dialect it
//! served. [`base_map`] undoes those entries, so every dialect below is a
//! delta over the real base, the way `super.sqlTemplates()` is in JS.

use crate::error::PlannerError;
use cubesqlplanner::rust_model::MockSqlTemplatesRender;
use std::collections::HashMap;

type Map = HashMap<String, String>;

fn set(map: &mut Map, key: &str, template: &str) {
    map.insert(key.to_string(), template.to_string());
}

fn unset(map: &mut Map, key: &str) {
    map.remove(key);
}

fn render(map: Map) -> Result<MockSqlTemplatesRender, PlannerError> {
    MockSqlTemplatesRender::try_new(map)
        .map_err(|e| PlannerError::model(format!("Failed to compile SQL templates: {}", e.message)))
}

/// `BaseQuery.sqlTemplates()` — the set every dialect starts from.
pub fn base_map() -> Map {
    let mut map = MockSqlTemplatesRender::default_templates_map();
    // Undo the Postgres deltas the shared map carries.
    set(&mut map, "params/param", "?");
    set(&mut map, "functions/CONCAT", "CONCAT({{ args_concat }})");
    set(
        &mut map,
        "functions/DATEDIFF",
        "DATEDIFF({{ date_part }}, {{ args[1] }}, {{ args[2] }})",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "CAST('{{ value }}' AS TIMESTAMP)",
    );
    set(&mut map, "types/string", "STRING");
    set(&mut map, "types/tinyint", "TINYINT");
    set(&mut map, "types/float", "FLOAT");
    set(&mut map, "types/double", "DOUBLE");
    set(&mut map, "types/binary", "BINARY");
    unset(&mut map, "statements/generated_time_series_select");
    unset(
        &mut map,
        "statements/generated_time_series_with_cte_range_source",
    );
    uncast_params(&mut map);
    // `BaseQuery`'s SELECT names `WITH RECURSIVE` when the planner asks for
    // it — MySQL's generated time series is a recursive CTE — which the
    // shared map's SELECT does not.
    set(
        &mut map,
        "statements/select",
        concat!(
            "{% if ctes %} WITH {% if recursive %}RECURSIVE {% endif %}\n",
            "{{ ctes | join(',\n') }}\n",
            "{% endif %}",
            "SELECT {% if distinct %}DISTINCT {% endif %}",
            "{{ select_concat | map(attribute='aliased') | join(', ') }} {% if from %}\n",
            "FROM (\n",
            "{{ from | indent(2, true) }}\n",
            ") AS {{ from_alias }}{% elif from_prepared %}\n",
            "FROM {{ from_prepared }}",
            "{% endif %}",
            "{% for join in joins %}\n{{ join }}{% endfor %}",
            "{% if filter %}\nWHERE {{ filter }}{% endif %}",
            "{% if group_by %}\nGROUP BY {{ group_by }}{% endif %}",
            "{% if having %}\nHAVING {{ having }}{% endif %}",
            "{% if order_by %}\nORDER BY {{ order_by | map(attribute='expr') | join(', ') }}{% endif %}",
            "{% if limit is not none %}\nLIMIT {{ limit }}{% endif %}",
            "{% if offset is not none %}\nOFFSET {{ offset }}{% endif %}"
        ),
    );
    map
}

/// `PostgresQuery.sqlTemplates()` (`adapter/PostgresQuery.ts:78-111`).
pub fn postgres() -> Result<MockSqlTemplatesRender, PlannerError> {
    render(postgres_map())
}

/// The Postgres template map, which Redshift, CrateDB and Materialize build on
/// the way their JS classes extend `PostgresQuery`.
fn postgres_map() -> Map {
    let mut map = MockSqlTemplatesRender::default_templates_map();
    set(
        &mut map,
        "functions/DATETRUNC",
        "DATE_TRUNC({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/DATEPART",
        "DATE_PART({{ args_concat }})",
    );
    set(&mut map, "functions/CURRENTDATE", "CURRENT_DATE");
    set(&mut map, "functions/LEAST", "LEAST({{ args_concat }})");
    set(
        &mut map,
        "functions/GREATEST",
        "GREATEST({{ args_concat }})",
    );
    set(&mut map, "functions/NOW", "NOW({{ args_concat }})");
    set(
        &mut map,
        "functions/UTCTIMESTAMP",
        "(NOW() AT TIME ZONE 'UTC')",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "({{ args[0] }} + '{{ interval }} {{ date_part }}'::interval)",
    );
    set(
        &mut map,
        "functions/CONCAT",
        "CONCAT({% for arg in args %}CAST({{arg}} AS TEXT){% if not loop.last %},{% endif %}{% endfor %})",
    );
    set(
        &mut map,
        "functions/DATEDIFF",
        concat!(
            "CASE WHEN LOWER('{{ date_part }}') IN ('year', 'quarter', 'month') ",
            "THEN (EXTRACT(YEAR FROM AGE(DATE_TRUNC('{{ date_part }}', {{ args[2] }}), DATE_TRUNC('{{ date_part }}', {{ args[1] }}))) * 12 ",
            "+ EXTRACT(MONTH FROM AGE(DATE_TRUNC('{{ date_part }}', {{ args[2] }}), DATE_TRUNC('{{ date_part }}', {{ args[1] }})))) ",
            "/ CASE LOWER('{{ date_part }}') WHEN 'year' THEN 12 WHEN 'quarter' THEN 3 WHEN 'month' THEN 1 END ",
            "ELSE EXTRACT(EPOCH FROM DATE_TRUNC('{{ date_part }}', {{ args[2] }}) - DATE_TRUNC('{{ date_part }}', {{ args[1] }})) ",
            "/ EXTRACT(EPOCH FROM '1 {{ date_part }}'::interval) END::bigint"
        ),
    );
    set(
        &mut map,
        "expressions/interval",
        "INTERVAL '{{ interval }}'",
    );
    set(
        &mut map,
        "expressions/extract",
        "EXTRACT({{ date_part }} FROM {{ expr }})",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "timestamptz '{{ value }}'",
    );
    set(&mut map, "window_frame_types/groups", "GROUPS");
    // The shared map spells the types the way `BaseQuery` does; Postgres has
    // no `STRING`, `TINYINT`, `FLOAT`/`DOUBLE` or `BINARY`.
    set(&mut map, "types/string", "TEXT");
    set(&mut map, "types/tinyint", "SMALLINT");
    set(&mut map, "types/float", "REAL");
    set(&mut map, "types/double", "DOUBLE PRECISION");
    set(&mut map, "types/binary", "BYTEA");
    set(
        &mut map,
        "operators/is_not_distinct_from",
        "IS NOT DISTINCT FROM",
    );
    map
}

/// `BaseQuery`'s parameter casts: none. The shared map casts with `::boolean`
/// and `::numeric`, which only Postgres itself reads the way it means them.
fn uncast_params(map: &mut Map) {
    set(map, "tesseract/bool_param_cast", "{{ expr }}");
    set(map, "tesseract/number_param_cast", "{{ expr }}");
}

/// The `{% for time_item in seria %}` union every dialect without a `VALUES`
/// table source renders a time series through, with each bound wrapped in
/// `cast`.
fn union_time_series(cast_from: &str, cast_to: &str, trailer: &str) -> String {
    format!(
        concat!(
            "SELECT {} date_from, {} date_to \n",
            "FROM (\n",
            "{{% for time_item in seria  %}}",
            "    select '{{{{ time_item[0] }}}}' f, '{{{{ time_item[1] }}}}' t \n",
            "{{% if not loop.last %}} UNION ALL\n{{% endif %}}",
            "{{% endfor %}}",
            "){}"
        ),
        cast_from, cast_to, trailer
    )
}

/// CubeStore, the dialect external pre-aggregations are read through.
pub fn cubestore() -> Result<MockSqlTemplatesRender, PlannerError> {
    Ok(MockSqlTemplatesRender::cubestore_templates())
}

/// `MysqlQuery.sqlTemplates()` (`adapter/MysqlQuery.ts:207-289`).
///
/// `CUBEJS_DB_MYSQL_USE_GENERATED_TIME_SERIES` defaults to true, so the
/// recursive-CTE time series is registered.
pub fn mysql() -> Result<MockSqlTemplatesRender, PlannerError> {
    render(mysql_map())
}

fn mysql_map() -> Map {
    let mut map = base_map();
    set(
        &mut map,
        "functions/STRING_AGG",
        "GROUP_CONCAT({% if distinct %}DISTINCT {% endif %}{{ args[0] }} SEPARATOR {{ args[1] }})",
    );
    set(&mut map, "functions/UTCTIMESTAMP", "UTC_TIMESTAMP()");
    set(
        &mut map,
        "functions/DATE_ADD",
        "DATE_ADD({{ args[0] }}, INTERVAL {% if date_part == \"MILLISECOND\" %}{{ interval }}000 MICROSECOND{% else %}{{ interval }} {{ date_part }}{% endif %})",
    );
    unset(&mut map, "functions/PERCENTILECONT");
    unset(&mut map, "functions/WIDTH_BUCKET");
    set(&mut map, "quotes/identifiers", "`");
    set(&mut map, "quotes/escape", "\\`");
    set(
        &mut map,
        "expressions/sort",
        "{{ expr }} IS NULL {% if nulls_first %}DESC{% else %}ASC{% endif %}, {{ expr }} {% if asc %}ASC{% else %}DESC{% endif %}",
    );
    set(
        &mut map,
        "expressions/int_division",
        "({{ left }} DIV {{ right }})",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "TIMESTAMP('{{ value | replace(\"T\", \" \") | replace(\"Z\", \"\") }}')",
    );
    unset(&mut map, "expressions/ilike");
    set(&mut map, "types/string", "CHAR");
    set(&mut map, "types/boolean", "TINYINT");
    set(&mut map, "types/timestamp", "DATETIME");
    unset(&mut map, "types/interval");
    set(&mut map, "types/binary", "BLOB");
    unset(&mut map, "join_types/full");
    set(
        &mut map,
        "expressions/concat_strings",
        "CONCAT({{ strings | join(',' ) }})",
    );
    set(
        &mut map,
        "filters/like_pattern",
        "CONCAT({% if start_wild %}'%'{% else %}''{% endif %}, LOWER({{ value }}), {% if end_wild %}'%'{% else %}''{% endif %})",
    );
    set(
        &mut map,
        "tesseract/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %}LIKE {{ pattern }}",
    );
    set(
        &mut map,
        "statements/time_series_select",
        concat!(
            "SELECT TIMESTAMP(dates.f) date_from, TIMESTAMP(dates.t) date_to \n",
            "FROM (\n",
            "{% for time_item in seria  %}",
            "    select '{{ time_item[0] }}' f, '{{ time_item[1] }}' t \n",
            "{% if not loop.last %} UNION ALL\n{% endif %}",
            "{% endfor %}",
            ") AS dates"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_select",
        concat!(
            "SELECT CAST(TIMESTAMP({{ start }}) AS DATETIME(6)) AS date_from,\n",
            "       CAST(DATE_SUB(DATE_ADD(TIMESTAMP({{ start }}), INTERVAL {{ granularity }}), INTERVAL 1000 MICROSECOND) AS DATETIME(6)) AS date_to\n",
            "UNION ALL\n",
            "SELECT DATE_ADD(date_from, INTERVAL {{ granularity }}),\n",
            "       CAST(DATE_SUB(DATE_ADD(DATE_ADD(date_from, INTERVAL {{ granularity }}), INTERVAL {{ granularity }}), INTERVAL 1000 MICROSECOND) AS DATETIME(6))\n",
            "FROM time_series\n",
            "WHERE DATE_ADD(date_from, INTERVAL {{ granularity }}) <= TIMESTAMP({{ end }})"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_with_cte_range_source",
        concat!(
            "SELECT CAST({{ range_source }}.{{ min_name }} AS DATETIME(6)) AS date_from,\n",
            "       CAST(DATE_SUB(DATE_ADD({{ range_source }}.{{ min_name }}, INTERVAL {{ granularity }}), INTERVAL 1000 MICROSECOND) AS DATETIME(6)) AS date_to,\n",
            "       {{ range_source }}.{{ max_name }} AS max_date\n",
            "FROM {{ range_source }}\n",
            "UNION ALL\n",
            "SELECT DATE_ADD(date_from, INTERVAL {{ granularity }}),\n",
            "       CAST(DATE_SUB(DATE_ADD(DATE_ADD(date_from, INTERVAL {{ granularity }}), INTERVAL {{ granularity }}), INTERVAL 1000 MICROSECOND) AS DATETIME(6)),\n",
            "       max_date\n",
            "FROM time_series\n",
            "WHERE DATE_ADD(date_from, INTERVAL {{ granularity }}) <= max_date"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_recursive",
        "true",
    );
    set(
        &mut map,
        "expressions/wrap_segment_select",
        "IF({{ expr }}, 1, 0)",
    );
    set(
        &mut map,
        "expressions/wrap_segment_filter",
        "{{ expr }} = 1",
    );
    map
}

/// `ClickHouseQuery.sqlTemplates()` (`adapter/ClickHouseQuery.ts:266-324`).
pub fn clickhouse() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(
        &mut map,
        "functions/DATETRUNC",
        "DATE_TRUNC({{ args_concat }})",
    );
    set(&mut map, "functions/UTCTIMESTAMP", "now('UTC')");
    set(
        &mut map,
        "functions/STRING_AGG",
        "arrayStringConcat(group{% if distinct %}Uniq{% endif %}Array({{ args[0] }}), {{ args[1] }})",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "({{ args[0] }} + INTERVAL {{ interval }} {{ date_part }})",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "parseDateTimeBestEffort('{{ value }}')",
    );
    unset(&mut map, "functions/PERCENTILECONT");
    unset(&mut map, "expressions/like_escape");
    set(&mut map, "quotes/identifiers", "`");
    set(&mut map, "quotes/escape", "\\`");
    set(&mut map, "types/string", "String");
    set(&mut map, "types/nullable", "Nullable({{ data_type }})");
    set(&mut map, "types/boolean", "BOOL");
    set(&mut map, "types/timestamp", "DATETIME");
    unset(&mut map, "types/time");
    unset(&mut map, "types/interval");
    unset(&mut map, "types/binary");
    set(
        &mut map,
        "expressions/is_not_distinct_from",
        "isNotDistinctFrom({{ left }}, {{ right }})",
    );
    set(
        &mut map,
        "expressions/int_division",
        "intDiv({{ left }}, {{ right }})",
    );
    set(
        &mut map,
        "statements/time_series_select",
        concat!(
            "SELECT parseDateTimeBestEffort(dates.f) date_from, parseDateTimeBestEffort(dates.t) date_to \n",
            "FROM (\n",
            "{% for time_item in seria  %}",
            "    select '{{ time_item[0] }}' f, '{{ time_item[1] }}' t \n",
            "{% if not loop.last %} UNION ALL\n{% endif %}",
            "{% endfor %}",
            ") AS dates"
        ),
    );
    set(
        &mut map,
        "statements/union",
        concat!(
            "{% for query in queries %}(\n",
            "{{ query | indent(2, true) }}\n",
            ")",
            "{% if not loop.last %}\nUNION {% if distinct %}DISTINCT{% else %}ALL{% endif %} {% endif %}",
            "{% endfor %}",
            "{% if limit is not none %}\nLIMIT {{ limit }}{% endif %}"
        ),
    );
    render(map)
}

/// `BigqueryQuery.sqlTemplates()` (`adapter/BigqueryQuery.ts:333-408`).
pub fn bigquery() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(&mut map, "quotes/identifiers", "`");
    set(&mut map, "quotes/escape", "\\`");
    set(
        &mut map,
        "functions/DATETRUNC",
        "TIMESTAMP(DATETIME_TRUNC(CAST({{ args[1] }} AS DATETIME), {% if date_part|upper == 'WEEK' %}{{ 'WEEK(MONDAY)' }}{% else %}{{ date_part }}{% endif %}))",
    );
    set(
        &mut map,
        "functions/LOG",
        "LOG({{ args_concat }}{% if args[1] is undefined %}, 10{% endif %})",
    );
    set(&mut map, "functions/BTRIM", "TRIM({{ args_concat }})");
    set(&mut map, "functions/STRPOS", "STRPOS({{ args_concat }})");
    set(
        &mut map,
        "functions/DATEDIFF",
        "DATETIME_DIFF(CAST({{ args[2] }} AS DATETIME), CAST({{ args[1] }} AS DATETIME), {{ date_part }})",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "DATETIME_ADD(DATETIME({{ args[0] }}), INTERVAL {{ interval }} {{ date_part }})",
    );
    set(&mut map, "functions/CURRENTDATE", "CURRENT_DATE");
    set(&mut map, "functions/UTCTIMESTAMP", "CURRENT_TIMESTAMP()");
    unset(&mut map, "functions/TO_CHAR");
    unset(&mut map, "functions/PERCENTILECONT");
    unset(&mut map, "functions/WIDTH_BUCKET");
    set(
        &mut map,
        "expressions/binary",
        "{% if op == '%' %}MOD({{ left }}, {{ right }}){% else %}({{ left }} {{ op }} {{ right }}){% endif %}",
    );
    set(&mut map, "expressions/interval", "INTERVAL {{ interval }}");
    set(
        &mut map,
        "expressions/int_division",
        "DIV({{ left }}, {{ right }})",
    );
    set(
        &mut map,
        "expressions/extract",
        "EXTRACT({% if date_part == 'DOW' %}DAYOFWEEK{% elif date_part == 'DOY' %}DAYOFYEAR{% else %}{{ date_part }}{% endif %} FROM {{ expr }})",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "TIMESTAMP('{{ value }}')",
    );
    set(
        &mut map,
        "expressions/rolling_window_expr_timestamp_cast",
        "TIMESTAMP({{ value }})",
    );
    set(
        &mut map,
        "expressions/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %}LIKE LOWER({{ pattern }})",
    );
    unset(&mut map, "expressions/like_escape");
    set(
        &mut map,
        "filters/like_pattern",
        "CONCAT({% if start_wild %}'%'{% else %}''{% endif %}, LOWER({{ value }}), {% if end_wild %}'%'{% else %}''{% endif %})",
    );
    set(
        &mut map,
        "tesseract/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %} LIKE {{ pattern }}",
    );
    set(
        &mut map,
        "tesseract/series_bounds_cast",
        "TIMESTAMP({{ expr }})",
    );
    set(
        &mut map,
        "tesseract/bool_param_cast",
        "CAST({{ expr }} AS BOOL)",
    );
    set(
        &mut map,
        "tesseract/number_param_cast",
        "CAST({{ expr }} AS FLOAT64)",
    );
    set(&mut map, "types/boolean", "BOOL");
    set(&mut map, "types/float", "FLOAT64");
    set(&mut map, "types/double", "FLOAT64");
    set(
        &mut map,
        "types/decimal",
        "BIGDECIMAL({{ precision }},{{ scale }})",
    );
    set(&mut map, "types/binary", "BYTES");
    set(
        &mut map,
        "operators/is_not_distinct_from",
        "IS NOT DISTINCT FROM",
    );
    set(
        &mut map,
        "statements/time_series_select",
        concat!(
            "SELECT TIMESTAMP(f) date_from, TIMESTAMP(t) date_to \n",
            "FROM (\n",
            "{% for time_item in seria  %}",
            "    select '{{ time_item[0] }}' f, '{{ time_item[1] }}' t \n",
            "{% if not loop.last %} UNION ALL\n{% endif %}",
            "{% endfor %}",
            ") AS dates"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_select",
        concat!(
            "SELECT TIMESTAMP(DATETIME(d)) AS date_from,\n",
            "TIMESTAMP(DATETIME_SUB(DATETIME_ADD(DATETIME(d),  INTERVAL {{ granularity }}), INTERVAL 1 MILLISECOND)) AS date_to \n",
            "FROM UNNEST(\n",
            "{% if minimal_time_unit|upper in [\"DAY\", \"WEEK\", \"MONTH\", \"QUARTER\", \"YEAR\"] %}",
            "GENERATE_DATE_ARRAY(DATE({{ start }}), DATE({{ end }}), INTERVAL {{ granularity }})\n",
            "{% else %}",
            "GENERATE_TIMESTAMP_ARRAY(TIMESTAMP({{ start }}), TIMESTAMP({{ end }}), INTERVAL {{ granularity }})\n",
            "{% endif %}",
            ") AS d"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_with_cte_range_source",
        concat!(
            "SELECT TIMESTAMP(DATETIME(d)) AS date_from,\n",
            "TIMESTAMP(DATETIME_SUB(DATETIME_ADD(DATETIME(d),  INTERVAL {{ granularity }}), INTERVAL 1 MILLISECOND)) AS date_to \n",
            "FROM {{ range_source }}, UNNEST(\n",
            "{% if minimal_time_unit|upper in [\"DAY\", \"WEEK\", \"MONTH\", \"QUARTER\", \"YEAR\"] %}",
            "GENERATE_DATE_ARRAY(DATE({{ range_source }}.{{ min_name }}), DATE({{ range_source }}.{{ max_name }}), INTERVAL {{ granularity }})\n",
            "{% else %}",
            "GENERATE_TIMESTAMP_ARRAY(TIMESTAMP({{ range_source }}.{{ min_name }}), TIMESTAMP({{ range_source }}.{{ max_name }}), INTERVAL {{ granularity }})\n",
            "{% endif %}",
            ") AS d"
        ),
    );
    set(
        &mut map,
        "statements/union",
        concat!(
            "{% for query in queries %}(\n",
            "{{ query | indent(2, true) }}\n",
            ")",
            "{% if not loop.last %}\nUNION {% if distinct %}DISTINCT{% else %}ALL{% endif %} {% endif %}",
            "{% endfor %}",
            "{% if limit is not none %}\nLIMIT {{ limit }}{% endif %}"
        ),
    );
    render(map)
}

/// `SnowflakeQuery.sqlTemplates()` (`adapter/SnowflakeQuery.ts:132-194`).
pub fn snowflake() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(
        &mut map,
        "functions/DATETRUNC",
        "DATE_TRUNC({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/DATEPART",
        "DATE_PART({{ args_concat }})",
    );
    set(&mut map, "functions/CURRENTDATE", "CURRENT_DATE");
    set(&mut map, "functions/NOW", "CURRENT_TIMESTAMP");
    set(&mut map, "functions/UTCTIMESTAMP", "SYSDATE()");
    set(
        &mut map,
        "functions/LOG",
        "LOG({% if args[1] is undefined %}10, {% endif %}{{ args_concat }})",
    );
    set(&mut map, "functions/DLOG10", "LOG(10, {{ args_concat }})");
    set(
        &mut map,
        "functions/CHARACTERLENGTH",
        "LENGTH({{ args[0] }})",
    );
    set(&mut map, "functions/BTRIM", "TRIM({{ args_concat }})");
    set(
        &mut map,
        "functions/STRING_AGG",
        "LISTAGG({% if distinct %}DISTINCT {% endif %}{{ args_concat }})",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "DATEADD({{ date_part }}, {{ interval }}, {{ args[0] }})",
    );
    set(
        &mut map,
        "expressions/extract",
        "EXTRACT({{ date_part }} FROM {{ expr }})",
    );
    set(
        &mut map,
        "expressions/int_division",
        "CAST(TRUNC({{ left }} / {{ right }}) AS BIGINT)",
    );
    set(
        &mut map,
        "expressions/extract_epoch_diff",
        "TIMESTAMPDIFF(MICROSECOND, {{ right }}, {{ left }}) / 1000000",
    );
    set(
        &mut map,
        "expressions/interval",
        "INTERVAL '{{ interval }}'",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "'{{ value }}'::timestamp_tz",
    );
    set(
        &mut map,
        "expressions/like",
        "{{ expr }} {% if negated %}NOT {% endif %}LIKE {{ pattern }}{% if default_escape %} ESCAPE '\\\\'{% endif %}",
    );
    set(
        &mut map,
        "expressions/ilike",
        "{{ expr }} {% if negated %}NOT {% endif %}ILIKE {{ pattern }}{% if default_escape %} ESCAPE '\\\\'{% endif %}",
    );
    set(
        &mut map,
        "operators/is_not_distinct_from",
        "IS NOT DISTINCT FROM",
    );
    set(
        &mut map,
        "tesseract/ilike",
        "{{ expr }} {% if negated %}NOT {% endif %}ILIKE {{ pattern }} ESCAPE '\\\\'",
    );
    set(&mut map, "tesseract/join_types_full", "FULL");
    set(
        &mut map,
        "statements/generated_time_series_select",
        concat!(
            "SELECT series_date AS \"date_from\",\n",
            "DATEADD(MILLISECOND, -1, DATEADD({{ minimal_time_unit }}, 1, series_date)) AS \"date_to\"\n",
            "FROM (SELECT DATEADD({{ minimal_time_unit }}, series_index.value::int, {{ start }}::timestamp_ntz) AS series_date\n",
            "FROM TABLE(FLATTEN(input => ARRAY_GENERATE_RANGE(0, DATEDIFF({{ minimal_time_unit }}, {{ start }}::timestamp_ntz, {{ end }}::timestamp_ntz) + 1))) AS series_index) AS series\n",
            "WHERE series_date <= {{ end }}::timestamp_ntz"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_with_cte_range_source",
        concat!(
            "SELECT series_date AS \"date_from\",\n",
            "DATEADD(MILLISECOND, -1, DATEADD({{ minimal_time_unit }}, 1, series_date)) AS \"date_to\"\n",
            "FROM (SELECT DATEADD({{ minimal_time_unit }}, series_index.value::int, {{ range_source }}.\"{{ min_name }}\") AS series_date,\n",
            "{{ range_source }}.\"{{ max_name }}\" AS series_end\n",
            "FROM {{ range_source }}, LATERAL FLATTEN(input => ARRAY_GENERATE_RANGE(0, DATEDIFF({{ minimal_time_unit }}, {{ range_source }}.\"{{ min_name }}\", {{ range_source }}.\"{{ max_name }}\") + 1)) AS series_index) AS series\n",
            "WHERE series_date <= series_end"
        ),
    );
    unset(&mut map, "types/interval");
    render(map)
}

/// `DatabricksQuery.sqlTemplates()`
/// (`packages/cubejs-databricks-jdbc-driver/src/DatabricksQuery.ts:171-249`).
pub fn databricks() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(&mut map, "functions/CURRENTDATE", "CURRENT_DATE");
    set(
        &mut map,
        "functions/UTCTIMESTAMP",
        "TO_UTC_TIMESTAMP(CURRENT_TIMESTAMP(), CURRENT_TIMEZONE())",
    );
    set(
        &mut map,
        "functions/DATETRUNC",
        "DATE_TRUNC({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/DATEPART",
        "DATE_PART({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/BTRIM",
        "TRIM({% if args[1] is defined %}{{ args[1] }} FROM {% endif %}{{ args[0] }})",
    );
    set(
        &mut map,
        "functions/LTRIM",
        "LTRIM({{ args|reverse|join(\", \") }})",
    );
    set(
        &mut map,
        "functions/RTRIM",
        "RTRIM({{ args|reverse|join(\", \") }})",
    );
    set(
        &mut map,
        "functions/DATEDIFF",
        "DATEDIFF({{ date_part }}, DATE_TRUNC('{{ date_part }}', {{ args[1] }}), DATE_TRUNC('{{ date_part }}', {{ args[2] }}))",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "({{ args[0] }} + INTERVAL {{ interval }} {{ date_part }})",
    );
    set(&mut map, "functions/LEAST", "LEAST({{ args_concat }})");
    set(
        &mut map,
        "functions/GREATEST",
        "GREATEST({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/TRUNC",
        "CASE WHEN ({{ args[0] }}) >= 0 THEN FLOOR({{ args_concat }}) ELSE CEIL({{ args_concat }}) END",
    );
    // Spark datetime patterns are not PostgreSQL `TO_CHAR` tokens, and the
    // format arrives as a bound parameter, so the common tokens are rewritten
    // in SQL (`DatabricksQuery.ts:184-199`).
    set(
        &mut map,
        "functions/TO_CHAR",
        concat!(
            "TO_CHAR({{ args[0] }}, ",
            "REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(",
            "{{ args[1] }}, ",
            "'HH12', 'hh'), 'HH24', '@H24@'), 'HH', 'hh'), '@H24@', 'HH'), ",
            "'MI', 'mm'), 'SS', 'ss'), 'MS', 'SSS'), 'US', 'SSSSSS'), ",
            "'YYYY', 'yyyy'), 'YY', 'yy'), 'Month', 'MMMM'), 'Mon', 'MMM'), ",
            "'DDD', '@DOY@'), 'Day', 'EEEE'), 'Dy', 'EEE'), 'DD', 'dd'), '@DOY@', 'DDD'), ",
            "'AM', 'a'), 'PM', 'a'))"
        ),
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "from_utc_timestamp('{{ value }}', 'UTC')",
    );
    set(
        &mut map,
        "expressions/int_division",
        "({{ left }} div {{ right }})",
    );
    set(
        &mut map,
        "expressions/extract",
        "{% if date_part|lower == \"epoch\" %}unix_timestamp({{ expr }}){% else %}EXTRACT({{ date_part }} FROM {{ expr }}){% endif %}",
    );
    set(
        &mut map,
        "expressions/interval_single_date_part",
        "INTERVAL '{{ num }}' {{ date_part }}",
    );
    set(&mut map, "quotes/identifiers", "`");
    set(&mut map, "quotes/escape", "``");
    set(
        &mut map,
        "statements/time_series_select",
        concat!(
            "SELECT date_from::timestamp AS `date_from`,\n",
            "date_to::timestamp AS `date_to` \n",
            "FROM(\n",
            "    VALUES ",
            "{% for time_item in seria  %}",
            "('{{ time_item | join('\\', \\'') }}')",
            "{% if not loop.last %}, {% endif %}",
            "{% endfor %}",
            ") AS dates (date_from, date_to)"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_select",
        concat!(
            "SELECT d AS date_from,\n",
            "(d + INTERVAL {{ granularity }}) - INTERVAL 1 MILLISECOND AS date_to\n",
            "  FROM (SELECT explode(sequence(\n",
            "    from_utc_timestamp({{ start }}, 'UTC'), from_utc_timestamp({{ end }}, 'UTC'), INTERVAL {{ granularity }}\n",
            "  )) AS d)"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_with_cte_range_source",
        concat!(
            "SELECT d AS date_from,\n",
            "(d + INTERVAL {{ granularity }}) - INTERVAL 1 MILLISECOND AS date_to\n",
            "FROM {{ range_source }}\n",
            "LATERAL VIEW explode(\n",
            "    sequence(\n",
            "        CAST({{ min_name }} AS TIMESTAMP),\n",
            "        CAST({{ max_name }} AS TIMESTAMP),\n",
            "        INTERVAL {{ granularity }}\n",
            "    )\n",
            ") dates AS d"
        ),
    );
    unset(&mut map, "types/time");
    unset(&mut map, "types/interval");
    render(map)
}

/// `MssqlQuery.sqlTemplates()` (`adapter/MssqlQuery.ts:294-414`).
pub fn mssql() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(&mut map, "functions/LEAST", "LEAST({{ args_concat }})");
    set(
        &mut map,
        "functions/GREATEST",
        "GREATEST({{ args_concat }})",
    );
    set(&mut map, "functions/UTCTIMESTAMP", "GETUTCDATE()");
    set(
        &mut map,
        "functions/ROUND",
        "ROUND({{ args_concat }}{% if args | length < 2 %}, 0{% endif %})",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "DATEADD({{ date_part }}, {{ interval }}, {{ args[0] }})",
    );
    unset(&mut map, "functions/STRING_AGG");
    unset(&mut map, "functions/PERCENTILECONT");
    unset(&mut map, "functions/WIDTH_BUCKET");
    unset(&mut map, "functions/NTH_VALUE");
    set(
        &mut map,
        "expressions/like",
        "{{ expr }} {% if negated %}NOT {% endif %}LIKE {{ pattern }}{% if default_escape %} ESCAPE '\\'{% endif %}",
    );
    unset(&mut map, "expressions/ilike");
    set(
        &mut map,
        "expressions/concat_strings",
        "{{ strings | join(' + ' ) }}",
    );
    set(
        &mut map,
        "expressions/sort",
        "CASE WHEN {{ expr }} IS NULL THEN 1 ELSE 0 END {% if nulls_first %}DESC{% else %}ASC{% endif %}, {{ expr }} {% if asc %}ASC{% else %}DESC{% endif %}",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "CONVERT(DATETIME2, '{{ value }}', 127)",
    );
    set(&mut map, "types/string", "VARCHAR");
    set(&mut map, "types/boolean", "BIT");
    set(&mut map, "types/integer", "INT");
    set(&mut map, "types/float", "FLOAT(24)");
    set(&mut map, "types/double", "FLOAT(53)");
    set(&mut map, "types/timestamp", "DATETIME2");
    unset(&mut map, "types/interval");
    set(&mut map, "types/binary", "VARBINARY");
    set(&mut map, "params/param", "@_{{ param_index + 1 }}");
    set(
        &mut map,
        "statements/group_by_exprs",
        "{{ group_by | map(attribute='expr') | join(', ') }}",
    );
    set(
        &mut map,
        "expressions/order_by",
        "{{ expr }} {% if asc %}ASC{% else %}DESC{% endif %}",
    );
    set(
        &mut map,
        "statements/time_series_select",
        concat!(
            "SELECT CAST(date_from AS DATETIME2) AS \"date_from\",\n",
            "CAST(date_to AS DATETIME2) AS \"date_to\" \n",
            "FROM(\n",
            "    VALUES ",
            "{% for time_item in seria  %}",
            "('{{ time_item[0] }}', '{{ time_item[1] }}')",
            "{% if not loop.last %}, {% endif %}",
            "{% endfor %}",
            ") AS dates (date_from, date_to)"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_select",
        concat!(
            "SELECT CAST({{ start }} AS DATETIME2) AS date_from,\n",
            "       DATEADD(MILLISECOND, -1, DATEADD({{ minimal_time_unit }}, 1, CAST({{ start }} AS DATETIME2))) AS date_to\n",
            "UNION ALL\n",
            "SELECT DATEADD({{ minimal_time_unit }}, 1, date_from),\n",
            "       DATEADD(MILLISECOND, -1, DATEADD({{ minimal_time_unit }}, 1, DATEADD({{ minimal_time_unit }}, 1, date_from)))\n",
            "FROM time_series\n",
            "WHERE DATEADD({{ minimal_time_unit }}, 1, date_from) <= CAST({{ end }} AS DATETIME2)"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_with_cte_range_source",
        concat!(
            "SELECT {{ range_source }}.{{ min_name }} AS date_from,\n",
            "       DATEADD(MILLISECOND, -1, DATEADD({{ minimal_time_unit }}, 1, {{ range_source }}.{{ min_name }})) AS date_to,\n",
            "       {{ range_source }}.{{ max_name }} AS max_date\n",
            "FROM {{ range_source }}\n",
            "UNION ALL\n",
            "SELECT DATEADD({{ minimal_time_unit }}, 1, date_from),\n",
            "       DATEADD(MILLISECOND, -1, DATEADD({{ minimal_time_unit }}, 1, DATEADD({{ minimal_time_unit }}, 1, date_from))),\n",
            "       max_date\n",
            "FROM time_series\n",
            "WHERE DATEADD({{ minimal_time_unit }}, 1, date_from) <= max_date"
        ),
    );
    set(
        &mut map,
        "tesseract/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %}LIKE LOWER({{ pattern }}) ESCAPE '\\'",
    );
    set(
        &mut map,
        "filters/like_pattern",
        "CONCAT({% if start_wild %}'%'{% else %}''{% endif %}, LOWER({{ value }}), {% if end_wild %}'%'{% else %}''{% endif %})",
    );
    set(
        &mut map,
        "statements/select",
        concat!(
            "{% if ctes %} WITH \n",
            "{{ ctes | join(',\n') }}\n",
            "{% endif %}",
            "SELECT {% if distinct %}DISTINCT {% endif %}{% if limit is not none and (not order_by or limit == 0) %}TOP {{ limit }} {% endif %}",
            "{{ select_concat | map(attribute='aliased') | join(', ') }} {% if from %}\n",
            "FROM (\n",
            "{{ from | indent(2, true) }}\n",
            ") AS {{ from_alias }}{% elif from_prepared %}\n",
            "FROM {{ from_prepared }}",
            "{% endif %}",
            "{% if filter %}\nWHERE {{ filter }}{% endif %}",
            "{% if group_by %}\nGROUP BY {{ group_by }}{% endif %}",
            "{% if having %}\nHAVING {{ having }}{% endif %}",
            "{% if order_by %}\nORDER BY {{ order_by | map(attribute='expr') | join(', ') }}",
            "{% if limit != 0 %}\nOFFSET {% if offset is not none %}{{ offset }}{% else %}0{% endif %} ROWS",
            "\nFETCH NEXT {% if limit is not none %}{{ limit }}{% else %}2147483647{% endif %} ROWS ONLY{% endif %}{% endif %}",
            "{% if ctes %}\nOPTION (MAXRECURSION 0){% endif %}"
        ),
    );
    set(
        &mut map,
        "statements/union",
        concat!(
            "{% if limit is not none %}SELECT TOP {{ limit }} * FROM (\n{% endif %}",
            "{% for query in queries %}(\n",
            "{{ query | indent(2, true) }}\n",
            ")",
            "{% if not loop.last %}\nUNION {% if not distinct %}ALL {% endif %}{% endif %}",
            "{% endfor %}",
            "{% if limit is not none %}\n) AS union_result{% endif %}"
        ),
    );
    set(
        &mut map,
        "expressions/wrap_segment_select",
        "CAST((CASE WHEN {{ expr }} THEN 1 ELSE 0 END) AS BIT)",
    );
    set(
        &mut map,
        "expressions/wrap_segment_filter",
        "{{ expr }} = 1",
    );
    render(map)
}

/// `RedshiftQuery.sqlTemplates()` (`adapter/RedshiftQuery.ts:88-109`), over
/// Postgres.
pub fn redshift() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = postgres_map();
    // `$1::numeric` is NUMERIC(18,0) on Redshift and would drop a filter
    // value's decimals; `RedshiftQuery` inherits `BaseQuery`'s uncast params.
    uncast_params(&mut map);
    set(&mut map, "functions/DLOG10", "LOG(10, {{ args_concat }})");
    // Redshift clusters always run in UTC, and GETDATE() runs on compute
    // nodes, unlike the leader-only NOW().
    set(&mut map, "functions/UTCTIMESTAMP", "GETDATE()");
    set(
        &mut map,
        "functions/DATEDIFF",
        "DATEDIFF({{ date_part }}, {{ args[1] }}, {{ args[2] }})",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "DATEADD({{ date_part }}, {{ interval }}, {{ args[0] }})",
    );
    set(
        &mut map,
        "functions/STRING_AGG",
        "LISTAGG({% if distinct %}DISTINCT {% endif %}{{ args_concat }})",
    );
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series("dates.f::timestamp", "dates.t::timestamp", " AS dates"),
    );
    unset(&mut map, "statements/generated_time_series_select");
    unset(&mut map, "operators/is_not_distinct_from");
    unset(&mut map, "functions/COVAR_POP");
    unset(&mut map, "functions/COVAR_SAMP");
    unset(&mut map, "window_frame_types/range");
    unset(&mut map, "window_frame_types/groups");
    set(&mut map, "types/binary", "VARBINARY");
    render(map)
}

/// `CrateQuery.sqlTemplates()` (`adapter/CrateQuery.ts`), over Postgres.
pub fn crate_db() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = postgres_map();
    uncast_params(&mut map);
    unset(&mut map, "functions/WIDTH_BUCKET");
    render(map)
}

/// `MongoBiQuery.sqlTemplates()` (`adapter/MongoBiQuery.ts:26-42`), over
/// MySQL.
///
/// The BI Connector speaks a MySQL 5.7-era dialect: no `OVER` clause, so the
/// window functions go, and no `WITH`, so the recursive-CTE time series the
/// MySQL dialect registers goes too — a time series is rendered as a `UNION`
/// of literals instead.
pub fn mongobi() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = mysql_map();
    for function in [
        "LAG",
        "LEAD",
        "ROW_NUMBER",
        "RANK",
        "DENSE_RANK",
        "PERCENT_RANK",
        "CUME_DIST",
        "NTILE",
        "FIRST_VALUE",
        "LAST_VALUE",
        "NTH_VALUE",
    ] {
        unset(&mut map, &format!("functions/{function}"));
    }
    unset(&mut map, "statements/generated_time_series_select");
    unset(
        &mut map,
        "statements/generated_time_series_with_cte_range_source",
    );
    unset(&mut map, "statements/generated_time_series_recursive");
    render(map)
}

/// `PrestodbQuery.sqlTemplates()` (`adapter/PrestodbQuery.ts:180-252`). Trino
/// and Athena inherit it unchanged.
pub fn presto() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(
        &mut map,
        "functions/DATETRUNC",
        "DATE_TRUNC({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/DATEPART",
        "DATE_PART({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/DATEDIFF",
        "DATE_DIFF('{{ date_part }}', {{ args[1] }}, {{ args[2] }})",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "DATE_ADD('{{ date_part }}', {{ interval }}, {{ args[0] }})",
    );
    set(&mut map, "functions/CURRENTDATE", "CURRENT_DATE");
    set(
        &mut map,
        "functions/UTCTIMESTAMP",
        "CAST(NOW() AT TIME ZONE 'UTC' AS TIMESTAMP)",
    );
    set(&mut map, "functions/TRUNC", "TRUNCATE({{ args_concat }})");
    set(
        &mut map,
        "functions/STRING_AGG",
        "ARRAY_JOIN(ARRAY_AGG({% if distinct %}DISTINCT {% endif %}{{ args[0] }}), COALESCE({{ args[1] }}, ''))",
    );
    // No exact percentile aggregate; APPROX_PERCENTILE is the one there is.
    unset(&mut map, "functions/PERCENTILECONT");
    set(
        &mut map,
        "functions/APPROXPERCENTILECONT",
        "APPROX_PERCENTILE({{ args_concat }})",
    );
    set(
        &mut map,
        "statements/select",
        concat!(
            "{% if ctes %} WITH \n",
            "{{ ctes | join(',\n') }}\n",
            "{% endif %}",
            "SELECT {% if distinct %}DISTINCT {% endif %}{{ select_concat | map(attribute='aliased') | join(', ') }}  {% if from %}\n",
            "FROM (\n  {{ from }}\n) AS {{ from_alias }} {% elif from_prepared %}\n",
            "FROM {{ from_prepared }}",
            "{% endif %}",
            "{% for join in joins %}\n{{ join }}{% endfor %}",
            "{% if filter %}\nWHERE {{ filter }}{% endif %}",
            "{% if group_by %} GROUP BY {{ group_by }}{% endif %}",
            "{% if having %}\nHAVING {{ having }}{% endif %}",
            "{% if order_by %} ORDER BY {{ order_by | map(attribute='expr') | join(', ') }}{% endif %}",
            "{% if offset is not none %}\nOFFSET {{ offset }}{% endif %}",
            "{% if limit is not none %}\nLIMIT {{ limit }}{% endif %}"
        ),
    );
    set(
        &mut map,
        "expressions/extract",
        "EXTRACT({{ date_part }} FROM {{ expr }})",
    );
    set(
        &mut map,
        "expressions/interval_single_date_part",
        "INTERVAL '{{ num }}' {{ date_part }}",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "from_iso8601_timestamp('{{ value }}')",
    );
    // Presto requires both sides of `||` to be VARCHAR.
    set(
        &mut map,
        "expressions/binary",
        "{% if op == '||' %}(CAST({{ left }} AS VARCHAR) || CAST({{ right }} AS VARCHAR)){% else %}({{ left }} {{ op }} {{ right }}){% endif %}",
    );
    set(
        &mut map,
        "expressions/like",
        "{{ expr }} {% if negated %}NOT {% endif %}LIKE {{ pattern }}{% if default_escape %} ESCAPE '\\'{% endif %}",
    );
    unset(&mut map, "expressions/ilike");
    set(&mut map, "types/string", "VARCHAR");
    set(&mut map, "types/float", "REAL");
    // Presto has YearMonth and DayTime interval types, but no universal one.
    unset(&mut map, "types/interval");
    set(&mut map, "types/binary", "VARBINARY");
    set(
        &mut map,
        "tesseract/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %} LIKE {{ pattern }}",
    );
    set(
        &mut map,
        "tesseract/bool_param_cast",
        "CAST({{ expr }} AS BOOLEAN)",
    );
    set(
        &mut map,
        "tesseract/number_param_cast",
        "CAST({{ expr }} AS DOUBLE)",
    );
    // No default LIKE escape: the pattern carries the ESCAPE clause, and the
    // escape character is restated so the two move together.
    set(
        &mut map,
        "filters/like_pattern",
        "CONCAT({% if start_wild %}'%'{% else %}''{% endif %}, LOWER({{ value }}), {% if end_wild %}'%'{% else %}''{% endif %}) ESCAPE '\\'",
    );
    set(&mut map, "filters/like_escape_char", "\\");
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series(
            "from_iso8601_timestamp(dates.f)",
            "from_iso8601_timestamp(dates.t)",
            " AS dates",
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_select",
        concat!(
            "SELECT d AS date_from,\n",
            "date_add('MILLISECOND', -1, d + interval {{ granularity }}) AS date_to\n",
            "FROM UNNEST(\n",
            "SEQUENCE(CAST(from_iso8601_timestamp({{ start }}) AS TIMESTAMP), CAST(from_iso8601_timestamp({{ end }}) AS TIMESTAMP), INTERVAL {{ granularity }})\n",
            ") AS dates(d)"
        ),
    );
    set(
        &mut map,
        "statements/generated_time_series_with_cte_range_source",
        concat!(
            "SELECT d AS date_from,\n",
            "date_add('MILLISECOND', -1, d + interval {{ granularity }}) AS date_to\n",
            "FROM {{ range_source }} CROSS JOIN UNNEST(\n",
            "SEQUENCE(CAST({{ range_source }}.{{ min_name }} AS TIMESTAMP), CAST({{ range_source }}.{{ max_name }} AS TIMESTAMP), INTERVAL {{ granularity }})\n",
            ") AS dates(d)"
        ),
    );
    render(map)
}

/// `VerticaQuery` defines no `sqlTemplates()`, so this is `BaseQuery`'s set.
/// Vertica has no `STRING` type, so a string cast names `VARCHAR`.
pub fn vertica() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(&mut map, "types/string", "VARCHAR");
    render(map)
}

/// `HiveQuery.sqlTemplates()` (`adapter/HiveQuery.ts:121-126`), plus what
/// `HiveQuery` and `HiveFilter` express outside the templates:
///
/// - identifiers are quoted with backticks (`escapeColumnName`) — a
///   double-quoted name is a string literal in HiveQL;
/// - `GROUP BY` names expressions, since a position is read as the constant
///   unless `hive.groupby.position.alias` is set (`HiveQuery.groupByClause`
///   groups by alias through a wrapping query for the same reason);
/// - Hive has no `ILIKE`: `likeIgnoreCase` is `LIKE CONCAT('%', ?, '%')`.
pub fn hive() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    unset(&mut map, "functions/WIDTH_BUCKET");
    set(&mut map, "quotes/identifiers", "`");
    set(&mut map, "quotes/escape", "``");
    set(
        &mut map,
        "statements/group_by_exprs",
        "{{ group_by | map(attribute='expr') | join(', ') }}",
    );
    set(
        &mut map,
        "tesseract/ilike",
        "{{ expr }} {% if negated %}NOT {% endif %}LIKE {{ pattern }}",
    );
    set(
        &mut map,
        "filters/like_pattern",
        "CONCAT({% if start_wild %}'%'{% else %}''{% endif %}, {{ value }}, {% if end_wild %}'%'{% else %}''{% endif %})",
    );
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series(
            "from_utc_timestamp(replace(replace(dates.f, 'T', ' '), 'Z', ''), 'UTC')",
            "from_utc_timestamp(replace(replace(dates.t, 'T', ' '), 'Z', ''), 'UTC')",
            " AS dates",
        ),
    );
    render(map)
}

/// `OracleQuery.sqlTemplates()` (`adapter/OracleQuery.ts:212-284`).
pub fn oracle() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(
        &mut map,
        "functions/UTCTIMESTAMP",
        "SYS_EXTRACT_UTC(SYSTIMESTAMP)",
    );
    // Oracle forbids `AS` before a table or subquery alias.
    set(
        &mut map,
        "expressions/query_aliased",
        "{{ query }} {{ quoted_alias }}",
    );
    // `/` keeps the fraction on NUMBER; TRUNC drops it toward zero.
    set(
        &mut map,
        "expressions/int_division",
        "TRUNC({{ left }} / {{ right }})",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "TO_TIMESTAMP('{{ value }}', 'YYYY-MM-DD\"T\"HH24:MI:SS.FF3\"Z\"')",
    );
    // No positional GROUP BY.
    set(
        &mut map,
        "statements/group_by_exprs",
        "{{ group_by | map(attribute='expr') | join(', ') }}",
    );
    set(
        &mut map,
        "statements/select",
        concat!(
            "{% if ctes %} WITH \n",
            "{{ ctes | join(',\n') }}\n",
            "{% endif %}",
            "SELECT {% if distinct %}DISTINCT {% endif %}",
            "{{ select_concat | map(attribute='aliased') | join(', ') }} {% if from %}\n",
            "FROM (\n",
            "{{ from | indent(2, true) }}\n",
            ") {{ from_alias }}{% elif from_prepared %}\n",
            "FROM {{ from_prepared }}",
            "{% endif %}",
            "{% for join in joins %}\n{{ join }}{% endfor %}",
            "{% if filter %}\nWHERE {{ filter }}{% endif %}",
            "{% if group_by %}\nGROUP BY {{ group_by }}{% endif %}",
            "{% if having %}\nHAVING {{ having }}{% endif %}",
            "{% if order_by %}\nORDER BY {{ order_by | map(attribute='expr') | join(', ') }}{% endif %}",
            "{% if offset is not none %}\nOFFSET {{ offset }} ROWS{% endif %}",
            "{% if limit is not none %}\nFETCH NEXT {{ limit }} ROWS ONLY{% endif %}"
        ),
    );
    set(
        &mut map,
        "statements/union",
        concat!(
            "{% for query in queries %}(\n",
            "{{ query | indent(2, true) }}\n",
            ")",
            "{% if not loop.last %}\nUNION {% if not distinct %}ALL {% endif %}{% endif %}",
            "{% endfor %}",
            "{% if limit is not none %}\nFETCH NEXT {{ limit }} ROWS ONLY{% endif %}"
        ),
    );
    // No `::` cast and no `VALUES` table source: TO_TIMESTAMP over a UNION ALL
    // of `SELECT … FROM DUAL`, and no `AS` before the derived table's alias.
    set(
        &mut map,
        "statements/time_series_select",
        concat!(
            "SELECT TO_TIMESTAMP(dates.f, 'YYYY-MM-DD\"T\"HH24:MI:SS.FF3') AS \"date_from\",\n",
            "TO_TIMESTAMP(dates.t, 'YYYY-MM-DD\"T\"HH24:MI:SS.FF3') AS \"date_to\" \n",
            "FROM (\n",
            "{% for time_item in seria %}",
            "SELECT '{{ time_item[0] }}' f, '{{ time_item[1] }}' t FROM DUAL",
            "{% if not loop.last %} UNION ALL\n{% endif %}",
            "{% endfor %}",
            ") dates"
        ),
    );
    set(
        &mut map,
        "expressions/like",
        "{{ expr }} {% if negated %}NOT {% endif %}LIKE {{ pattern }}{% if default_escape %} ESCAPE '\\'{% endif %}",
    );
    unset(&mut map, "expressions/ilike");
    // Oracle has no default LIKE escape character, so the clause is
    // unconditional on the filter path.
    set(
        &mut map,
        "tesseract/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %}LIKE LOWER({{ pattern }}) ESCAPE '\\'",
    );
    // No `STRING` type, and VARCHAR2 needs a length.
    set(&mut map, "types/string", "VARCHAR2(4000)");
    render(map)
}

/// `SqliteQuery.sqlTemplates()` (`adapter/SqliteQuery.ts:99-106`), plus
/// `castToString` (`CAST(… as TEXT)`: `STRING` has NUMERIC affinity in SQLite)
/// and `SqliteFilter.likeIgnoreCase`. SQLite has no ILIKE and no default LIKE
/// escape character; its LIKE is already case-insensitive for ASCII.
pub fn sqlite() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    unset(&mut map, "functions/WIDTH_BUCKET");
    // A compound select takes no parenthesised operands in SQLite.
    unset(&mut map, "statements/union");
    set(&mut map, "types/string", "TEXT");
    set(
        &mut map,
        "tesseract/ilike",
        "{{ expr }} {% if negated %}NOT {% endif %}LIKE {{ pattern }} ESCAPE '\\'",
    );
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series("dates.f", "dates.t", " AS dates"),
    );
    render(map)
}

/// `DruidQuery.sqlTemplates()`
/// (`packages/cubejs-druid-driver/src/DruidQuery.ts:53-77`).
pub fn druid() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(
        &mut map,
        "expressions/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %}LIKE LOWER({{ pattern }})",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "TIME_PARSE('{{ value }}')",
    );
    unset(&mut map, "expressions/like_escape");
    set(
        &mut map,
        "filters/like_pattern",
        "CONCAT({% if start_wild %}'%'{% else %}''{% endif %}, LOWER({{ value }}), {% if end_wild %}'%'{% else %}''{% endif %})",
    );
    // Druid's LIKE has no default escape character, while the planner escapes
    // `%`, `_` and `\` in filter values with a backslash.
    set(
        &mut map,
        "tesseract/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %}LIKE {{ pattern }} ESCAPE '\\'",
    );
    set(&mut map, "functions/UTCTIMESTAMP", "CURRENT_TIMESTAMP");
    unset(&mut map, "functions/WIDTH_BUCKET");
    unset(&mut map, "statements/union");
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series("TIME_PARSE(dates.f)", "TIME_PARSE(dates.t)", " AS dates"),
    );
    render(map)
}

/// `FireboltQuery.sqlTemplates()`
/// (`packages/cubejs-firebolt-driver/src/FireboltQuery.ts:52-62`).
pub fn firebolt() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(
        &mut map,
        "expressions/timestamp_literal",
        "TIMESTAMPTZ '{{ value }}'",
    );
    set(
        &mut map,
        "tesseract/bool_param_cast",
        "CAST({{ expr }} AS BOOLEAN)",
    );
    unset(&mut map, "functions/WIDTH_BUCKET");
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series(
            "dates.f::timestampntz",
            "dates.t::timestampntz",
            " AS dates",
        ),
    );
    render(map)
}

/// `DremioQuery.sqlTemplates()`
/// (`packages/cubejs-dremio-driver/driver/DremioQuery.js:167-182`), plus
/// `DremioFilter` and `DremioQuery`'s string helpers: parameters are cast by
/// type, and case-insensitive matching is `LOWER(…) LIKE LOWER(…)` with an
/// explicit escape (the filter's `ILIKE(col, pattern)` call form has no
/// negated spelling).
pub fn dremio() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(&mut map, "functions/CURRENTDATE", "CURRENT_DATE");
    set(
        &mut map,
        "functions/DATETRUNC",
        "DATE_TRUNC('{{ date_part }}', {{ args_concat }})",
    );
    set(
        &mut map,
        "functions/DATEPART",
        "DATE_PART('{{ date_part }}', {{ args_concat }})",
    );
    set(
        &mut map,
        "functions/DATE",
        "TO_DATE({{ args_concat }},'YYYY-MM-DD', 1)",
    );
    set(
        &mut map,
        "functions/DATEDIFF",
        "DATE_DIFF(DATE, DATE_TRUNC('{{ date_part }}', {{ args[1] }}), DATE_TRUNC('{{ date_part }}', {{ args[2] }}))",
    );
    set(
        &mut map,
        "functions/STRING_AGG",
        "LISTAGG({% if distinct %}DISTINCT {% endif %}{{ args_concat }})",
    );
    set(
        &mut map,
        "expressions/interval_single_date_part",
        "CAST({{ num }} as INTERVAL {{ date_part }})",
    );
    set(
        &mut map,
        "expressions/like",
        "{{ expr }} {% if negated %}NOT {% endif %}LIKE {{ pattern }}{% if default_escape %} ESCAPE '\\'{% endif %}",
    );
    unset(&mut map, "expressions/ilike");
    unset(&mut map, "functions/WIDTH_BUCKET");
    set(&mut map, "quotes/identifiers", "\"");
    set(
        &mut map,
        "tesseract/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %}LIKE LOWER({{ pattern }}) ESCAPE '\\'",
    );
    set(
        &mut map,
        "tesseract/bool_param_cast",
        "CAST({{ expr }} AS BOOLEAN)",
    );
    set(
        &mut map,
        "tesseract/number_param_cast",
        "CAST({{ expr }} AS DOUBLE)",
    );
    set(&mut map, "types/string", "VARCHAR");
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series(
            "TO_TIMESTAMP(dates.f, 'YYYY-MM-DD\"T\"HH24:MI:SS.FFF')",
            "TO_TIMESTAMP(dates.t, 'YYYY-MM-DD\"T\"HH24:MI:SS.FFF')",
            " AS dates",
        ),
    );
    set(
        &mut map,
        "expressions/concat_strings",
        "CONCAT({{ strings | join(', ') }})",
    );
    set(
        &mut map,
        "expressions/wrap_segment_select",
        "IF({{ expr }}, 1, 0)",
    );
    set(
        &mut map,
        "expressions/wrap_segment_filter",
        "{{ expr }} = 1",
    );
    render(map)
}

/// `KsqlQuery.sqlTemplates()`
/// (`packages/cubejs-ksql-driver/src/KsqlQuery.ts:71-101`).
pub fn ksql() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(&mut map, "quotes/identifiers", "`");
    set(&mut map, "quotes/escape", "``");
    // No `||` string operator.
    set(
        &mut map,
        "expressions/concat_strings",
        "CONCAT({{ strings | join(', ') }})",
    );
    // A trailing 'Z' makes ksqlDB's string cast return NULL, so the markers are
    // stripped and the zone is passed explicitly.
    set(
        &mut map,
        "expressions/timestamp_literal",
        "PARSE_TIMESTAMP('{{ value | replace(\"T\", \" \") | replace(\"Z\", \"\") }}', 'yyyy-MM-dd HH:mm:ss.SSS', 'UTC')",
    );
    set(
        &mut map,
        "filters/like_pattern",
        "{% if start_wild or end_wild %}CONCAT({% if start_wild %}'%', {% endif %}{{ value }}{% if end_wild %}, '%'{% endif %}){% else %}{{ value }}{% endif %}",
    );
    // No positional GROUP BY.
    set(
        &mut map,
        "statements/group_by_exprs",
        "{{ group_by | map(attribute='expr') | join(', ') }}",
    );
    unset(&mut map, "functions/WIDTH_BUCKET");
    unset(&mut map, "statements/union");
    // `BaseQuery.seriesSql` through `KsqlQuery.dateTimeCast`: ksqlDB has no
    // `::` cast.
    set(
        &mut map,
        "statements/time_series_select",
        concat!(
            "SELECT CAST(date_from AS TIMESTAMP) AS `date_from`,\n",
            "CAST(date_to AS TIMESTAMP) AS `date_to` \n",
            "FROM(\n",
            "    VALUES ",
            "{% for time_item in seria  %}",
            "('{{ time_item[0] }}', '{{ time_item[1] }}')",
            "{% if not loop.last %}, {% endif %}",
            "{% endfor %}",
            ") AS dates (date_from, date_to)"
        ),
    );
    render(map)
}

/// `QuestQuery.sqlTemplates()`
/// (`packages/cubejs-questdb-driver/src/QuestQuery.ts:271-322`), plus what
/// `QuestQuery` and `QuestFilter` express outside the templates: `= NULL`
/// null checks, `concat(…)`, `count_distinct(…)` and grouping by expression
/// (`groupByClause` groups by alias, never by position).
pub fn questdb() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(&mut map, "params/param", "${{ param_index + 1 }}");
    // No NULLS FIRST / LAST.
    set(
        &mut map,
        "expressions/sort",
        "{{ expr }} {% if asc %}ASC{% else %}DESC{% endif %}",
    );
    set(
        &mut map,
        "expressions/order_by",
        "{% if index %}{{ index }}{% else %}{{ expr }}{% endif %} {% if asc %}ASC{% else %}DESC{% endif %}",
    );
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series(
            "cast(dates.f as timestamp)",
            "cast(dates.t as timestamp)",
            " AS dates",
        ),
    );
    // `LIMIT lo, hi` instead of `LIMIT n OFFSET m`.
    set(
        &mut map,
        "statements/select",
        concat!(
            "{% if ctes %} WITH {% if recursive %}RECURSIVE {% endif %}\n",
            "{{ ctes | join(',\n') }}\n",
            "{% endif %}",
            "SELECT {% if distinct %}DISTINCT {% endif %}",
            "{{ select_concat | map(attribute='aliased') | join(', ') }} {% if from %}\n",
            "FROM (\n",
            "{{ from | indent(2, true) }}\n",
            ") AS {{ from_alias }}{% elif from_prepared %}\n",
            "FROM {{ from_prepared }}",
            "{% endif %}",
            "{% for join in joins %}\n{{ join }}{% endfor %}",
            "{% if filter %}\nWHERE {{ filter }}{% endif %}",
            "{% if group_by %}\nGROUP BY {{ group_by }}{% endif %}",
            "{% if having %}\nHAVING {{ having }}{% endif %}",
            "{% if order_by %}\nORDER BY {{ order_by | map(attribute='expr') | join(', ') }}{% endif %}",
            "{% if offset is not none and limit is not none %}\nLIMIT {{ offset }}, {{ (offset | int) + (limit | int) }}",
            "{% elif offset is not none %}\nLIMIT {{ offset }}, 2147483647",
            "{% elif limit is not none %}\nLIMIT {{ limit }}{% endif %}"
        ),
    );
    unset(&mut map, "functions/WIDTH_BUCKET");
    set(
        &mut map,
        "statements/group_by_exprs",
        "{{ group_by | map(attribute='expr') | join(', ') }}",
    );
    set(
        &mut map,
        "expressions/concat_strings",
        "concat({{ strings | join(', ') }})",
    );
    set(
        &mut map,
        "functions/COUNT_DISTINCT",
        "count_distinct({{ args_concat }})",
    );
    set(&mut map, "filters/set_where", "{{ column }} != NULL");
    set(&mut map, "filters/not_set_where", "{{ column }} = NULL");
    set(
        &mut map,
        "filters/or_is_null_check",
        " OR {{ column }} = NULL",
    );
    render(map)
}

/// `PinotQuery.sqlTemplates()`
/// (`packages/cubejs-pinot-driver/src/PinotQuery.ts:216-264`), plus
/// `PinotFilter.castParameter`.
pub fn pinot() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(
        &mut map,
        "functions/DATETRUNC",
        "DATE_TRUNC({{ args_concat }})",
    );
    set(&mut map, "functions/UTCTIMESTAMP", "NOW()");
    set(
        &mut map,
        "functions/STRING_AGG",
        "LISTAGG({% if distinct %}DISTINCT {% endif %}{{ args_concat }})",
    );
    unset(&mut map, "functions/WIDTH_BUCKET");
    // LIMIT before OFFSET.
    set(
        &mut map,
        "statements/select",
        concat!(
            "{% if ctes %} WITH \n",
            "{{ ctes | join(',\n') }}\n",
            "{% endif %}",
            "SELECT {% if distinct %}DISTINCT {% endif %}",
            "{{ select_concat | map(attribute='aliased') | join(', ') }} {% if from %}\n",
            "FROM (\n",
            "{{ from | indent(2, true) }}\n",
            ") AS {{ from_alias }}{% elif from_prepared %}\n",
            "FROM {{ from_prepared }}",
            "{% endif %}",
            "{% for join in joins %}\n{{ join }}{% endfor %}",
            "{% if filter %}\nWHERE {{ filter }}{% endif %}",
            "{% if group_by %}\nGROUP BY {{ group_by }}{% endif %}",
            "{% if having %}\nHAVING {{ having }}{% endif %}",
            "{% if order_by %}\nORDER BY {{ order_by | map(attribute='expr') | join(', ') }}{% endif %}",
            "{% if limit is not none %}\nLIMIT {{ limit }}{% endif %}",
            "{% if offset is not none %}\nOFFSET {{ offset }}{% endif %}"
        ),
    );
    set(
        &mut map,
        "expressions/extract",
        "EXTRACT({{ date_part }} FROM {{ expr }})",
    );
    set(
        &mut map,
        "expressions/int_division",
        "CAST({{ left }} / {{ right }} AS LONG)",
    );
    set(
        &mut map,
        "expressions/timestamp_literal",
        "fromDateTime('{{ value }}', 'yyyy-MM-dd''T''HH:mm:ss.SSS''Z''')",
    );
    set(
        &mut map,
        "expressions/sort",
        "{{ expr }} IS NULL {% if nulls_first %}DESC{% else %}ASC{% endif %}, {{ expr }} {% if asc %}ASC{% else %}DESC{% endif %}",
    );
    set(
        &mut map,
        "expressions/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %}LIKE LOWER({{ pattern }})",
    );
    set(
        &mut map,
        "filters/like_pattern",
        "CONCAT({% if start_wild %}'%'{% else %}''{% endif %}, LOWER({{ value }}), {% if end_wild %}'%'{% else %}''{% endif %})",
    );
    set(
        &mut map,
        "tesseract/ilike",
        "LOWER({{ expr }}) {% if negated %}NOT {% endif %} LIKE {{ pattern }}",
    );
    set(
        &mut map,
        "tesseract/series_bounds_cast",
        "CAST({{ expr }} AS TIMESTAMP)",
    );
    set(
        &mut map,
        "tesseract/bool_param_cast",
        "CAST({{ expr }} AS BOOLEAN)",
    );
    set(
        &mut map,
        "tesseract/number_param_cast",
        "CAST({{ expr }} AS DOUBLE)",
    );
    set(
        &mut map,
        "expressions/rolling_window_expr_timestamp_cast",
        "CAST({{ value }} AS TIMESTAMP)",
    );
    set(
        &mut map,
        "statements/time_series_select",
        &union_time_series("CAST(f AS TIMESTAMP)", "CAST(t AS TIMESTAMP)", " AS dates"),
    );
    set(&mut map, "quotes/identifiers", "\"");
    unset(&mut map, "types/time");
    unset(&mut map, "types/interval");
    unset(&mut map, "types/binary");
    render(map)
}

/// `DuckDBQuery.sqlTemplates()`
/// (`packages/cubejs-duckdb-driver/src/DuckDBQuery.ts:58-86`), plus
/// `DuckDBFilter.castParameter`.
pub fn duckdb() -> Result<MockSqlTemplatesRender, PlannerError> {
    let mut map = base_map();
    set(
        &mut map,
        "functions/DATETRUNC",
        "DATE_TRUNC({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/UTCTIMESTAMP",
        "(NOW() AT TIME ZONE 'UTC')",
    );
    set(&mut map, "functions/LEAST", "LEAST({{ args_concat }})");
    set(
        &mut map,
        "functions/GREATEST",
        "GREATEST({{ args_concat }})",
    );
    set(
        &mut map,
        "functions/STRING_AGG",
        "STRING_AGG({% if distinct %}DISTINCT {% endif %}{{ args[0] }}, COALESCE({{ args[1] }}, ''))",
    );
    set(
        &mut map,
        "functions/DATE_ADD",
        "({{ args[0] }} + '{{ interval }} {{ date_part }}'::interval)",
    );
    unset(&mut map, "functions/WIDTH_BUCKET");
    set(
        &mut map,
        "expressions/like",
        "{{ expr }} {% if negated %}NOT {% endif %}LIKE {{ pattern }}{% if default_escape %} ESCAPE '\\'{% endif %}",
    );
    set(
        &mut map,
        "expressions/ilike",
        "{{ expr }} {% if negated %}NOT {% endif %}ILIKE {{ pattern }}{% if default_escape %} ESCAPE '\\'{% endif %}",
    );
    // No default LIKE escape character: unconditional on the filter path.
    set(
        &mut map,
        "tesseract/ilike",
        "{{ expr }} {% if negated %}NOT {% endif %}ILIKE {{ pattern }} ESCAPE '\\'",
    );
    // `//` is integer division truncating toward zero.
    set(
        &mut map,
        "expressions/int_division",
        "({{ left }} // {{ right }})",
    );
    set(
        &mut map,
        "tesseract/number_param_cast",
        "CAST({{ expr }} AS DOUBLE)",
    );
    render(map)
}

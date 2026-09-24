# Tesseract without JavaScript — specification of the `cube_bridge` surface

Derived from `rust/cube/cubesqlplanner/cubesqlplanner/src/cube_bridge/` (RS) and
`packages/cubejs-schema-compiler/src/` (JS). Part of the Node.js → Rust migration,
see `../MIGRATION.md` (workstream 2). Goal: implement every bridge trait over a
parsed YAML model (`cubemodel`) so the planner runs with no Node.js.

## 0. Entry point and the shape of the boundary

- `nativeBuildSqlAndParams(queryParams)` is called from `BaseQuery.buildSqlAndParamsRust` (JS `adapter/BaseQuery.js:952-1002`). `queryParams` is a live JS object graph: plain data (measures, dimensions, timeDimensions, filters, order, limit/rowLimit/offset, flags) plus four live handles — `cubeEvaluator`, `joinGraph`, `baseTools: this` (the `BaseQuery`), `securityContext` (raw JSON).
- Rust sees it through `BaseQueryOptions` (RS `cube_bridge/base_query_options.rs:229-249`). Everything else in `cube_bridge/` hangs off those four handles.
- The `#[nativebridge::native_bridge]` macro (`nativebridge/src/lib.rs:14-40`) generates, per trait: the trait, a `Native*<IT>` struct holding a `NativeObjectHandle`, and a bridge impl. Rules: default = method call on the JS object (camelCase of the Rust ident, `lib.rs:792-802`); `#[nbridge(field)]` = property read; `#[nbridge(optional)]` = absent ⇒ `None` plus `has_<name>()`; `#[nbridge(vec)]` = per-element bridge; `native_bridge(Static, with_static_meta)` = eagerly deserialized pure data.

A pure-Rust port = implement the ~30 traits below over a parsed model, plus one genuinely hard callback: `BaseTools::compile_member_sql`.

## 1. Every bridge trait

### 1.1 `CubeEvaluator` — RS `cube_bridge/evaluator.rs:36-73`; JS `compiler/CubeEvaluator.ts`
Static: `CubeEvaluatorStatic { primary_keys: HashMap<String, Vec<String>> }` ← `CubeEvaluator.primaryKeys` (`:209`).

| Rust signature | JS |
|---|---|
| `parse_path(path_type, path) -> Vec<String>` | `parsePath` `:1099-1103` — validates via `byPath`, splits on `.`; `UserError` on unknown cube/member; `path_type` ∈ measures/dimensions/segments/preAggregations |
| `measure_by_path / dimension_by_path / segment_by_path` | `:1009/1013/1017` → `byPath` `:1071-1097` |
| `cube_from_path(String) -> Rc<dyn CubeDefinition>` | `:1037-1047` |
| `is_measure/is_dimension/is_segment(Vec<String>) -> bool` | `:997/1001/1005` → `isInstanceOfType` `:1049-1053` |
| `cube_exists(String) -> bool` | `:1021-1023` |
| `resolve_granularity(Vec<String>) -> Rc<dyn GranularityDefinition>` | `CubeSymbols.resolveGranularity` `CubeSymbols.ts:1531-1550`; path `[cube, dim, "granularities", name]`; predefined names synthesize `{interval: "1 <name>"}` unless a calendar cube overrides |
| `pre_aggregations_for_cube_as_array(String)` | `:904-909` |
| `pre_aggregation_description_by_name(cube, name) -> Option` | `:911-928` |
| `evaluate_rollup_references(cube, sql: Rc<dyn MemberSql>) -> Vec<String>` | `:1161-1163` → `evaluateReferences` `CubeSymbols.ts:1194-1240` — **evaluation**: returns `cube.member` path strings |

### 1.2 `BaseTools` — RS `cube_bridge/base_tools.rs:23-53`; JS = the `BaseQuery` instance
| Rust | JS |
|---|---|
| `driver_tools(external: bool) -> Rc<dyn DriverTools>` | `BaseQuery.js:945-950` — `this` or `this.externalQuery()` (CubeStore dialect) |
| `sql_templates() -> Rc<dyn SqlTemplatesRender>` | `BaseQuery.js:4559-4859` (§3) |
| `sql_utils_for_rust() -> Rc<dyn SqlUtils>` | `BaseQuery.js:5287-5292` — `{convertTz, urlEncode}` |
| `get_allocated_params() -> Vec<FilterValue>` | `BaseQuery.js:1074-1076` |
| `all_cube_members(path) -> Vec<String>` | `BaseQuery.js:1068-1072` |
| `interval_and_minimal_time_unit(String) -> Vec<String>` | `BaseQuery.js:2182+` |
| `get_pre_aggregation_by_name(cube, name)` | `BaseQuery.js:1088-1090` |
| `pre_aggregation_table_name(cube, name) -> String` | `BaseQuery.js:4391+` |
| `join_tree_for_hints(Vec<JoinHintItem>) -> Rc<dyn JoinDefinition>` | `BaseQuery.js:423-482` — fixpoint: build join, collect hints from members of the tree, rebuild until stable |
| `compile_member_sql(member_sql, security_context, arg_names) -> CompiledMemberTemplate` | `BaseQuery.js:5298+` → `MemberSqlTemplateCompiler.compileMemberSql` (§2.3). **The hard callback.** |

### 1.3 `DriverTools` — RS `cube_bridge/driver_tools.rs:15-52`; JS = dialect `BaseQuery` subclass
`convert_tz` (`BaseQuery.js:4131` / `PostgresQuery.ts:27`); `time_grouped_column` (`:4152` / PG `:31`); `sql_templates()`; `timestamp_precision() -> u32` (`:4122`); `time_stamp_cast` (`:2167`); `date_time_cast` (`:2171`); `in_db_time_zone` (`:4108`); `get_allocated_params`; `#[nbridge(field)] should_reuse_params() -> bool` (`:1092`, PG `:113` = true); `subtract_interval` (`:1214`); `add_interval` (`:1224`); `interval_string` (`:1238` / PG `:50-60`); `add_timestamp_interval` (`:1247`); `interval_and_minimal_time_unit` (`:2182`); `hll_init` (`:3996` / PG `:62`); `hll_merge` (`:4000` / PG `:66`); `hll_cardinality_merge` (`:4012`); `count_distinct_approx` (`:4021` / PG `:70`); `support_generated_series_for_custom_td` (`:1230` / PG `:74`); `date_bin` (`:4165` / PG `:41-48`).

### 1.4 `JoinGraph` / `JoinDefinition` / `JoinItem` / `JoinItemDefinition`
- `JoinGraph::build_join(Vec<JoinHintItem>)` (RS `join_graph.rs:12-18`) ← `JoinGraph.buildJoin` (`compiler/JoinGraph.ts:223-259`): for every hint as root, shortest-path join tree (`:269-345`), fewest joins wins, `multiplicationFactor` per cube (`:350-378`). Throws `Can't find join path…`.
- `JoinDefinitionStatic { root, multiplication_factor }` + `joins() -> Vec<JoinItem>`; `JoinItemStatic { from, to, original_from, original_to }` + `join() -> JoinItemDefinition`; `JoinItemDefinitionStatic { relationship }` + `sql() -> MemberSql`; `JoinHintItem = Single(String) | Vector(Vec<String>)`.

### 1.5 Member/cube definitions (all `with_static_meta`; statics are pure data)
- `CubeDefinitionStatic { name, sql_alias, is_view, is_calendar, join_map }` + optional `sql_table()`, `sql()`, `default_filters()`.
- `MeasureDefinitionStatic { measure_type, owned_by_cube, multi_stage, reduce_by_references, add_group_by_references, group_by_references, time_shift_references, rolling_window }` + `sql()`, `case()`, `filters()`, `filter()`, `grain()`, `drill_filters()`, `order_by()`, `mask_sql()`.
- `DimensionDefinitionStatic { dimension_type, owned_by_cube, multi_stage, add_group_by_references, sub_query, propagate_filters_to_sub_query, values, primary_key }` + `sql()`, `case()`, `latitude()/longitude()`, `time_shift()`, `filter()`, `mask_sql()`.
- `SegmentDefinitionStatic { segment_type, owned_by_cube }` + `sql()`; `MemberDefinitionStatic { member_type }`; `GranularityDefinitionStatic { interval, origin, offset }`; `TimeShiftDefinitionStatic { interval, timeshift_type, name }`.
- `PreAggregationDescriptionStatic { name, pre_aggregation_type, granularity, sql_alias, external, allow_non_strict_date_range_match }` + reference `MemberSql`s evaluating to arrays of path strings (`SqlTemplate::StringVec`); `PreAggregationObjStatic { table_name, pre_aggregation_name, cube, pre_aggregation_id }`.
- `ViewFilterDefinitionStatic`, `MultiStageFilterReferencesStatic`, `MultiStageGrainReferencesStatic`, `CaseDefinition`/`CaseSwitchDefinition`/`CaseVariant`, `MemberExpressionDefinitionStatic`, `OptionsMember`, `SubqueryJoinStatic`.

### 1.6 Opaque / callback traits
- `SecurityContext`, `SqlUtils`, `FilterParams`, `FilterGroup`: empty handle traits.
- `MemberSql` (RS `member_sql.rs:382-385`): `args_names()` + `as_any()`; names come from the JS function parameter list (`CubeSymbols.funcArguments`, `CubeSymbols.ts:267, 1312-1325`).
- `FilterParamsCallback` (`filter_params_callback.rs:9-16`): `call(&Vec<String>) -> String`, a raw JS closure.
- `SqlTemplatesRender` (`sql_templates_render.rs:12-16`) is **already pure Rust** (minijinja): flattens `{group: {name: template}}` into `"group/name"` keys (`:69-79`).

## 2. Pure data vs. evaluation

**Pure data:** every `*Static`; join shapes; `CubeEvaluator` lookups; `BaseTools::all_cube_members`, `pre_aggregation_table_name`, `get_allocated_params`; `SqlTemplatesRender`. **A working prototype exists in-tree:** RS `test_fixtures/cube_bridge/mock_evaluator.rs:119-260`, `mock_schema.rs:29-56` (`MockSchema::from_yaml`), `test_fixtures/cube_bridge/yaml/*.rs` (1805 lines of serde model), `mock_join_graph.rs:79-260` (full Rust `buildJoin`), `mock_base_tools.rs`, `mock_driver_tools.rs` (Postgres + CubeStore), `mock_sql_templates_render.rs` (154 template inserts).

**Requires evaluation:** `compile_member_sql` (every `sql:`), `evaluate_rollup_references`, `build_join`/`join_tree_for_hints` (graph search + fixpoint), `DriverTools` string builders, `sqlTemplates()` per dialect.

### 2.1 YAML `sql` → JS function
`YamlCompiler.transpileYaml` (`compiler/YamlCompiler.ts:219-300`) wraps strings as Python f-strings, transpiles to a JS template literal (`:232-237, 356-374`), and `CubePropContextTranspiler.replaceValueWithArrowFunction` (`transpilers/CubePropContextTranspiler.ts:95-126`) makes free identifiers that resolve to cube/member/context symbols the arrow function parameters. `sql: "{CUBE.city} || {users.name}"` ⇒ `(CUBE, users) => \`${CUBE.city} || ${users.name}\``, `args_names = ["CUBE","users"]`. Context names: `CONTEXT_SYMBOLS` (`CubeSymbols.ts:268-277`: `SECURITY_CONTEXT`/`security_context`/`securityContext`, `FILTER_PARAMS`, `FILTER_GROUP`, `SQL_UTILS`); `CURRENT_CUBE_CONSTANTS = ['CUBE','TABLE']` (`:279`).

### 2.2 `resolveSymbolsCall` / `resolveSymbol` (legacy path)
`CubeSymbols.ts:1281-1299`, `:1408-1454`, proxies `:1456-1524`; `evaluateReferences` `:1194-1240`; `collectUsedCubeReferences` (`CubeEvaluator.ts:1124-1159`).

### 2.3 What Tesseract uses: `compileMemberSql` (`adapter/MemberSqlTemplateCompiler.js`)
Produces a *template plus dependency list*, resolved by Rust (`planner/sql_call_builder.rs:50-135`):
- Entry `compileMemberSql(sqlFn, argNames, securityContext, sqlUtils)` (`:407-415`): one proxy per arg (`buildArg` `:381-389`); result via `parseTemplateResult` (`:391-399`).
- `memberReferenceProxy(path, state)` (`:81-102`): property access extends the path; `toString`/`valueOf` records the path (`uniqueInsertPath`) and returns `{arg:N}`; `.sql` records `[...path,'__sql_fn']`.
- `FILTER_PARAMS.<cube>.<member>.filter(column)` (`:213-259`) pushes `{cube_name, name, time_shift_name, column}` → `{fp:N}`; `.time_shifts.<name>.filter(...)`; function columns compiled recursively (`compileColumnCallback` `:181-203`) with `{fpv:N}` placeholders, unless rest params / unreadable list (`:109-153`, `:165-173`) → `FilterParamsColumn::Callback`.
- `FILTER_GROUP(a, b, …)` (`:273-285`) → `{fg:N}`.
- `SECURITY_CONTEXT` (`:289-377`) resolved eagerly: `.filter(col)` → `col = {sv:N}` / `col IN ({sv:…})` / `1 = 1` / `1 = 0`; `.requiredFilter` throws when absent; `.unsafeValue()` raw; values dedup via `uniqueInsertString`.
- `SQL_UTILS` passed straight through.
- Result: `CompiledMemberTemplate { template: SqlTemplate, args: SqlTemplateArgs{symbol_paths, filter_params, filter_groups, security_context} }` (RS `member_sql.rs:288-330`); `SqlCallBuilder::build_from_template` (RS `planner/sql_call_builder.rs:66-135`).

## 3. `sqlTemplates()` structure (`BaseQuery.js:4559-4859`)
Two-level dict flattened to `"<group>/<name>"`, values are Jinja2 templates.
- **functions** (~75): aggregates `SUM/MIN/MAX/COUNT/COUNT_DISTINCT/AVG/STDDEV_POP|SAMP/VAR_*/COVAR_*/GROUP_ANY/STRING_AGG/PERCENTILECONT`; window `LAG/LEAD/ROW_NUMBER/RANK/DENSE_RANK/PERCENT_RANK/CUME_DIST/NTILE/FIRST_VALUE/LAST_VALUE/NTH_VALUE`; scalar `COALESCE/CONCAT/FLOOR/CEIL/TRUNC/LOWER/UPPER/LEFT/RIGHT/SQRT/ABS/ACOS/ASIN/ATAN/ATAN2/COS/COT/EXP/LN/LOG/DLOG10/PI/POWER/SIN/TAN/DEGREES/RADIANS/SIGN/REPEAT/NULLIF/ROUND/STDDEV/SUBSTR/CHARACTERLENGTH/BTRIM/LTRIM/RTRIM/ASCII/STRPOS/REPLACE/DATEDIFF/TO_CHAR/DATE/WIDTH_BUCKET`. `LEAST`/`GREATEST` absent from base (`:4603-4610`).
- **statements**: `select, group_by_exprs, join, union, cte, time_series_select, time_series_get_range, calc_groups_join`.
- **expressions**: `column_reference, column_aliased, query_aliased, case, is_null, binary, int_division, sort, order_by, cast, window_function, window_frame_bounds, in_list, subquery, in_subquery, rollup, cube, negative, not, add_interval, sub_interval, true, false, like, ilike, like_escape, within_group, concat_strings, wrap_segment_select, wrap_segment_filter, rolling_window_expr_timestamp_cast, timestamp_literal, between`.
- **tesseract**: `ilike, series_bounds_cast, bool_param_cast, number_param_cast, join_types_inner, join_types_left`.
- **filters**: `equals, not_equals, or_is_null_check, set_where, not_set_where, in, not_in, time_range_filter, time_not_in_range_filter, gt, gte, lt, lte, like_pattern, like_escape_char ('\\'), always_true`.
- **operators** (empty), **quotes** (`identifiers: '"'`, `escape: '""'`), **params** (`param: '?'`), **join_types**, **window_frame_types**, **window_frame_bounds**, **types**.

**Postgres deltas** (`adapter/PostgresQuery.ts:78-111`): `params.param = '${{ param_index + 1 }}'`; adds `functions.DATETRUNC/DATEPART/CURRENTDATE/LEAST/GREATEST/NOW/UTCTIMESTAMP/DATE_ADD`, overrides `CONCAT`, `DATEDIFF`; adds `expressions.interval`, `expressions.extract`, `timestamp_literal = "timestamptz '{{ value }}'"`; `window_frame_types.groups`; `types.string=TEXT, tinyint=SMALLINT, float=REAL, double=DOUBLE PRECISION, binary=BYTEA`; `operators.is_not_distinct_from`; `statements.generated_time_series_select`, `generated_time_series_with_cte_range_source`. Non-template dialect behaviour: `convertTz` (`:27`), `timeGroupedColumn`=`date_trunc` (`:31`), `dateBin` (`:41`), `intervalString` (`:50`), HLL (`:62-72`), `supportGeneratedSeriesForCustomTd=true` (`:74`), `shouldReuseParams=true` (`:113`).

## 4. Recommended Rust design

### 4.1 Crate layout
```
cubemodel/
  model.rs        # Cube, Measure, Dimension, Segment, Join, PreAggregation, Granularity, TimeShift, Case, ViewFilter
  parse.rs        # YAML → model (start from test_fixtures/cube_bridge/yaml/*.rs)
  member_sql.rs   # MemberSqlSource { raw, args_names } + the mini-parser
  evaluator.rs    # Evaluator { model, security_context, dialect }: CubeEvaluator + BaseTools + JoinGraph + *Definition
  join_graph.rs   # Rust buildJoin (port mock_join_graph.rs:79-330)
  dialect/        # trait Dialect: DriverTools + templates(); postgres.rs (PostgresQuery.ts port)
```
`Evaluator` implements the existing bridge traits verbatim, so `cubesqlplanner` needs no change: `BaseQueryOptions` becomes a plain Rust struct (mirror `test_fixtures/cube_bridge/base_query_options.rs`).

### 4.2 Member-SQL mini-parser (replaces `compileMemberSql`)
Output `CompiledMemberTemplate` exactly as `member_sql.rs:288-330`, so `SqlCallBuilder` is untouched.
1. Lex into literal runs and `{ … }` interpolations (handle `{{`/`}}`). Extend RS `test_fixtures/cube_bridge/mock_member_sql.rs:149-260`.
2. Classify by head identifier: `CUBE`/`TABLE`/cube name/bare member → dotted path, `{arg:N}` (`.sql()` ⇒ `__sql_fn`); `FILTER_PARAMS.<cube>.<member>[.time_shifts.<s>].filter(<col>)` → `{fp:N}` with lambdas compiled to `{fpv:i}`; `FILTER_GROUP(...)` → `{fg:N}`; `SECURITY_CONTEXT.<path>.filter/requiredFilter/unsafeValue` → rules of `MemberSqlTemplateCompiler.js:289-361`, `{sv:N}` / `1 = 1` / `1 = 0`; `SQL_UTILS.convertTz/urlEncode` → dialect.
3. `args_names` = ordered unique head identifiers.
4. Reference lists (pre-agg `dimensions:`/`measures:`, `rollup_references`) → `SqlTemplate::StringVec` (see `mock_member_sql.rs::pre_agg_array_templates`).

### 4.3 `SqlTemplates`
`base_templates()` + per-dialect overrides, fed to `NativeSqlTemplatesRender::try_new` (or a non-generic twin). Promote `mock_sql_templates_render.rs` out of `test_fixtures`.

### 4.4 Hard parts
- JS functions in models (`.js` cubes, helper calls): no pure-Rust answer; reject with a clear error or keep an optional embedded-JS escape hatch (QuickJS/Boa, not Node).
- `FILTER_PARAMS` column callbacks with rest parameters: support fixed-arity lambdas only.
- `sub_query` dimensions: JS builds a nested `BaseQuery` (`BaseQuery.js:4356-4368`); the planner must build the sub-plan.
- `multi_stage` reference sets are evaluated reference lists → parser reference-list mode.
- `time_shift` bindings and calendar cubes' `sql`-bearing shifts.
- `case` dimensions: each `when[].sql` / `label` is its own `MemberSql`; `StringOrSql`.
- `join_tree_for_hints` fixpoint depends on hints collected from member SQL → after the parser.
- Views: `is_view`, `join_map`, `default_filters`, member aliasing are flattened by `CubeEvaluator.prepareCube`/`prepareViewFilters` (`CubeEvaluator.ts:250-541`) — the Rust loader must do that flattening too.

### 4.5 Ordered implementation plan
1. Promote `test_fixtures/cube_bridge/yaml/*` + `mock_schema.rs` into `cubemodel`; keep the bridge traits as the interface.
2. Promote `mock_sql_templates_render.rs` to `cubemodel::dialect::base_templates()`; add the Postgres delta.
3. Promote `mock_driver_tools.rs` to `dialect::postgres::PostgresDialect: DriverTools`.
4. Build the member-SQL mini-parser from `mock_member_sql.rs:149-260` for real model syntax; validate against `test_fixtures/schemas/yaml_files/symbol_evaluator/` and existing compilation tests.
5. Implement `Evaluator: CubeEvaluator + BaseTools`; `evaluate_rollup_references` reuses the parser in reference-list mode.
6. Promote `mock_join_graph.rs` to `cubemodel::join_graph`; implement the `join_tree_for_hints` fixpoint.
7. Add view flattening, `sub_query`, `multi_stage`, `time_shift`, `case`.
8. Wire a non-Neon `BaseQueryOptions` and run the planner's snapshot tests without Node.

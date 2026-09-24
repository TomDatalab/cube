# Node.js → Rust backend migration

Tracking document for replacing the Node.js runtime of Cube with Rust.
**Constraint: the final backend contains no Node.js.** The end state is a
single Rust binary (`cube-server`, crate `rust/cube/cubeserver`) that serves
the REST API, compiles data models, plans SQL (Tesseract), orchestrates
queries and talks to databases through Rust drivers. `@cubejs-backend/*`
packages and the Neon addon in `packages/cubejs-backend-native` are deleted
once every surface below is ported.

Update this file whenever a surface moves.

## Target architecture

```
                 ┌──────────────────────────── cube-server (bin) ─────────────────────────────┐
HTTP ──► axum router (cubeserver::app) ─► handlers ─► service traits (cubeserver::services)  │
                 │                                       │                                    │
                 │   AuthService ◄─── cubeauth (JWT/JWK, api scopes, playground token)        │
                 │   MetaService ◄─── cubemodel (YAML+Jinja model loader → meta config)        │
                 │   QueryService ◄── cubequery (REST query normalisation)                     │
                 │                    + cubesqlplanner (Tesseract) over cubemodel::Evaluator   │
                 │                    + orchestrator (queue/cache/pre-aggs) over cubedriver    │
                 │   HealthService ◄─ cubedriver (Driver trait, Postgres, CubeStore, …)        │
                 └────────────────────────────────────────────────────────────────────────────┘
SQL API (Postgres wire) ── rust/cubesql ── TransportService → same services (no Node bridge)
Pre-aggregation storage ── rust/cubestore (unchanged)
```

Rules:
- Handlers depend only on the traits in `cubeserver::services`; a surface is
  "ported" when its trait has a Rust implementation with tests.
- Every ported endpoint keeps the exact HTTP contract of
  `packages/cubejs-api-gateway/src/gateway.ts` (paths, query params, body,
  error body `{ "error": "...", "requestId"? }`, status codes) and the same
  `CUBEJS_*` environment variables. Node.js tests are ported alongside.
- Until the binary is complete, the Node.js server keeps running in
  production; the transitional Express → native gateway proxy
  (`CUBEJS_NATIVE_API_GATEWAY_REST_ROUTES`) only exists to test Rust
  endpoints against real deployments and is removed with the Node.js code.

## Crates

| Crate | Replaces | Status |
|---|---|---|
| `rust/cube/cubeserver` | `cubejs-server`, HTTP layer of `cubejs-api-gateway` | routes for `/readyz`, `/livez`, `/v1/meta`, `/v1/load`, `/v1/sql`, `/v1/dry-run`, `/cubejs-system/v1/context`; auth middleware; CORS; body limit; JSON 404; config from env. Query endpoints answer 501 until the planner + orchestrator are wired. |
| `rust/cube/cubeauth` | auth in `cubejs-api-gateway` (`checkAuth`, JWT/JWK, `contextToApiScopes`) | done: HS/RS/ES JWT, `CUBEJS_API_SECRET(S)`, `CUBEJS_JWT_*`, JWK URL cache with rotation, playground secret, claims namespace, legacy `u` claim, default scopes; wired into `cube-server` (`auth_adapter.rs`). User hooks (`checkAuth`/`contextToApiScopes` in config) pending. |
| `rust/cube/cubequery` | `cubejs-api-gateway/src/query.js`, `date-parser.js`, request parsing | done: lenient `Query` model, `normalize_query` (limits, timezone canonicalization, granularity dimensions, order remap, cache mode), filter normalization with relative dates inside groups, a `chrono-node`/`moment` port for relative date strings, `parse_query_param`, `compare_date_range_transformer`, `get_normalized_queries`, `get_pivot_query`. Wired into `cube-server`'s query handlers. |
| `rust/cube/cubemodel` | `cubejs-schema-compiler` model loading (YAML + Jinja), validation, views, meta config | done for YAML: loader, Jinja, `extends`, validation with Node's messages, view resolution (`includes`/`excludes`/prefix/alias/join paths), hierarchies, folders, formats, `connectedComponent`, and the `/v1/meta` response. JS and Python models are rejected with a clear error. Wired into `cube-server`. |
| `rust/cube/cubeplanner` | public planning API over Tesseract without JS callbacks | done: model, evaluator, Rust `build_join`, Postgres + CubeStore dialects, and a member-SQL parser covering `{CUBE.member}`, `FILTER_PARAMS`, `FILTER_GROUP`, `SECURITY_CONTEXT`, `SQL_UTILS`. Unsupported constructs fail with a named error. Wired into `cube-server` for `/v1/sql` and `/v1/dry-run`. Spec: `docs/tesseract-evaluator-spec.md` |
| `rust/cube/cubedriver` | `cubejs-base-driver` + drivers | done: `Driver` trait (full `DriverInterface` surface), env config incl. multi data source, generic type mapping, result type detection, and a driver for every `CUBEJS_DB_TYPE` except `jdbc` (27 types; table under "Drivers" below). Each driver sits behind its own Cargo feature, all on by default. Wired into `cube-server` for the health probes and for query execution. |
| `rust/cube/cubesqlplanner` + `rust/cube/cubeplanner` | `BaseQuery.js` planning | done for YAML models: evaluator, Rust `build_join`, member-SQL parser, member expressions, all eleven query options, strict eager validation, the alias→member map, and all 24 dialects of the Node schema compiler and driver packages. `Dialect::for_db_type` maps every `CUBEJS_DB_TYPE`; `cube-server` picks the dialect per data source and refuses an unknown type instead of planning it as Postgres. Spec: `docs/tesseract-evaluator-spec.md` |
| `rust/cube/cubeorchestrator` | result transform | done |
| `rust/cube/cubecache` | `QueryCache`'s keys, entries and cache drivers | done: `getCacheHash` byte compatible with Node (golden tested against the JS implementation), the cache decision table, in-memory driver and result LRU |
| `rust/cube/cubequeue` | `QueryQueue` | done: the engine (execute, reconcile, dispatch, heartbeat, timeouts, cancellation, stages), `LocalQueueDriver`, the `Continue wait` contract, and persistent (streaming) queries — `QueryStream`, the `streams` map, `waitForQueryStream` and the `@processUid` reconcile filter. The CubeStore-backed driver is a later step |
| `rust/cube/cubeorch` | `QueryOrchestrator`, `QueryCache`, refresh keys, pre-aggregations | `QueryCache` (all four branches, persistent queries included), refresh keys, `QueryOrchestrator` and the "Continue wait" contract are done. Pre-aggregations: the pure hashing/naming/version functions and the loader are done; **partitioned pre-aggregations and the two external build strategies are not**, and they fail with a named error instead of degrading. Wired into `cube-server` for `/v1/load`. Spec: `docs/orchestrator-spec.md` |
| `rust/cube/cubesqlbridge` | the SQL API without the Neon bridge | done: `TransportService` and `SqlAuthService` in Rust (meta, compiler id, `sql`, user switching, logging), the SQL→REST conversion behind `/v1/convert-query` and `/v1/cubesql`, and query execution injected. Started by `cube-server` when `CUBEJS_PG_SQL_PORT` is set, sharing the orchestrator with the REST API |
| `rust/cube/cubegraphql` | `cubejs-api-gateway/src/graphql.ts` | done: dynamic schema from the meta config, the GraphQL → Cube query translation, execution and response shaping, GraphiQL. Wired into `cube-server` |
| `rust/cube/cubeconfig` | `cube.js` configuration, replaced by declarative config | done: `cube.yml` with data sources, tenants, API and scheduled refresh; environment wins over the file. Inventory of all 84 options in `docs/config-migration.md` (36 to env, 19 to config, 29 dropped). Wired into `cube-server` |
| `rust/cubesql` | SQL API | done; `TransportService` must be re-implemented over the Rust services instead of the Node bridge |
| `rust/cubestore` | pre-aggregation storage | done |

## Endpoint status (`packages/cubejs-api-gateway/src/gateway.ts`)

| Endpoint | Rust status |
|---|---|
| `GET /readyz`, `GET /livez` | done: probes test every configured data source through `cubedriver` and the orchestrator's store, like `testConnections` + `testOrchestratorConnections` (`readyz` only in standalone mode, like Node). Drivers are built on the first probe and reused, so a probe every few seconds does not churn connection pools and a server whose database is still starting boots and reports `DOWN` until it answers |
| `GET /v1/connectors` | new in Rust; no Node equivalent. Lists every `CUBEJS_DB_TYPE` Cube knows, whether this build implements it, and the configured data sources using it. `?onlyConfigured=true` keeps only the ones in use. Behind the `meta` scope |
| `GET /v1/meta` | done: served from the YAML model by `cubemodel`, including `onlyViews` and the dev-mode/playground visibility rules |
| `GET|POST /v1/sql` | done: planned by `cubeplanner` from the YAML model, with bound parameters. The orchestrator-derived fields of the Node response (`preAggregations`, `cacheKeyQueries`, `aliasNameToMember`, `canUseTransformedQuery`, `external`, `dataSource`) are not present yet |
| `GET|POST /v1/dry-run` | done except `transformedQueries`, which comes from the orchestrator's pre-aggregation matching |
| `GET|POST /v1/load` | done: planned once per query by `cubeplanner`, executed through the orchestrator's cache and queue against a real driver, with rows keyed by member name. `cubeserver::load_response` (shared with the WebSocket transport) runs every query of a `compareDateRange`/blending request, fills `annotation` from the meta config (`prepareAnnotation`) and answers `queryType=multi` — which `@cubejs-client/core` always sends — with `{ queryType, results, pivotQuery, slowQuery }` |
| `GET /cubejs-system/v1/context` | done |
| `POST /v1/convert-query` | done: turns a SQL statement into the REST query it is equivalent to, with Node's three body checks and its error-in-the-body contract |
| `POST /v1/cubesql` | done: reports the statement the data source would run, or that the query needs post-processing |
| `GET|POST /v1/subscribe` | done: the same handler as `/v1/load`, as in Node.js |
| WebSocket transport (`{basePath}/ws`) | done: the full message protocol (`authorization`, `unsubscribe`, `load`/`sql`/`dry-run`/`meta`/`subscribe`/`unsubscribe`), the subscription store with stale eviction, and the refresh loop that re-runs live subscriptions |
| `DELETE /v1/running-query/:requestId` | routed with the `data` scope and the `{ "result": bool }` body; the cancel itself lands with the orchestrator |
| `POST /v1/pre-aggregations/can-use`, `/jobs` | routed with the `meta` and `jobs` scopes; answer 501 until the orchestrator lands |
| `/cubejs-system/v1/pre-aggregations`, `/security-contexts`, `/timezones`, `/partitions`, `/preview`, `/build`, `/queue`, `/cancel` | routed behind the playground-token check, with the Node response envelopes (`securityContexts`, `timezones`); answer 501 until the orchestrator lands |
| `POST /v1/graphql-to-json` | done: translates a GraphQL document into the REST query, always answering 200 like Node |
| `GET|POST /graphql` | done: executes through the same `QueryService` as `/v1/load`, and serves GraphiQL on GET |
| `POST /v1/run-scheduled-refresh` | n/a in Node either; refresh scheduler is a background loop |
| `GET /` and the Playground assets | done when `CUBEJS_PLAYGROUND_PATH` (or a `playground/` directory) holds a build. Served at the root, as the Node dev server does, because the bundle fetches its helper API relatively and derives the API address from the page URL. An unknown path still answers the API's JSON 404 |
| `GET /playground/context` | done: the bootstrap call. Returns `basePath` (this server's API prefix, not Node's `/cubejs-api`), a signed `cubejsToken`, and `telemetry`/`livePreview`/`shouldStartConnectionWizardFlow` all false |
| `GET /playground/files`, `GET /playground/db-schema`, `GET /playground/driver`, `POST /playground/token` | done |
| `POST /playground/generate-schema`, `/env`, `/test-connection`, `/schema/pre-aggregation`, the dashboard-app and live-preview routes | not ported: they generate a model with the JavaScript compiler, rewrite `.env`, install npm driver packages or talk to Cube Cloud. They answer 501 with a message pointing at the YAML model |

## The Playground

The query-builder slice runs on the Rust server. The app is the upstream
build, served as static files, so no JavaScript runs on the server side.

What works: the data model browser, the query builder, running queries,
charts, the generated SQL tab, the GraphQL tab, filter value autocomplete and
the security-context editor. The builder talks to the ordinary REST API
(`/v1/meta`, `/v1/load`, `/v1/sql`, `/v1/dry-run`) through
`@cubejs-client/core`, not through `/playground/*`, so nothing special is
needed to serve it.

What does not: generating a data model from the database, the connection
wizard, the dashboard-app scaffolding and live preview.

Two things are worth knowing about the bundle.

It fetches its helper API with a relative URL (`fetch('playground/context')`)
and computes the REST API address from `window.location.href` minus the hash.
Served under a prefix, both resolve one directory too deep, so the app must
be served at `/`. It navigates by hash (`#/build`), so no history fallback is
needed and an unknown path can keep answering the API's JSON 404.

Its `index.html` carries a Segment analytics snippet that loads a third-party
script and reports every page view. It is unconditional and ignores the
`telemetry: false` the server sends, so the deployment's install script
strips it. The same script rewrites the addresses the Frontend Integrations
page offers for copy-paste, which upstream hard-codes as
`http://localhost:4000/cubejs-api`, and the dead `/cubejs-api` default of
`buildApiUrl`. Nothing in the served bundle then names the old prefix.

The helper routes are not behind the auth middleware, because the app calls
`/playground/context` to obtain its token. The server therefore refuses to
mount the Playground when `NODE_ENV=production` enforces authentication, and
says so at start-up. Node.js has the same property: its dev server only runs
outside production.

## Workstreams and order

1. **Foundations (this branch):** `cubeserver`, `cubeauth`, `cubequery`,
   `cubemodel` (YAML → meta), `cubedriver` (Postgres). Wire them into the
   binary so `/v1/meta` and probes run with zero Node.js.
2. **Planner without JS:** implement the `cube_bridge` traits of Tesseract
   over `cubemodel` (`Evaluator`, `BaseTools`, `DriverTools`, `SqlTemplates`
   per dialect, member SQL reference resolution `{CUBE.member}`,
   `FILTER_PARAMS`, `SECURITY_CONTEXT`). Then `/v1/sql` and `/v1/dry-run`.
3. **Orchestrator:** query queue + cache on CubeStore's `QUEUE`/`CACHE`
   commands (or in-memory for single node), `fetchQuery` state machine,
   "Continue wait" contract, result transform (already Rust). Then
   `/v1/load` without pre-aggregations, `DELETE /v1/running-query`.
4. **Pre-aggregations:** loader, partitions, refresh keys, external upload to
   CubeStore, refresh scheduler, `/v1/pre-aggregations/*` and system routes.
5. **SQL API on Rust services:** replace `NodeBridgeTransport` with an
   implementation over `cubemodel`/orchestrator, drop the Neon addon.
6. **Drivers:** done — every `CUBEJS_DB_TYPE` but `jdbc` (see "Drivers").
7. **Remaining API:** WebSocket subscriptions, GraphQL, `convert-query`,
   `cubesql` endpoint, `dev-server`/playground assets.
8. **Delete Node.js:** remove `packages/cubejs-server*`, `cubejs-api-gateway`,
   `cubejs-query-orchestrator`, `cubejs-schema-compiler`, drivers and
   `cubejs-backend-native`; keep client libraries.

## Decisions

**Pure Rust only** (decided by the user on 2026-09-23, after the no-Node.js
constraint): no embedded JavaScript engine (QuickJS, Boa, ...) and no embedded
Python runtime. Consequences:

- **Data models**: YAML only. A `*.js` model file is rejected with a clear
  error naming the file; the same applies to YAML whose `sql` needs a JS
  helper. Jinja templating stays, because it is already native (minijinja).
- **Configuration**: environment variables (`CUBEJS_*`) and static files.
  `cube.js` and `cube.py` are not supported, so the config hooks that were
  JavaScript functions (`checkAuth`, `contextToApiScopes`, `queryRewrite`,
  `driverFactory`, `scheduledRefreshContexts`, `extendContext`, ...) need a
  native replacement: declarative configuration where the hook only selects
  among built-in behaviours, and otherwise nothing. Any hook that cannot be
  expressed declaratively is dropped, and the migration notes must say so.
- **Playground / dev server**: static assets are embedded in the binary or
  the feature is dropped; no Node.js process serves them.
- **Python `@template.function`** in YAML models is not supported.

This is a deliberate reduction in surface: models and deployments that rely
on JavaScript or Python must be converted to YAML plus environment
configuration. Every such case gets an explicit error rather than a silent
difference.

## Running

```bash
cd rust/cube
cargo test -p cubeserver -p cubeauth -p cubequery -p cubemodel -p cubedriver -p cubeplanner
cargo run -p cubeserver
```

Environment: `PORT`, `CUBEJS_SCHEMA_PATH` (the model directory), `CUBEJS_API_SECRET`,
`CUBEJS_DB_TYPE`, `CUBEJS_DB_*`, `CUBEJS_DEV_MODE`, `NODE_ENV`. With a model
directory and an API secret the binary already serves `/readyz`, `/livez`,
`/v1/meta`, `/v1/sql`, `/v1/dry-run` and `/cubejs-system/v1/context` with no
Node.js in the process.

### Note on the planner's threading model

`cubeplanner::Model` is built on `Rc` (it mirrors the single-threaded
`cube_bridge` traits), so it is neither `Send` nor `Sync`. `cubeserver`'s
`PlannerPool` gives each worker thread its own compiled model and talks to
them over channels. Making the planner's model thread-safe would remove that
indirection and is worth doing before the orchestrator adds more concurrency.

## Known gaps, in priority order

1. **Lambda rollups.** `rollupLambdaId` is refused with a named error. It
   needs the CSV query mode and inline tables, which the queue and the driver
   trait do not carry yet. Everything else in the orchestrator is ported,
   including partitioned pre-aggregations and both external build strategies.
2. **Driver gaps.** Every driver is ported (see "Drivers" below); what is
   missing inside them fails with a named error:
   - export-bucket unload: BigQuery, Snowflake, Redshift, Databricks;
   - Presto/Trino: S3 without static keys (IRSA, instance profiles), GCS
     without a service account, `CUBEJS_DB_SSL_PASSPHRASE`/`_SERVERNAME`;
   - ksql: `download_table`/`download_query_results` for streaming
     pre-aggregations need a streaming variant of `DownloadedData` and a
     string `stream_offset` (the Cube Store import helpers exist);
   - Vertica: Kerberos, OAuth, TOTP, `COPY`; Hive: Kerberos (as in Node);
   - Oracle: `TIMESTAMP WITH TIME ZONE` values stored with a region name
     (the pinned `oracledb` beta panics; the driver turns it into an error).
     Re-pin `oracledb` once 26.0.0 is released;
   - Snowflake password auth and encrypted keys, MS SQL Windows auth and
     client certificates, Redshift IAM, Firebolt server-side parameters;
   - never run against the real cloud service: Athena, Databricks.
3. **Deleting the Node.js packages** (`docs/node-removal-plan.md`), once the
   integration suites and the Docker image run against `cube-server`.

## Drivers

Every `CUBEJS_DB_TYPE` of the Node.js server, with the Cargo feature that
builds it (all in `default`; `cargo build -p cubeserver --no-default-features --features cubedriver/…` builds a
subset, and a type left out fails at start-up with a named error).

| `CUBEJS_DB_TYPE` | Feature | Client | Verified against |
|---|---|---|---|
| postgres, redshift | — | tokio-postgres | Postgres |
| mysql | — | mysql_async | MySQL 8 |
| clickhouse | — | HTTP | ClickHouse 24.8 |
| cubestore | — | WebSocket | Cube Store |
| mssql | — | tiberius | SQL Server 2022 |
| bigquery, snowflake, druid, firebolt | — | REST | — |
| crate | `cratedb` | Postgres wire | CrateDB 6.4 |
| materialize | `materialize` | Postgres wire | Materialize v26 |
| questdb | `questdb` | Postgres wire | QuestDB 10 |
| vertica | `vertica` | own client (`vertica/wire.rs`); tokio-postgres cannot read Vertica's type OIDs | Vertica 9.2 |
| mongobi | `mongobi` | MySQL wire | mongosqld 2.14 + MongoDB 6 |
| prestodb, trino | `prestodb`, `trino` | HTTP (`nextUri`) | Presto 0.294, Trino 483 (+ S3 unload) |
| pinot | `pinot` | HTTP broker | Pinot QuickStart |
| athena | `athena` | AWS SDK (rustls) | mock + LocalStack S3 |
| mysqlauroraserverless | `mysqlauroraserverless` | AWS RDS Data API | local-data-api, MySQL 5.7 and 8.4 |
| dremio | `dremio` | REST | Dremio OSS |
| ksql | `ksql` | REST + rskafka | ksqlDB 0.29 + Kafka + Cube Store |
| databricks-jdbc | `databricks` | SQL Statement Execution API (no JVM) | mock HTTP |
| hive | `hive` | Thrift HiveServer2 (SASL PLAIN, NOSASL via `CUBEJS_DB_HIVE_AUTH`) | Hive 4.0 |
| oracle | `oracle` | `oracledb` thin (pure Rust, no Instant Client) | Oracle Free |
| sqlite | `sqlite` | rusqlite, bundled | in-memory / file |
| duckdb | `duckdb` | duckdb, bundled (C++) | in-memory / file |

`jdbc` is dropped: it loads JDBC jars into a JVM. It fails at start-up
naming the native replacements (mysql, athena, hive — which also serves
Spark SQL through the Spark Thrift server — and databricks-jdbc).

Embedded engines (SQLite, DuckDB) are bundled C/C++, the same trade-off the
Node drivers make with their native addons; see "TLS and the no C toolchain
claim". Unload output that is gzip compressed or uses caret delimiters
(`^A`) is decompressed and parsed by `csv_import`, so it also reaches
drivers other than Cube Store.

## A deliberate divergence from Node.js: readiness probes every data source

`ApiGateway.readiness` tests only the `default` data source and carries the
comment `todo: test other data sources`. A deployment with a second data
source would therefore report `HEALTH` while every query against that source
failed, which is the opposite of what a readiness probe is for. The Rust
probes test all of them, so the todo is closed rather than carried over.

## A deliberate divergence from Node.js: foreign keys

The Rust drivers fix a bug the Node.js drivers have, rather than reproducing
it, because it silently loses data-model information.

`BaseDriver.getColumnsForSpecificTables` builds its condition from
`columns.table_schema` / `columns.table_name` and passes it to
`foreignKeysQuery`. In both Node queries the alias `columns` points at the
*referenced* side of the constraint, so asking for the columns of `orders`
filtered on `target = orders`, matched nothing, and every join was dropped
during incremental schema loading. MySQL had two more faults: it joined
`information_schema.key_column_usage` to itself, so `target_table` repeated
the referencing table, and it compared a table name against a list of schema
names.

The Rust queries give the `columns` alias to the *referencing* table, so the
condition selects what the caller asked for, and read the referenced side from
`constraint_column_usage` (Postgres) or from the row's own
`referenced_table_name` / `referenced_column_name` (MySQL). Both are verified
against a live server in `cubedriver/tests/{postgres,mysql}_integration.rs`,
and the unit tests pin the SQL.

Node.js remains wrong here. The same fix belongs upstream in
`packages/cubejs-postgres-driver/src/PostgresDriver.ts:201` and
`packages/cubejs-mysql-driver/src/MySqlDriver.ts:216`, and in every other
driver that defines `foreignKeysQuery`, until those packages are deleted.

## Worker stack size

The SQL API's planner (DataFusion plus the e-graph rewriter) recurses deeply
enough to overflow tokio's 2 MiB default stack while planning an ordinary
grouped query — the process aborts with `stack overflow` rather than failing
the request. `cube-server` therefore builds its runtime with an 8 MiB worker
stack, the same figure cubesql's own harness uses
(`cubesql/src/compile/test/mod.rs:1460`). `CUBEJS_WORKER_STACK_SIZE` raises it
for a model that needs more, and a smaller value is ignored.

Anything else that embeds cubesql must do the same. Node.js never hit this
because its threads already have large stacks.

## Multi-tenancy and model reload

A `tenants:` block in `cube.yml` selects, per security context, the model to
compile, the data source to query and the cache prefix to use — the
declarative replacement for `contextToAppId`, `repositoryFactory` and
`driverFactory`. `cubeserver::tenants::TenantRegistry` turns a resolved tenant
into its own meta service, planner pool, orchestrator and GraphQL schema,
compiled on first use and kept until a reload.

- A deployment with no `tenants:` block resolves to a single tenant, so the
  single-tenant path is the multi-tenant path with one entry. `AppState`
  carries `tenants: Option<Arc<TenantRegistry>>`, and the service fields stay
  as the single-tenant answer.
- A tenant rule may narrow `api_scopes`, and that narrowing wins over what the
  auth service grants.
- Two tenants never share a cached result: the orchestrator's cache prefix is
  the tenant's `orchestrator_id`.
- An unresolvable security context is refused with 403 and a message naming
  the claim and the file.

Reload watches the modification times under each model directory and
recompiles when they move. `CUBEJS_MODEL_RELOAD_INTERVAL` is the number of
seconds, defaulting to five in dev mode and off otherwise. A model that stops
compiling keeps serving the previous one and the failure is logged, because
dropping a working model over a typo would take the deployment down. A reload
builds a new runtime and swaps it in, so a request already in flight finishes
against the model it started with.

Still missing: `COMPILE_CONTEXT` is resolved per tenant but not yet passed
into the model compiler, so a model cannot branch on it.

## Streaming

Persistent queries are ported end to end. `cubequeue::QueryStream` is the
stream the cache hands back, with back-pressure at
`CUBEJS_DB_QUERY_STREAM_HIGH_WATER_MARK`, cancellation when the consumer drops
it, and errors delivered to the consumer rather than swallowed. `cubeorch`
returns it from `cached_query_result` for a persistent query body, and
`cube-server` converts each batch into an Arrow `RecordBatch` for the SQL
API's `stream_mode`.

Verified over the Postgres wire protocol with `CUBESQL_STREAM_MODE=true`: a
grouped query and a 5000-row result set both come back correctly, with the
rows travelling in batches rather than being buffered whole.

Streaming runs on the in-process queue driver, which is what a persistent
`@processUid` key means; the Cube Store queue driver is unchanged.

## Jinja rendering, and why both crates share one engine

`cubemodel` renders Jinja before parsing YAML; `cubeplanner` used to parse the
raw file. A valid Jinja model therefore compiled for `/v1/meta` and failed
every query — the server would start, serve its metadata and refuse to answer.

Both now render through `cubemodel::jinja`, the same environment with the same
JSON auto-escape the Node engine uses, rather than through two copies that can
drift. `Model::from_dir_with_context` and
`ModelMetaService::load_with_context` take the tenant's `COMPILE_CONTEXT`, so
a multi-tenant model branches identically on both sides.

Note that Cube renders `{{ x }}` as `"x"`, with quotes: the auto-escape is
JSON, which makes every interpolated value a safe YAML scalar. A model that
concatenates (`name: is_{{ x }}`) produces invalid YAML in Node too.

## TLS and the "no C toolchain" claim

MS SQL is built with `tiberius`' `rustls` feature, so `CUBEJS_DB_SSL=true`
encrypts the connection and Azure SQL works. `CUBEJS_DB_SSL_REJECT_UNAUTHORIZED`
and `CUBEJS_DB_SSL_CA` mean what they mean in the other drivers; an inline CA
is written to a temporary file because `tiberius` reads it from a path.
Verified against SQL Server 2022, with `sys.dm_exec_connections` reporting the
session as encrypted and a self-signed certificate refused once verification
is on.

That feature brings `openssl-probe`. Despite the name it has no dependencies,
no build script, and does nothing but look up the system certificate paths —
it links no C library.

Worth recording plainly: the tree already contains crates that *do* link C,
and they predate this change. `rustls` pulls `aws-lc-sys`, and the MySQL
driver pulls `libz-sys` and `zstd-sys` through `mysql_async`. So "pure Rust"
here means no scripting runtime and no OpenSSL, not the absence of every C
dependency. A build that must avoid them entirely would need rustls on the
`ring` provider and a MySQL client without compression.

## Endpoint paths

The Rust server serves `cube`-prefixed paths rather than the `cubejs-` ones
Node uses:

| Node.js | Rust |
|---|---|
| `/cubejs-api/v1/*` | `/cube/v1/*` |
| `/cubejs-api/graphql` | `/cube/graphql` |
| `/cubejs-api/ws` | `/cube/ws` |
| `/cubejs-system/v1/*` | `/cube-system/v1/*` |

`/cube` is only the default base path: `api.base_path` in `cube.yml` and
`CUBEJS_BASE_PATH` still override it, so a deployment that has to keep the old
paths sets `base_path: /cubejs-api`. The system routes are fixed, as they are
in Node.

Environment variables keep their `CUBEJS_` names: renaming those would break
every existing deployment for no gain, and they are not endpoints.

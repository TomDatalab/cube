# Configuration migration: `cube.js` / `cube.py` → `cube.yml`

The Node.js backend was configured by a JavaScript (or Python) module that
exported **callbacks**: `driverFactory`, `contextToAppId`, `queryRewrite`,
`checkAuth` and friends. The pure-Rust backend embeds no JavaScript and no
Python runtime, so those files are gone. Everything they expressed is now one
of three things:

- **env** — an environment variable (`CUBEJS_*`),
- **config** — a declarative key in `cube.yml` (implemented by the `cubeconfig` crate),
- **dropped** — no longer expressible; the "Replacement / what to do instead"
  column says what to do.

This document is the complete, honest inventory. If an option you rely on is
marked *dropped*, that is a capability the pure-Rust backend does not have.

Verdict counts are at the [bottom](#verdict-counts).

---

## 1. The configuration file

`cubeconfig` looks for `cube.yml` (or `cube.yaml`) in the project directory.
Having both is an error. A deployment with a single data source and a single
tenant needs no file at all — `CubeConfig::from_env()` builds an equivalent
configuration from `CUBEJS_DB_*` alone.

**The environment always wins over the file.** A file value is the default, an
environment variable is the override. This is the inverse of the Node.js
behaviour for a few options (where `cube.js` beat the environment), and it is
deliberate: containers configure by environment, and an operator must be able
to change a value without rebuilding the image.

### Schema

```yaml
version: 1                              # optional; only 1 is understood

log_level: info                         # trace | debug | info | warn | error
telemetry: true
model_path: model                       # default data model directory
pre_aggregations_schema: prod_pre_aggregations
app_id: STANDALONE                      # used when there is no `tenants` block

api:
  base_path: /cubejs-api
  default_scopes: [graphql, meta, data, sql]   # jobs is opt-in
  max_request_size: 50mb                        # 100kb .. 64mb
  cors:
    enabled: true
    origin: ["https://app.example.com"]         # or ["*"]
    methods: [GET, POST, OPTIONS]
    allowed_headers: ["*"]
    exposed_headers: []
    credentials: false                          # cannot be combined with "*"
    max_age: 600

data_sources:
  default:
    type: postgres                      # one of the supported driver types
    url: { env: CUBEJS_DB_URL }         # any scalar may be an env reference
    host: { env: CUBEJS_DB_HOST, default: localhost }
    port: 5432
    database: analytics
    schema: public
    user: { env: CUBEJS_DB_USER }
    password: { env: CUBEJS_DB_PASS }   # secrets belong in the environment
    ssl: true
    max_pool: 8
    export_bucket: s3://cube-export
    export_bucket_type: s3
    options:                            # driver-specific passthrough
      warehouse: COMPUTE_WH

tenants:
  claim: tenant_id                      # JWT security-context claim; dotted
                                        # paths such as acl.org.id work
  on_missing: error                     # error | default
  default:                              # used when the claim is absent
    app_id: shared
    model_path: model/shared
    data_source: default
  rules:
    - match: acme                       # exact
      app_id: acme
      model_path: model/acme
      data_source: warehouse
      pre_aggregations_schema: acme_pre_aggregations
      orchestrator_id: acme             # defaults to app_id
      api_scopes: [meta, data, sql, jobs]
      compile_context:                  # merged into COMPILE_CONTEXT
        region: us
    - match_any: [beta, gamma]          # any of
      app_id: "tenant_{value}"          # {value} = the matched claim value
      model_path: "model/{value}"
    - match_pattern: "eu_*"             # glob, * only
      app_id: "{value}"
      model_path: model/eu

scheduled_refresh:
  enabled: true
  interval: 30                          # seconds
  timezones: [UTC, America/Los_Angeles]
  concurrency: 4
  batch_size: 1
  contexts:                             # STATIC list; see §5
    - security_context: { tenant_id: acme }
    - security_context: { tenant_id: beta }
```

### Rust API

```rust
let config = CubeConfig::load("/cube")?;                 // file + process env
let config = CubeConfig::from_env()?;                    // env only, no file
let tenant = config.for_security_context(&claims)?;      // ResolvedTenant
let ds     = config.data_source(&tenant.data_source);    // DataSource
let jobs   = config.refresh_tenants()?;                  // background contexts
```

`ResolvedTenant` carries `app_id`, `orchestrator_id`, `model_path`,
`data_source`, `pre_aggregations_schema`, `api_scopes` and `compile_context`
— the resolved equivalent of `contextToAppId` + `contextToOrchestratorId` +
`repositoryFactory` + `driverFactory` + `COMPILE_CONTEXT`.

Every validation failure names the file and the key
(`cube.yml: key \`data_sources.default.type\`: invalid value "mongodb": …`) or
the environment variable (`environment variable \`CUBEJS_DB_PORT\`: …`).

---

## 2. `CreateOptions`: server and HTTP

| `cube.js` option | Verdict | Replacement / what to do instead |
| --- | --- | --- |
| `webSockets` | **env** | `CUBEJS_WEB_SOCKETS` |
| `webSocketsBasePath` | **env** | `CUBEJS_WEB_SOCKETS_BASE_PATH` (new name for what was file-only) |
| `processSubscriptionsInterval` | **env** | `CUBEJS_PROCESS_SUBSCRIPTIONS_INTERVAL` (new name for what was file-only) |
| `http.cors` | **config** | `api.cors` (overrides: `CUBEJS_CORS_ENABLED`, `CUBEJS_CORS_ORIGIN`, `CUBEJS_CORS_MAX_AGE`, all new). Only literal origins and `"*"` — the `cors` middleware's function and `RegExp` origins are gone (see §7). |
| `gracefulShutdown` | **env** | `CUBEJS_GRACEFUL_SHUTDOWN` |
| `serverHeadersTimeout` | **env** | `CUBEJS_SERVER_HEADERS_TIMEOUT` |
| `serverKeepAliveTimeout` | **env** | `CUBEJS_SERVER_KEEP_ALIVE_TIMEOUT` |
| `basePath` | **config** | `api.base_path` (override: `CUBEJS_API_BASE_PATH`, new) |
| `gatewayPort` | **env** | `CUBEJS_NATIVE_API_GATEWAY_PORT` |
| `sqlPort` | **env** | `CUBEJS_SQL_PORT` |
| `pgSqlPort` | **env** | `CUBEJS_PG_SQL_PORT` |
| `devServer` | **env** | `CUBEJS_DEV_MODE` |
| `serverless` | **dropped** | Internal flag for the Node serverless packages, which do not exist in the Rust backend. |
| `dashboardAppPath` | **dropped** | Dev-only scaffolding that shelled out to `npx create-react-app`. Generate a dashboard app yourself. |
| `dashboardAppPort` | **dropped** | Same as above. |
| `livePreview` | **env** | `CUBEJS_LIVE_PREVIEW` |
| `fastReload` | **env** | `CUBEJS_FAST_RELOAD_ENABLED` |

## 3. Data sources and drivers

| `cube.js` option | Verdict | Replacement / what to do instead |
| --- | --- | --- |
| `driverFactory` (returning a `DriverConfig`) | **config** | `data_sources.<name>` + `tenants.rules[].data_source`. All connection fields accept `{ env: NAME }`. |
| `driverFactory` (returning a `BaseDriver` instance) | **dropped** | Instantiating a JS driver object is impossible. Use a declared `data_sources` entry; if you wrapped a driver to add behaviour, that behaviour has no home. |
| `dbType` | **dropped** | Already removed in v1.7.0. Use `data_sources.<name>.type` or `CUBEJS_DB_TYPE` / `CUBEJS_DS_<NAME>_DB_TYPE`. |
| `dialectFactory` | **dropped** | The SQL dialect is derived from the data source `type`. A custom `BaseQuery` subclass cannot be loaded. |
| `externalDbType` | **env** | `CUBEJS_EXT_DB_TYPE` (in practice always `cubestore`). |
| `externalDriverFactory` | **env** | `CUBEJS_CUBESTORE_HOST` / `_PORT` / `_USER` / `_PASS`, or the `CUBEJS_EXT_DB_*` family. |
| `externalDialectFactory` | **dropped** | Determined by the external store type. |
| `cacheAndQueueDriver` | **env** | `CUBEJS_CACHE_AND_QUEUE_DRIVER` (`cubestore` \| `memory`) |
| per-data-source `CUBEJS_DS_<NAME>_*` | **env** | Unchanged. `CUBEJS_DATASOURCES` still declares the names, and the file's `data_sources` keys are merged with it. |

## 4. Multi-tenancy

| `cube.js` option | Verdict | Replacement / what to do instead |
| --- | --- | --- |
| `contextToAppId` | **config** | `tenants.claim` + `tenants.rules[].app_id`. Covers the overwhelmingly common `` `CUBE_APP_${securityContext.tenantId}` `` shape, including `{value}` templating. |
| `contextToOrchestratorId` | **config** | `tenants.rules[].orchestrator_id`; defaults to the rule's `app_id` (the Node default was the constant `STANDALONE`, which silently shared one query cache across tenants). |
| `contextToDataSourceId` | **dropped** | Already removed upstream; it threw at startup. |
| `repositoryFactory` | **config** | `tenants.rules[].model_path`. A per-tenant *directory* is expressible; a per-tenant *generated* file list is not. |
| `contextToApiScopes` | **config** | `api.default_scopes` + `tenants.rules[].api_scopes`. Per-*role* scopes computed from arbitrary claims are gone; express them as tenant rules or as data-model access policies. |
| `contextToGroups` | **dropped** | Dynamic group membership computed in JS. Put group logic in the data model's `access_policy` blocks, which read the security context declaratively. |
| `contextToCubeStoreRouterId` | **dropped** | Cube Store router sharding per context. Run separate deployments if you need this. |
| `extendContext` | **config** (partly) | `tenants.rules[].compile_context` adds static values per tenant. Computing values per request (an HTTP call to fetch a tenant profile, a DB lookup) is **dropped**. |
| `COMPILE_CONTEXT` in the data model | **config** | Still populated: `securityContext` and `security_context` plus `compile_context` extras from the matched rule. The values are the resolved, static ones — `COMPILE_CONTEXT` is no longer an arbitrary JS object. |
| `schemaVersion` | **dropped** | A callback used to bust the compiler cache when an external model store changed. The Rust compiler keys its cache on the content of `model_path`. |
| `CUBEJS_APP` | **env** | Unchanged; supplies the default `app_id` when there is no `tenants` block. |

### Tenant selection rules

1. Read `tenants.claim` out of the security context (dotted paths supported;
   strings, numbers and booleans are stringified).
2. If the claim is **absent**: use `tenants.default`; if there is none, reject
   the request.
3. Otherwise evaluate `tenants.rules` **in order** and take the first whose
   `match` / `match_any` / `match_pattern` accepts the value.
4. If nothing matches: reject (`on_missing: error`, the default) or fall back
   to `tenants.default` (`on_missing: default`).
5. Fields the rule omits fall back to the top-level `app_id`, `model_path`,
   `pre_aggregations_schema`, first `data_sources` entry and
   `api.default_scopes`. `{value}` in any of the rule's string fields expands
   to the matched claim value.

Rejecting by default is a deliberate difference from `contextToAppId`, which
could return any string for any input and would happily serve an unknown
tenant out of the shared model.

## 5. Scheduled refresh

| `cube.js` option | Verdict | Replacement / what to do instead |
| --- | --- | --- |
| `scheduledRefreshTimer` (bool) | **config** | `scheduled_refresh.enabled` (env: `CUBEJS_REFRESH_WORKER`) |
| `scheduledRefreshTimer` (number) | **config** | `scheduled_refresh.interval` in seconds (env: `CUBEJS_SCHEDULED_REFRESH_TIMER`) |
| `scheduledRefreshTimeZones` (array) | **config** | `scheduled_refresh.timezones` (env: `CUBEJS_SCHEDULED_REFRESH_TIMEZONES`) |
| `scheduledRefreshTimeZones` (function) | **dropped** | Per-context time zone lists. Use the union of all time zones, at the cost of extra partitions. |
| `scheduledRefreshContexts` | **config** | `scheduled_refresh.contexts`: a **static** list of security contexts. This is the single most consequential change — see §7. |
| `scheduledRefreshConcurrency` | **config** | `scheduled_refresh.concurrency` (env: `CUBEJS_SCHEDULED_REFRESH_QUERIES_PER_APP_ID`) |
| `scheduledRefreshBatchSize` | **config** | `scheduled_refresh.batch_size` (env: `CUBEJS_SCHEDULED_REFRESH_BATCH_SIZE`) |

`cubeconfig` refuses to start a multi-tenant deployment with
`scheduled_refresh.enabled: true` and an empty `contexts` list, because nothing
can enumerate tenants at runtime any more. The Node.js backend only logged a
warning in this situation and then silently refreshed the wrong tenant.

## 6. Everything else in `CreateOptions`

| `cube.js` option | Verdict | Replacement / what to do instead |
| --- | --- | --- |
| `apiSecret` | **env** | `CUBEJS_API_SECRET` |
| `apiSecrets` | **env** | `CUBEJS_API_SECRETS` (comma-separated rotation window) |
| `jwt.*` (`key`, `algorithms`, `issuer`, `audience`, `subject`, `claimsNamespace`) | **env** | `CUBEJS_JWT_KEY`, `CUBEJS_JWT_ALGS`, `CUBEJS_JWT_ISSUER`, `CUBEJS_JWT_AUDIENCE`, `CUBEJS_JWT_SUBJECT`, `CUBEJS_JWT_CLAIMS_NAMESPACE` |
| `jwt.jwkUrl` (string) | **env** | `CUBEJS_JWK_URL` |
| `jwt.jwkUrl` (function) | **dropped** | A per-token JWKS endpoint. Use one issuer, or run one deployment per issuer. |
| `jwt.jwkRetry`, `jwkDefaultExpire`, `jwkRefetchWindow` | **env** | `CUBEJS_JWK_RETRY`, `CUBEJS_JWK_DEFAULT_EXPIRE`, `CUBEJS_JWK_REFETCH_WINDOW` (new names for what was file-only) |
| `checkAuth` | **dropped** | Arbitrary auth code. Use the built-in JWT/JWKS verification (`cubeauth`) via the `CUBEJS_JWT_*` variables. Session cookies, opaque-token introspection and calls to an external authorization service must move into a proxy in front of Cube. |
| `checkSqlAuth` | **dropped** | Same, for the SQL API. Use `CUBEJS_SQL_USER` / `CUBEJS_SQL_PASSWORD` or a JWT. |
| `canSwitchSqlUser` | **dropped** | Use `CUBEJS_SQL_SUPER_USER`, which allows the fixed super user to impersonate. Per-pair rules are gone. |
| `sqlUser` | **env** | `CUBEJS_SQL_USER` |
| `sqlPassword` | **env** | `CUBEJS_SQL_PASSWORD` |
| `sqlSuperUser` | **env** | `CUBEJS_SQL_SUPER_USER` |
| `queryRewrite` | **dropped** | Mandatory row-level filters injected per request. Express them as `access_policy` / `row_level` rules in the data model, which are evaluated against the security context by the Rust planner. Rewrites that *change* the requested measures or dimensions have no replacement. |
| `queryTransformer` | **dropped** | Deprecated alias of `queryRewrite`. |
| `logger` | **dropped** | A custom log sink. The Rust backend emits structured JSON on stdout; filter with `log_level` / `CUBEJS_LOG_LEVEL` and ship it with your log agent. |
| `telemetry` | **config** | `telemetry` (env: `CUBEJS_TELEMETRY`) |
| `preAggregationsSchema` (string) | **config** | `pre_aggregations_schema` (env: `CUBEJS_PRE_AGGREGATIONS_SCHEMA`) |
| `preAggregationsSchema` (function) | **config** | `tenants.rules[].pre_aggregations_schema`, with `{value}` templating. |
| `schemaPath` | **config** | `model_path` (env: `CUBEJS_SCHEMA_PATH`) |
| `compilerCacheSize` | **env** | `CUBEJS_COMPILER_CACHE_SIZE` |
| `maxCompilerCacheKeepAlive` | **dropped** | A knob on the JS compiler cache's LRU eviction. The Rust compiler cache is sized by `CUBEJS_COMPILER_CACHE_SIZE` only. |
| `updateCompilerCacheKeepAlive` | **dropped** | Same. |
| `allowUngroupedWithoutPrimaryKey` | **env** | `CUBEJS_ALLOW_UNGROUPED_WITHOUT_PRIMARY_KEY` |
| `allowJsDuplicatePropsInSchema` | **dropped** | A parser flag for JavaScript data models, which no longer exist. |
| `allowNodeRequire` | **dropped** | `require()` inside data models. There is no Node. |
| `semanticLayerSync` | **dropped** | Returned BI-tool sync configs from JS. Configure BI sync out of band. |
| `sqlCache` | **env** | `CUBEJS_SQL_CACHE` (new name for what was file-only; defaulted to `true`) |
| `orchestratorOptions` (function form) | **dropped** | Per-request orchestrator tuning. The static form below applies deployment-wide. |
| `orchestratorOptions.redisPrefix` | **dropped** | Redis was removed as a cache/queue backend; Cube Store and the in-memory driver are the options. |
| `orchestratorOptions.rollupOnlyMode` | **env** | `CUBEJS_ROLLUP_ONLY` |
| `orchestratorOptions.testConnectionTimeout` | **env** | `CUBEJS_TEST_CONNECTION_TIMEOUT` (new name for what was file-only) |
| `orchestratorOptions.queryCacheOptions.refreshKeyRenewalThreshold` | **env** | `CUBEJS_REFRESH_KEY_RENEWAL_THRESHOLD` (new name for what was file-only) |
| `orchestratorOptions.queryCacheOptions.backgroundRenew` | **env** | `CUBEJS_BACKGROUND_RENEW` (new name for what was file-only) |
| `orchestratorOptions.*.queueOptions` (object) | **env** | `CUBEJS_CONCURRENCY` / `CUBEJS_DS_<NAME>_CONCURRENCY`, `CUBEJS_DB_QUERY_TIMEOUT`, `CUBEJS_REFRESH_WORKER_CONCURRENCY` |
| `orchestratorOptions.*.queueOptions` (function of `dataSource`) | **dropped** | Per-data-source concurrency is still available through `CUBEJS_DS_<NAME>_CONCURRENCY`; arbitrary computation is not. |
| `orchestratorOptions.preAggregationsOptions.externalRefresh` | **env** | `CUBEJS_PRE_AGGREGATIONS_BUILDER` / `CUBEJS_ROLLUP_ONLY` decide it, as before. |
| `orchestratorOptions.preAggregationsOptions.maxPartitions` | **env** | `CUBEJS_MAX_PARTITIONS_PER_CUBE` |
| `orchestratorOptions.preAggregationsOptions.maxSourceRowLimit` | **env** | `CUBEJS_MAX_SOURCE_ROW_LIMIT` |

### `cube.py` specifically

| Construct | Verdict | Replacement / what to do instead |
| --- | --- | --- |
| `@config` decorated functions | **dropped** | Same verdicts as the `cube.js` hooks above; there is no Python runtime. |
| `@template.function` / Jinja helpers registered from `cube.py` | **dropped** | Dynamic data-model generation from Python. Generate the YAML model ahead of time in CI and ship the result. |

---

## 7. What is genuinely lost

Ranked by how much it hurts.

1. **Programmable authentication (`checkAuth`, `checkSqlAuth`).** Anything
   other than JWT/JWKS verification — session cookies, opaque tokens
   introspected against an IdP, an authorization microservice consulted per
   request, a security context assembled from a user database — now has to
   live in a proxy in front of Cube. This is the most common non-trivial use
   of `cube.js` in production, and it has no in-process replacement.

2. **Programmable query rewriting (`queryRewrite`).** Mandatory filters derived
   from the security context can be re-expressed as data-model access
   policies, but rewrites that add or remove members, clamp time ranges, or
   enforce a limit based on a plan tier cannot. Teams using `queryRewrite` as
   their security boundary must re-validate that the access policies cover the
   same ground.

3. **Runtime tenant discovery (`scheduledRefreshContexts`, and dynamic
   `contextToAppId`).** Tenants used to be discovered by querying a control
   database at startup. Now the tenant list is a static file, so onboarding a
   tenant means writing `cube.yml` and reloading. Deployments with thousands of
   tenants, or with tenants created by end users, need a generator that emits
   `cube.yml` and a restart/reload pipeline.

4. **Computed per-request context (`extendContext`, `contextToGroups`).**
   Enriching the security context by calling out to another service during a
   request is gone. Only static, per-tenant values survive, via
   `tenants.rules[].compile_context`.

5. **Per-tenant model generation (`repositoryFactory`).** Returning a
   synthesized set of model files per tenant — the pattern behind
   "each customer gets cubes built from their own column list" — is gone.
   A per-tenant *directory* works; a per-tenant *program* does not. Generate
   the directories offline.

6. **Custom drivers and dialects (`driverFactory` returning an instance,
   `dialectFactory`).** A JS class that subclassed `BaseDriver` or `BaseQuery`
   to talk to an unsupported warehouse, or to bend SQL generation, cannot be
   loaded. Such a driver has to be ported to Rust in `cubedriver`.

7. **Custom logging (`logger`).** Routing logs to Datadog/Sentry from inside
   the process is replaced by structured stdout plus a log shipper.

8. **CORS expressiveness.** `http.cors` took the Express `cors` options,
   including a function or a `RegExp` for `origin`. `api.cors.origin` is a list
   of exact origins or `"*"`. Wildcard subdomains (`https://*.example.com`)
   are not supported; enumerate them, or terminate CORS at the proxy.

9. **Per-request orchestrator tuning.** `orchestratorOptions` as a function of
   the request context (different concurrency or cache thresholds for a heavy
   tenant) is now a deployment-wide setting. Isolate noisy tenants with a
   separate deployment.

---

## Verdict counts

| Verdict | Count |
| --- | --- |
| Replaced by an environment variable (**env**) | 36 |
| Replaced by a declarative construct in `cube.yml` (**config**) | 19 |
| **Dropped** | 29 |
| **Total options and hooks inventoried** | 84 |

One of the 19 *config* verdicts is partial: `extendContext` keeps only its
static, per-tenant half (`tenants.rules[].compile_context`).

Thirteen environment variables are new names for settings that previously
existed only inside `cube.js`: `CUBEJS_API_BASE_PATH`,
`CUBEJS_WEB_SOCKETS_BASE_PATH`, `CUBEJS_PROCESS_SUBSCRIPTIONS_INTERVAL`,
`CUBEJS_SQL_CACHE`, `CUBEJS_REFRESH_KEY_RENEWAL_THRESHOLD`,
`CUBEJS_BACKGROUND_RENEW`, `CUBEJS_JWK_RETRY`, `CUBEJS_JWK_DEFAULT_EXPIRE`,
`CUBEJS_JWK_REFETCH_WINDOW`, `CUBEJS_CORS_ENABLED`, `CUBEJS_CORS_ORIGIN`,
`CUBEJS_CORS_MAX_AGE` and `CUBEJS_TEST_CONNECTION_TIMEOUT`. Every other
`CUBEJS_*` variable documented in `packages/cubejs-backend-shared/src/env.ts`
keeps its name and meaning.

The `cubeconfig` crate itself reads only the variables that map onto the
`cube.yml` schema: `CUBEJS_LOG_LEVEL`, `CUBEJS_TELEMETRY`,
`CUBEJS_SCHEMA_PATH`, `CUBEJS_PRE_AGGREGATIONS_SCHEMA`, `CUBEJS_APP`,
`CUBEJS_API_BASE_PATH`, `CUBEJS_DEFAULT_API_SCOPES`, `CUBEJS_MAX_REQUEST_SIZE`,
`CUBEJS_CORS_*`, `CUBEJS_DATASOURCES`, the `CUBEJS_DB_*` /
`CUBEJS_DS_<NAME>_DB_*` connection family, `CUBEJS_REFRESH_WORKER`,
`CUBEJS_SCHEDULED_REFRESH*`. The rest stay with the crate that consumes them
(`cubeauth` for `CUBEJS_JWT_*`, `cubeserver` for the HTTP timeouts, and so on).

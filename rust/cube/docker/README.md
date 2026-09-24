<p align="center">
  <img src="https://raw.githubusercontent.com/TomDatalab/cube/d7a068e6d03c282ef49cc76ea8ceaeb555911477/rust/cube/docker/logo.png" alt="cube-rust logo" width="160"/>
</p>

# Cube, rebuilt in Rust

<p align="center">
  <a href="https://github.com/TomDatalab/cube"><b>Source code on GitHub</b></a> ·
  <a href="https://hub.docker.com/r/blockmill/cube">Docker Hub</a> ·
  <a href="https://github.com/TomDatalab/cube#readme">Architecture &amp; migration</a> ·
  <a href="https://github.com/TomDatalab/cube/issues">Issues</a> ·
  <a href="https://docs.cube.dev">Cube docs</a>
</p>

**The Cube semantic layer as one small Rust binary, with no Node.js inside.**

`blockmill/cube` is the Docker image of
[github.com/TomDatalab/cube](https://github.com/TomDatalab/cube), a community fork of
[Cube](https://github.com/cube-js/cube) whose whole backend was rewritten in Rust.

Cube's REST, GraphQL, WebSocket and SQL (Postgres wire) APIs, the query
planner, the cache and queue, the database drivers and the Playground UI all
ship in a single `cube-server` process.

- **Small and fast to start.** About 70 MB to download, multi-arch
  (`linux/amd64`, `linux/arm64`), and ready to answer a few milliseconds after
  the container starts.
- **Light on memory.** Around 55 MB resident while serving queries on a small
  model, measured with `docker stats`.
- **Drop-in configuration.** The same `CUBEJS_*` environment variables and
  YAML data models as upstream Cube.
- **27 databases, all native.** Postgres, Snowflake, BigQuery, Databricks,
  ClickHouse, Trino, DuckDB, Oracle and more, without a JVM, Instant Client or
  ODBC driver.
- **Locked down by default.** A distroless base with no shell and no package
  manager, running as a non-root user.

> **Community build.** This project is not affiliated with or endorsed by
> Cube Dev, Inc. It is based on the Apache-2.0 licensed
> [Cube](https://github.com/cube-js/cube) source and is distributed under the
> same license. Report problems with this image here, not upstream.

---

## Quick start

Put your YAML data model in `./model`, then run:

```bash
docker run -p 4000:4000 -p 15432:15432 \
  -e CUBEJS_DB_TYPE=postgres \
  -e CUBEJS_DB_HOST=db.example.com \
  -e CUBEJS_DB_NAME=analytics \
  -e CUBEJS_DB_USER=cube \
  -e CUBEJS_DB_PASS=secret \
  -e CUBEJS_API_SECRET=change-me \
  -e CUBEJS_DEV_MODE=true \
  -e CUBEJS_PG_SQL_PORT=15432 \
  -e CUBEJS_SQL_USER=cube -e CUBEJS_SQL_PASSWORD=cube \
  -v "$PWD":/cube/conf \
  blockmill/cube
```

Open **http://localhost:4000** and start querying in the Playground.

| Endpoint | Address |
|---|---|
| Playground | `http://localhost:4000/` |
| REST API | `http://localhost:4000/cube/v1/load` |
| GraphQL | `http://localhost:4000/cube/graphql` |
| WebSocket | `ws://localhost:4000/cube/ws` |
| SQL API | `psql -h localhost -p 15432 -U cube` |
| Health | `/readyz`, `/livez` |
| Available drivers | `http://localhost:4000/cube/v1/connectors` |

### Docker Compose

```yaml
services:
  cube:
    image: blockmill/cube:latest
    ports: ["4000:4000", "15432:15432"]
    environment:
      CUBEJS_DB_TYPE: postgres
      CUBEJS_DB_HOST: postgres
      CUBEJS_DB_NAME: analytics
      CUBEJS_DB_USER: cube
      CUBEJS_DB_PASS: secret
      CUBEJS_API_SECRET: change-me
      CUBEJS_DEV_MODE: "true"
      CUBEJS_PG_SQL_PORT: "15432"
      CUBEJS_SQL_USER: cube
      CUBEJS_SQL_PASSWORD: cube
    volumes:
      - ./:/cube/conf
    depends_on: [postgres]

  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: analytics
      POSTGRES_USER: cube
      POSTGRES_PASSWORD: secret
```

For pre-aggregations, add a `cubejs/cubestore` service and point
`CUBEJS_CUBESTORE_HOST` at it.

---

## Connectors

**27 native connectors, no JVM, ODBC or Oracle Instant Client.** Pick one with
`CUBEJS_DB_TYPE`. Most connectors also use the common variables
`CUBEJS_DB_HOST`, `CUBEJS_DB_PORT`, `CUBEJS_DB_NAME`, `CUBEJS_DB_USER`,
`CUBEJS_DB_PASS` and `CUBEJS_DB_SSL`. The table lists the settings specific to each
one. `GET /cube/v1/connectors` on a running server lists the connectors in the
build and the data sources using them.

### Cloud data warehouses

| Database | `CUBEJS_DB_TYPE` | Connects via | Specific settings |
|---|---|---|---|
| Snowflake | `snowflake` | SQL REST API | `CUBEJS_DB_SNOWFLAKE_ACCOUNT`, `_AUTHENTICATOR`, `_PRIVATE_KEY`, `_OAUTH_TOKEN` |
| Google BigQuery | `bigquery` | REST API | `CUBEJS_DB_BQ_PROJECT_ID`, `CUBEJS_DB_BQ_KEY_FILE` or `_CREDENTIALS`, `_LOCATION` |
| Amazon Redshift | `redshift` | Postgres wire | common variables |
| Databricks | `databricks-jdbc` | SQL Statement Execution API (no JVM) | `CUBEJS_DB_DATABRICKS_URL`, `_TOKEN` or `_OAUTH_CLIENT_ID`/`_SECRET`, `_CATALOG` |
| Amazon Athena | `athena` | AWS SDK | `CUBEJS_AWS_KEY`, `_SECRET`, `_REGION`, `_S3_OUTPUT_LOCATION`, `CUBEJS_AWS_ATHENA_WORKGROUP` |
| Firebolt | `firebolt` | HTTP API | `CUBEJS_FIREBOLT_ACCOUNT`, `_ENGINE_NAME`, `_API_ENDPOINT` |

### Query engines and lakehouses

| Database | `CUBEJS_DB_TYPE` | Connects via | Specific settings |
|---|---|---|---|
| Trino | `trino` | HTTP client protocol | `CUBEJS_DB_PRESTO_CATALOG`, `CUBEJS_DB_PRESTO_AUTH_TOKEN` |
| Presto | `prestodb` | HTTP client protocol | `CUBEJS_DB_PRESTO_CATALOG`, `CUBEJS_DB_PRESTO_AUTH_TOKEN` |
| Dremio (OSS and Cloud) | `dremio` | REST API | `CUBEJS_DB_URL` + `CUBEJS_DB_DREMIO_AUTH_TOKEN` for Cloud |
| Apache Hive / Spark Thrift | `hive` | Thrift HiveServer2 | `CUBEJS_DB_HIVE_AUTH` (`PLAIN` or `NOSASL`), `CUBEJS_DB_HIVE_VER` |

### Real-time and OLAP

| Database | `CUBEJS_DB_TYPE` | Connects via | Specific settings |
|---|---|---|---|
| ClickHouse | `clickhouse` | HTTP interface | `CUBEJS_DB_CLICKHOUSE_READONLY`, `_COMPRESSION` |
| Apache Druid | `druid` | SQL over HTTP | `CUBEJS_DB_URL` |
| Apache Pinot | `pinot` | HTTP broker | `CUBEJS_DB_PINOT_AUTH_TOKEN`, `_NULL_HANDLING` |
| ksqlDB | `ksql` | REST + Kafka | `CUBEJS_DB_URL`, `CUBEJS_DB_KAFKA_HOST`, `_USER`, `_PASS`, `_USE_SSL` |
| Materialize | `materialize` | Postgres wire | `CUBEJS_DB_MATERIALIZE_CLUSTER` |
| QuestDB | `questdb` | Postgres wire | common variables |
| CrateDB | `crate` | Postgres wire | common variables |

### Relational databases

| Database | `CUBEJS_DB_TYPE` | Connects via | Specific settings |
|---|---|---|---|
| PostgreSQL | `postgres` | Postgres wire | common variables |
| MySQL | `mysql` | MySQL protocol | common variables |
| Aurora Serverless MySQL | `mysqlauroraserverless` | RDS Data API | `CUBEJS_DATABASE_SECRET_ARN`, `CUBEJS_DATABASE_CLUSTER_ARN` |
| Microsoft SQL Server / Azure SQL | `mssql` | TDS (rustls) | `CUBEJS_DB_SSL_CA`, `CUBEJS_DB_DOMAIN` |
| Oracle | `oracle` | thin protocol, pure Rust | common variables; no Instant Client needed |
| Vertica | `vertica` | Vertica protocol | common variables |
| MongoDB (BI Connector) | `mongobi` | MySQL protocol | common variables |

### Embedded

| Database | `CUBEJS_DB_TYPE` | Connects via | Specific settings |
|---|---|---|---|
| DuckDB / MotherDuck | `duckdb` | embedded engine | `CUBEJS_DB_DUCKDB_DATABASE_PATH`, `_MOTHERDUCK_TOKEN`, `_EXTENSIONS`, `_S3_*` |
| SQLite | `sqlite` | embedded engine | `CUBEJS_DB_NAME` (file path) |

### Pre-aggregation storage

| Database | `CUBEJS_DB_TYPE` | Connects via | Specific settings |
|---|---|---|---|
| Cube Store | `cubestore` | WebSocket | `CUBEJS_CUBESTORE_HOST`, `_PORT` |

**Several data sources.** List their names in `CUBEJS_DATASOURCES` and prefix
each source's variables with `CUBEJS_DS_<NAME>_`, for example
`CUBEJS_DS_WAREHOUSE_DB_TYPE=snowflake`, as upstream. A `cube.yml` can declare
them too.

If a connector does not support a feature yet, such as export-bucket unload
on some warehouses, you get an explicit error instead of a silently different
result. The full status per connector is in
[MIGRATION.md](https://github.com/TomDatalab/cube/blob/main/rust/cube/MIGRATION.md#drivers).

---

## Coming from the official image

Most projects run unchanged. The differences:

- **Data models must be YAML.** Jinja templating works. JavaScript and Python
  models are rejected with an error that names the file.
- **Configuration comes from environment variables plus an optional
  `cube.yml`,** instead of `cube.js` / `cube.py`. `cube.yml` covers data
  sources, multi-tenancy, API options and scheduled refresh.
- **The API is served under `/cube`,** not `/cubejs-api`. Set
  `CUBEJS_BASE_PATH=/cubejs-api` to keep the old paths.
- **`CUBEJS_DB_TYPE=jdbc` is not available,** because it needs a JVM. Use
  the native `mysql`, `athena`, `hive` or `databricks-jdbc` driver instead.
- **The Playground is disabled when `NODE_ENV=production`,** because its
  helper routes are unauthenticated.

---

## Tags

| Tag | |
|---|---|
| `latest` | the most recent release |
| `0.1.0` | the first public release |

Both tags are multi-arch (`linux/amd64`, `linux/arm64`).

`docker run blockmill/cube --version` prints the server version.

---

## About this image

| | |
|---|---|
| Source code | [github.com/TomDatalab/cube](https://github.com/TomDatalab/cube), a fork of [cube-js/cube](https://github.com/cube-js/cube) |
| Docker Hub | [hub.docker.com/r/blockmill/cube](https://hub.docker.com/r/blockmill/cube) |
| Dockerfile | [`rust/cube/docker/Dockerfile`](https://github.com/TomDatalab/cube/blob/main/rust/cube/docker/Dockerfile) |
| How it was built | [Architecture and migration write-up](https://github.com/TomDatalab/cube#readme) |
| Base | `gcr.io/distroless/cc-debian12:nonroot`: no shell, no package manager, non-root user |
| Contents | `/usr/local/bin/cube-server` with all 27 drivers, and the Playground and Vizard at `/cube/playground` |
| Size | ~70 MB compressed per platform |
| Issues | [github.com/TomDatalab/cube/issues](https://github.com/TomDatalab/cube/issues) |

The image contains no Node.js. The Rust server is compiled for each platform, with
arm64 cross-compiled rather than emulated, and copied onto the distroless base. The
Playground is a static build served by the same process.

## Building

The build context is the root of the [repository](https://github.com/TomDatalab/cube).
The Playground is copied pre-built, so build it once first. This needs Node.js on
the build machine only:

```bash
(cd packages/cubejs-playground && yarn build:playground)
(cd packages/cubejs-playground/vizard && yarn && yarn build)

docker buildx build -f rust/cube/docker/Dockerfile \
  --platform linux/amd64,linux/arm64 -t my/cube:dev .
```

The bundled DuckDB engine accounts for most of the build time.

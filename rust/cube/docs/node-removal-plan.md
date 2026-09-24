# Removing the Node.js backend

The last step of the migration (`../MIGRATION.md`, workstream 8). It happens
only after the Rust binary serves every surface the Node server does, so this
document is a checklist, not a instruction to run today.

## What is removed

These packages implement the backend that the Rust binary replaces. Deleting
them is what makes the constraint real: with them gone, no Node.js process can
be started for the backend.

| Package | Replaced by |
|---|---|
| `cubejs-server` | `rust/cube/cubeserver` (binary `cube-server`) |
| `cubejs-server-core` | `cubeserver` + `cubeconfig` + `cubeorch` |
| `cubejs-api-gateway` | `cubeserver` (REST, WebSocket) + `cubeauth` + `cubequery` + `cubegraphql` |
| `cubejs-query-orchestrator` | `cubeorch` + `cubecache` + `cubequeue` |
| `cubejs-schema-compiler` | `cubemodel` + `cubeplanner` (the planner was already Rust) |
| `cubejs-backend-native` | nothing: the Neon bridge exists only to let Node call Rust |
| `cubejs-backend-shared` | `cubeconfig` (env handling) and each crate's own config |
| `cubejs-base-driver` and the 32 `*-driver` packages | `cubedriver` |
| `cubejs-cli` | a Rust CLI, or `cube-server` flags |

## What stays

- **Client libraries** (`cubejs-client-core`, `-react`, `-vue3`, `-ngx`,
  `-ws-transport`, `-dx`): they run in the browser and talk to the API over
  HTTP. They are not the backend and are unaffected.
- **`cubejs-playground`**: a React app. It needs a host; either the Rust
  binary serves its built assets or the feature is dropped (decided in
  `config-migration.md`).
- **`cubejs-docker`**, `cubejs-testing`, `cubejs-testing-drivers`: rebuilt
  around the Rust binary rather than deleted, so the integration suites keep
  running against the new server.
- **`cubejs-linter`, `cubejs-templates`, `cubejs-dbt-schema-extension`,
  `cubejs-backend-maven`, `cubejs-backend-cloud`**: decide case by case; none
  of them is on the request path.

## Order

1. Every REST, WebSocket, GraphQL and SQL API surface passes its contract
   tests against the Rust binary.
2. The driver coverage matches what deployments actually use; a `CUBEJS_DB_TYPE`
   with no Rust driver must fail at start-up with a clear message, never
   silently.
3. The integration suites in `cubejs-testing` and `cubejs-testing-drivers` run
   against `cube-server`.
4. The Docker image builds the Rust binary and stops installing Node.
5. Delete the packages in the table above, drop them from the Yarn workspace
   and from Lerna, and remove the Rust↔Node bridge crates
   (`packages/cubejs-backend-native`, and `nativebridge` if nothing else uses
   it).
6. The transitional Express → native gateway proxy
   (`CUBEJS_NATIVE_API_GATEWAY_REST_ROUTES`, `ApiGateway.NATIVE_V1_ROUTES`)
   disappears with `cubejs-api-gateway`.

## Deliberate losses

Recorded in `config-migration.md`: JavaScript and Python data models,
`cube.js`/`cube.py` configuration and every hook that was a JavaScript
function. Each one fails with an explicit error naming the file or option
rather than behaving differently.

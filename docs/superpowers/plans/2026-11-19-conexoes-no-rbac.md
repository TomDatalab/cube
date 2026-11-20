# Conexões no RBAC — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With RBAC on, every data source's settings (secrets encrypted) live in `cube_app.data_sources` instead of `cube.yml`/`.env`; each has a UUID; a platform admin can take a connection out of its organization; the Data Source page shows the id, fits without horizontal scroll, and lets connections go back to Platform.

**Architecture:** A new table `data_sources` holds the settings as the `CUBEJS_DB_*` variable map the Data Source handlers already use, with secret variables sealed by AES-256-GCM under `CUBE_RBAC_SECRET_KEY`. At start-up, before `declare_data_sources`, the server imports any `cube.yml` data source missing from the table (then strips it from the files, with backups), and rebuilds `cube_config.data_sources` from the table. The Data Source handlers read/write through a small backend switch (files without RBAC, table with RBAC) and keep the existing restart-by-exec. Ownership stays in `connections`; unassign deletes its row and SpiceDB relationships.

**Tech Stack:** Rust (axum, tokio-postgres, deadpool, `aes-gcm` 0.11.1 and `base64` 0.22 already in `Cargo.lock`), React + antd (Playground), vitest, bash.

**Spec:** `docs/superpowers/specs/2026-11-19-conexoes-no-rbac-design.md`

## Global Constraints

- Repo: `/home/antonio-rodrigues/Documents/GitHub/cube`, branch `feat/datasource-owner-organization`. Do **not** build or edit `/root/cube` (another branch with uncommitted work).
- Build the server with `cargo build --release -p cubeserver --bin cube-server --target-dir /root/cube/rust/cube/target` from `rust/cube` (that path is what `start-all.sh` runs). Use `--offline` if the network is unavailable.
- Key: `CUBE_RBAC_SECRET_KEY`, base64 of exactly 32 bytes. Required when `CUBE_RBAC=spicedb`; missing/invalid → server refuses to start with a message naming the variable.
- Sealed format: 12-byte random nonce ‖ AES-256-GCM ciphertext of the JSON object `{VAR: value}`.
- Secret variable = `data_sources::is_secret(var)` (existing rule: contains PASS, PWD, SECRET, TOKEN, CREDENTIAL, PRIVATE_KEY, or ends with KEY) or a `*_URL` carrying a password.
- Without RBAC nothing changes (files, as today).
- `default` stays Platform forever: never assignable, never unassignable (400).
- No route switches to ids; ids are only returned and shown.
- Bootstrap variables stay in `.env`: `CUBE_APP_DB_URL`, `SPICEDB_*`, `CUBE_RBAC*`, `CUBEJS_API_SECRET`, ports, and the infra variables `start-all.sh`/`sync-sofiadb-model.py` use (`CUBEJS_DB_*` of the container, `SOFIADB_*`). Only `CUBEJS_DS_<NAME>_*` lines are removed from `.env` on import.
- UI copy in English, like the rest of the Playground.

## Review Focus

- A server restart after import must not re-import or duplicate: import is skipped for names already in `data_sources` (idempotent).
- `cube.yml` left with an empty `data_sources:` must still load (the default comes from the table, not from `CUBEJS_DB_*` fallback).
- Editing with the password field empty keeps the stored secret (sealed blob merged, not replaced).
- A cleared field must not survive the restart through a stale `CUBEJS_DS_<NAME>_*` in the process environment (restart removes the source's own keys).
- Unassigning a connection released to workspaces removes those releases; a concurrent move → 409, nothing written to SpiceDB.

---

### Task 1: Secret sealing

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/secrets.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (add `pub mod secrets;`), `rust/cube/cubeserver/Cargo.toml` (add `aes-gcm = "0.11"`)

**Interfaces:**
- Produces: `pub struct SecretKey` (no `Debug` of the bytes); `SecretKey::from_base64(s: &str) -> Result<SecretKey, String>`; `SecretKey::seal(&self, plain: &[u8]) -> Vec<u8>`; `SecretKey::open(&self, sealed: &[u8]) -> Result<Vec<u8>, String>`; `SecretKey::from_env() -> Result<SecretKey, String>` reading `CUBE_RBAC_SECRET_KEY`.

- [ ] **Step 1: Unit tests in `secrets.rs`:** `seal_then_open_roundtrips` (b"{\"CUBEJS_DB_PASS\":\"x\"}"), `sealing_twice_differs` (nonce random), `wrong_key_fails` (open with another key → Err), `tampered_fails` (flip last byte → Err), `short_input_fails` (`open(&[0;5])` → Err), `key_must_be_32_bytes` (base64 of 16 bytes → Err mentioning `CUBE_RBAC_SECRET_KEY`).
- [ ] **Step 2:** `cargo test -p cubeserver --lib rbac::secrets` → FAIL (module missing).
- [ ] **Step 3:** Implement with `aes_gcm::{Aes256Gcm, KeyInit, aead::Aead}`, nonce from `aes_gcm::aead::OsRng` (or `rand`), `base64::engine::general_purpose::STANDARD`.
- [ ] **Step 4:** Tests PASS.
- [ ] **Step 5:** Commit `feat(cubeserver): seal RBAC secrets with AES-256-GCM`.

### Task 2: `data_sources` table and store

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/migrations.sql` (append), `rust/cube/cubeserver/src/rbac/store.rs`
- Test: `rust/cube/cubeserver/tests/rbac_store.rs`

**Interfaces:**
- Produces in `store.rs`:
  - `pub struct DataSourceRow { pub id: Uuid, pub name: String, pub db_type: String, pub settings: BTreeMap<String, String>, pub secrets: Option<Vec<u8>> }`
  - `Store::data_sources(&self) -> Result<Vec<DataSourceRow>, StoreError>` (ordered by name)
  - `Store::data_source(&self, name: &str) -> Result<Option<DataSourceRow>, StoreError>`
  - `Store::put_data_source(&self, name: &str, db_type: &str, settings: &BTreeMap<String,String>, secrets: Option<&[u8]>) -> Result<Uuid, StoreError>` — insert or update by name (keeps `id`, bumps `updated_at`), returns id
  - `Store::delete_data_source(&self, name: &str) -> Result<bool, StoreError>`
  - Transaction `delete_connection_of(&self, name: &str, org: Uuid) -> Result<bool, StoreError>` (`DELETE ... WHERE name=$1 AND organization_id=$2`), added to `shared_creates!` like `move_connection`.

- [ ] **Step 1: Migration:** `CREATE TABLE IF NOT EXISTS data_sources (id uuid UNIQUE NOT NULL DEFAULT gen_random_uuid(), name text PRIMARY KEY, db_type text NOT NULL, settings jsonb NOT NULL DEFAULT '{}', secrets bytea, created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now());`
- [ ] **Step 2: Tests (`db_test!`):** `data_sources_put_get_list_delete` (put → id; get returns same settings/secrets; second put with other settings keeps id and replaces settings; list ordered; delete → true then false); `delete_connection_of_requires_the_organization` (row in org A: `delete_connection_of(name, B)` → false, row still there; with A → true).
- [ ] **Step 3:** Run `CUBE_TEST_PG_URL=postgres://postgres:$CUBEJS_DB_PASS@127.0.0.1:55432/postgres cargo test -p cubeserver --test rbac_store data_sources delete_connection_of` → FAIL.
- [ ] **Step 4:** Implement.
- [ ] **Step 5:** Tests PASS; commit `feat(cubeserver): data_sources table in the RBAC store`.

### Task 3: Stored data sources (seal/split/load)

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/data_sources.rs` (`pub mod data_sources;` in `rbac/mod.rs`)
- Modify: `rust/cube/cubeserver/src/handlers/data_sources.rs` (make `is_secret`, `kept_if_empty`, `current_variables`, `own_setting`, `remove_from_config`, `env_keys`, `update_env`, `header`, `write_atomically` `pub(crate)`)
- Modify: `rust/cube/cubeconfig/src/data_source.rs` + `lib.rs` export: `pub fn data_source_from_variables(name: &str, variables: &BTreeMap<String,String>) -> Result<DataSource>` — builds an `Env` whose keys are `key_by_data_source(var, name)` and resolves `DataSourceSpec::default()` with it (file path `Path::new("rbac")` for error origins).

**Interfaces:**
- Produces in `rbac/data_sources.rs`:
  - `pub struct StoredDataSource { pub id: Uuid, pub name: String, pub variables: BTreeMap<String,String> }` (variables include `CUBEJS_DB_TYPE` and decrypted secrets)
  - `pub fn split(variables: &BTreeMap<String,String>) -> (BTreeMap<String,String>, BTreeMap<String,String>)` → (plain without `CUBEJS_DB_TYPE`, secret) by `kept_if_empty`
  - `pub async fn load_all(store: &Store, key: &SecretKey) -> Result<Vec<StoredDataSource>, String>`
  - `pub async fn load(store: &Store, key: &SecretKey, name: &str) -> Result<Option<StoredDataSource>, String>`
  - `pub async fn save(store: &Store, key: &SecretKey, name: &str, variables: &BTreeMap<String,String>) -> Result<Uuid, String>` (requires `CUBEJS_DB_TYPE`)

- [ ] **Step 1: Tests:** cubeconfig unit `data_source_from_variables_resolves_fields` (`{CUBEJS_DB_TYPE: clickhouse, CUBEJS_DB_HOST: h, CUBEJS_DB_PORT: 8201, CUBEJS_DB_USER: u, CUBEJS_DB_PASS: p}` for `sofiadb` → host h, port 8201, user u, password p) and `..._unknown_type_errs`; cubeserver unit `split_puts_passwords_and_urls_with_passwords_aside` (`CUBEJS_DB_PASS` and `CUBEJS_DB_URL=postgres://u:p@h/db` secret; `CUBEJS_DB_HOST` plain; `CUBEJS_DB_TYPE` in neither); store test `save_then_load_decrypts` (secrets column is not plaintext: does not contain `"p"`'s JSON).
- [ ] **Step 2:** Run → FAIL. **Step 3:** Implement. **Step 4:** PASS.
- [ ] **Step 5:** Commit `feat(cubeserver): data source settings stored sealed in RBAC`.

### Task 4: Start-up — key, import, load

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/config.rs` (`RbacConfig.secret_key: SecretKey`, read in `from_env`, error if missing/invalid; keep it out of `Debug`)
- Create: `rust/cube/cubeserver/src/rbac/bootstrap.rs`
- Modify: `rust/cube/cubeserver/src/main.rs:63-80` (call bootstrap before `declare_data_sources`; `cube_config` becomes `mut`)
- Modify: `rust/cube/cubeserver/tests/rbac_api.rs::rbac_config` (add a fixed test key)

**Interfaces:**
- Produces: `pub async fn data_sources_from_rbac(config: &RbacConfig, cube_config: &mut CubeConfig, config_file: Option<&Path>) -> Result<ImportReport, String>`; `pub struct ImportReport { pub imported: Vec<String>, pub loaded: usize }`.
- Behavior: connect `Store::connect(&config.db_url)`; for each `cube_config.data_sources` name absent from the table → `current_variables` (env = process env merged with `.env`) → `save`. If anything was imported and `config_file` is set: copy `cube.yml` and `.env` to `<file>.bak-<YYYYMMDD-HHMMSS>`, remove each imported block with `remove_from_config`, remove its own `CUBEJS_DS_<NAME>_*` keys with `update_env`. Then `cube_config.data_sources` = `load_all` mapped through `data_source_from_variables`, and set each variable in the process env as `key_by_data_source(var, name)` (driver-specific ones included). Print `🔑 N data source(s) from RBAC (imported: a, b)`.
- `main.rs`: if `RbacConfig::from_env()?` is `Some`, run it; any `Err` → `eprintln!("RBAC error: {e}")`, exit 1.

- [ ] **Step 1: Tests** (`tests/rbac_store.rs`, `db_test!`, temp dir with `cube.yml` declaring `default` (postgres, literal host) and `extra` (sqlite) and `.env` with `CUBEJS_DS_EXTRA_DB_NAME=/tmp/x.db`):
  - `import_moves_file_data_sources_into_rbac`: report.imported == [default, extra]; table has both; `cube.yml` no longer contains `extra:`; `.env` no longer contains `CUBEJS_DS_EXTRA_`; a `.bak-` copy of each exists; `cube_config.data_sources["extra"].database == Some("/tmp/x.db")`.
  - `import_is_idempotent`: second run → imported empty, loaded 2, files unchanged.
  - `a_config_with_no_data_sources_loads_them_from_rbac`: after import, `CubeConfig::load_file_with_env` of the stripped `cube.yml` succeeds.
  - config unit: `rbac_requires_a_secret_key` (env without `CUBE_RBAC_SECRET_KEY` → Err naming it).
- [ ] **Step 2:** Run → FAIL. **Step 3:** Implement. If `cube.yml` with an empty `data_sources:` map fails to load, make `CubeConfig` accept it (fix in cubeconfig with its own unit test). **Step 4:** PASS (and full `cargo test -p cubeserver` still green).
- [ ] **Step 5:** Commit `feat(cubeserver): load data sources from RBAC at start-up, importing cube.yml once`.

### Task 5: Data Source handlers write to RBAC

**Files:**
- Modify: `rust/cube/cubeserver/src/handlers/data_sources.rs` (`list`, `get`, `test`, `create`, `update`, `delete`)
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Consumes: Task 3 (`load`, `load_all`, `save`), `Store::delete_data_source`, `rbac.config.secret_key`.
- With `state.rbac` `Some`: read current variables from `load(...)` instead of files (404 when absent); `create` refuses an existing name in the table; `create`/`update` call `save` after `test_variables` passes (update merges: empty secret keeps stored one — reuse `split_changes`/`edited`); `delete` keeps the cubes-in-use check, then `delete_data_source`; none of them touch `cube.yml`/`.env`; `editable` is `true`. Restart via `restart_if_enabled(&state, vec![], own_keys)` where `own_keys` = every process-env key that is the source's own (`own_setting`, or `CUBEJS_DB_*` for `default`), so cleared values do not survive. `list` returns `id` for each row (also with RBAC on: rows come from `load_all`, `active` = present in `state.config.data_sources`).
- `GET` never returns secret values (unchanged contract: `secrets`, `hasPassword`).

- [ ] **Step 1: Tests** (rbac_api, real store via `CUBE_TEST_PG_URL`, `restart_on_config_change = false`, a sqlite data source so `test_variables` passes): `rbac_create_stores_in_rbac_not_files` (POST with `organizationId` → table row exists, `cube.yml` unchanged), `rbac_update_keeps_password_when_empty`, `rbac_delete_removes_row`, `rbac_list_returns_ids`, `rbac_get_hides_secrets`.
- [ ] **Step 2:** Run → FAIL. **Step 3:** Implement (one `enum Backend<'a> { Files(Paths), Rbac(&'a Rbac) }` with `variables(name)`, `names()`, `write(name, vars)`, `remove(name)` keeps the handlers' flow single). **Step 4:** PASS, existing file-mode tests (`tests/playground.rs`) still PASS.
- [ ] **Step 5:** Commit `feat(cubeserver): with RBAC the Data Source page writes to the RBAC store`.

### Task 6: Unassign a connection

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/handlers/connections.rs` (new `unassign_organization`, module doc: drop "never back to none"), `rust/cube/cubeserver/src/rbac/handlers/mod.rs:103` (`.delete(connections::unassign_organization)` on the same route)
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- `DELETE /v1/connections/:name/organization` → 204. Platform admin only (403). `default` → 400 "The default data source always belongs to the platform". No `connections` row → 404. Reads releases (`releases_of`), opens a tx, `delete_connection_of(name, row.organization_id)` false → 409 "The connection changed concurrently; try again" (nothing written to SpiceDB); then one `authz.write` deleting every release + `connection#organization`; failed write → tx dropped (rollback); failed commit → write them back (same pattern as `move_connection`).

- [ ] **Step 1: Tests:** `unassign_returns_a_connection_to_platform` (assign to org, release to a workspace, DELETE → 204; `/v1/connections` lacks it; FakeAuthz has no relationship for it; a member of the workspace now gets 403 on a query naming that cube), `unassign_is_for_platform_admins` (org owner → 403), `unassign_default_is_400`, `unassign_unknown_is_404`, `assign_after_unassign_works`.
- [ ] **Step 2:** Run → FAIL. **Step 3:** Implement. **Step 4:** PASS.
- [ ] **Step 5:** Commit `feat(cubeserver): a platform admin can take a connection out of its organization`.

### Task 7: Playground — id, no scroll, back to Platform

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Admin/api.ts` (`unassignConnection(name) → DELETE /v1/connections/{name}/organization`)
- Modify: `packages/cubejs-playground/src/pages/DataSources/organizationDrop.ts` (`dropAction(ds, target: string | null)`: target `null` = Platform zone → `'unassign'` if it has an organization, `'none'` if already Platform, `'forbidden'` for default; add `export function connectionLabel(ds: {host?, port?, database?, user?}): string` → `user@host:port/database`, parts omitted when missing, `'—'` when all missing)
- Modify: `packages/cubejs-playground/src/pages/DataSources/OrganizationDropPanel.tsx` (a "Platform" drop zone on top listing Platform data sources; tags in organizations get `closable` → `Popconfirm`/`Modal.confirm` "Remove {name} from {org}? Its releases to N workspace(s) are withdrawn and it goes back to Platform." → `api.unassignConnection`; tags draggable with the same `DATA_SOURCE_DRAG_TYPE`)
- Modify: `packages/cubejs-playground/src/pages/DataSources/DataSourcesPage.tsx` (type `id?: string`; Name column = bold name + `Text code copyable` small id when present; replace Host/Database/User with one **Connection** column using `connectionLabel`, `ellipsis: true`, title = full text; drop `scroll` and `fixed`; `tableLayout="fixed"` with widths: drag 32, Name 200, Type 140, Owner 110, Status 110, actions 112, Connection takes the rest)
- Test: `packages/cubejs-playground/src/__tests__/organization-drop.test.ts`

- [ ] **Step 1: Tests:** `dropAction` to Platform: owned → `'unassign'`, Platform → `'none'`, default → `'forbidden'`; `connectionLabel`: full (`default@127.0.0.1:8201/cluster_test`), no user (`127.0.0.1:8201/cluster_test`), only database (`/tmp/cube-example.db`), nothing (`—`).
- [ ] **Step 2:** `node_modules/.bin/vitest run src/__tests__/organization-drop.test.ts` → FAIL. **Step 3:** Implement. **Step 4:** PASS; `node_modules/.bin/tsc --noEmit -p .` clean; full vitest green.
- [ ] **Step 5:** Commit `feat(playground): connection ids, no side scroll, back to Platform`.

### Task 8: Scripts, README, deploy and end-to-end check

**Files:**
- Modify: `/root/cube-rust/start-all.sh` (before starting the server: if `CUBE_RBAC=spicedb` and `.env` lacks `CUBE_RBAC_SECRET_KEY`, back up `.env` and append `CUBE_RBAC_SECRET_KEY=$(openssl rand -base64 32)`, printing "generated CUBE_RBAC_SECRET_KEY in .env — keep it: without it the stored passwords cannot be read")
- Modify: `/root/cube-rust/README.md` (RBAC variables table: `CUBE_RBAC_SECRET_KEY`; "Assigned vs created connections": unassign route; new section "Where connections are stored"; routes table row for `DELETE /connections/{name}/organization`)
- Modify: `rust/cube/cubeserver` docs comments touched above.

- [ ] **Step 1:** Back up `/root/cube-rust/cube.yml`, `.env`, and `pg_dump` of `cube_app` to `/root/cube-rust/backup-20261119/`.
- [ ] **Step 2:** Build the server (Global Constraints command), `vite build` the Playground, `./install-playground.sh <build>`.
- [ ] **Step 3:** `./start-all.sh restart`. Expected: key generated once; log `🔑 3 data source(s) from RBAC (imported: default, example, sofiadb)`; `/readyz` 200.
- [ ] **Step 4:** Verify: `cube.yml` has no data source blocks; `.env` has no `CUBEJS_DS_EXAMPLE_*`; `psql cube_app -c "select id,name,db_type,settings,secrets is not null from data_sources"` shows 3 rows and no plaintext password; `/cube/v1/load` on a `default` cube and a `dfos_*` (sofiadb) cube answers 200; the ownership of `sofiadb`/`example` (Digital) is unchanged.
- [ ] **Step 5:** Restart again → log says `imported:` nothing; files unchanged.
- [ ] **Step 6:** Browser (playwright-cli, admin login): Data Source page has no horizontal scrollbar at 1280 px (`document.querySelector('.ant-table-content').scrollWidth <= clientWidth`), ids visible; drag `example` to Platform → confirm → it shows Platform; drag it back to Digital → owned again (end state = as before).
- [ ] **Step 7:** Commit repo changes `docs: connections stored in RBAC`; report the backups' location.

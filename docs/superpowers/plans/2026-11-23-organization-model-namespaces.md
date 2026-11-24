# Organization Model Namespaces Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With RBAC on, two organizations may each have a cube (or view) with the same name: each organization's model is compiled and served on its own.

**Architecture:** The RBAC model stays one set of rows/one `live` folder, but it is compiled per *scope*: an organization's scope is `_platform/` + `<slug>/`; the Platform scope is `_platform/` + files with no owner. Every request resolves a scope (from its workspace, or for a platform admin from `X-Cube-Organization`), and `TenantRegistry` keeps one runtime per scope. Validation, the model index, Generate and the Playground all work per scope. Without RBAC nothing changes.

**Tech Stack:** Rust (axum, tokio, `cubemodel`, `cubeserver`), TypeScript/React Playground (vitest).

**Spec:** this document, section *Design* (agreed in conversation on 2026-11-23: option A, "namespace real por organização").

## Design

1. **Scopes.** `ModelScope::Organization(slug)` compiles `_platform/**` + `<slug>/**`. `ModelScope::Platform` compiles `_platform/**` + every file outside any organization's folder (so the admin still sees their rejections). Without RBAC there is one scope, `ModelScope::All`, which is today's behaviour.
2. **Names.** A name must be unique *within a scope*. Two organizations may share a name. A Platform cube's name may not be reused by any organization (it is in every organization's scope), and vice versa: the write that introduces the clash is refused, as today.
3. **Which scope a request gets.**
   - With `X-Cube-Workspace`: the workspace's organization.
   - Platform admin without a workspace: `X-Cube-Organization: <slug>` if sent (`_platform` or an organization's slug; 404 for an unknown one), otherwise Platform.
   - Anyone else without a workspace: the organization of their workspaces if they are all in one; otherwise 400 `X-Cube-Workspace is required`.
4. **Runtimes.** One `TenantRuntime` per scope: `app_id` `<tenant app_id>@<slug>`, its own orchestrator id (query cache never shared), and pre-aggregation schema `<schema>_<slug>` (two organizations' `orders` pre-aggregations never write the same table). Each runtime reads its own materialized folder `<cache>/scopes/<slug>/v<version>/`.
5. **Writes.** A change is checked in every scope it touches (its organization's; every scope when it touches `_platform/` or an unowned file). Only problems new in some scope are refused, as today.
6. **Model index (Data Model tree, `/v1/meta/groups`, role restrictions, Generate).** Built per scope and merged. A cube is identified by `(owner, name)`, never by name alone.
7. **Playground.** For platform admins the header's scope selector changes: "Todos (admin)" becomes "Platform (admin)", and each organization appears as "<Org> (admin)", which sends `X-Cube-Organization`. Members keep picking a workspace.
8. **SQL API (port 15432).** It is outside RBAC and compiles one model at start-up. It serves **one scope**, named by `CUBE_SQL_API_SCOPE` (an organization slug or `_platform`, default `_platform`). Serving every organization on that port is out of scope here (see *Not in this plan*).

## Global Constraints

- Without RBAC (`state.rbac == None`) every route answers exactly as before: one scope, `ModelScope::All`, `state.config.schema_path`.
- Error text to owners must not reveal another organization's files or cube names (keep `model_redact` semantics).
- `X-Cube-Organization` is accepted only from platform admins; from anyone else it is ignored if it matches the workspace's organization, and 403 otherwise.
- CORS must allow the `x-cube-organization` header wherever it allows `x-cube-workspace`.
- Existing tests in `cubeserver/tests/{org_tree,model_api,rbac_api}.rs` and the playground's vitest suite keep passing.
- Integration tests need `CUBE_TEST_PG_URL` (`CUBE_APP_DB_URL` with the database set to `postgres`).

## Review Focus

- A Platform cube renamed to a name an organization already uses: refused, the problem names the Platform file only to admins.
- An admin with an `X-Cube-Organization` of an organization created after start-up: compiles on first use, no restart.
- A connection moved between organizations: its old scope's runtime drops its cubes, the new scope's gains them, without restart (the existing reload path must reach every scope runtime).
- A model row deleted while a scope runtime is compiling: the stale scope folder is pruned like `live` versions are.
- Two organizations with the same cube name and the same pre-aggregation: distinct tables (`<schema>_<slug>`).

---

### Task 1: `ModelScope` and scoped sources

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/scope.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (add `pub mod scope;`)
- Test: unit tests in `scope.rs`

**Interfaces:**
- Produces:
  - `pub enum ModelScope { All, Platform, Organization(String) }` (`Clone, Debug, PartialEq, Eq, Hash`), `impl ModelScope { pub fn key(&self) -> &str }` (`"_all"`, `"_platform"`, the slug).
  - `pub fn scope_sources(sources: &[(String, String)], scope: &ModelScope, org_slugs: &BTreeSet<String>) -> Vec<(String, String)>`: the subset of `(path, content)` the scope compiles (rule 1).
  - `pub fn scopes_touched(paths: &[&str], org_slugs: &BTreeSet<String>) -> BTreeSet<ModelScope>`: rule 5 (a `_platform/` or unowned path ⇒ `Platform` and every organization).

- [ ] **Step 1: failing tests** `scope_sources_keeps_platform_and_own_folder` (`digital/a.yml`, `on/a.yml`, `_platform/u.yml`, `stray/x.yml`, `root.yml` → Organization("digital") gets `_platform/u.yml`, `digital/a.yml`; Platform gets `_platform/u.yml`, `stray/x.yml`, `root.yml`; All gets all five), `scopes_touched_platform_reaches_every_org`, `scopes_touched_org_file_only_its_org`.
- [ ] **Step 2:** `cargo test -p cubeserver --lib rbac::scope` → FAIL (module missing).
- [ ] **Step 3:** implement; a path's owner folder is its first segment.
- [ ] **Step 4:** same command → PASS.
- [ ] **Step 5:** commit `feat(rbac): model scopes, one per organization plus Platform`.

### Task 2: Validate writes per scope

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/model_validate.rs`, `rust/cube/cubeserver/src/rbac/model_writer.rs:150-185`
- Test: `rust/cube/cubeserver/tests/model_api.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces: `pub fn check_scoped(sources: &[(String, String)], scopes: &BTreeSet<ModelScope>, org_slugs: &BTreeSet<String>, ctx: &TemplateContext, policy: Option<&OwnershipPolicy>) -> Check`: the union of `check(scope_sources(..))` over `scopes`. `ModelWriter::write` uses it for `before` and `after` with `scopes_touched` of the changed paths (old and new path of a move). `definitions` becomes per scope: `pub fn definitions_in(sources, scope, org_slugs) -> BTreeMap<String, String>` for the redaction of the first failing scope.

- [ ] **Step 1: failing tests** in `model_api.rs`:
  - `same_cube_name_in_two_organizations_is_allowed`: `digital/a.yml` and `on/a.yml` both define cube `a` (each on its own organization's connection) → both PUTs 200.
  - `platform_name_clash_with_an_organization_is_refused`: `digital/a.yml` defines `a`; PUT `_platform/a.yml` defining `a` → 400 `A mudança não compila`, problems mention `a`.
  - `duplicate_inside_one_organization_is_still_refused`: two files in `digital/` defining `a` → 400 with `duplicate_name_message("a")` for the owner.
- [ ] **Step 2:** `cargo test -p cubeserver --test model_api same_cube_name` → FAIL (400 today).
- [ ] **Step 3:** implement as in Interfaces.
- [ ] **Step 4:** the three tests and the whole `model_api` binary → PASS.
- [ ] **Step 5:** commit `feat(rbac): a cube name is unique per organization, not per platform`.

### Task 3: Materialize one folder per scope and serve one runtime per scope

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/model_runtime.rs`, `rust/cube/cubeserver/src/rbac/model_snapshot.rs`, `rust/cube/cubeserver/src/tenants.rs`, `rust/cube/cubeserver/src/app.rs:50-100`, `rust/cube/cubeserver/src/main.rs:384-400`
- Test: `rust/cube/cubeserver/tests/org_tree.rs` (uses `env_live`)

**Interfaces:**
- Consumes: Task 1.
- Produces:
  - `ModelRuntime::scope_path(&self, scope: &ModelScope) -> Result<PathBuf, StoreError>`: `<cache>/scopes/<key>/v<version>/`, materialized on first use for the current version, pruned like `live` versions on `refresh`.
  - `TenantRegistry::resolve_scoped(&self, security_context: &Value, scope: &ModelScope, path: PathBuf) -> Result<Arc<TenantRuntime>, ApiError>`: keys runtimes by `<app_id>@<key>`, sets `model_path = path`, `orchestrator_id = <id>@<key>`, `pre_aggregations_schema = <schema>_<key>` (not for `All`). A runtime whose `model_path` differs from `path` (a newer version) is recompiled.
  - `AppState::scope_for(&self, ctx: &RequestContext) -> Result<ModelScope, ApiError>`: Design rule 3. `RequestContext` gains `pub organization: Option<String>` read from header `x-cube-organization` (constant `ORGANIZATION_HEADER` in `handlers/auth.rs`, added to CORS in `app.rs`).
  - `AppState::runtime_for` uses `scope_for` + `resolve_scoped` when RBAC is on; unchanged otherwise.

- [ ] **Step 1: failing test** `two_organizations_serve_the_same_cube_name` in `org_tree.rs` (`env_live`): orgs Digital and ON, each a connection and a workspace with member `ann`/`bia`; write `digital/a.yml` (title "A Digital") and `on/a.yml` (title "A ON"). `GET /cube/v1/meta` in Digital's workspace → one cube `a` titled "A Digital"; in ON's → "A ON". Admin with `X-Cube-Organization: on` → "A ON"; admin with no header → no cube `a`.
- [ ] **Step 2: failing test** `member_with_workspaces_in_two_orgs_needs_a_workspace`: `ann` in both → `GET /cube/v1/meta` without the header → 400 `X-Cube-Workspace is required`; `X-Cube-Organization` from `ann` naming another org → 403.
- [ ] **Step 3:** run `cargo test -p cubeserver --test org_tree two_organizations` → FAIL.
- [ ] **Step 4:** implement the Interfaces; `ModelWriter::write` refreshes and then calls `tenants.reload_all()` as today, which must now reach every scope runtime.
- [ ] **Step 5: failing test** `moving_a_connection_moves_its_cubes_between_scopes` (move the connection Digital→ON, then move the file; ON's meta shows it, Digital's does not, no restart) → make it pass.
- [ ] **Step 6:** `cargo test -p cubeserver --test org_tree --test model_api --test rbac_api` → PASS.
- [ ] **Step 7:** commit `feat(rbac): each organization's model is compiled and served on its own`.

### Task 4: Model index, tree, `/v1/meta/groups`, roles and Generate per scope

**Files:**
- Modify: `rust/cube/cubeserver/src/model_index.rs`, `rust/cube/cubeserver/src/handlers/tree.rs`, `rust/cube/cubeserver/src/handlers/meta_groups.rs`, `rust/cube/cubeserver/src/rbac/handlers/roles.rs:420-450`, `rust/cube/cubeserver/src/handlers/scaffold.rs:95-190`, `rust/cube/cubeserver/src/rbac/ownership.rs:80-100`
- Test: `rust/cube/cubeserver/tests/org_tree.rs`, `rust/cube/cubeserver/tests/model_index.rs`

**Interfaces:**
- Consumes: Tasks 1, 3.
- Produces: `ModelIndex::load_scoped(model_path: &Path, policy: Option<&OwnershipPolicy>, org_slugs: &BTreeSet<String>) -> ModelIndex`: loads each scope's sources and merges, keeping each `IndexedCube` once per `(owner, name)` (Platform cubes once). Compile errors are kept per scope with the scope's key. `ModelIndexCache::get` uses it when a policy is installed.
  - `meta_groups` looks cubes up by `(owner of the request's scope, name)`.
  - `roles.rs` `model-members` uses `state.meta_for` with the organization's scope instead of `state.meta`.
  - Generate's "existing names" are those of the target scope only; the message for a clash stays `duplicate_name_message`.

- [ ] **Step 1: failing tests**: `tree_lists_same_name_in_two_orgs` (admin `/playground/tree/digital` and `/playground/tree/on` each list cube `a`, no errors); `generate_reuses_a_name_taken_in_another_org` (Generate a table into ON making `dfos_schema_meta` while `digital/` has one → 200, file `on/dfos_schema_meta.yml`); `meta_groups_in_org_scope` (the admin's groups with `X-Cube-Organization: on` list ON's `a` only); `model_index` unit test `load_scoped_keeps_both_owners`.
- [ ] **Step 2:** run them → FAIL.
- [ ] **Step 3:** implement.
- [ ] **Step 4:** `cargo test -p cubeserver` (all binaries) → PASS.
- [ ] **Step 5:** commit `feat(rbac): the Data Model tree, Generate and groups work per organization`.

### Task 5: SQL API serves one scope

**Files:**
- Modify: `rust/cube/cubeserver/src/main.rs:340-372`, `rust/cube/cubeserver/src/sql_api.rs:282-315`
- Test: unit test in `main.rs` or `sql_api.rs` for the scope choice

**Interfaces:**
- Consumes: Tasks 1, 3 (`ModelRuntime::scope_path`).
- Produces: `fn sql_api_scope(env: &cubeconfig::Env) -> ModelScope` from `CUBE_SQL_API_SCOPE` (`_platform` default; a slug ⇒ `Organization`). With RBAC the SQL API starts on `runtime.scope_path(&scope)` instead of `schema_path`; it logs `🐘 SQL API serves <scope>`.

- [ ] **Step 1: failing test** `sql_api_scope_defaults_to_platform` / `sql_api_scope_reads_a_slug`.
- [ ] **Step 2–4:** implement, run `cargo test -p cubeserver --bin cube-server sql_api_scope` → PASS.
- [ ] **Step 5:** commit `feat(cubeserver): the SQL API serves one organization's model`.

### Task 6: Playground: admins choose an organization

**Files:**
- Modify: `packages/cubejs-playground/src/shared/auth.ts`, `packages/cubejs-playground/src/components/Header/WorkspaceSelector.tsx`, `packages/cubejs-playground/src/components/Header/workspaceChoice.ts`
- Test: `packages/cubejs-playground/src/__tests__/workspace-choice.test.ts`, `packages/cubejs-playground/src/__tests__/auth.test.ts`

**Interfaces:**
- Consumes: Task 3's `x-cube-organization` header.
- Produces: `auth.ts`: `ORGANIZATION_HEADER = 'X-Cube-Organization'`, `getOrganizationScope(): string | null`, `chooseOrganizationScope(slug: string | null)`; `workspaceHeaders()` returns `X-Cube-Organization` when an organization scope is set and no workspace is. Choosing a workspace clears the organization scope and vice versa.
  - Selector for admins: `Platform (admin)` (value `_platform`) and `<Org name> (admin)` per organization (from `GET /cube/v1/organizations`), then the workspaces as today.

- [ ] **Step 1: failing tests**: `workspaceHeaders sends the organization scope without a workspace`; `choosing a workspace clears the organization scope`; `admin options list Platform and each organization`.
- [ ] **Step 2:** `npx vitest run src/__tests__/workspace-choice.test.ts src/__tests__/auth.test.ts` → FAIL.
- [ ] **Step 3:** implement.
- [ ] **Step 4:** whole vitest suite and `npx tsc --noEmit -p .` → PASS.
- [ ] **Step 5:** commit `feat(playground): platform admins pick an organization's model`.

### Task 7: Deploy and document

**Files:**
- Modify: `/root/cube-rust/README.md` (sections *The data model* and *SQL API*: replace "Cube names stay unique across the whole platform" with Design rules 2, 3, 7, 8), `/root/cube-rust/.env` only if `CUBE_SQL_API_SCOPE` must be set.

- [ ] **Step 1:** back up binary, `playground/`, `README.md` (`*.bak-<date>-namespaces`); `pg_dump` `cube_app`.
- [ ] **Step 2:** release build, `vite build`, `install-playground.sh`, `./start-all.sh restart`.
- [ ] **Step 3:** verify live: bossa's Generate of the table that made `dfos_schema_meta` now succeeds; `/cube/v1/meta` with `X-Cube-Organization: bossa` lists it; Digital's still has its own.
- [ ] **Step 4:** commit the README change is outside the repo; commit nothing else.

## Not in this plan

- The SQL API serving every organization at once (e.g. one Postgres database name per organization). Needs `cubesqlbridge` to hold several models per process.
- Renaming or migrating existing cubes.
- Per-organization pre-aggregation schemas (Design rule 4): two organizations defining the same pre-aggregation on connections to the same database would share its table. Needs the planned pre-aggregation descriptions to carry the scope's schema, quoted (slugs contain `-`) and ≤63 bytes.
- The `sofiadb` connection now owned by ON with Digital's 24 cubes rejected: a data decision, separate from this change.

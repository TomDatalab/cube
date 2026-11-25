# Cube Models: plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- Each organization gets a "Models" group in the Data Model tree.
- A model is a canvas: tables dragged in, columns linked by relationships.
- It is saved in Postgres and built into one YAML cube with a SQL JOIN.

**Architecture:**
- A pure Rust module validates a model definition and generates the cube YAML.
- RBAC handlers store models in `cube_models` and build them through the `ModelWriter`.
- The Playground adds the tree group and a React Flow editor. Its logic lives in a pure TS module.

**Tech Stack:**
- Server: Rust (axum, tokio-postgres, serde_yaml, sha2).
- Playground: React 18, TypeScript, `@xyflow/react` 12, Tailwind v4 (`tw:` prefix) and shadcn components, vitest.

**Spec:** `docs/superpowers/specs/2026-11-25-cube-models-design.md`

## Global Constraints

**Paths, scope and permissions**
- Repo: `/home/antonio-rodrigues/Documents/GitHub/cube`, branch `feat/cube-member-editing`. Do not push. Commit per task.
- Organizations only, with no Platform models.
- Permission is the same as the org model-file routes: `visible_organization(rbac, id, "admin", user)`.
- A model of another organization answers 404.

**Generated cube**
- One cube per model (option B): `sql` with JOINs, one dimension per column named `snake("<alias>_<column>")`, and a `count` measure.
- `count` becomes `count_distinct` on the primary key when any join, oriented from the principal, is `one_to_many`.
- Identifiers are quoted with scaffold `quote(db_type, ..)`.
- Column types come from `cube_type`. Columns with `None` are left out, and refused as primary key.
- File header line: `# Built from model <name> (<id>): Build cube rewrites this file`.
- The cube name is fixed at first build (`cube_name`). A later rename of the model changes neither the cube nor the file.
- Rebuild compares the current file content's sha256 with `cube_built_hash`. On a mismatch it answers 409 `"O cubo foi editado desde o último build"` unless `force: true`.
- If the file is gone, the build recreates it at the same path. Deleting a model leaves the cube file alone.

**Definition and joins**
- Definition JSON keys:
  - `dataSource`, `primary`, `primaryKey`;
  - `tables[{alias, schema, table, x, y}]`;
  - `joins[{from:{alias,column}, to:{alias,column}, relationship, join}]`.
- `relationship` ∈ `one_to_one|one_to_many|many_to_one`; `join` ∈ `left|inner`.
- The joins must form a tree rooted at `primary`. All aliases are connected, with no second link between the same two aliases and no cycle.
- Server error messages are in Portuguese. UI text is in English, like the rest of the Playground.

**Server tests**
- Command, from `rust/cube`:
  `set -a; . /root/cube-rust/.env; set +a; URL="${CUBE_APP_DB_URL%/cube_app*}/postgres"; env -i PATH="$PATH" HOME="$HOME" CUBE_TEST_PG_URL="$URL" cargo test -p cubeserver <filter>`
- Never print `.env` values. Do not run `cargo fmt` on the crate, because the repo is not rustfmt-clean. Format only your new files with `rustfmt <file>`.

**Playground tests**
- Commands, from `packages/cubejs-playground`:
  - `node_modules/.bin/vitest run <files>`
  - `node_modules/.bin/tsc --noEmit -p .`
  - `../../node_modules/.bin/oxlint <files>`

**Tailwind and shadcn**
- Use the `tw:` prefix everywhere. There is no preflight.
- Hand-made buttons get `type="button"` and `data-slot`.
- No hardcoded colors: use tokens, or the palette with `dark:` variants.

## Review Focus

1. **A join drawn "backwards"** (the `to` is already in the tree). The generator must flip it and mirror the relationship. Covered by a `cube_model` test.
2. **Column names that collide after `snake`** (`Order ID` and `order_id`) must get the `_2` suffix, not a duplicate dimension. Covered by a `cube_model` test.
3. **The same table dragged twice** must get the alias `<table>_2`, and both copies must appear in the SQL with their own alias. Covered by `modelGraph` and `cube_model` tests.
4. **A rebuild after a hand edit** must answer 409, and with `force` it overwrites. Covered by an integration test.
5. **A model of org A read through org B's owner** must give 404. Covered by an integration test.

---

### Task 1: Pure generator `rbac/cube_model.rs`

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/cube_model.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (add `pub mod cube_model;`)
- Modify: `rust/cube/cubeserver/src/handlers/scaffold.rs`: make `cube_type`, `quote` and `snake` `pub(crate)`.

**Interfaces:**
- Produces:
  - `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] #[serde(rename_all="camelCase", deny_unknown_fields)] pub struct Definition { pub data_source: String, pub tables: Vec<ModelTable>, pub primary: Option<String>, pub primary_key: Option<String>, pub joins: Vec<ModelJoin> }`
  - `ModelTable { alias, schema, table: String, x: f64, y: f64 }`
  - `ModelJoin { from: End, to: End, relationship: String, join: String }`
  - `End { alias, column: String }`
  - `pub fn validate(def: &Definition) -> Result<(), String>`: structure only (aliases unique and non-empty, `primary` among them if set, joins reference existing aliases, valid enum values, tree connectivity). A model with no tables is valid; it is a draft.
  - `pub fn build(name: &str, model_id: Uuid, cube_name: &str, def: &Definition, columns: &cubedriver::DatabaseStructure, db_type: &str) -> Result<String, String>`: runs `validate`, then requires `primary` and `primary_key`, checks every table and join column exists in `columns[schema][table]`, and returns YAML text starting with the header line.
- Build-time errors (Portuguese):
  - `"Escolha a tabela principal"`
  - `"Escolha a chave primária"`
  - `"A tabela <schema>.<table> não existe na conexão <ds>"`
  - `"A coluna <alias>.<column> não existe"`
  - `"A tabela <alias> não está ligada à principal"`
  - `"<alias1> e <alias2> já estão ligadas"`
  - `"A chave primária <col> não tem tipo no Cube"`

**Steps:**

- [ ] **Step 1: Write the failing tests** in the module's `#[cfg(test)]`. Use a `columns` fixture `public.orders[id int, customer_id int, Order ID text, order_id int]`, `public.customers[id int, region text]` and db_type `postgres`.
  - `builds_a_left_join_cube`: orders (primary, pk `id`) many_to_one customers, left. The YAML contains:
    - `FROM "public"."orders" AS "orders"`;
    - `LEFT JOIN "public"."customers" AS "customers" ON "orders"."customer_id" = "customers"."id"`;
    - a dimension `customers_region` of type `string`;
    - `orders_id` with `primary_key: true` and type `number`;
    - a measure `count` of `type: count`.
  - `a_one_to_many_join_counts_distinct`: customers primary (pk `id`), one_to_many to orders. The measure is `type: count_distinct` with `sql` referencing `customers_id`.
  - `a_backwards_join_is_flipped`: orders primary, with the join drawn `from customers.id to orders.customer_id`, relationship `one_to_many`. The SQL still joins customers onto orders, and `count` stays plain `count` (mirrored to many_to_one).
  - `colliding_names_get_a_suffix`: the orders columns `Order ID` and `order_id` produce `orders_order_id` and `orders_order_id_2`.
  - `the_same_table_twice_uses_both_aliases`: aliases `customers` and `customers_2`, both joined from orders. The SQL has `AS "customers_2"`.
  - `validate_refuses_bad_shapes`, one assertion per case:
    - an unlinked table gives `"A tabela x não está ligada à principal"`;
    - two links between the same pair are refused;
    - a cycle is refused;
    - an unknown relationship value is refused;
    - a duplicate alias is refused.
  - `build_requires_primary_and_key_and_existing_columns`: missing `primary`, missing `primaryKey`, a missing column and a missing table each give their message.
  - `the_data_source_line`: it is omitted when `dataSource` is `default`, and is `data_source: pg` otherwise.
- [ ] **Step 2: Run them and see them fail.** Command: `cargo test -p cubeserver --lib cube_model`. Expected: compile errors or FAIL.
- [ ] **Step 3: Implement.**
  - Walk BFS from `primary`. Flip a join whose `from` is not yet visited by swapping its ends and mirroring `one_to_many`↔`many_to_one`.
  - SELECT lists every typed column as `"alias"."col" AS "<dim>"`.
  - A dimension's `sql` is `{CUBE}.<quote(dim)>`.
  - Emit YAML with serde_yaml (like `scaffold::cube_yaml`) and prepend the header.
- [ ] **Step 4: Run the tests.** Expected: PASS.
- [ ] **Step 5: Commit:** `feat(rbac): generate one cube from a model of joined tables`.

### Task 2: Storage `cube_models`

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/migrations.sql`:
  - add `CREATE TABLE IF NOT EXISTS cube_models (id uuid PRIMARY KEY, organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE, name text NOT NULL, definition jsonb NOT NULL, cube_name text, cube_file_id uuid, cube_built_hash text, version int NOT NULL DEFAULT 1, created_at timestamptz NOT NULL DEFAULT now(), created_by text NOT NULL, updated_at timestamptz NOT NULL DEFAULT now(), updated_by text NOT NULL, UNIQUE (organization_id, name))`;
  - follow the `groups` block's style.
- Modify: `rust/cube/cubeserver/src/rbac/store.rs`.
- Test: `rust/cube/cubeserver/tests/rbac_store.rs`.

**Interfaces:**
- Produces:
  - `pub struct CubeModel { id, organization_id: Uuid, name: String, definition: serde_json::Value, cube_name: Option<String>, cube_file_id: Option<Uuid>, cube_built_hash: Option<String>, version: i32, created_at, updated_at: DateTime<Utc>, created_by, updated_by: String }`.
- Store methods:
  - `cube_models_of(org) -> Vec<CubeModel>` (by name);
  - `cube_model(id) -> Option<CubeModel>`;
  - `create_cube_model(org, name, &Value, actor) -> CubeModel`. A duplicate name gives `StoreError::Conflict`, the same way `create_group` maps unique violations.
  - `update_cube_model(id, name: Option<&str>, definition: Option<&Value>, expected_version, actor) -> CubeModel`. A stale version gives `Conflict`, a missing row gives `NotFound`, and the version is incremented.
  - `set_cube_model_build(id, cube_name, file_id, hash) -> ()`, which does not bump the version;
  - `delete_cube_model(id, expected_version) -> ()`.

**Steps:**

- [ ] **Step 1: Failing test.** `cube_models_crud` creates an org and a model, then checks:
  - a duplicate name gives Conflict;
  - an update with the right version gives version 2;
  - a stale version gives Conflict;
  - `set_cube_model_build` keeps the version and stores the fields;
  - delete, then `cube_model` gives `None`;
  - deleting the org cascades.
- [ ] **Step 2: Run it.** `cargo test -p cubeserver --test rbac_store cube_models`. Expected: FAIL.
- [ ] **Step 3: Implement** the migration and the store methods (sql via `deadpool`'s client, like groups).
- [ ] **Step 4: Run it.** Expected: PASS.
- [ ] **Step 5: Commit:** `feat(rbac): store cube models`.

### Task 3: Routes, build, tree detail and API catalog

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/handlers/cube_models.rs`
- Modify: `rbac/handlers/mod.rs` (routes), `rbac/handlers/model.rs`:
  - `detail_of` adds `models: [{id, name, cubeName}]` when `scope.organization` is set;
  - expose what the build needs: `pub(crate) fn writer`, a `pub(crate)` scope helper or an equivalent.
- Modify: `packages/cubejs-playground/src/pages/Api/catalog.ts`. Add the 6 routes to the "Data model files" section (or its equivalent), with who: `organization owner`.
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`.

**Interfaces:**
- Routes:
  - `GET|POST /v1/organizations/:id/cube-models`;
  - `GET|PUT|DELETE /v1/cube-models/:id`;
  - `POST /v1/cube-models/:id/build`.
- JSON view of a model: `{id, organizationId, name, definition, cubeName, version, updatedAt, updatedBy}`. The list returns `{models: [view]}`.
- Request bodies:
  - POST `{name, definition}` → 201. `validate` runs, and a failure gives 400 with its message.
  - PUT `{name?, definition?, version}` → view. A stale version gives 409 `"O model mudou desde que você abriu; abra de novo"`.
  - DELETE `?version=` → 204.
  - `POST build {force?: bool}` → `{fileName, content, cubeName, detail}`, where `detail` is the new org detail.
- Build steps:
  1. Check the model's `dataSource` is a connection of the org, else 400 `"A conexão <ds> não é desta organização"`.
  2. Load its columns via `crate::handlers::playground::data_source_schema(ds)`, holding the `handlers::data_sources::WRITE` lock like `generate_route`.
  3. Get `db_type` from `state.config.data_sources`.
  4. Set `cube_name = model.cube_name.unwrap_or(snake(name))`.
  5. Call `cube_model::build`, then write through `writer.write(actor, "build model", ..)`:
     - with `cube_file_id` and the row present, and its sha256 == `cube_built_hash` or `force`, write `Put{id: Some(row.id), path: row.path, expected_version: Some(row.version), ..}`;
     - with a present row and a mismatching hash without `force`, answer 409 with the spec message;
     - with no row, write `Put{id: None, organization_id: Some(org), path: "<cube_name>.yml", ..}`.
  6. Get the written file's id by re-reading the files of the org with that path after the write, if `write` does not return it.
  7. Call `set_cube_model_build`.

**Steps:**

- [ ] **Step 1: Failing integration tests.** Write a helper `model_def(ds)` using the scenario's configured test data source and real tables. Look at how existing tests use `generate` with a real connection (e.g. `grep -n "model/generate" tests/rbac_api.rs`) and reuse that connection and table. Tests:
  - `cube_models_crud_and_scope`:
    - create, list, get, rename (version 2), stale PUT (409), validate failure (400);
    - another org's owner gets 404 on GET, PUT, DELETE and build;
    - a non-owner gets 404 or 403 as the model-file routes do;
    - delete gives 204.
  - `building_a_model_writes_one_cube`:
    - build gives 200 with `fileName` ending `<cube>.yml`;
    - the org detail lists the cube and `models[0].cubeName`;
    - renaming the model and rebuilding keeps the same `fileName`.
  - `rebuilding_after_a_hand_edit_needs_force`:
    - build, then PUT the file with edited content, then build gives 409;
    - build with `{force:true}` gives 200, and the content starts with `# Built from model`.
  - `deleting_the_model_keeps_the_cube`.
- [ ] **Step 2: Run them.** `cargo test -p cubeserver --test rbac_api cube_model`. Expected: FAIL.
- [ ] **Step 3: Implement** the handlers and routes, and the `models` in the detail.
- [ ] **Step 4: Run them and the catalog test.** Run the rbac_api filter above, then `node_modules/.bin/vitest run src/__tests__/api-catalog.test.ts`. Expected: PASS.
- [ ] **Step 5: Commit:** `feat(rbac): cube model routes and Build cube`.

### Task 4: Playground API client and tree group

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/tree.ts`:
  - `OrgDetail.models?: {id: string; name: string; cubeName?: string | null}[]`;
  - add `'models' | 'model'` to `ParsedKey['kind']` and `KINDS`;
  - `parseKey('org:<slug>:models')` gives `{kind:'models', slug}`;
  - `detailNodes` adds the group `org:<slug>:models` titled `Models (n)` right after Cubes, with leaves `model:<slug>:<id>` titled with the name;
  - `iconKind` maps `models` and `model` to a new `'model'` icon kind.
- Modify: `pages/Schema/api.ts`. Add to `modelApi`: `models(org)`, `model(id)`, `createModel(org, name, definition)`, `saveModel(id, {name?, definition?, version})`, `deleteModel(id, version)`, `buildModel(id, force?)`. These are RBAC-only and use `call`.
- Modify: wherever `iconKind` is rendered (grep `iconKind(` in SchemaPage) to show a lucide `Workflow` icon for `'model'`.
- Test: the existing tree tests (grep `detailNodes` in `src/__tests__`) plus new cases.

**Steps:**

- [ ] **Step 1: Failing tests:**
  - `detailNodes` with `models: [{id:'m1', name:'Sales'}]` gives the second group with key `org:acme:models`, title `Models (1)` and child `model:acme:m1`;
  - without `models` it gives `Models (0)`;
  - `parseKey('model:acme:m1')` gives `{kind:'model', slug:'acme', rest:'m1'}`;
  - `parseKey('org:acme:models')` gives the kind `models`.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them, plus tsc and oxlint on the touched files.** Expected: PASS and clean.
- [ ] **Step 5: Commit:** `feat(playground): Models group in the data model tree`.

### Task 5: Model logic module `modelGraph.ts`

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/modelGraph.ts`
- Test: `packages/cubejs-playground/src/__tests__/model-graph.test.ts`

**Interfaces:**
- Produces the TS types mirroring `Definition` (camelCase): `ModelDefinition`, `ModelTable`, `ModelJoin`, `Relationship`, `JoinKind`.
- Functions:
  - `emptyDefinition(): ModelDefinition`, with `dataSource: ''` and the rest empty.
  - `addTable(def, ds, schema, table, x, y): {def; error?: string}`:
    - the first table sets `dataSource` and `primary`;
    - another ds gives the error `Tables of one connection only: this model reads <ds>`;
    - the alias is `table`, then `table_2`, `table_3`, and so on.
  - `removeTable(def, alias)`: drops its joins. If it was primary, the next table becomes primary (or none). If none is left, `dataSource` resets to `''`.
  - `canConnect(def, from: End, to: End): string | null`. It refuses, with a message:
    - the same alias;
    - an existing link between the pair;
    - a link that would close a cycle (union-find over the aliases).
  - `connect(def, from, to, relationship, join)`, `updateJoin(def, index, patch)` and `removeJoin(def, index)`.
  - `joinLabel(j)`: `N:1 · LEFT` and similar. Relationships map `one_to_one`→`1:1`, `one_to_many`→`1:N`, `many_to_one`→`N:1`.
  - `unlinked(def): string[]`: aliases not reachable from primary, for a warning before Build.

**Steps:**

- [ ] **Step 1: Failing tests,** one `it` per function, with the exact messages and aliases above, including the cycle case (a–b, b–c, then c–a refused).
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement** as pure functions that return new objects.
- [ ] **Step 4: Run them, plus oxlint.** Expected: PASS.
- [ ] **Step 5: Commit:** `feat(playground): model graph logic`.

### Task 6: `ModelEditor` with React Flow, wired into the Schema page

**Files:**
- Modify: `packages/cubejs-playground/package.json`. Add `"@xyflow/react": "12.3.6"`, installed with the repo's package manager (check for `yarn.lock` at the repo root, use the same tool, and do not upgrade others).
- Create: `pages/Schema/models/ModelEditor.tsx`, `pages/Schema/models/TableNode.tsx`, `pages/Schema/models/JoinDialog.tsx`.
- Modify: `pages/Schema/SchemaPage.tsx`:
  - `selected.kind === 'model'` renders `<ModelEditor>`;
  - `selected.kind === 'models'` shows the list and a "New model" button, which opens `NameDialog`, creates the model and selects it;
  - after a build, reload the org detail and show a toast with an "Open cube" action that selects `cube:<slug>:<cubeName>`.
- Import `@xyflow/react/dist/style.css` once in ModelEditor.
- Test: `src/__tests__/model-editor.test.ts` (jsdom; mock `@xyflow/react` to render nodes and edges as plain lists, if it does not render in jsdom).

**Interfaces:**
- Consumes the Task 4 api methods, the Task 5 functions, and `OrgDetail.tables` (`tables[ds][schema][table] = [{name,type}]`).
- `ModelEditor` props: `{ org: OrgNode; modelId: string; tables: OrgDetail['tables']; onBuilt(cubeName: string): void }`.

**Behavior:**
- Left panel: search, then the tree ds › schema › table. Each table is `draggable`, and its `dataTransfer` carries `application/x-cube-table` with JSON `{ds, schema, table}`. Tables of another ds than `def.dataSource` (when set) are disabled with a title tooltip.
- Canvas:
  - `onDrop` converts the coordinates with `screenToFlowPosition` and calls `addTable`;
  - `TableNode` shows the alias, a "principal" badge, and per column the name, type and a source and target handle whose id is the column name;
  - `onConnect` runs `canConnect` (a toast on error), else opens `JoinDialog`;
  - an edge click opens `JoinDialog` with Delete;
  - node drags update x and y.
- `JoinDialog` offers relationship radios (1:1, 1:N, N:1) and join radios (LEFT, INNER), with the hint "INNER keeps only rows with a match on both sides".
- Toolbar:
  - the name input;
  - a connection label;
  - a principal select (aliases);
  - a primary key select (the principal's columns);
  - Save (`saveModel` with the version; on 409 a toast "Changed elsewhere: reopen");
  - Build cube.
- Build cube:
  - warns when `unlinked` is not empty or the model is unsaved, and saves first;
  - calls `buildModel`;
  - a 409 with the edited message opens a confirm "The cube was edited by hand since the last build. Overwrite?", which then retries with `force`;
  - on success calls `onBuilt`.
- A model whose tables are not in `tables` still opens, with an Alert listing the missing ones.

**Steps:**

- [ ] **Step 1: Failing tests,** with mocked `modelApi`:
  - rendering with a saved definition shows the table aliases and the edge label `N:1 · LEFT`;
  - clicking Build calls `buildModel` and then `onBuilt('sales')`;
  - a 409 then confirm calls `buildModel(id, true)`;
  - tables of another connection are disabled in the panel.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Install the dependency and implement.**
- [ ] **Step 4: Run vitest (whole suite), tsc and oxlint on the touched files.** Expected: PASS.
- [ ] **Step 5: Commit:** `feat(playground): drag tables into a model and build its cube`.

### Task 7: Deploy and check in the browser (controller)

- [ ] Run the full `cargo test -p cubeserver` and the full vitest.
- [ ] Back up `/root/cube/rust/cube/target/release/cube-server` and `/root/cube-rust/playground` with the suffix `bak-20261125-models`.
- [ ] Build the release server and copy it over (`.new`, then `mv`).
- [ ] Run `vite build` and `/root/cube-rust/install-playground.sh <build>`.
- [ ] Run `./start-all.sh restart` and check that `/readyz` answers 200.
- [ ] With playwright-cli as e2e_owner, open the org, then Models, then New model:
  - drag 2 tables and link them;
  - Save, then Build cube;
  - the cube appears in Cubes.
- [ ] Record the outcome. Do not commit without the user asking.

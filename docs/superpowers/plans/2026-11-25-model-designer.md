# Model designer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Models editor into a modeling tool: designed (draft)
tables, colors, a toolbar, better table nodes, and creating the draft tables
in the model's database.

**Architecture:** Draft tables and colors are optional fields of the model's
JSON definition. The server validates them, refuses to build while drafts
remain, and generates and runs the `CREATE TABLE` statements on a new
model route, after the same check `/v1/execute` makes. The Playground adds
pure graph helpers, a toolbar, a table dialog and a richer node.

**Tech Stack:** Rust (axum, serde, sqlx store), cubedriver; React 18,
@xyflow/react 12, Tailwind v4 `tw:` + shadcn, vitest.

**Spec:** `docs/superpowers/specs/2026-11-25-model-designer-design.md`

## Global Constraints

- Identifiers (draft table, schema, column): `^[A-Za-z_][A-Za-z0-9_]*$`, at most 63 chars.
- Portable types, exactly: `text, integer, bigint, numeric, boolean, date, timestamp`.
- Postgres: `text, integer, bigint, numeric(18,4), boolean, date, timestamp`; MySQL: `text, int, bigint, decimal(18,4), boolean, date, datetime`.
- Other db types on create: 400 `Criar tabelas não é suportado para <type>`.
- Palette names, exactly: `slate, blue, green, amber, red, violet, pink, teal`.
- Build with drafts: 400 `A tabela <schema>.<table> ainda não existe no banco: use Criar no banco`.
- Existing table on create: 409 `A tabela <schema>.<table> já existe no banco`.
- Create permission: `visible_model` (org admin) AND `crate::rbac::enforce::authorize_execute(&state, &ctx, Some(ds))`.
- Server messages in Portuguese; Playground UI text in English (as the rest of the editor).
- Old definitions (no `color`, no `draft`) load unchanged; omitted fields are not serialized (`skip_serializing_if = "Option::is_none"`).
- Never run `cargo fmt` on the crate (it is not rustfmt-clean). Tailwind: `tw:` prefix, no hardcoded colors in components (tokens in `src/shadcn.css`), hand-made buttons need `type="button"`.
- Rust tests: `set -a; . /root/cube-rust/.env; set +a; URL="${CUBE_APP_DB_URL%/cube_app*}/postgres"; env -i PATH="$PATH" HOME="$HOME" CUBE_TEST_PG_URL="$URL" cargo test -p cubeserver <filter>` from `rust/cube`. Never print `.env` values.
- Playground (from `packages/cubejs-playground`): `node_modules/.bin/vitest run`, `node_modules/.bin/tsc --noEmit -p .`, `../../node_modules/.bin/oxlint <files>`.

## Review Focus

- A draft named like an existing table of the connection: create must refuse with 409 and run nothing.
- A model saved before this change (no `color`/`draft`) must still load, save and build.
- A draft column name that is an SQL keyword (`order`, `select`): the DDL quotes it and it works.
- Create partly failing (2nd statement errors): the 1st table loses `draft`, the model is saved, the response names the error.
- Removing a draft table or changing the primary away from it leaves no dangling primaryKey or join (existing `removeTable` rules apply to drafts too).

---

### Task 1: Definition fields, validation, DDL generation (server, pure)

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/cube_model.rs`
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `ModelTable { ..., #[serde(default, skip_serializing_if = "Option::is_none")] color: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] draft: Option<DraftTable> }`
  - `pub struct DraftTable { pub columns: Vec<DraftColumn> }` (camelCase, deny_unknown_fields)
  - `pub struct DraftColumn { pub name: String, #[serde(rename = "type")] pub kind: String, #[serde(default)] pub nullable: bool, #[serde(default)] pub primary_key: bool }`
  - `pub const COLORS: [&str; 8]`, `pub const TYPES: [&str; 7]`
  - `pub fn create_statements(def: &Definition, db_type: &str) -> Result<Vec<(String /*schema.table*/, String /*sql*/)>, String>` — one per draft table, in `def.tables` order.

- [ ] **Step 1: Write failing tests** (names and assertions):
  - `old_definitions_still_parse`: JSON without color/draft parses; re-serialized has no `color`/`draft` keys.
  - `validate_checks_drafts`: a draft with a bad name `1x` → Err containing `inválido`; unknown type `uuid` → Err; no columns → Err; duplicate column → Err; two drafts with the same schema.table → Err; color `orange` → Err; a valid draft with color `blue` → Ok.
  - `build_refuses_drafts`: a model whose second table is a draft → `Err("A tabela public.items ainda não existe no banco: use Criar no banco")`.
  - `postgres_create_statement`: draft `public.items` columns `id integer pk not null`, `order text nullable`, `price numeric` → exactly `CREATE TABLE "public"."items" ("id" integer NOT NULL, "order" text, "price" numeric(18,4) NOT NULL, PRIMARY KEY ("id"))`.
  - `mysql_create_statement`: same with `` ` `` quotes, `int`, `decimal(18,4)`; `timestamp` → `datetime`.
  - `unsupported_db_type`: `"bigquery"` → `Err("Criar tabelas não é suportado para bigquery")`.
  - A primary-key column is always `NOT NULL`; a table without pk columns has no `PRIMARY KEY` clause.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement.** Validation inside existing `validate` (structure, at save). Build refusal at the start of `build`. Use `quote` from scaffold for identifiers (check its db-type behavior; MySQL must use backticks). Postgres family: `postgres`, `redshift`? — only `postgres` and `mysql` are supported; everything else is the 400 message.
- [ ] **Step 4: Run `cargo test -p cubeserver --lib cube_model`, pass.**
- [ ] **Step 5: Commit** `feat(models): draft tables, colors and their CREATE TABLE`.

### Task 2: Create-tables route (server)

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/handlers/cube_models.rs`, `rust/cube/cubeserver/src/rbac/handlers/mod.rs`
- Modify: `packages/cubejs-playground/src/pages/Api/catalog.ts` (one entry, who: "org owner with execute")
- Test: `rust/cube/cubeserver/tests/model_api.rs` (use the existing `build_test!` macro, which points data source `sofiadb` at the test DB and has `shop.orders`/`shop.customers`)

**Interfaces:**
- Consumes: Task 1 `create_statements`, `Definition`, `DraftTable`.
- Produces: `POST {base}/v1/cube-models/:id/create-tables`, body `{version: i32, dryRun?: bool}`:
  - dryRun → 200 `{statements: [sql...]}` (no permission beyond `visible_model`, nothing run).
  - run → 200 `{model: <model json as GET>, created: ["schema.table"], error?: string, detail: <org detail as build returns>}`.
  - Stale version → 409 `O model mudou desde que você abriu; abra de novo` (same as PUT).
  - No drafts → 400 `Não há tabelas novas para criar`.

- [ ] **Step 1: Failing tests:**
  - `create_tables_dry_run_lists_statements`: model with draft `shop.items` → statements = [`CREATE TABLE "shop"."items" (...)`].
  - `create_tables_creates_and_clears_drafts`: run → `created == ["shop.items"]`, the table exists in the test DB (query `information_schema.tables`), GET model shows no `draft` on it, version bumped; a following build no longer refuses.
  - `create_tables_refuses_an_existing_table`: draft `shop.orders` → 409 `A tabela shop.orders já existe no banco`, nothing created.
  - `create_tables_needs_execute`: a user who is org owner but has no `execute` on any workspace → 4xx from `authorize_execute`, nothing created. (If the test fixtures cannot express "owner without execute", assert a non-admin non-owner gets 404 and note it in the report.)
  - Drop the created test tables at the end of each test.
- [ ] **Step 2: Run, fail.**
- [ ] **Step 3: Implement** in `cube_models.rs`: `visible_model` → parse definition → check version → connection belongs to the org (as `build`) → `create_statements(&def, db_type)` → dryRun returns. Run path: `authorize_execute(&state, &ctx, Some(ds))`; take `crate::handlers::data_sources::WRITE` lock; `data_source_schema(ds)` to refuse existing tables (409, before running anything); then for each statement, a driver from `crate::orchestrator_adapter::driver_factory()(ds)` and `driver.query(sql, &[], &QueryOptions::default())` with a 120s timeout, release the driver; stop at the first error (keep it as `error`). Clear `draft` on created tables and save with the store's update (version + 1). Return `detail` the same way `build` does.
- [ ] **Step 4: Run `cargo test -p cubeserver --test model_api`, pass; also `--lib`.**
- [ ] **Step 5: Commit** `feat(models): create a model's new tables in its database`.

### Task 3: Graph helpers and API client (Playground, pure)

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/models/modelGraph.ts`, `packages/cubejs-playground/src/pages/Schema/api.ts`
- Test: `packages/cubejs-playground/src/__tests__/model-graph.test.ts`

**Interfaces:**
- Produces (modelGraph.ts):
  - types `ColumnType = 'text'|'integer'|'bigint'|'numeric'|'boolean'|'date'|'timestamp'`, `COLUMN_TYPES`, `TableColor` (8 names), `TABLE_COLORS`, `DraftColumn {name; type: ColumnType; nullable: boolean; primaryKey: boolean}`, `DraftTable {columns: DraftColumn[]}`; `ModelTable` gains `color?: TableColor; draft?: DraftTable`.
  - `IDENTIFIER = /^[A-Za-z_][A-Za-z0-9_]*$/` and `draftProblem(schema, table, columns, def, editingAlias?) => string | null` (English messages: bad name, no columns, duplicate column, same schema.table as another table of the model).
  - `addDraftTable(def, ds, schema, table, columns, x, y) => {def, error?}` (alias rules as `addTable`; first table becomes primary; ds sets dataSource when empty, refused when another).
  - `updateDraftTable(def, alias, schema, table, columns) => {def, error?}` — keeps alias; drops joins whose column on this alias no longer exists; clears primaryKey if it pointed at a removed column of the primary.
  - `setColor(def, alias, color | undefined) => def`.
  - `layout(def) => def` — BFS layers from primary over joins (both directions); layer i at x = i*320, tables in a layer stacked at y += height(table)+40 where height = 44 + 24*columns (columns unknown for non-drafts → use 6); unlinked tables in a last layer.
  - `drafts(def) => ModelTable[]`.
- Produces (api.ts): `createTables(id: string, version: number, dryRun: boolean) => Promise<{statements?: string[]; model?: CubeModelView; created?: string[]; error?: string; detail?: OrgDetail}>`.

- [ ] **Step 1: Failing tests** for each helper (names: `adds a draft table and makes the first one the primary`, `refuses a draft with a bad name or duplicate columns`, `editing a draft drops links to removed columns`, `colors a table and clears its color`, `lays tables out in layers from the principal`).
- [ ] **Step 2–4: Run fail, implement, run pass** (`vitest run src/__tests__/model-graph.test.ts`, tsc, oxlint).
- [ ] **Step 5: Commit** `feat(playground): draft tables, colors and layout in the model graph`.

### Task 4: Toolbar, table dialog, richer nodes, create-in-database (Playground UI)

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/ModelToolbar.tsx`, `.../models/TableDialog.tsx`, `.../models/CreateTablesDialog.tsx`
- Modify: `.../models/TableNode.tsx`, `.../models/ModelEditor.tsx`, `packages/cubejs-playground/src/shadcn.css` (8 color tokens `--model-<name>` light + dark)
- Test: `packages/cubejs-playground/src/__tests__/model-editor.test.ts` (+ new `model-toolbar.test.ts` if clearer)

**Interfaces:**
- Consumes: Task 3 helpers and `createTables`.

- [ ] **Step 1: Failing tests:**
  - toolbar buttons exist with aria-labels: `New table`, `Table color`, `Edit table`, `Remove table`, `Arrange`, `Zoom in`, `Zoom out`, `Fit view`, `Create in database`; Color/Edit/Remove disabled with no selection; Edit enabled only for a draft; Create disabled with no drafts.
  - New table dialog: fill name `items`, column `id` integer pk → saved definition (after Save) has a draft `items` with that column; a bad name shows the error and does not add.
  - Create in database: calls `createTables(id, version, true)`, shows the statements; confirming calls `createTables(id, version, false)`; on success the node loses the `new` badge and a toast says `Created: shop.items`; a response with `error` shows it in the dialog.
  - Create while the editor is dirty saves first (same as Build).
  - Node: draft shows badge `new` and its draft columns; pk column has `aria-label="primary key"` icon; a linked column has `aria-label="linked"`; the node's header uses `var(--model-<color>)`.
- [ ] **Step 2: Run, fail.**
- [ ] **Step 3: Implement.** Toolbar above the canvas, inside the editor (lucide icons, shadcn Button `variant="ghost" size="icon"`, Tooltip if the kit has one, else `title`). Selection from React Flow (`onSelectionChange` / node `selected`). Color picker: a Popover (or DropdownMenu) of 8 swatches + "No color". Zoom/fit via `useReactFlow()`. Double-click a draft node opens Edit. TableDialog: name, schema (SimpleSelect of the connection's schemas + free text), rows of column name / type select / PK checkbox / nullable checkbox, add/remove row; validation with `draftProblem`. CreateTablesDialog: AlertDialog listing statements in a `<pre>`, buttons `Cancel` / `Create`, shows `error`, warns `This changes the database of <ds>`. After a run: replace the model with `result.model` (version), patch org detail with `result.detail` (as after build), toast. Node visuals per the spec (colored header band, icons per type: Hash for numbers, Type for text, Calendar for date/timestamp, ToggleLeft for boolean, KeyRound pk, Link linked; hover row `tw:hover:bg-accent`, selected `tw:ring-2 tw:ring-ring`).
- [ ] **Step 4: Run full `vitest run`, tsc, oxlint on touched files; pass.**
- [ ] **Step 5: Commit** `feat(playground): model toolbar, table designer and create in database`.

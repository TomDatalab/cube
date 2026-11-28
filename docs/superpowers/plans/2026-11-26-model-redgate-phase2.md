# Model estilo Redgate — Fase 2 (modelagem de banco) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Physical modeling as in Redgate, without breaking today's build:
- dialect types;
- full table properties: columns with details, PK, unique keys, indexes, checks, scripts;
- full reference properties: name, cardinality, mandatory, ON UPDATE/DELETE, column pairs, swap;
- an automatic FK column on new links;
- validation with Issues;
- Generate SQL (Ctrl+G) and a richer "Create in database".

**Architecture:**
- The definition keeps its `joins` array. Each join becomes a Redgate reference by gaining optional physical fields (`id`, `name`, `cardinality`, `mandatory`, actions, extra pairs), while `relationship`/`join` stay where the build reads them.
- Setters keep the two views consistent: cardinality and direction drive `relationship`, mandatory drives `join`.
- Draft tables gain the physical design.
- DDL moves to a new Rust module, used by `create-tables` and a new `POST /v1/cube-models/:id/sql`.
- Validation is a pure TS module whose issues feed a left-pane section and per-object sections.

**Tech Stack:** as phase 1; Rust `cubeserver` (serde, `regex` if already a dependency — else hand parsing).

**Spec:** `docs/superpowers/specs/2026-11-26-model-redgate-design.md` (Fase 2 + "Migração ao longo das fases" + "Conexões que não são Postgres nem MySQL").

## Global Constraints

- **Dialect:** it is `postgres` or `mysql`, from the connection's type in the server config (`state.config.data_sources`). Any other type is "no dialect": the 7 portable types; Generate SQL and Create in database are disabled with hint "Only for Postgres and MySQL connections"; dialect rules are off.
- **Closed type list per dialect** (`(n)` = one size, `(p,s)` = precision with optional scale):
  - **postgres:** `smallint integer bigint serial bigserial numeric(p,s) decimal(p,s) real double precision boolean char(n) varchar(n) text date time timestamp timestamptz interval uuid json jsonb bytea`;
  - **mysql:** `tinyint smallint mediumint int bigint decimal(p,s) float double boolean bit(n) char(n) varchar(n) tinytext text mediumtext longtext date time datetime timestamp year json blob binary(n) varbinary(n)`.
  - A parameter is optional where the base type allows none (`varchar` requires `(n)` in mysql).
  - The 7 portable types (`text integer bigint numeric boolean date timestamp`) stay accepted everywhere: old models.
- **Portable → dialect on load** (the client converts, the next save stores it):
  - postgres: `numeric` → `numeric(18,4)`;
  - mysql: `integer` → `int`, `numeric` → `decimal(18,4)`, `timestamp` → `datetime`;
  - the rest stay the same.
- **Names:** all object names follow `^[A-Za-z_][A-Za-z0-9_]*$`, ≤ 63 chars. Free text (default, check expression, scripts, comments) is unrestricted but only executed by Create in database, which keeps `authorize_execute`.
- **Server messages:** Portuguese. **UI text:** English, Redgate's labels ("Primary key", "Alternate (unique) keys", "Indexes", "Checks", "Additional SQL scripts", "SQL preview", "Cardinality", "Mandatory", "Update constraint action", "Delete constraint action", "Swap reference", "Issues", "Errors", "Warnings").
- **Reference actions:** `none restrict cascade set null set default`. **Cardinality** (child side): `0..*` `0..1` `1..*` `1..1`.
- **Build:**
  - the YAML for an existing model is byte-identical: the existing `cube_model::build` tests must pass unchanged;
  - existing tables stay read-only in structure;
  - no `ALTER TABLE` on existing tables.

## Review Focus

1. **Model with a mysql connection and old portable draft types:** it opens, shows `int`/`decimal(18,4)`/`datetime`, saves, and creates. Test in Task 3 (conversion) and Task 2 (server accepts both).
2. **Changing a reference's cardinality or mandatory:** keeps `relationship`/`join` consistent, so build output follows. Test in Task 3.
3. **Renaming or deleting a draft column that a key, index, check or reference uses:** the dependent entries follow or drop, never pointing at a missing column. Test in Task 3.
4. **Typing in the Generate SQL dialog or the column details popover:** canvas shortcuts don't fire. Covered by phase 1's dialog rule; add one test in Task 9.
5. **Non-Postgres/MySQL connection:** Generate SQL and Create are disabled, validation shows no dialect-type issues. Test in Task 9.

---

### Task 1: Server definition — physical fields, dialect types, dialects in the model view

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/cube_model.rs` (structs, `validate`, new `validate_types`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/cube_models.rs` (create/update call `validate_types` with the connection's db type; `get` adds `dialects`)
- Test: `cube_model.rs` tests; `rust/cube/cubeserver/tests/model_api.rs` only if an existing GET test asserts the exact body

**Interfaces — Produces (JSON camelCase, all optional):**
- `DraftColumn` +:
  - `comment`, `default`, `check` (strings);
  - `autoIncrement`, `unsigned`, `zerofill` (bools).
- `DraftTable` +:
  - `primaryKeyName`;
  - `uniqueKeys: [{name, columns: [string]}]`;
  - `indexes: [{name, columns: [{name, desc?: bool}], unique?: bool}]`;
  - `checks: [{name, expression}]`;
  - `scriptBefore`, `scriptAfter`.
- `ModelJoin` +:
  - `id`, `name`, `comment`;
  - `cardinality` (`0..*|0..1|1..*|1..1`), `mandatory` (bool);
  - `onUpdate`, `onDelete` (one of the actions);
  - `fromRole`, `toRole`;
  - `pairs: [{from, to}]` — extra column pairs after `from.column`/`to.column`.
- `Definition.settings`:
  - `validation?: {[rule: string]: bool}`;
  - `format?: {showTypes?: bool, showNullable?: bool}`;
  - `sqlGeneration?: {drop?: bool, comments?: bool}`;
  - `scripts?: {createBefore?, createAfter?, dropBefore?, dropAfter?}`.
- `pub fn dialect(db_type: &str) -> Option<&'static str>` returns `postgres`/`mysql`/None.
- `pub fn valid_type(dialect: Option<&str>, ty: &str) -> bool`.
- `pub fn validate_types(def: &Definition, db_type: &str) -> Result<(), String>`. Error text: `Tipo inválido para {table}.{column}: {ty}`.
- `GET /v1/cube-models/:id` adds `"dialects": {"<ds>": "postgres"|"mysql"|null}` for every configured data source.

- [ ] **Step 1: Write failing tests.**
  - `accepts_physical_fields`: round-trips every new field.
  - `refuses_bad_physical_fields`, one assertion each:
    - a unique key with no columns → `"A chave única {name} precisa de colunas"`;
    - an index on an unknown column → `"O índice {name} usa a coluna {col}, que não existe"`;
    - a bad cardinality → `"Cardinalidade inválida"`;
    - a bad action → `"Ação inválida"`;
    - a duplicate reference id → `"A ligação {id} se repete"`;
    - an invalid key/index/check name → `"Nome inválido"`.
  - `dialect_types`:
    - `valid_type(Some("postgres"), "varchar(64)")` and `"numeric(18,4)"` and `"double precision"`;
    - not `"varchar(x)"`, not `"int"` in postgres, not `"varchar"` in mysql;
    - portable `"integer"` valid in both;
    - `valid_type(None, "jsonb")` false.
  - `validate_types_names_the_column`.
- [ ] **Step 2:** Run `cd rust/cube && cargo test -p cubeserver --lib rbac::cube_model` and check that it fails.
- [ ] **Step 3: Implement.**
  - Validation rules for drafts:
    - key, index and check names are valid identifiers;
    - the columns they use exist in the draft;
    - unique-key and index names are unique within the table.
  - `validate` keeps its existing errors and order.
  - Handlers: `create`/`update` resolve the db type of `definition.dataSource` (as `build` does) and call `validate_types`.
- [ ] **Step 4:** Run the same command, plus `cargo test -p cubeserver --test model_api 2>&1 | tail -3` (with `CUBE_TEST_PG_URL` set as in the deploy memory). Check that both pass.
- [ ] **Step 5:** Commit `feat(models): physical design fields and dialect types on the server`.

### Task 2: DDL module and Generate SQL endpoint

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/ddl.rs` (registered in `rbac/mod.rs`)
- Modify:
  - `cube_model.rs`: `create_statements` delegates to `ddl`;
  - `handlers/cube_models.rs`: new `sql` handler;
  - the router where `create-tables` is routed: route `POST /v1/cube-models/:id/sql`.
- Test: `ddl.rs` tests

**Interfaces — Produces:**
- `pub struct ScriptOptions { pub drop: bool, pub only_new: bool, pub only: Option<Vec<String>>, pub comments: bool }`.
- `pub fn script(def: &Definition, columns: &DatabaseStructure, db_type: &str, opts: &ScriptOptions) -> Result<String, String>`. The order is:
  1. model `createBefore`;
  2. DROPs if `drop` (reverse order, `dropBefore`/`dropAfter`);
  3. per table: `scriptBefore`, `CREATE TABLE`, `scriptAfter`;
  4. all `CREATE [UNIQUE] INDEX`;
  5. all FKs as `ALTER TABLE child ADD CONSTRAINT name FOREIGN KEY (...) REFERENCES parent (...) [ON UPDATE x] [ON DELETE y]`;
  6. comments (`COMMENT ON` for postgres, inline `COMMENT '...'` for mysql columns/tables);
  7. `createAfter`.
  - Statements end with `;\n`.
  - Existing tables use the connection's column types; their PK comes from `primaryKey` only when they are the principal.
- `pub fn create_statements(def, db_type) -> Result<Vec<Statement>, String>` with `pub struct Statement { pub table: Option<String>, pub sql: String }`:
  - only drafts: `CREATE TABLE` (table = Some), then indexes, then FKs whose child is a draft (table = None);
  - no DROP, no model scripts;
  - the draft's own `scriptBefore`/`scriptAfter` are included.
- `CREATE TABLE` body:
  - each column `name type [AUTO_INCREMENT|GENERATED BY DEFAULT AS IDENTITY] [NOT NULL] [DEFAULT x] [CHECK (x)] [UNSIGNED] [ZEROFILL]` — UNSIGNED/ZEROFILL in mysql only, placed after the type;
  - then `CONSTRAINT pk PRIMARY KEY (...)`, `CONSTRAINT uk UNIQUE (...)`, `CONSTRAINT ck CHECK (...)`.
- FK name: `reference.name`, else `fk_<child>_<parent>`.
- Handler `sql`:
  - body `{definition, options: {drop?, onlyNew?, only?}}`;
  - checks `visible_model`, parses and validates the definition, resolves the connection like `build`;
  - refuses a non-dialect connection with `"Gerar SQL só para conexões Postgres ou MySQL"`;
  - responds `{script}`;
  - runs nothing.
- `create_tables` uses `Statement.table` for `created`/`clear_drafts`. Statements with `table: None` are reported by their first line on error.

- [ ] **Step 1: Write failing tests (in `ddl.rs`)**, asserting exact substrings:
  - `postgres_table_with_keys_indexes_checks`: a draft `shop.items(id integer pk autoIncrement, sku varchar(32) not null, price numeric(10,2) default 0 check(price >= 0))` with unique `uk_sku(sku)`, index `ix_price(price desc)` and check `ck_x(price < 1000)`. The script contains:
    - `"id" integer GENERATED BY DEFAULT AS IDENTITY NOT NULL`;
    - `"price" numeric(10,2) DEFAULT 0 CHECK (price >= 0)`;
    - `CONSTRAINT "uk_sku" UNIQUE ("sku")`;
    - `CREATE INDEX "ix_price" ON "shop"."items" ("price" DESC);`.
  - `mysql_unsigned_auto_increment`: `` `id` int UNSIGNED AUTO_INCREMENT NOT NULL ``.
  - `fk_with_actions`: `ALTER TABLE "shop"."orders" ADD CONSTRAINT "fk_orders_customers" FOREIGN KEY ("customer_id") REFERENCES "shop"."customers" ("id") ON DELETE CASCADE;`.
  - `script_options`:
    - `only_new` leaves existing tables out;
    - `only` limits to listed aliases;
    - `drop` puts `DROP TABLE` before the creates;
    - the model scripts wrap everything.
  - `create_statements_tags_tables`: the CREATE carries `table: Some("shop.items")`, the FK `None`.
  - `old_portable_types_still_create`: a v1 draft with `numeric` creates `numeric(18,4)` (postgres) as before. The existing `create_statements` tests keep passing.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement. Reuse `quote`; strings in comments are quoted with `'` doubled.
- [ ] **Step 4:** Run `cargo test -p cubeserver --lib rbac::` and check that it passes.
- [ ] **Step 5:** Commit `feat(models): DDL generation and the Generate SQL endpoint`.

### Task 3: Client model — types, conversion, physical helpers, reference setters

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/models/modelGraph.ts` (types)
- Create:
  - `packages/cubejs-playground/src/pages/Schema/models/dialect.ts`;
  - `packages/cubejs-playground/src/pages/Schema/models/physical.ts`.
- Modify: `api.ts` (`model()` returns `dialects`; new `modelSql(id, definition, options)`)
- Test:
  - `src/__tests__/model-dialect.test.ts`;
  - `src/__tests__/model-physical.test.ts`.

**Interfaces — Produces:**
- `dialect.ts`:
  - `type Dialect = 'postgres' | 'mysql' | null`;
  - `TYPES: Record<'postgres'|'mysql', string[]>`, the base names from Global Constraints;
  - `validType(d: Dialect, ty: string): boolean`, the same rules as Rust;
  - `toDialect(d: Dialect, ty: string): string`, the portable → dialect map;
  - `convertDrafts(def, d): ModelDefinition`, which returns `def` itself when nothing changes.
- `physical.ts` (pure, each returns a new definition):
  - Columns:
    - `setColumn(def, alias, index, patch: Partial<DraftColumn>)`;
    - `addColumn(def, alias)` appends `column_<n>`, the first free, with the dialect's text type (`text`/`varchar(255)`);
    - `removeColumn(def, alias, name)`;
    - `moveColumn(def, alias, from, to)`;
    - `renameColumn(def, alias, from, to)`, which follows keys, indexes, checks' column lists and references (`from.column`, `to.column`, `pairs`). Removing a column drops it from keys and indexes, drops keys and indexes left empty, and drops references using it.
  - Keys, indexes, checks:
    - `setPrimaryKeyName`;
    - `setUniqueKeys(def, alias, keys)`;
    - `setIndexes(def, alias, indexes)`;
    - `setChecks(def, alias, checks)`;
    - `setTableScripts(def, alias, {before?, after?})`.
  - References:
    - `ensureIds(def)` gives every join without `id` the first free `r<n>`;
    - `referenceEnds(j): { parent: End; child: End }`. The parent is `to` for `many_to_one`/`one_to_one` and `from` for `one_to_many`;
    - `setReference(def, id, patch: { name?, comment?, cardinality?, mandatory?, onUpdate?, onDelete?, fromRole?, toRole?, pairs? })`:
      - a cardinality ending `..1` sets `relationship` `one_to_one`;
      - one ending `..*` sets `many_to_one` if the child is `from`, else `one_to_many`;
      - `mandatory` true sets `join: 'inner'`, false sets `'left'`.
    - `swapReference(def, id)` swaps parent and child: `many_to_one` ↔ `one_to_many`; `one_to_one` swaps `from`/`to`.
    - `addReference(def, parentAlias, childAlias, columnsOf): { def; id }`. Following Redgate, it sets `cardinality: '0..*'`, `mandatory: true`, `join: 'inner'`, `relationship: 'many_to_one'`, `from` = child, `to` = parent's first PK. The child column is chosen as follows:
      - a draft child gains `<parent>_<pk>` (first free name) with the parent PK's type, nullable false;
      - an existing child uses its same-named column, else `<parent>_<pk>` if present, else its first column.
- `api.model()` type gains `dialects?: Record<string, 'postgres' | 'mysql' | null>`.

- [ ] **Step 1: Write failing tests.**
  - `toDialect maps portable types and leaves others`.
  - `validType mirrors the server cases`, with the same cases as the Rust test.
  - `convertDrafts changes only portable draft types`.
  - `renameColumn follows keys, indexes and references`.
  - `removeColumn drops empty keys and the references using it`.
  - `setReference keeps relationship and join in step` (the 4 cardinalities × child side, and mandatory).
  - `swapReference swaps parent and child`.
  - `addReference adds the FK column to a draft child with the parent key's type`.
  - `addReference reuses a same-named column of an existing child`.
  - `ensureIds numbers the joins that have none`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass. Run `tsc` and check that it is clean.
- [ ] **Step 5:** Commit `feat(models): dialect types and physical design helpers`.

### Task 4: Validation rules

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/validate.ts`
- Test: `src/__tests__/model-validate.test.ts`

**Interfaces — Produces:**
- `type Issue = { rule: string; severity: 'error' | 'warning'; target: { kind: 'table'; alias: string } | { kind: 'reference'; id: string }; message: string }`.
- `RULES: { id: string; group: 'Table' | 'Table column' | 'Table alternate key' | 'Table check' | 'Table index' | 'Reference'; label: string; severity: 'error' | 'warning'; dialect?: true }[]`. The Redgate list for those groups, ids `table.emptyName`, `table.uniqueName`, `table.noColumns`, `table.generatedName`, `table.invalidName`, `table.nameLength`, `table.hasPk`, `column.emptyName`, `column.emptyType`, `column.uniqueName`, `column.pkNotNull`, `column.percent`, `column.generatedName`, `column.nameLength`, `column.invalidName`, `column.supportedType` (dialect), `ak.emptyName`, `ak.uniqueName`, `ak.columns`, `ak.invalidName`, `ak.nameLength`, `check.emptyExpression`, `check.invalidName`, `index.emptyName`, `index.uniqueName`, `index.columns`, `index.invalidName`, `index.nameLength`, `reference.emptyName`, `reference.uniqueName`, `reference.keys`, `reference.invalidName`, `reference.nameLength`, `reference.selfMandatory`. Label and severity are as Redgate shows them (see the spec). Notes:
  - "Generated name" means `<base>_<digits>`;
  - `reference.emptyName` only fires for a reference that has a `name` key set to `''`;
  - `reference.keys` checks that the parent columns are the parent's PK or a unique key (or, for an existing table, are named in its model PK), that the column counts match, and that the child/parent types are equal where both are known.
- `issues(def, columnsOf, dialect: Dialect): Issue[]` skips the rules whose `def.settings?.validation?.[id] === false` and the dialect rules when `dialect` is null.

- [ ] **Step 1: Write failing tests.** One per group with exact messages:
  - `table.hasPk`: `"Table items has no primary key"`;
  - `column.pkNotNull`: `"Column items.id is in the primary key and allows nulls"`;
  - `index.columns`: `"Index ix_a of items has no columns"`;
  - `reference.keys`: `"Reference r1: orders.customer_id does not match a key of customers"`;
  - `reference.selfMandatory`: `"Reference r2 links items to itself and is mandatory"`.
  - Also `a rule switched off in settings is skipped` and `dialect rules are off without a dialect`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): model validation rules`.

### Task 5: Crow's-foot from cardinality; format settings on the node

**Files:**
- Modify: `referencePath.ts` (`endMarks`), `TableNode.tsx`, `ModelEditor.tsx` (node data)
- Test: `model-reference-path.test.ts`, `model-editor.test.ts` (table node)

**Interfaces:**
- `endMarks(j)`:
  - when `j.cardinality` is set: child end `{many: card ends '*', optional: card starts '0'}`, parent end `{many: false, optional: !(j.mandatory ?? j.join === 'inner')}`;
  - without it: phase-1 behaviour;
  - the ends map back to `from`/`to` via `referenceEnds`.
- `TableNodeData` +:
  - `showTypes: boolean` and `showNullable: boolean` (from `def.settings.format`, default true and false);
  - `nullable: string[]` (the columns allowing nulls: draft `nullable`; unknown for existing tables).
- The marks cell shows `N` after PK/FK when `showNullable` and the column allows nulls.

- [ ] **Step 1: Write failing tests.**
  - `endMarks uses cardinality and mandatory`: `1..*` child → `{many:true, optional:false}`; `mandatory:false` parent → optional.
  - `hides types and shows N by the model's format`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): crow's foot from cardinality and format settings`.

### Task 6: Table properties — full physical design

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/TableProperties.tsx`, which moves `TableProperties` out of `PropertiesPanel.tsx` and grows it
- Modify: `PropertiesPanel.tsx`
- Test: `model-editor.test.ts` (`describe('table properties')`)

**Interfaces:**
- Consumes: `physical.ts`, `dialect.ts`, `validate.ts` (Tasks 3–4).
- Sections, in Redgate's order:
  - **General:** Name, Comment, the "SQL preview" link in the header.
  - **Columns:** a grid of rows with
    - a drag handle (`aria-label="Move <col>"`, reorder by drag; keyboard Alt+↑/↓ also moves);
    - Name input (`aria-label="Column <i> name"`);
    - Type input with datalist of `TYPES[dialect]` (`aria-label="Column <i> type"`);
    - N checkbox and PK checkbox;
    - a "…" button (`aria-label="Column <i> details"`) opening a popover with Comment, Default value, Check expression, Auto increment, and in mysql only Unsigned and Zerofill;
    - plus "Add column" and a remove × per row.
  - **Primary key:** Name and the column list (from the PK flags, read-only order).
  - **Alternate (unique) keys:** list of {name, columns multi-check}, "Add key".
  - **Indexes:** {name, columns with ASC/DESC toggle, unique}, "Add index".
  - **Checks:** {name, expression}, "Add check".
  - **Additional SQL scripts:** before/after textareas, committed on blur.
  - **Format:** Color swatches (moved from General).
  - **Table issues:** Errors (n), Warnings (n), the messages for this table.
- Database (non-draft) tables: Columns read-only (name, type), no key/index/check editing (message "The structure comes from the database"); Comment, Format and Issues stay.
- Text inputs commit on blur or Enter (one undo step); checkboxes commit at once.
- "SQL preview" opens the Generate SQL dialog of Task 9 with `only: [alias]`.
- `TableDialog` stays for New table only. "Edit columns" goes away, and a double click on a draft selects it and focuses its first column name.

- [ ] **Step 1: Write failing tests.**
  - `edits a draft column name, type, N and PK and saves them`.
  - `opens a column's details and sets default and comment`.
  - `adds a unique key, an index with DESC and a check`.
  - `reorders columns with Alt+ArrowDown`.
  - `a database table's columns are read-only`.
  - `shows the table's issues`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): Redgate table properties`.

### Task 7: Reference properties and the reference tool

**Files:**
- Create: `ReferenceProperties.tsx`, moved out of `PropertiesPanel.tsx` and grown
- Modify: `ModelEditor.tsx` (selection by reference id; reference tool uses `addReference`)
- Test: `model-editor.test.ts` (`describe('reference properties')`)

**Interfaces:**
- Selection of a reference becomes its `id`. `selectedEdge: string | null`, the edge id is the reference id, and `ensureIds` runs on load. This resolves the phase-1 deferred ruling.
- Sections:
  - **General:**
    - Name, Comment;
    - Cardinality (select of the 4);
    - "Swap reference" button;
    - a Primary | Foreign table header, Role inputs, column pairs (selects of parent and child columns, × per extra pair, "+ Add");
    - Mandatory checkbox;
    - Update constraint action and Delete constraint action (selects);
    - Hidden on the canvas;
    - "Cube join" (read-only `joinLabel`): what the build will use;
    - Delete reference.
  - **Reference issues.**
- Reference tool (palette 4): click the parent, then the child. It calls `addReference` and selects the new reference, with no JoinDialog. Drag or click between columns keeps the JoinDialog.

- [ ] **Step 1: Write failing tests.**
  - `sets cardinality 1..1 and the cube join becomes 1:1`.
  - `unchecking mandatory makes the join LEFT`.
  - `swap reference makes the other table the parent`.
  - `adds a column pair and picks its columns`.
  - `the reference tool adds the FK column to a new child table and selects the reference`.
  - `undo keeps the same reference selected by id`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): Redgate reference properties and FK-creating reference tool`.

### Task 8: Model properties sections and the Issues pane

**Files:**
- Modify: `PropertiesPanel.tsx` (model sections), `StructurePanel.tsx` (Issues section enabled), `ModelEditor.tsx` (issues computed once per definition with `useMemo`)
- Test: `model-editor.test.ts` (`describe('issues')`)

**Interfaces:**
- Model properties sections:
  - **General:** as now.
  - **Additional SQL scripts:** Beginning/End of the create script, Beginning/End of the drop script.
  - **Model validation settings:** "Select all | Deselect all", then the checkbox per `RULES`, grouped, labelled `<label> (errors|warnings)`.
  - **Format:** Show column types, Show N (nullable).
  - **SQL generation settings:** Generate DROP statements, Generate comments.
- **Left pane "Issues":** enabled, with a badge of the count. It shows "Errors (n)" and "Warnings (n)" lists; a click on an issue selects its target and centres on it.

- [ ] **Step 1: Write failing tests.**
  - `lists the issues and selects the table of one`.
  - `switching a rule off hides its issues`.
  - `edits the model's scripts and SQL generation settings`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): model settings and the Issues pane`.

### Task 9: Generate SQL dialog, dialect wiring, Create in database

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/SqlDialog.tsx`
- Modify: `ModelEditor.tsx`, `ModelToolbar.tsx` (enable "Generate SQL script", Ctrl+G), `api.ts`
- Test: `model-editor.test.ts` (`describe('generate sql')`)

**Interfaces:**
- `SqlDialog` props: `{ open; script: string | null; error: string | null; options: {drop, onlyNew, onlySelection}; canSelection: boolean; onOptions(o); onClose(); loading }`.
  - It shows the script in `NumberedCode`.
  - It has Copy (clipboard) and Download (`<model name>.sql` via a Blob link).
  - Options are checkboxes; changing one re-asks the server.
- The editor:
  - `dialect = dialects[def.dataSource] ?? null`;
  - applies `convertDrafts` once on load, if it changes anything (then `history.reset` with the converted def, not dirty-marked — the next save stores it);
  - disables Generate SQL / Create in database without a dialect, with hint "Only for Postgres and MySQL connections";
  - sends `api.modelSql(id, withFormat(def), {drop, onlyNew, only})` without saving.

- [ ] **Step 1: Write failing tests.**
  - `Ctrl+G opens the script and the options re-ask the server`.
  - `downloads the script as <name>.sql`.
  - `Generate SQL and Create are disabled for a non-Postgres/MySQL connection`.
  - `converts portable draft types for a mysql connection on open`.
  - `ignores canvas shortcuts while typing in the SQL dialog`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the full suite, `tsc` and `oxlint`, and check that all are green.
- [ ] **Step 5:** Commit `feat(models): Generate SQL dialog and dialect-aware editor`.

### Task 10: Build, install, browser check

- [ ] **Step 1:** Back up the server binary and the Playground as `*.bak-20261126-redgate2`.
- [ ] **Step 2:** Build the server (release), swap the binary (`cp` to `.new` + `mv`), build and install the Playground, then `./start-all.sh restart`.
- [ ] **Step 3: Browser check** on a mocked model with a draft table and a postgres connection. Expected, with no page errors:
  - edit a column's type and details;
  - add a unique key and an index;
  - draw a reference with the tool: an FK column appears in the draft;
  - set cardinality and actions;
  - the Issues list shows a missing PK, and clicking it selects the table;
  - Ctrl+G shows CREATE TABLE with constraints and the FK;
  - take light and dark screenshots.
- [ ] **Step 4:** Commit any fixes. Do not push without being asked.

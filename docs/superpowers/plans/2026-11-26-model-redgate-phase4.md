# Model estilo Redgate — Fase 4 (cubos) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cube objects on the model canvas, as spec approach A describes them:
- tool 8 places a cube;
- a cube reads from a table, from a set of linked tables, or from free SQL, and a dashed line joins it to the tables it reads;
- it has dimensions, measures and segments, edited in its properties pane; "Generate members" fills them from the columns;
- tool 4 between two cubes creates a Cube join, drawn as a labelled crow's-foot link;
- Build writes one `.yml` per cube and removes the file of a cube taken out of the model;
- an existing model becomes one "linked tables" cube that builds the same YAML as before.

**Architecture:**
- The definition gains `cubes` and `cubeJoins`. `primary` and `primaryKey` become legacy fields: the server reads them, converts them to a cube (`cube_model::upgrade`) and never writes them again.
- The conversion runs **on the server only**: on every GET of a model (so the browser opens it converted) and on build. The browser never converts, so the cube name cannot drift between two implementations.
- A cube whose `dimensions` and `measures` are both empty is in **auto** mode: the build generates the members from the columns exactly as today's build does. A converted model is in auto mode, which is what keeps its YAML byte-identical.
- References keep `relationship` and `join`: `physical.ts` already keeps them in sync with cardinality and mandatory, and the "linked tables" cube reads them from its primary table outward, as today's build does. There is no per-reference cube field to move.
- Build records move from `cube_models.cube_*` to a new table `cube_model_cubes`, one row per built cube, keyed by the cube id. One build is one model write that puts, updates and deletes every cube file of the model.

**Tech Stack:** as phases 1–3: React 18, `@xyflow/react` 12.3.6, Tailwind `tw:` + shadcn, lucide (`Box`), vitest + jsdom; Rust (serde, serde_yaml, tokio-postgres) in `cubeserver`.

**Spec:** `docs/superpowers/specs/2026-11-26-model-redgate-design.md` (Fase 4; "Migração ao longo das fases" Fase 4; "Build que remove o arquivo de um cubo"; Servidor "Build por cubo"; Anti-metas "Regressão nos models de hoje").

## Global Constraints

- **Ids:** cubes `c<k>`, cube joins `cj<k>`, each with the smallest free `k ≥ 1` in its own kind. The node id is `cube:<id>`; the edge id is `cubejoin:<id>`. A converted model's cube is `c1`.
- **Names:**
  - cube and member names follow `^[A-Za-z_][A-Za-z0-9_]*$`, ≤ 63 chars;
  - cube names are unique in the model; member names (dimensions, measures and segments together) are unique in their cube.
- **Closed lists (server and client share them):**
  - source kinds: `table`, `joined`, `sql`;
  - dimension types: `string`, `number`, `time`, `boolean`, `geo`;
  - measure types: `count`, `count_distinct`, `count_distinct_approx`, `sum`, `avg`, `min`, `max`, `number`;
  - measure formats: `percent`, `currency`;
  - cube join relationships: `one_to_one`, `one_to_many`, `many_to_one`.
- **Free text:** cube `sql`, member `sql`, segment `sql`, titles and descriptions are unrestricted.
- **Colors:** a cube's `color` is a `TABLE_COLORS` name, default `violet`, drawn with `var(--model-<color>)` like a table header. No hex in a component, no new token.
- **Build output:**
  - every file starts with the existing header line `# Built from model {name} ({id}): Build cube rewrites this file`;
  - a converted model builds byte-identical YAML to today's build. Task 2 pins this with a golden test taken **before** the code changes;
  - the file of a new cube is `<cubeName>.yml`; a rebuilt cube stays in the file of its last build, even after a rename.
- **Server messages:** Portuguese. The hand-edit 409 keeps the exact text `O cubo foi editado desde o último build` (the client matches it).
- **UI labels (English, Redgate style):**
  - pane titles: "Cube properties", "Cube join properties";
  - sections: "General", "Source", "Dimensions", "Measures", "Segments", "Format", "Issues";
  - the button "Generate members";
  - the structure groups "Cubes" and "Cube joins".
- **Undo:** every change to cubes and cube joins is one undo step, through the existing `change()` path.

## Review Focus

1. **A model saved before this phase** (with `primary`, `primaryKey`, a `cubeName` and joins): it opens with one cube `c1` named after the old cube, builds the same file content into the same file, and a second build is a no-op rewrite with no 409. Test in Task 2 (golden) and Task 3 (API).
2. **Deleting or renaming a table a cube reads:** the cube's source and its `alias.column` members follow a rename. After a delete the source table is cleared and the client flags it; the server never refuses the next save. Test in Task 4.
3. **Removing a cube whose file was edited by hand:** the build answers 409 with the edit message. With `force` it deletes the file. A cube removed whose file is already gone just drops its record. Test in Task 3.
4. **A cube named like a cube in a file the model does not own:** the build is refused and names the file. Nothing is written, not even the other cubes. Test in Task 3.
5. **Typing in a member name, a cube SQL or a segment SQL:** the canvas shortcuts (Delete, digits, Ctrl+A/C/V) don't fire. Test in Task 8.

---

### Task 1: Server definition — cubes, cube joins, conversion

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/cube_model.rs`:
  - new structs;
  - `Definition` fields;
  - `validate` (after the view checks, keeping every existing error and its order);
  - new `upgrade`.
- Test: `cube_model.rs` tests.

**Interfaces — Produces (JSON camelCase; arrays default empty and are skipped when empty; every struct `deny_unknown_fields` + `Default`):**
- `Definition` + `cubes: Vec<CubeNode>`, `cube_joins: Vec<CubeJoin>` (JSON `cubeJoins`). `primary` and `primary_key` gain `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- `CubeNode { id, name, title?, description?, color?, source: CubeSource, dimensions: Vec<CubeDimension>, measures: Vec<CubeMeasure>, segments: Vec<CubeSegment>, x, y, width?, height?, z?: i32 }`.
- `CubeSource { kind: String, table?: String, primary_key?: String, sql?: String, tables: Vec<String> }`:
  - `table` is the table alias for `table`, the primary table for `joined`;
  - `primary_key` is a column of that table, used in auto mode;
  - `sql` is the query for `sql`;
  - `tables` lists the aliases a `sql` cube reads, for the dashed line.
- `CubeDimension { name, kind (JSON "type"), column?: String, sql?: String, primary_key: bool (skip false), title? }`. `column` is `alias.column` in a `joined` cube and a plain column name otherwise.
- `CubeMeasure { name, kind (JSON "type"), column?, sql?, title?, format? }`.
- `CubeSegment { name, sql: String, title? }`.
- `CubeJoin { id, from: String, to: String, relationship: String, on: Vec<ColumnPair>, points: Vec<Point>, hidden: bool (skip false) }`. `from` and `to` are cube ids, and `on` pairs name dimensions of `from` and `to`.
- `pub const DIMENSION_TYPES`, `MEASURE_TYPES`, `MEASURE_FORMATS`, `SOURCE_KINDS`, with the values in Global Constraints.
- `pub fn upgrade(def: &mut Definition, cube_name: &str)`:
  - when `primary` is set and `cubes` is empty, push cube `c1` named `cube_name`, source `{kind:"joined", table: primary, primaryKey}`, at `x` = the largest table `x` + 320 and `y` = the primary table's `y`, with no members;
  - in every case, clear `primary` and `primary_key`.

- [ ] **Step 1: Write failing tests.**
  - `accepts_cubes_and_cube_joins`: round-trips a definition with two cubes (one of each source kind among them, with every member kind) and one cube join. An old definition serializes with no `cubes`/`cubeJoins` keys.
  - `upgrade_turns_primary_into_a_cube`: the `model_def` fixture with `primary: orders`, `primaryKey: id` and `cube_name "vendas"` gives exactly one cube `{id:"c1", name:"vendas", source:{kind:"joined", table:"orders", primaryKey:"id"}}`, at the position above, and `primary`/`primaryKey` become none. Running `upgrade` again changes nothing. A definition without `primary` gets no cube.
  - `refuses_bad_cubes`, one assertion each:
    - a repeated cube id → `"O objeto {id} se repete"`;
    - a cube name `"1x"` → `"Nome de cubo inválido: 1x"`;
    - two cubes named `x` → `"O cubo x se repete"`;
    - source kind `"view"` → `"Origem inválida no cubo {name}: view"`;
    - a source table or a `tables` entry that is not an alias → `"O cubo {name} lê a tabela {t}, que não está no model"`;
    - a bad member name → `"Nome de membro inválido: {cube}.{m}"`;
    - a dimension and a measure with the same name → `"O membro {cube}.{m} se repete"`;
    - a bad dimension type, measure type or format → `"Tipo inválido em {cube}.{m}: {t}"` (the same message for a format);
    - a member with both `sql` and `column`, or a non-count measure or a dimension with neither → `"O membro {cube}.{m} precisa de SQL ou de coluna, não dos dois"`;
    - a bad cube color → `"Cor inválida: {c}"`.
  - `refuses_bad_cube_joins`:
    - an unknown cube id → `"A ligação {id} liga cubos que não estão no model"`;
    - `from == to` → the same message;
    - a bad relationship → `"Relacionamento inválido: {r}"`;
    - an empty `on` → `"A ligação {id} não tem colunas"`;
    - a second join between the same two cubes, either direction → `"Os cubos {a} e {b} já estão ligados"`, naming the cubes.
- [ ] **Step 2:** Run `cd rust/cube && cargo test -p cubeserver --lib rbac::cube_model` and check that the new tests fail.
- [ ] **Step 3: Implement** the structs, the checks and `upgrade`. Reuse `valid_ident`, `COLORS`, `RELATIONSHIPS` and the size finiteness check. The cube's source table may be empty while drafting: `validate` accepts a missing `table`, and the build refuses it (Task 2).
- [ ] **Step 4:** Run the same command, then the `model_api` suite with the test DB (see the deploy memory): `cargo test -p cubeserver --test model_api 2>&1 | tail -3`. Check that both pass.
- [ ] **Step 5:** Commit `feat(models): cubes and cube joins in the model definition`.

### Task 2: Server build — one YAML per cube

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/cube_model.rs` (`build` becomes `build_cube` and `build_all`)
- Test: `cube_model.rs` tests

**Interfaces:**
- Consumes: Task 1 types and `upgrade`.
- Produces:
  - `pub struct BuiltCube { pub cube_id: String, pub cube_name: String, pub content: String }`;
  - `pub fn build_all(name: &str, model_id: Uuid, def: &Definition, columns: &cubedriver::DatabaseStructure, db_type: &str) -> Result<Vec<BuiltCube>, String>`, in `def.cubes` order. The old `build` is removed, and its callers move to `build_all` in Task 3.

**Rules of one cube's YAML (`cubes: [<cube>]`), keys in this order:** `name`, `data_source` (as today), `sql` or `sql_table`, `title`, `description`, `joins`, `dimensions`, `measures`, `segments`. Optional keys are left out when empty.
- **Source:**
  - `table`: `sql_table: <q(schema)>.<q(table)>`;
  - `joined`: the same `SELECT … FROM … JOIN …` as today's build, walking `def.joins` breadth-first from `source.table`. The tables it does not reach are not part of the cube, and that is not an error;
  - `sql`: `sql: <source.sql trimmed>`.
- **Read tables** (`table`/`joined`): each must exist in `columns`, with the existing messages. A draft table refuses the build with the existing draft message. Tables no cube reads are not checked.
- **Auto mode** (no dimensions and no measures):
  - `joined`: today's output exactly, including the `count_distinct` on the key when a step is `one_to_many`. Messages: `"Escolha a tabela principal do cubo {name}"` and `"Escolha a chave primária do cubo {name}"`;
  - `table`: one dimension per typed column, named `snake(column)` and deduplicated like today; `sql` `{CUBE}.<q(column)>`; `primary_key` on `source.primaryKey`; and a `count`;
  - `sql`: refused with `"O cubo {name} não tem membros"`.
- **Explicit members:**
  - a `column` member gets `sql: {CUBE}.<q(c)>`. In a `joined` cube, `alias.column` maps to the member name that column has in the SELECT;
  - a `column` that is not in the read tables → `"A coluna {c} não existe no cubo {name}"` (not checked for `sql` cubes);
  - a member's `sql` is written verbatim;
  - `primary_key: true`, `title` and `format` are written when set.
- **Joins:** each `CubeJoin` is written on its `from` cube as `{name: <to cube name>, relationship, sql}`. The `sql` is the pairs as `{CUBE.<from>} = {<to cube>.<to>}`, joined by ` AND `. A pair naming a dimension the cube does not have (auto names included) → `"A ligação {id} usa {cube}.{m}, que não é dimensão do cubo"`.

- [ ] **Step 1: Before touching `build`, pin the golden output.** Add `const GOLDEN_LEFT_JOIN: &str` and `const GOLDEN_ONE_TO_MANY: &str`. These are the exact strings the **current** `build` returns for the existing `builds_a_left_join_cube` fixture and for a `one_to_many` variant of it. Capture them by running the current code once, in a scratch test that prints them. Commit `test(models): golden output of today's cube build`.
- [ ] **Step 2: Write failing tests.**
  - `converted_model_builds_the_golden_yaml`: both fixtures go through `upgrade(def, "vendas")` and then `build_all`, and each matches its golden byte for byte.
  - `table_cube_auto_members`: names, `sql_table` and `primary_key`.
  - `sql_cube_needs_members`.
  - `explicit_members_on_each_source`: a `joined` cube with dimension `column: "customers.region"` gets `sql: '{CUBE}."customers_region"'`; a `table` cube with a `sum` measure `column: amount`, `format: currency`; a `sql` cube with a free-SQL dimension and a segment.
  - `cube_join_on_the_from_cube`: the YAML of the `from` cube has `joins: [{name: customers, relationship: many_to_one, sql: "{CUBE.customer_id} = {customers.id}"}]`, and the `to` cube has no `joins`.
  - `built_cubes_compile`: the files of a two-cube model with a join pass `crate::rbac::model_validate::check` with no problems.
  - `unknown_join_member_is_refused` and `unknown_column_is_refused`, with the messages above.
  - `a_draft_table_only_blocks_the_cubes_that_read_it`.
  - The existing build tests move to `build_all` over upgraded definitions. Their error assertions change only where the rules above rename a message, and nowhere else.
- [ ] **Step 3:** Run `cd rust/cube && cargo test -p cubeserver --lib rbac::cube_model` and check that the new tests fail.
- [ ] **Step 4: Implement.** Reuse `cube_type`, `quote` and `snake` from `handlers::scaffold`.
- [ ] **Step 5:** Run the same command. Check that it passes.
- [ ] **Step 6:** Commit `feat(models): build one YAML per cube`.

### Task 3: Server storage and handler — build records per cube

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/migrations.sql`, `rust/cube/cubeserver/src/rbac/store.rs`, `rust/cube/cubeserver/src/rbac/handlers/cube_models.rs`, `rust/cube/cubeserver/src/rbac/handlers/model.rs` (organization detail)
- Test: `rust/cube/cubeserver/tests/model_api.rs`

**Interfaces:**
- Consumes: Task 2 `build_all`, Task 1 `upgrade`.
- Produces (HTTP):
  - **model view** (GET, list, PUT, create answers): `definition` already upgraded, using the stored `cube_name`, or `snake(model.name)` when there is none. `cubeName` is replaced by `cubeNames: string[]`, the cube names of the last build, by name.
  - **organization detail** `models: [{id, name, cubeNames}]`.
  - **`POST /v1/cube-models/:id/build` `{force?}`** answers `{ cubes: [{cubeName, fileName}], removed: [fileName], detail }`.
- Produces (store):
  - `struct BuiltRecord { cube_id: String, cube_name: String, file_id: Uuid, built_hash: String }`;
  - `cube_model_cubes(model_id) -> Vec<BuiltRecord>`;
  - `set_cube_model_cubes(model_id, &[BuiltRecord])` replaces them all in one transaction.

**Migration** (the file runs on every start, so each statement must be idempotent):

```sql
CREATE TABLE IF NOT EXISTS cube_model_cubes (
    model_id uuid NOT NULL REFERENCES cube_models (id) ON DELETE CASCADE,
    cube_id text NOT NULL,
    cube_name text NOT NULL,
    file_id uuid NOT NULL,
    built_hash text NOT NULL,
    PRIMARY KEY (model_id, cube_id)
);
INSERT INTO cube_model_cubes (model_id, cube_id, cube_name, file_id, built_hash)
    SELECT id, 'c1', cube_name, cube_file_id, cube_built_hash FROM cube_models
    WHERE cube_name IS NOT NULL AND cube_file_id IS NOT NULL AND cube_built_hash IS NOT NULL
    ON CONFLICT DO NOTHING;
UPDATE cube_models SET cube_file_id = NULL, cube_built_hash = NULL WHERE cube_file_id IS NOT NULL;
```

`cube_models.cube_name` stays: it names the converted cube. Nothing else writes `cube_models.cube_*` any more.

**Build, inside the single `writer.write` closure:**
1. Read every cube's file: the record's `file_id`, or else a file at `<cubeName>.yml` that carries this model's header. The record is lost in that case, so it counts as edited.
2. **Edited:** a found file whose sha differs from its record answers 409 `EDITED` unless `force`. The same applies to the file of a record whose cube is no longer in the model.
3. **Name collision:** if any organization file other than this model's cube files declares a cube with the same name (read `cubes[].name` from its YAML, as `model_sync` does), answer 409 `"O cubo {name} já existe em {path}, fora deste model"`. `force` does not override this.
4. **Path taken:** a new cube whose `<cubeName>.yml` belongs to another file keeps the existing 409 message.
5. **Changes:** `Put` each cube over its file, or create it. `Delete` the file of each cube no longer in the model, with its `expected_version`. A record whose file is already gone is just dropped.
6. **No cubes and no records** → 400 `"Crie um cubo antes do build"`.

After the write, read the rows back by path and set the records.

- [ ] **Step 1: Write failing tests** (adapt the existing build tests to `cubes[0].cubeName` and `cubeNames`, and keep their assertions):
  - `a_model_saved_before_cubes_opens_converted`: create with `model_def`; GET shows `cubes[0] = {id:"c1", name:<snake of the model name>, …}` and no `primary`.
  - `two_cubes_build_two_files`, then `removing_a_cube_removes_its_file`: `removed` names it, and the file is gone from the organization.
  - `removing_an_edited_cube_needs_force`.
  - `a_cube_named_like_a_foreign_cube_is_refused`: a hand-written file `x.yml` declares cube `orders`, the model has a cube `orders` and another cube. Both cubes are refused with the message above, and no file is written.
  - `an_old_build_record_moves_to_cube_model_cubes`: set `cube_name`, `cube_file_id` and `cube_built_hash` on a model by SQL, as the old build wrote them, and run the migration again. The model then rebuilds over the same file with no 409, and `cube_models.cube_file_id` is null.
- [ ] **Step 2:** Run `cargo test -p cubeserver --test model_api` with the test DB and check that the new tests fail.
- [ ] **Step 3: Implement.** The migration runner is whatever runs `migrations.sql` today. Keep `deleting_the_model_keeps_the_cube` true: the cascade drops records, not files.
- [ ] **Step 4:** Run the `model_api` suite, then `cargo test -p cubeserver --lib`. Check that both pass.
- [ ] **Step 5:** Commit `feat(models): build records per cube; build writes and removes every cube file`.

### Task 4: Client model — cube types, cubes module, clipboard

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/models/modelGraph.ts`:
  - types;
  - remove `primary`/`primaryKey`, `setPrimary` and `unlinked`;
  - `addTable`, `removeTable` and `renameTable` stop touching `primary`, and keep cube sources and members.
- Create: `packages/cubejs-playground/src/pages/Schema/models/cubes.ts`
- Modify: `objects.ts` (kind `cube`), `clipboard.ts`, `physical.ts` (key lookups), `src/pages/Schema/api.ts` (model view `cubeNames`, build answer).
- Test: create `src/__tests__/model-cubes.test.ts`; extend `model-objects`, `model-clipboard` and `model-graph` tests; fix the fixtures that set `primary`.

**Interfaces — Produces (TS, `modelGraph.ts`):** `CubeSourceKind`, `CubeSource`, `CubeDimension`, `CubeMeasure`, `CubeSegment`, `CubeNode` and `CubeJoin`, field for field as Task 1, with `type` for `kind`. `ModelDefinition` + `cubes?: CubeNode[]; cubeJoins?: CubeJoin[]`. Export the constants `DIMENSION_TYPES`, `MEASURE_TYPES`, `MEASURE_FORMATS` and `SOURCE_KINDS`.

**Interfaces — Produces (`cubes.ts`):**
- `addCube(def, x, y): { def; id }`: name `cube_<k>`, free among cubes; color `violet`; source `{kind:'table', table: <first table alias or none>}`.
- `setCube(def, id, patch)`: a patch value of `undefined` removes the key.
- `renameCube(def, id, next): { def; error?: string }`: `Invalid name: x`, `x is already in this model`.
- `setCubeSource(def, id, source)`: when the kind or table changes, a `column` member that no longer resolves keeps its name and loses nothing. The client validation flags it (Task 5).
- `addMember(def, id, group: 'dimensions'|'measures'|'segments'): { def; name }`: names `dimension_<k>`, `measure_<k>` and `segment_<k>`. The defaults are type `string`, type `count`, and sql `1 = 1`.
- `setMember(def, id, group, index, patch)`, `removeMember(def, id, group, index)`.
- `cubeReads(def, cube): string[]`:
  - `table` gives `[table]`;
  - `joined` gives the aliases reached from `table` over `def.joins`, primary first, the same walk as the server;
  - `sql` gives `source.tables`.
- `snake(name)` and `cubeType(rawType)`: ports of `scaffold::snake` and `scaffold::cube_type`, same outputs.
- `autoMembers(def, cube, columnsOf): { dimensions: CubeDimension[]; measures: CubeMeasure[] }`: the members the server's auto mode writes, with the same names, and `column` set (`alias.column` in `joined`). `sql` cubes give none.
- `generateMembers(def, id, columnsOf)`: replaces the cube's members with `autoMembers`. That is the "Generate members" button.
- `dimensionNames(def, cube, columnsOf): string[]`: the explicit dimensions, or the auto names in auto mode. Join pairs pick from this list.
- `addCubeJoin(def, from, to, relationship, on): { def; id }`, `setCubeJoin(def, id, patch)`, `removeCubeJoin(def, id)`.
- `cubeKeyOf(def, alias): string | undefined`: the `primaryKey` of a `table` or `joined` cube whose `source.table` is `alias`. This replaces `def.primary === alias && def.primaryKey` in `physical.ts`, `ModelEditor` and `TableNode`.

**`objects.ts`:**
- `ObjectKind` + `'cube'`;
- `nodeIdOf('cube', id)` gives `cube:<id>`;
- `removeObjects` removes cubes and every cube join that touches them;
- `placeObjects` places cubes with the view rules (`null` size frees);
- `nodeIdsOf` includes cubes;
- `previewIdOf` ignores cubes.

**`modelGraph.ts`:**
- `renameTable` renames `source.table`, `source.tables` and the `alias.` prefix of `joined` columns;
- `removeTable` clears a matching `source.table` and drops it from `source.tables`.

**`clipboard.ts`:**
- `Clip` + `cubes`, `cubeJoins`;
- pasted cubes get new ids and free names;
- sources follow renamed pasted tables;
- a cube join is pasted only when both its cubes are.

- [ ] **Step 1: Write failing tests** in `model-cubes.test.ts`:
  - `snake and cubeType match the server`: cases `fiberCableId`→`fiber_cable_id`, `Order ID`→`order_id`, `1a`→`c_1a`, `""`→`col`; `int4`→`number`, `timestamptz`→`time`, `jsonb`→none, `Nullable(String)`→`string`;
  - `autoMembers of a joined cube`: the `orders`/`customers` fixture gives dimension names `orders_id, orders_customer_id, orders_amount, customers_id, customers_region`, and a `count`;
  - `generateMembers makes them explicit`, and `dimensionNames` is the same before and after;
  - `cubeReads walks references from the primary`;
  - `adds cubes and cube joins with the smallest free id`;
  - `renaming a table renames cube sources and joined columns`;
  - `removing a table clears the cube source`;
  - `removing a cube removes its cube joins`.

  In `model-clipboard.test.ts`:
  - `pasting two joined cubes keeps their join`;
  - `pasting one of them drops it`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-cubes.test.ts src/__tests__/model-clipboard.test.ts` (in `packages/cubejs-playground`) and check that they fail.
- [ ] **Step 3:** Implement. Remove the "Principal"/"Primary key" fields from the model pane in `PropertiesPanel.tsx` here, since their functions are gone; cubes take that role.
- [ ] **Step 4:** Run the full vitest, `node_modules/.bin/tsc --noEmit -p .` and `../../node_modules/.bin/oxlint src/pages/Schema/models src/__tests__`. Check that they are green. The `loose` banner and its build guard in `ModelEditor` go away here too: tsc forces it.
- [ ] **Step 5:** Commit `feat(models): cubes and cube joins in the client model`.

### Task 5: Client validation and search for cubes

**Files:**
- Modify: `validate.ts` (`IssueTarget` + `cube`, `cubeJoin`), `search.ts`
- Test: extend `model-validate` and `model-search` tests

**Interfaces:**
- Consumes: Task 4.
- Produces:
  - issue rules mirroring every server rule of Tasks 1–2 that the client can see: names, uniqueness, types, sql-or-column, source table missing, `column` not in the read tables, an `sql` cube without members, a join pair outside `dimensionNames`, a duplicate join;
  - search hits for cube names, titles and member names, which select the cube.

- [ ] **Step 1: Write failing tests**, one per rule. The message texts are English and follow the existing rule style.
- [ ] **Step 2:** Run the two test files and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the full suite (vitest, tsc, oxlint) and check that it is green.
- [ ] **Step 5:** Commit `feat(models): validation and search for cubes`.

### Task 6: Canvas — cube node, cube join edge, source lines

**Files:**
- Create: `CubeNode.tsx`, `CubeJoinEdge.tsx`
- Test: extend `model-editor.test.ts` (render)

**Interfaces:**
- Consumes: Task 4 types; `NODE_HANDLE_STYLE` from `nodeHandle.ts`; `DependencyEdge` from phase 3.
- Produces:
  - `CubeNode` data `{ cube: CubeNode; dimensions: string[]; measures: string[]; issues: number; onResize }`:
    - the table box design: a header in `var(--model-<color>)` with the lucide `Box` icon and the name;
    - two sections, "Dimensions" and "Measures", each with the member names (the auto names in auto mode, in italic, with the hint "generated at build");
    - the invisible `node` handle, plus source and target handles for tool 4.
  - `CubeJoinEdge`:
    - the `ReferenceEdge` crow's-foot drawing and orthogonal path, reused, not copied;
    - its label is the relationship's existing `REL_LABEL` text;
    - it is selectable.
  - Source lines: one `DependencyEdge` from each cube to each alias of `cubeReads`.

- [ ] **Step 1: Write failing tests:**
  - `a cube renders its dimensions and measures`;
  - `an auto cube shows generated members in italic`;
  - `a cube join is drawn with its label`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-editor.test.ts` and check that they fail.
- [ ] **Step 3:** Implement. If reusing the `ReferenceEdge` drawing needs it split, extract the shared part into `ReferenceEdge.tsx` exports.
- [ ] **Step 4:** Run the full suite and check that it is green.
- [ ] **Step 5:** Commit `feat(models): cube nodes and cube join edges`.

### Task 7: Editor wiring — tool 8, cube joins, build

**Files:**
- Modify: `ModelEditor.tsx`, `ToolPalette.tsx` (cube enabled), `ModelToolbar.tsx` (Build enabled when the model has cubes or `cubeNames`), `PropertiesPanel.tsx` (`Selected` kinds `cube`, `cubeJoin`), `src/pages/Schema/SchemaPage.tsx` (model list and after-build toast)
- Create: `CubeJoinDialog.tsx`
- Test: extend `model-editor.test.ts`

**Behavior:**
- Tool 8, then a pane click, places a cube (`addCube`) and selects it.
- Tool 4 dragged from a cube to another cube opens `CubeJoinDialog`:
  - relationship, default `many_to_one`;
  - one or more pairs of dimension selects from `dimensionNames`;
  - OK runs `addCubeJoin`.

  Tool 4 between a cube and a table does nothing. Tool 4 between tables is unchanged.
- Select, move, resize, delete, copy, cut, paste, Ctrl+A, align and undo cover cubes and cube joins, through `objects.ts`.
- Build:
  - saves first, as today;
  - takes the new answer, and the toast reads `Built cubes a, b` (plus `; removed x.yml` when files were removed);
  - the Open cube action opens the first built cube;
  - a 409 with the edit message asks to overwrite, as today, and other 409s show their message;
  - `SchemaPage` lists `cube a, b` or `not built yet` from `cubeNames`.
- `onPickIssue` selects `cube:<id>` or the cube join's edge.

- [ ] **Step 1: Write failing tests:**
  - `tool 8 places a cube`;
  - `tool 4 between two cubes creates a join after the dialog`;
  - `deleting a cube removes it and its joins, and one undo brings both back`;
  - `build shows the built cubes`, with the API mocked to answer two cubes and one removed file;
  - `the overwrite question still appears on the edit 409`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-editor.test.ts` and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the full suite and check that it is green.
- [ ] **Step 5:** Commit `feat(models): place cubes, join them and build them from the editor`.

### Task 8: Properties panes and structure tree

**Files:**
- Create: `CubeProperties.tsx`, `CubeJoinProperties.tsx`
- Modify: `PropertiesPanel.tsx` (dispatch), `StructurePanel.tsx` (groups "Cubes" and "Cube joins")
- Test: extend `model-editor.test.ts`

**Behavior:**
- **Cube properties:**
  - **General:** name (renameCube, with the error shown), title and description;
  - **Source:**
    - a kind select;
    - for `table` and `joined`: a table select, and a primary key column select;
    - for `sql`: a SQL textarea and a "Reads from" checklist of tables;
  - **Dimensions**, **Measures** and **Segments:** rows with name, type (or format), column select or SQL, primary key (dimensions) and title, plus add and remove. The column select lists the columns of the read tables (`alias.column` in `joined`);
  - **"Generate members":** disabled for `sql` cubes. It asks before replacing existing members;
  - **Format:** color;
  - **Issues:** the cube's issues.
- **Cube join properties:** from and to cubes (read-only), relationship, and the pairs (editable, add and remove).
- **Structure tree:** "Cubes" lists cubes by name, with their members under each. "Cube joins" lists `from → to`. A click selects.

- [ ] **Step 1: Write failing tests:**
  - `renaming a cube in its pane`;
  - `changing the source kind to sql shows the SQL editor`;
  - `Generate members fills dimensions and a count`;
  - `adding and removing a measure`;
  - `typing in a member name does not delete the cube`;
  - `the structure tree lists cubes and cube joins and selects on click`.
- [ ] **Step 2:** Run the editor tests and check that the new ones fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the full suite (vitest, tsc, oxlint) and check that it is green.
- [ ] **Step 5:** Commit `feat(models): cube and cube join properties and structure`.

### Task 9: Build, install, browser check

- [ ] **Step 1:** Back up the server binary and the Playground as `*.bak-20261126-redgate4` (see the deploy memory).
- [ ] **Step 2:** Build the server (release) and swap the binary (`cp` to `.new`, then `mv -f`). Build and install the Playground, then run `./start-all.sh restart`. Check that `/readyz` answers 200 and that the server log shows the migration ran with no error.
- [ ] **Step 3: Browser check** with playwright, using `page.route` mocks for every model read and write. Never touch the user's real models, and never click "Create in database" or a real Build: the real build is covered by the Task 3 API tests. Expected, with no page errors:
  - an existing model opens with one cube named after its old cube;
  - tool 8 places a cube; set its source to a table; Generate members fills it;
  - tool 4 between the two cubes creates a join, drawn with its label;
  - the dashed lines go from each cube to its tables;
  - one Ctrl+Z after deleting a cube brings it back;
  - save, reload: everything is still there;
  - Build, with the build answer mocked as two cubes and one removed file, shows the toast `Built cubes a, b; removed x.yml`;
  - take light and dark screenshots.
- [ ] **Step 4:** Commit any fixes. Do not push without being asked.

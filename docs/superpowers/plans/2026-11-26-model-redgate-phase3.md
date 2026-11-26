# Model estilo Redgate — Fase 3 (notas, áreas e views) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Text notes, subject areas and views on the model canvas, as Redgate has them:
- notes, areas and views are created with tools 5, 6 and 7;
- they are edited in the properties pane, listed in the structure tree and found by search;
- copy, paste, cut, delete, arrange and undo work on them;
- moving an area carries what lies entirely inside it;
- views enter Generate SQL as `CREATE VIEW`, and their dependencies are dashed lines.

**Architecture:**
- The definition gains three optional arrays: `notes`, `areas` and `views`. The cube build ignores them.
- Canvas node ids get a kind prefix (`note:n1`, `area:a1`, `view:v1`). A table's node id stays its alias, or `alias~shortcut` for a shortcut. Aliases cannot hold `:`, so the two forms never clash.
- A new pure module `objects.ts` owns:
  - the ids and the node-id parsing;
  - create, update and delete of the three kinds;
  - placement, z-order and area containment.
- The editor routes every move, size and delete through `objects.ts`, which falls back to the existing table functions for tables.
- DDL: `ddl::script` writes the views after the FKs, ordered by their view dependencies. "Create in database" stays tables-only (spec Fase 2).

**Tech Stack:** as phases 1–2: React 18, `@xyflow/react` 12.3.6 (`NodeResizer`, `BaseEdge`), Tailwind `tw:` + shadcn, lucide, vitest + jsdom; Rust (serde) in `cubeserver`.

**Spec:** `docs/superpowers/specs/2026-11-26-model-redgate-design.md` (Fase 3; Fase 1 "Order" decision; Fase 2 "Gerar SQL" mentions views; Restrições "SQL em texto livre").

## Global Constraints

- **Ids:**
  - notes `n<k>`, areas `a<k>`, views `v<k>`, each with the smallest free `k ≥ 1` in its own kind;
  - the node ids are `note:<id>`, `area:<id>` and `view:<id>`.
- **Names:**
  - a view name and a view schema follow `^[A-Za-z_][A-Za-z0-9_]*$`, ≤ 63 chars;
  - view names are unique among views, and a view may not share `schema.name` with a draft table.
- **Free text:** note text, area and view comments, the view query and the view scripts are unrestricted. They run only in "Create in database", which does not run views in this phase.
- **Colors:** only palette names from `TABLE_COLORS`, drawn with `var(--model-<color>)`. Never a hex in a component.
  - area fill: `color-mix(in oklch, var(--model-<fill>) 12%, var(--card))`, default fill `slate`;
  - area border: `var(--model-<border>)`, 1.5px dashed, default border `slate`;
  - view: background `var(--model-view)`, border `var(--model-view-border)`. Both tokens are added to `src/shadcn.css` next to `--model-*`, with a `--pg-` fallback, and redefined under `:root[data-theme="dark"]`;
  - note: background `var(--model-note)` (same treatment), or a palette color at 18% when chosen.
- **Z-order:**
  - areas are always behind everything (React Flow `zIndex = -1000 + z`);
  - notes and views use `zIndex = z`, tables `0`;
  - Order › Bring to front / Send to back apply to notes and areas only, and stay disabled for anything else (Redgate).
- **Default sizes:** note 200×100; area 400×300; view auto (no fixed size), like a table.
- **UI labels (English, Redgate):**
  - pane titles: "Text note properties", "Subject area properties", "View properties";
  - sections: "General", "SQL query", "Columns", "Dependencies", "Additional SQL scripts", "Format", "Issues";
  - structure groups as today ("Text notes", "Views", "Subject areas").
- **Server messages:** Portuguese.
- **Build:** the cube YAML of every model is byte-identical. The existing `cube_model::build` tests and `ddl` tests pass unchanged.

## Review Focus

1. **Deleting or renaming a table or view that a view depends on:** the dependency follows the rename or drops; it never points at a missing object, and the server never refuses the next save. Test in Task 3.
2. **Dragging an area that contains a table and a shortcut:** both move by the same delta in ONE undo step. An object only partly inside stays put. Test in Task 3 (pure) and Task 6 (editor).
3. **Views that depend on each other:** Generate SQL writes them in dependency order. A cycle is refused with a message, not looped forever. Test in Task 2.
4. **Typing in a note's text, a view's query or a column name:** the canvas shortcuts (Delete, digits, Ctrl+A/C/V) don't fire. Test in Task 7.
5. **An old model without notes, areas and views:**
   - opens with empty groups;
   - saves without writing empty arrays (Rust `skip_serializing_if = "Vec::is_empty"`);
   - builds the same YAML.
   Test in Task 1.

---

### Task 1: Server definition — notes, areas, views

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/cube_model.rs`:
  - new structs;
  - `Definition` fields;
  - `validate` (after the reference checks, keeping every existing error and its order);
  - `validate_types` (view columns).
- Test: `cube_model.rs` tests.

**Interfaces — Produces (JSON camelCase; all arrays default empty and are skipped when empty):**
- `Definition` + `notes: Vec<Note>`, `areas: Vec<Area>`, `views: Vec<ModelView>`.
- `Note { id, x, y, width?, height?, z?: i32, text: String, color?: String }`.
- `Area { id, name, comment?, x, y, width, height, z?: i32, fill?: String, border?: String }`.
- `ModelView`:
  - `{ id, name, schema, comment?, sql: String, columns: Vec<ViewColumn>, depends_on_tables: Vec<String>, depends_on_views: Vec<String>, script_before?, script_after?, x, y, width?, height?, z?: i32 }`;
  - JSON field names: `dependsOnTables`, `dependsOnViews`, `scriptBefore`, `scriptAfter`.
- `ViewColumn { name, type }` (`#[serde(rename = "type")] kind`).
- Every struct gets `deny_unknown_fields` and `Default`.

- [ ] **Step 1: Write failing tests.**
  - `accepts_notes_areas_views`: round-trips a definition with one of each, and serializes an old definition with no `notes`/`areas`/`views` keys.
  - `refuses_bad_objects`, one assertion each:
    - a repeated id within a kind → `"O objeto {id} se repete"`;
    - an area with width 0 → `"Tamanho inválido em {id}: 0"`;
    - a bad color in fill, border or note color → `"Cor inválida: {c}"`;
    - a view name `"1x"` → `"Nome de view inválido: 1x"`;
    - two views named `v` in the same schema → `"A view {schema}.{name} se repete"`;
    - a view named like a draft table in the same schema → the same message;
    - a dependency on a missing alias → `"A view {name} depende de {dep}, que não está no model"`;
    - a dependency on a missing view id, or on itself → the same message.
  - `validate_types_checks_view_columns`: a postgres model with a view column `jsonb` passes, and `int` fails with `"Tipo inválido para {view}.{column}: int"`.
- [ ] **Step 2:** Run `cd rust/cube && cargo test -p cubeserver --lib rbac::cube_model` and check that it fails.
- [ ] **Step 3: Implement** the structs, the checks and the `validate_types` loop over view columns. Use the same `valid_ident` and `COLORS` as tables, and the same finiteness check as table sizes.
- [ ] **Step 4:** Run the same command. Then load `.env` as the deploy memory says and run `cargo test -p cubeserver --test model_api 2>&1 | tail -3`. Check that both pass.
- [ ] **Step 5:** Commit `feat(models): notes, areas and views in the model definition`.

### Task 2: Views in Generate SQL

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/ddl.rs` (`script`, new `view_order`, `view_create`)
- Test: `ddl.rs` tests

**Interfaces:**
- Consumes: Task 1 `ModelView`.
- Produces:
  - `fn view_order(views: &[&ModelView]) -> Result<Vec<&ModelView>, String>`: a topological order over `depends_on_views` that keeps the definition order among independent views. A cycle errors with `"As views {a}, {b} dependem umas das outras"`, naming the views left, sorted.
  - `ScriptOptions.only` may also hold view ids. A view is included when `only` is none or contains its id; `only_new` keeps every view, since views are always the model's own.

- [ ] **Step 1: Write failing tests.**
  - `views_after_foreign_keys`: a postgres model with a table, an FK and a view. The script has `CREATE VIEW "shop"."v_orders" AS select 1;` after the `ALTER TABLE … FOREIGN KEY`, and before `createAfter`. The view's `scriptBefore` and `scriptAfter` surround it.
  - `views_in_dependency_order`: v2 depends on v1, and v1 is listed second; v1 comes first.
  - `view_cycle_is_refused`: v1 ↔ v2 → `Err("As views v1, v2 dependem umas das outras")`.
  - `drop_views_first`: with `drop`, `DROP VIEW IF EXISTS` comes in reverse order before the FK drops.
  - `view_comment`: postgres writes `COMMENT ON VIEW … IS '…'` when comments are on; mysql writes none.
  - `only_selects_views`: `only: ["v1"]` writes only v1, and no table.
  - A view query ending in `;` or in whitespace is written once with a single `;` (the existing `push` trims).
- [ ] **Step 2:** Run `cd rust/cube && cargo test -p cubeserver --lib rbac::ddl` and check that it fails.
- [ ] **Step 3: Implement.**
  - The view is `CREATE VIEW <qualified(schema, name)> AS <sql trimmed>`, using the existing quoting for the dialect.
  - `create_statements` is unchanged: Create in database stays tables-only.
- [ ] **Step 4:** Run the same command. Check that every test passes, the existing ones unchanged.
- [ ] **Step 5:** Commit `feat(models): views in Generate SQL`.

### Task 3: Client model — types, objects module, dependencies, clipboard

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/models/modelGraph.ts`:
  - types;
  - `removeTable` and `renameTable` keep view dependencies.
- Create: `packages/cubejs-playground/src/pages/Schema/models/objects.ts`
- Modify: `packages/cubejs-playground/src/pages/Schema/models/clipboard.ts`
- Test:
  - create `src/__tests__/model-objects.test.ts`;
  - extend `src/__tests__/model-clipboard.test.ts`.

**Interfaces — Produces (TS, in `modelGraph.ts`):**
- `Note = { id: string; x: number; y: number; width?: number; height?: number; z?: number; text: string; color?: TableColor }`.
- `Area = { id: string; name: string; comment?: string; x: number; y: number; width: number; height: number; z?: number; fill?: TableColor; border?: TableColor }`.
- `ViewColumn = { name: string; type: string }`.
- `ModelView = { id; name; schema; comment?; sql: string; columns: ViewColumn[]; dependsOnTables: string[]; dependsOnViews: string[]; scriptBefore?; scriptAfter?; x; y; width?; height?; z? }`.
- `ModelDefinition` + `notes?: Note[]; areas?: Area[]; views?: ModelView[]`.

**Interfaces — Produces (`objects.ts`):**
- `type ObjectKind = 'table' | 'note' | 'area' | 'view'`.
- `objectOf(nodeId: string): { kind: ObjectKind; id: string; shortcut?: string }`: `note:n1` gives `{kind:'note', id:'n1'}`; `orders~s1` gives `{kind:'table', id:'orders', shortcut:'s1'}`.
- `nodeIdOf(kind: Exclude<ObjectKind,'table'>, id: string): string`.
- `addNote(def, x, y): { def; id }`.
- `addArea(def, x, y): { def; id }`: name `Area_<k>`.
- `addView(def, x, y, schema: string): { def; id }`: name `view_<k>`, free among views. The editor passes the schema of the first table, or `public` when there is none.
- `setNote(def, id, patch: Partial<Omit<Note,'id'>>)`, `setArea(...)`, `setView(...)`. A patch value of `undefined` removes the key.
- `renameView(def, id, next): { def; error?: string }`: same messages as `renameTable` (`Invalid name: x`, `x is already in this model`).
- `removeObjects(def, nodeIds: string[]): ModelDefinition`:
  - tables go through `removeTable`, shortcuts through `removeShortcut`;
  - a removed view leaves every `dependsOnViews`.
- `placeObjects(def, places: Map<string, Place>): ModelDefinition`:
  - keyed by node id;
  - tables and shortcuts go through `placeTables`;
  - a `null` size removes `width`/`height` on notes and views, and is ignored on areas (their size is required).
- `bringToFront(def, nodeIds)` and `sendToBack(def, nodeIds)`:
  - only notes and areas;
  - `z` becomes the max (or min) z of the same kind ±1, keeping the order among the moved ones.
- `insideArea(area: Box, boxes: Box[]): string[]`: the ids of the boxes entirely within `area`, edges included; the area's own id excluded.
- `nodeIdsOf(def): string[]`: every canvas node id, for Ctrl+A.

`modelGraph.ts`:
- `removeTable` drops the alias from every view's `dependsOnTables`;
- `renameTable` renames it there.

`clipboard.ts`:
- `Clip` gains `notes: Note[]; areas: Area[]; views: ModelView[]`;
- `copy` and `cut` take node ids of any kind; `cut` uses `removeObjects`;
- `paste` gives the pasted objects new ids, shifted by the offset:
  - a pasted view's name is freed like a table's (`v` becomes `v_1`);
  - its `dependsOnTables` follow pasted tables that were renamed, and its `dependsOnViews` keep only views pasted with it, remapped to their new ids;
- `paste` returns `nodeIds: string[]` in place of `aliases`. The editor's single caller is updated in Task 6.

- [ ] **Step 1: Write failing tests** in `model-objects.test.ts`:
  - `objectOf parses every kind of node id`;
  - `adds notes, areas and views with the smallest free id`, also after deleting `n1` of `n1,n2`;
  - `renaming a table renames it in view dependencies`;
  - `removing a table or view drops it from view dependencies`;
  - `placeObjects moves tables, shortcuts, notes, areas and views in one call`;
  - `a null size frees a note but keeps an area's size`;
  - `bring to front and send to back reorder only notes and areas`;
  - `insideArea takes only boxes entirely inside`, including one touching the edge.

  In `model-clipboard.test.ts`:
  - `copies and pastes a view with the table it depends on`: the dependency points at the pasted table's new alias;
  - `pasting a view alone drops view dependencies on views not pasted`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-objects.test.ts src/__tests__/model-clipboard.test.ts` (in `packages/cubejs-playground`) and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same tests and `node_modules/.bin/tsc --noEmit -p .`. Check that they pass.
- [ ] **Step 5:** Commit `feat(models): notes, areas and views in the client model`.

### Task 4: Validation and search for the new objects

**Files:**
- Modify: `validate.ts`, `search.ts`
- Test: `src/__tests__/model-validate.test.ts`, `src/__tests__/model-search.test.ts`

**Interfaces:**
- `IssueTarget` + `{ kind: 'view'; id: string }`.
- `Group` + `'View' | 'View column'`.
- New `RULES`, in this order after the reference rules:
  - `view.emptyName` "Name can't be empty" (error);
  - `view.uniqueName` "Unique name" (error);
  - `view.emptyQuery` "Query can't be empty" (error);
  - `view.invalidName` "Invalid name" (warning);
  - `view.nameLength` "Name length" (warning);
  - `viewColumn.emptyName` "Name can't be empty" (error);
  - `viewColumn.uniqueName` "Unique name" (error);
  - `viewColumn.emptyType` "Empty type" (error);
  - `viewColumn.supportedType` "Supported database type" (warning, `dialect: true`).
- `Hit.kind` + `'note' | 'area' | 'view'`. Notes match on text (the label is its first 40 chars), areas on name, and views on name and on their columns (`view.column`). `nodeId` is the prefixed node id.

- [ ] **Step 1: Write failing tests:**
  - `flags a view with no query and a repeated column`;
  - `a dialect-less model gives no view column type issue`;
  - `a disabled view rule gives no issue`;
  - `search finds notes, areas, views and view columns`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-validate.test.ts src/__tests__/model-search.test.ts` and check that it fails.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same command and check that it passes.
- [ ] **Step 5:** Commit `feat(models): validate and search views, notes and areas`.

### Task 5: Canvas nodes and the dependency line

**Files:**
- Create: `NoteNode.tsx`, `AreaNode.tsx`, `ViewNode.tsx`, `DependencyEdge.tsx` (all in `models/`)
- Modify:
  - `TableNode.tsx`: one invisible node-level handle, id `node`, both source and target;
  - `src/shadcn.css`: `--model-view`, `--model-view-border` and `--model-note`, light and dark.
- Test: `src/__tests__/model-editor.test.ts` (node render tests, in the style of the existing "table node" tests)

**Interfaces — Produces:**
- `NoteFlowNode = Node<{ id; text; color?; onResize }, 'note'>`:
  - the text is shown with line breaks;
  - `NodeResizer` when selected.
- `AreaFlowNode = Node<{ id; name; fill?; border?; onResize }, 'area'>`:
  - a dashed rectangle with the name at the bottom-right corner;
  - `NodeResizer` when selected.
- `ViewFlowNode = Node<{ id; name; columns: ViewColumn[]; showTypes: boolean; fixedSize: boolean; onResize }, 'view'>`:
  - a rounded box with the name centred in bold in the header and one row per column (name and type), like `TableNode`;
  - the `node` handle.
- `DependencyFlowEdge = Edge<{}, 'dependency'>`:
  - a dashed `BaseEdge`, stroke `var(--muted-foreground)`;
  - from the view's `node` handle to the table's or view's `node` handle;
  - not selectable.
- `onResize(nodeId, box)` has the same signature as `TableNodeData.onResize`.

- [ ] **Step 1: Write failing tests:**
  - `note node shows its text`;
  - `area node shows its name and is dashed`;
  - `view node lists its columns`;
  - `table node has a node handle`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-editor.test.ts -t "node"` and check that it fails.
- [ ] **Step 3:** Implement, colors per Global Constraints.
- [ ] **Step 4:** Run the same command and check that it passes.
- [ ] **Step 5:** Commit `feat(models): note, area and view nodes`.

### Task 6: Editor wiring — tools, drag, order, edit commands

**Files:**
- Modify: `ModelEditor.tsx`, `ToolPalette.tsx`, `ModelToolbar.tsx`
- Test: `src/__tests__/model-editor.test.ts`

**Interfaces:**
- Consumes: Tasks 3–5.
- `ToolPalette`: `view`, `note` and `area` lose `soon`; `cube` keeps it.
- Tool number keys are `String(TOOLS.indexOf(t) + 1)` for every tool without `soon`: today's index runs over the filtered list, which would shift keys once one in the middle is enabled.
- `ModelToolbar`: `arrange.order: { canOrder: boolean; onFront(): void; onBack(): void }`. The Order menu holds "Bring to front" and "Send to back", enabled when the selection holds a note or an area.

Behaviour:
- **Tools:** with tool view, note or area active, a click on the empty canvas:
  - creates the object at `screenToFlowPosition` of the click, rounded;
  - selects it;
  - goes back to Select (`onPaneClick`).
  The click must not also clear the new selection.
- **Nodes:**
  - every node is built from `def` like the table nodes, keeping React Flow's measured fields;
  - `zIndex` follows Global Constraints;
  - the edges get one dependency edge per view dependency that exists.
- **Area drag:**
  - `onNodeDragStart` on an area node records `insideArea(area, every other node's box)`;
  - `onNodeDrag` moves those nodes in `nodes` state by the area's delta;
  - `onNodeDragStop` commits the area plus those nodes with ONE `placeObjects` call, so the whole drag is a single `change`.
- **Everything else moves through `placeObjects`:** the plain drag, resize, Align, Distribute and Size (Same width and Same height also apply to notes, areas and views; "Adjust to text" frees notes and views).
- **Edit commands:**
  - delete, cut, copy and paste go through Task 3's functions;
  - Ctrl+A uses `nodeIdsOf`;
  - a paste selects `nodeIds`.
- **Double-click** on a note or a view selects it and bumps `focusKey`, as a draft table does.
- **`propertiesOf`** follows `objectOf(selection[0])`: a note, an area or a view. `Selected` gains `{kind:'note'|'area'|'view'; id}`.
- **Generate SQL "only the selection"** sends table aliases and view ids.
- **Problems:** a `view` issue picks `view:<id>`.

- [ ] **Step 1: Write failing tests:**
  - `the note tool places a note where the canvas is clicked`;
  - `the area and view tools place theirs and select them`;
  - `dragging an area moves what is inside it in one undo step`, using the existing drag helpers of the suite;
  - `bring to front raises a note above another`;
  - `delete removes a selected view and its dependency line`;
  - `copy and paste a note`;
  - `keys 5, 6, 7 choose the view, note and area tools, and 8 does nothing`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-editor.test.ts` and check that the new tests fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the full suite: `node_modules/.bin/vitest run`, `node_modules/.bin/tsc --noEmit -p .`, `../../node_modules/.bin/oxlint src/pages/Schema/models src/__tests__`. All must be green.
- [ ] **Step 5:** Commit `feat(models): place, move, order and edit notes, areas and views`.

### Task 7: Properties panes and the structure tree

**Files:**
- Create: `NoteProperties.tsx`, `AreaProperties.tsx`, `ViewProperties.tsx`
- Modify: `PropertiesPanel.tsx` (titles, dispatch, model size line), `StructurePanel.tsx` (the three groups)
- Test: `src/__tests__/model-editor.test.ts`

**Interfaces:**
- Each pane takes `{ def; id; onChange(next: ModelDefinition): void; focusKey?: number }`. The view pane also takes:
  - `issues: Issue[]`;
  - `dialect: Dialect`;
  - `columnsOf`;
  - `onSqlPreview?(id: string)`, which opens Generate SQL with `only: [id]`;
  - `onRenamed(from: string, to: string)`, so the selection follows.
- **Text note properties:**
  - "General": the text (a multi-line `DraftText`, focused on `focusKey`);
  - "Format": color (the palette swatches of `TableProperties`, plus "none").
- **Subject area properties:**
  - "General": name and comment;
  - "Format": fill color and border color (two swatch rows).
- **View properties:**
  - a "SQL preview" link at the top, when there is a dialect;
  - "General": name, schema, comment;
  - "SQL query": the `CodeEditor` from `../NumberedCode`, committed on blur as one undo step;
  - "Columns": a grid of name and type, with "+ Add" and a remove button per row. The type input lists the dialect types (as the draft columns do), or the 7 portable types when there is no dialect;
  - "Dependencies": a check box per table alias and per other view;
  - "Additional SQL scripts": before and after;
  - "Issues": the view's issues.
- **Model properties "Size":** `N tables, N views, N notes, N areas`; zero counts left out, except tables.
- **Structure tree:**
  - "Text notes", "Views" and "Subject areas" list their objects (note by its first 40 chars, or `Note n1` when empty);
  - a click picks the node;
  - views expand to their columns as tables do.

- [ ] **Step 1: Write failing tests:**
  - `edits a note's text and color from its properties`;
  - `renames a view and the selection follows`;
  - `sets a view's query, a column and a dependency, and the dashed line appears`;
  - `typing Delete in the view query does not delete the view`;
  - `the structure tree lists notes, views and areas and selects on click`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-editor.test.ts` and check that the new tests fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the full suite (vitest, tsc, oxlint, as in Task 6) and check that it is green.
- [ ] **Step 5:** Commit `feat(models): note, area and view properties and structure`.

### Task 8: Build, install, browser check

- [ ] **Step 1:** Back up the server binary and the Playground as `*.bak-20261126-redgate3` (see the deploy memory).
- [ ] **Step 2:** Build the server (release), swap the binary (`cp` to `.new`, then `mv -f`), build and install the Playground, then run `./start-all.sh restart`.
- [ ] **Step 3: Browser check** with playwright, on a model with a draft table on a postgres connection. Expected, with no page errors:
  - tool 6 places a note; edit its text;
  - tool 7 places an area; drag the table into it, then drag the area: the table follows, and one Ctrl+Z puts both back;
  - Send to back on a note puts it under a table;
  - tool 5 places a view; set the query `select * from <table>`, a column and the table as dependency: a dashed line appears;
  - Ctrl+G shows `CREATE VIEW` after `CREATE TABLE`;
  - save, reload: everything is still there;
  - take light and dark screenshots.
- [ ] **Step 4:** Commit any fixes. Do not push without being asked.

# Model estilo Redgate — Fase 5 (explorador, exportação e colaboração) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The last group of Redgate features, as spec Fase 5 lists them:
- the **Model explorer**, a SQL query over the model itself that runs in the browser. Results show in a grid with "Find in diagram" and "Export to CSV";
- **Export to image** (PNG and SVG) of the whole diagram or the selection;
- **Export to PDF**, with page size, orientation and margins from the model properties, and a preview of the page breaks on the canvas;
- **Generate documentation**, a downloadable HTML file of the model;
- **one editor at a time**, with a read-only mode and "Take control";
- **Share**, which shows how the model is shared today (through the organization's owners).

**Architecture:**
- **Edit lock (server).** A lock is one row in the new table `cube_model_locks`. A browser tab claims it with a random token and renews it every 30 s. The lock expires 120 s after the last renewal.
  - Every request that changes the model (`PUT`, `DELETE`, build, create-tables) sends the token in the header `X-Model-Lock`.
  - Those requests get 423 when a live lock is held under another token.
  - A model nobody holds stays writable, so API clients keep working.
- **Read-only mode (client).** It is one flag in `ModelEditor`. Every definition change already goes through the history setter. A guard there drops changes while the editor is read-only, and the controls are disabled on top of that.
- **Explorer and exports are client-only.** They load their libraries with `import()` the first time they are used, so opening the editor costs nothing:
  - explorer: `sql.js`;
  - image: `html-to-image`;
  - PDF: `jspdf` plus `html-to-image`.
  - The documentation is a pure function from the definition to an HTML string.
- **The new code lives in new files.** `ModelEditor.tsx` (≈1 750 lines) only wires it in.

**Tech Stack:** as phases 1–4: React 18, `@xyflow/react` 12.3.6, Tailwind `tw:` + shadcn, lucide, vitest + jsdom. Rust (tokio-postgres, axum) in `cubeserver`. New npm dependencies, at exact versions, all loaded on demand:
- `sql.js@1.13.0` and `@types/sql.js@1.4.9` (dev);
- `html-to-image@1.11.11` (the version React Flow's docs pin for export);
- `jspdf@4.2.1`.

**Spec:** `docs/superpowers/specs/2026-11-26-model-redgate-design.md`. Relevant sections:
- Fase 5;
- Servidor, "Controle de edição (fase 5)";
- Restrições: dependencies only when needed, loaded on demand;
- A cargo de quem constrói: English UI, Portuguese server messages.

## Global Constraints

- **Lock timing:**
  - renewal ("heartbeat") every **30 s**;
  - expiry **120 s** after the last heartbeat (`LOCK_TTL_SECS = 120` in the server, the only place the value lives).
- **Lock header:** `X-Model-Lock: <token>`. The token is a `crypto.randomUUID()` made once per editor mount.
- **Lock routes:** they share the model's permission ("admin" on the organization, via `visible_model`), and each answers the lock state `{ "held": bool, "holder": string | null }`. `holder` is the username of whoever holds a live lock, or `null`.
  - `POST /v1/cube-models/:id/lock`, body `{ "token" }`: claims the lock when it is free, expired or already ours (renewing it). Otherwise it leaves the lock alone and answers `held: false`.
  - `POST /v1/cube-models/:id/lock/take`, body `{ "token" }`: always gives us the lock and answers `held: true`.
  - `DELETE /v1/cube-models/:id/lock?token=` → 204. It removes the lock only when the token is ours.
- **423 message (server, Portuguese, exact):** `O model está sendo editado por {holder}`. The client shows it as is.
- **UI copy (English, exact):**
  - banner: `Read-only mode · The document is being edited by {holder}`;
  - banner button: `Take control`;
  - toast to the one who lost control: `{holder} took control. Your unsaved changes were discarded.`;
  - toolbar buttons (labels, used as `aria-label`): `Export model to image`, `Export model to PDF file`, `Generate model documentation`, `Model explorer`, `Share`;
  - explorer: dialog title `Model explorer`; buttons `Run`, `Find in diagram`, `Export to CSV`;
  - properties section `Print settings` with the fields `Page size`, `Orientation`, `Margins (mm)` and the check `Show page breaks`.
- **Print settings (definition, server and client):** `settings.print`, every field optional:
  - `pageSize`: `A4` | `A3` | `Letter` | `Legal`, default `A4`;
  - `orientation`: `portrait` | `landscape`, default `landscape`;
  - `margin`: a number of mm from 0 to 50, default 10;
  - `showPageBreaks`: bool, default false.
- **Page sizes (mm):**

  | Size | Width × height |
  |---|---|
  | A4 | 210 × 297 |
  | A3 | 297 × 420 |
  | Letter | 215.9 × 279.4 |
  | Legal | 215.9 × 355.6 |

  Landscape swaps width and height. One canvas unit prints as one CSS pixel: `mm = px × 25.4 / 96`.
- **Explorer tables (SQLite names, snake_case columns).** Every table has a `node` column holding the canvas node id that "Find in diagram" selects.
  - `tables(node, alias, schema, name, comment, is_draft)`
  - `columns(node, table_alias, name, type, nullable, primary_key, comment, default_value)`. For a table that exists in the database: `type` comes from the database, and `nullable`, `primary_key`, `comment` and `default_value` are NULL.
  - `primary_keys(node, table_alias, name, columns)`
  - `unique_keys(node, table_alias, name, columns)`
  - `indexes(node, table_alias, name, columns, is_unique)`
  - `checks(node, table_alias, name, expression)`
  - `foreign_keys(node, name, from_table, from_column, to_table, to_column, cardinality, mandatory)`: the references. `references` is a reserved word in SQLite, so the table has this name.
  - `views(node, name, schema, comment, sql)`
  - `view_columns(node, view_name, name, type)`
  - `areas(node, name, comment)`
  - `notes(node, text)`
  - `cubes(node, name, title, source_kind, source_table, sql)`
  - `dimensions(node, cube_name, name, type, column_name, sql, primary_key)`
  - `measures(node, cube_name, name, type, column_name, sql, format)`
  - Rules for every table:
    - in `columns`, `foreign_keys` and the key, index and check tables, `node` is the owning table's alias;
    - `columns` lists in key, index and unique-key rows join the names with `, `;
    - booleans are 0/1;
    - a missing value is NULL.
- **File names:** `{modelName}.png`, `{modelName}.svg`, `{modelName}.pdf`, `{modelName}-documentation.html`, `{modelName}-explorer.csv`.
- **Colors:** exports and documentation read our CSS tokens (`getComputedStyle(document.documentElement)`). There is no hex in a component. The documentation HTML may hold the token *values* it read at generation time.
- **Undo:** print settings change through `change()`, one undo step each. Exports, explorer and the lock never touch history.

## Review Focus

1. **The holder's tab closes without releasing** (crash, no network): someone else is locked out for at most 120 s, and then claims the lock normally. Test in Task 1 (store, with a TTL of 0).
2. **Someone takes control while the holder has unsaved edits:**
   - the holder's next save gets 423 and is not applied;
   - the holder's next heartbeat (or the 423) switches the tab to read-only, reloads the model from the server and shows the toast.
   - Test in Task 1 (API 423) and Task 2 (editor).
3. **Names, comments, SQL or note text holding `<script>`, `&` or quotes,** in the documentation HTML and the CSV: the HTML escapes them and the CSV quotes them per RFC 4180. Test in Task 5 (doc) and Task 3 (CSV).
4. **Export or PDF of the selection when only an edge or nothing is selected:** the "Selection" choice is disabled. The whole diagram of an empty model exports a blank image with the margin and does not throw. Test in Task 4.
5. **A bad explorer query** (syntax error, unknown table): the error text shows under the editor, the previous grid is cleared, and nothing throws. Test in Task 3.

---

### Task 1: Server edit lock

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/migrations.sql` (append):

  ```sql
  CREATE TABLE IF NOT EXISTS cube_model_locks (
      model_id uuid PRIMARY KEY REFERENCES cube_models (id) ON DELETE CASCADE,
      user_name text NOT NULL,
      token text NOT NULL,
      heartbeat_at timestamptz NOT NULL DEFAULT now()
  );
  ```

- Modify: `rust/cube/cubeserver/src/rbac/store.rs`, next to `cube_model_cubes`.
- Modify: `rust/cube/cubeserver/src/rbac/handlers/cube_models.rs`:
  - new handlers `lock`, `take_lock`, `release_lock`;
  - a check in `update`, `delete`, `build` and `create_tables`.
- Modify: `rust/cube/cubeserver/src/rbac/handlers/mod.rs`: the routes.
- Test: `rust/cube/cubeserver/tests/model_store.rs` (store) and `rust/cube/cubeserver/tests/model_api.rs` (API).

**Interfaces — Produces:**
- `pub struct ModelLock { pub user_name: String, pub token: String }`.
- `Store::claim_cube_model_lock(&self, model_id: Uuid, user: &str, token: &str, take: bool, ttl_secs: i64) -> Result<Option<ModelLock>, StoreError>`:
  - it runs in one transaction with `SELECT ... FOR UPDATE` on the lock row;
  - when `take` is true, or when the row is missing, older than `ttl_secs`, or holds `token`, it upserts `(user, token, now())`;
  - it returns the live lock **after** the call. That is ours when claimed, and the other holder's otherwise.
- `Store::release_cube_model_lock(&self, model_id: Uuid, token: &str) -> Result<(), StoreError>`: deletes the row only when its token matches.
- `Store::live_cube_model_lock(&self, model_id: Uuid, ttl_secs: i64) -> Result<Option<ModelLock>, StoreError>`.
- Handler constant `pub const LOCK_TTL_SECS: i64 = 120;`. It also adds `fn check_lock(store, model_id, headers) -> Result<(), ApiError>`, which answers 423 with `O model está sendo editado por {user}` when a live lock's token differs from the header, or the header is missing.

- [ ] **Step 1: Write the failing store test** `cube_model_lock_claim_take_expire` in `model_store.rs`. It uses the file's existing setup for a model.
  - A claims with `t1` and gets `user_name == "a"`.
  - B claims with `t2` and gets `a`'s lock back.
  - B claims with `t2`, `ttl_secs = 0`, and gets B's lock. An expired lock is free.
  - A takes with `t1` and gets A's lock.
  - `release(t2)` leaves A's lock in place; `release(t1)` removes it, and `live_cube_model_lock(.., 120)` is `None`.
- [ ] **Step 2: Write the failing API test** `model_edit_lock` in `model_api.rs`. Use two org owners (the `owner_org` helper, plus a second owner added with `PUT /v1/organizations/:id/owners/:user`).
  - `POST lock {token:"a1"}` as A → `{held:true, holder:"<A>"}`.
  - `POST lock {token:"b1"}` as B → `{held:false, holder:"<A>"}`.
  - `PUT` as B with `X-Model-Lock: b1` → 423, `error` equal to `O model está sendo editado por <A>`.
  - The same `PUT` with no header → 423.
  - `PUT` as A with `a1` → 200.
  - `POST lock/take {token:"b1"}` as B → `held:true`. A's `PUT` with `a1` → 423, and B's with `b1` → 200.
  - `DELETE lock?token=b1` → 204. A `PUT` with no header → 200, since nobody holds the model.
  - Build and create-tables as A while B holds the model → 423 (one assertion each).
- [ ] **Step 3: Run both tests and see them fail.** Run: `cd rust/cube && (set -a; . /root/cube-rust/.env; unset CUBEJS_DB_TYPE; export CUBE_TEST_PG_URL="postgres://$CUBEJS_DB_USER:$CUBEJS_DB_PASS@127.0.0.1:55432/postgres?sslmode=disable"; cargo test -p cubeserver --test model_store cube_model_lock; cargo test -p cubeserver --test model_api model_edit_lock)`. Expected: compile errors or FAIL.
- [ ] **Step 4: Implement** the migration, the store functions and the handlers. Wire the routes:
  - `{base}/v1/cube-models/:id/lock` → `post(lock).delete(release_lock)`;
  - `{base}/v1/cube-models/:id/lock/take` → `post(take_lock)`.

  The holder name is the `actor` from `visible_model`. Call `check_lock` after `visible_model` in `update`, `delete`, `build` and `create_tables`. `create_tables` checks the lock even on `dryRun`. The `423` status comes from an `ApiError` constructor; add one in the same style as `bad_request` if none exists.
- [ ] **Step 5: Run the tests again, plus the whole suite.** Run Step 3's commands, then `cargo test -p cubeserver --lib` and `cargo test -p cubeserver --test model_api`. Expected: all pass, with the existing model tests unchanged. They send no header and hold no lock, so they stay writable.
- [ ] **Step 6: Commit** `feat(models): one editor at a time — server edit lock`.

### Task 2: Client edit lock, read-only mode, Take control

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/api.ts`:
  - new `lockModel`, `takeModelLock` and `releaseModelLock`;
  - `saveModel`, `deleteModel`, `buildModel` and `createTables` take an optional `lock?: string` and send `X-Model-Lock` through `call`'s `extra`.
- Create: `packages/cubejs-playground/src/pages/Schema/models/useEditLock.ts`.
- Create: `packages/cubejs-playground/src/pages/Schema/models/ReadOnlyBanner.tsx`.
- Modify: `packages/cubejs-playground/src/pages/Schema/models/ModelEditor.tsx`, the wiring.
- Test: `packages/cubejs-playground/src/__tests__/model-lock.test.ts` (new) and `model-editor.test.ts`.

**Interfaces — Produces:**
- `type LockState = { held: boolean; holder: string | null }`.
- `ModelApi.lockModel(id: string, token: string): Promise<LockState>`.
- `ModelApi.takeModelLock(id: string, token: string): Promise<LockState>`.
- `ModelApi.releaseModelLock(id: string, token: string): Promise<void>`. It uses `fetch(..., { keepalive: true })` so it also works from `pagehide`.
- `useEditLock(api: Pick<ModelApi, 'lockModel' | 'takeModelLock' | 'releaseModelLock'>, modelId: string, onLost: (holder: string) => void): { token: string; status: 'pending' | 'editing' | 'readOnly'; holder: string | null; take(): Promise<void>; lost(holder: string): void }`:
  - it claims on mount and renews every 30 000 ms with `setInterval`;
  - it releases on unmount and on `pagehide`;
  - `status` becomes `editing` when `held` is true and `readOnly` otherwise;
  - when a renewal finds that we held the lock and no longer do, it calls `onLost(holder)` once;
  - `lost(holder)` is for a 423 the editor received: it moves to `readOnly` and calls `onLost`;
  - a failed renewal (network) keeps the current status.

- [ ] **Step 1: Write the failing hook tests** in `model-lock.test.ts`. Use vitest fake timers and a mocked api.
  - `claims on mount and renews every 30 s`: one `lockModel` call at mount and a second after `advanceTimersByTime(30_000)`.
  - `goes read-only when another tab holds the lock`: `lockModel` resolves `{held:false, holder:'bob'}`, so the status is `readOnly` with holder `bob`, and `onLost` is **not** called (we never held it).
  - `calls onLost once when control is taken`: first `{held:true}`, then `{held:false, holder:'bob'}` → `onLost('bob')` exactly once, still once after another renewal.
  - `take() claims through takeModelLock`: the status becomes `editing`.
  - `releases on unmount`: `releaseModelLock(id, token)` is called.
- [ ] **Step 2: Write the failing editor tests** in `model-editor.test.ts`, with the existing api mock extended with the three lock methods.
  - `read-only mode: banner, no edits, Take control`. `lockModel` answers `{held:false, holder:'bob'}`.
    - The page shows `Read-only mode · The document is being edited by bob`.
    - `Save changes`, `Delete`, `Paste` and `Create in database` are disabled.
    - Pressing `Delete` with a table selected leaves `def.tables` unchanged.
    - The properties pane's inputs are disabled (inside a `fieldset[disabled]`).
    - Clicking `Take control` calls `takeModelLock`, and the banner goes away.
  - `save sends the lock token, and a 423 turns the editor read-only`:
    - `saveModel` is called with the token from the latest `lockModel` call;
    - when `saveModel` rejects with `ApiError('O model está sendo editado por bob', 423)`, the banner shows with `bob`, the model is reloaded (`api.model` is called again), and the toast `bob took control. Your unsaved changes were discarded.` shows.
- [ ] **Step 3: Run the tests and see them fail.** Run: `cd packages/cubejs-playground && node_modules/.bin/vitest run src/__tests__/model-lock.test.ts src/__tests__/model-editor.test.ts -t "lock|read-only|423"`. Expected: FAIL.
- [ ] **Step 4: Implement.** In `ModelEditor`:
  - `readOnly = lock.status !== 'editing'`;
  - the history setter guard ignores every set while `readOnly`, except the reload after a lost lock;
  - in read-only mode:
    - `ReactFlow` gets `nodesDraggable={!readOnly}` and `nodesConnectable={!readOnly}`;
    - the toolbar's file and edit tools that change the model are disabled;
    - the tool palette's tools that add objects (table, view, note, area, reference, cube) are disabled, while selection and panning stay;
    - `PropertiesPanel` and `StructurePanel` actions sit inside `<fieldset disabled={readOnly} className="tw:contents">`;
    - the shortcut map drops the editing combos.
  - Exports, explorer, documentation, zoom and search stay enabled.
  - `onLost(holder)` reloads the model (as the open does), clears history, and shows the toast with `toast` from the existing toaster.
  - `ReadOnlyBanner({ holder, onTake })` renders the banner text and a `Take control` button above the canvas. It uses `tw:bg-muted` and `tw:text-muted-foreground`, without a new token.
  - `pending` counts as read-only until the first answer, so nothing is edited before the claim.
- [ ] **Step 5: Run the tests and see them pass,** then the whole vitest suite. Run Step 3's command, then `node_modules/.bin/vitest run`, `node_modules/.bin/tsc --noEmit -p .` and `npx oxlint src/pages/Schema src/__tests__`. Expected: all clean. Existing editor tests need the lock methods in the shared api mock, with `lockModel` answering `{held:true, holder:'me'}`.
- [ ] **Step 6: Commit** `feat(models): read-only mode and Take control`.

### Task 3: Model explorer

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/explorer/explorerTables.ts`, pure: the virtual tables.
- Create: `packages/cubejs-playground/src/pages/Schema/models/explorer/runQuery.ts`, which loads sql.js on demand.
- Create: `packages/cubejs-playground/src/pages/Schema/models/explorer/csv.ts`.
- Create: `packages/cubejs-playground/src/pages/Schema/models/explorer/ExplorerDialog.tsx`.
- Modify: `ModelToolbar.tsx`, with a `Model explorer` tool (lucide `TableProperties`) after `Generate model documentation`. Add the `explorer: { onOpen(): void }` prop group.
- Modify: `ModelEditor.tsx`, the wiring: "Find in diagram" calls the existing `pickNode(nodeId)`.
- Modify: `packages/cubejs-playground/package.json`: `sql.js` `1.13.0`, and `@types/sql.js` `1.4.9` in devDependencies.
- Test: `packages/cubejs-playground/src/__tests__/model-explorer.test.ts` (new).

**Interfaces — Produces:**
- `type ExplorerTable = { name: string; columns: string[]; rows: (string | number | null)[][] }`.
- `explorerTables(def: ModelDefinition, columnsOf: (t: ModelTable) => { name: string; type: string }[]): ExplorerTable[]`. It returns the 14 tables in Global Constraints, in that order, with exactly those columns.
- `runQuery(tables: ExplorerTable[], sql: string): Promise<{ columns: string[]; rows: (string | number | null)[][] }>`:
  - it creates an in-memory sql.js database once per call, holding every table;
  - it runs `sql`, returns the **last** statement's result, and rejects with the SQLite message.
  - Load the library with `import('sql.js')`, and the wasm with `import wasmUrl from 'sql.js/dist/sql-wasm.wasm?url'` passed as `locateFile`.
  - Tests run with the Node build: `initSqlJs()` with no `locateFile` under vitest (`import.meta.env.MODE === 'test'`).
- `toCsv(columns: string[], rows: (string | number | null)[][]): string`, per RFC 4180:
  - fields holding `,`, `"`, CR or LF are quoted, with `"` doubled;
  - NULL is an empty field;
  - lines end with `\r\n`.

- [ ] **Step 1: Write the failing tests:**
  - `explorerTables lists every object kind`. The fixture has a draft table with a PK, a unique key, an index and a check; an existing table; a reference; a view with columns; an area; a note; and a cube with one dimension and one measure.
    - The names are exactly the 14, in order.
    - The `columns` row of a draft column is `['orders', 'orders', 'id', 'integer', 0, 1, null, null]`.
    - An existing table's columns have `nullable === null`.
    - A cube row's `node` is `cube:c1`.
  - `runQuery runs SQL over the model`:
    - `SELECT name FROM columns WHERE table_alias = 'orders' ORDER BY name` returns the column names;
    - a join across `dimensions` and `cubes` works.
  - `runQuery rejects with the SQLite message`: `SELECT * FROM nope` rejects with a message containing `no such table: nope`.
  - `toCsv quotes per RFC 4180`: `toCsv(['a','b'], [['x,y', 'say "hi"'], [null, 3]])` is `'a,b\r\n"x,y","say ""hi"""\r\n,3\r\n'`.
  - `explorer dialog: run, error, find, export`:
    - it opens from `Model explorer`;
    - the default query `SELECT * FROM tables` runs on open and shows a grid;
    - a bad query shows the error and clears the grid;
    - selecting a row of `SELECT * FROM cubes` and clicking `Find in diagram` selects `cube:c1` on the canvas;
    - `Export to CSV` downloads `{modelName}-explorer.csv`, checked by spying on `URL.createObjectURL` and the anchor's `download`.
    - `Find in diagram` is disabled when the result has no `node` column.
- [ ] **Step 2: Run the tests and see them fail.** Run: `node_modules/.bin/vitest run src/__tests__/model-explorer.test.ts`. Expected: FAIL, the module is missing.
- [ ] **Step 3: Add the dependency** (`yarn add sql.js@1.13.0 && yarn add -D @types/sql.js@1.4.9` in `packages/cubejs-playground`), then implement.
  - The dialog is shadcn `Dialog`, wide. It has a monospace `textarea` (Ctrl+Enter runs), a `Run` button, an error line in `tw:text-destructive`, and a scrollable grid. Rows are selectable by click; one row is selected at a time.
  - The dialog lists the 14 table names as clickable chips that insert `SELECT * FROM <name>`.
  - Typing in the textarea must not fire canvas shortcuts. The existing `typing()` check covers a textarea; the test asserts that Delete in it leaves the selection.
- [ ] **Step 4: Run the tests and see them pass,** then `vite build` to confirm that sql.js lands in its own chunk. Run: `node_modules/.bin/vitest run src/__tests__/model-explorer.test.ts && node_modules/.bin/vite build | grep -i "sql-wasm\|sql-wasm.*wasm"`. Expected: PASS, and a separate `sql-wasm` asset or chunk.
- [ ] **Step 5: Commit** `feat(models): Model explorer — SQL over the model in the browser`.

### Task 4: Export to image and PDF, print settings, page breaks

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/export/pages.ts`, pure: page sizes and the page grid.
- Create: `packages/cubejs-playground/src/pages/Schema/models/export/renderDiagram.ts`, which loads `html-to-image` on demand.
- Create: `packages/cubejs-playground/src/pages/Schema/models/export/ExportImageDialog.tsx` (format and scope).
- Create: `packages/cubejs-playground/src/pages/Schema/models/export/exportPdf.ts`, which loads `jspdf` on demand.
- Create: `packages/cubejs-playground/src/pages/Schema/models/export/PageBreaks.tsx`.
- Modify: `ModelToolbar.tsx`, to enable `Export model to image` and `Export model to PDF file` through a new `exports: { onImage(): void; onPdf(): void; busy: boolean }` prop group.
- Modify: `PropertiesPanel.tsx` `ModelSettings`, adding a `Print settings` section (closed by default) through the existing `setSetting`.
- Modify: `modelGraph.ts` `ModelSettings`: `print?: { pageSize?: 'A4' | 'A3' | 'Letter' | 'Legal'; orientation?: 'portrait' | 'landscape'; margin?: number; showPageBreaks?: boolean }`.
- Modify: `rust/cube/cubeserver/src/rbac/cube_model.rs`:
  - `Settings.print: Option<PrintSettings>` (camelCase, `deny_unknown_fields`, every field optional);
  - `validate` refuses a page size outside the 4 (`Tamanho de página inválido: {v}`), an orientation outside the 2 (`Orientação inválida: {v}`), and a margin that is not finite or is outside 0..=50 (`Margem inválida: {v}`).
- Modify: `ModelEditor.tsx`, the wiring. `PageBreaks` renders inside React Flow's `ViewportPortal` when `showPageBreaks` is on.
- Modify: `package.json`: `html-to-image` `1.11.11` and `jspdf` `4.2.1`.
- Test: `src/__tests__/model-export.test.ts` (new); `cube_model.rs` tests.

**Interfaces — Produces:**
- `type PrintSettings = NonNullable<ModelSettings['print']>`.
- `pageSizeMm(print: PrintSettings | undefined): { width: number; height: number }`, with the defaults applied.
- `mmToPx(mm: number): number`, which is `mm * 96 / 25.4`.
- `pageGrid(bounds: { x: number; y: number; width: number; height: number }, print: PrintSettings | undefined): { x: number; y: number; width: number; height: number }[]`:
  - each rectangle is the printable area of one page, in canvas units. That area is the page size minus the margin on every side, converted with `mmToPx`;
  - the grid starts at the bounds' top-left corner and runs row by row, left to right;
  - it has `ceil(bounds.w / page.w)` × `ceil(bounds.h / page.h)` pages, and at least 1.
- `diagramBounds(nodes: Node[], scope: 'all' | 'selection'): Rect | null`. It uses React Flow's `getNodesBounds` and pads by 20. It returns `null` for an empty selection, and `{ x: 0, y: 0, width: 40, height: 40 }` for `all` on a model with no nodes.
- `renderDiagram(viewportEl: HTMLElement, bounds: Rect, format: 'png' | 'svg', pixelRatio?: number): Promise<string>`, a data URL:
  - it renders `.react-flow__viewport` with `transform: translate(-x, -y) scale(1)` at the bounds' size;
  - the background is `var(--background)` resolved;
  - a `filter` drops `.react-flow__minimap`, `.react-flow__controls`, handles (`.react-flow__handle`) and the page-break overlay.
- `exportPdf(viewportEl: HTMLElement, bounds: Rect, print: PrintSettings | undefined, fileName: string): Promise<void>`:
  - it renders one PNG at `pixelRatio` 2;
  - for each `pageGrid` page, it draws the slice onto a canvas and adds it to a jsPDF page of the chosen size and orientation, at the margin;
  - then it calls `save(fileName)`.
- `download(dataUrlOrBlob: string | Blob, fileName: string): void`, shared with Tasks 3 and 5. It goes in `export/download.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `pageSizeMm defaults to A4 landscape and swaps for portrait`: `{297, 210}` and `{210, 297}`; `Letter` portrait gives `{215.9, 279.4}`.
  - `pageGrid splits the bounds into printable pages`. With A4 landscape and margin 10, a printable page is `mmToPx(277) × mmToPx(190)`, about 1046.9 × 718.1. Bounds `{0,0,1500,800}` give 4 pages, the second at `x ≈ 1046.9, y = 0`. Bounds smaller than one page give 1 page.
  - `diagramBounds is null for an empty selection`, pads by 20 otherwise, and gives the 40 × 40 blank for `all` with no nodes. `pageGrid` of that blank is 1 page.
  - `export dialog`:
    - `Export model to image` opens a dialog with `PNG` and `SVG`, and `Whole diagram` and `Selection`;
    - `Selection` is disabled with no node selected;
    - Export calls `renderDiagram` (mocked module) with the chosen format and downloads `{modelName}.png`.
  - `print settings edit the definition`: choosing `A3` and `portrait` in `Print settings` writes `settings.print` (one undo step), and `Show page breaks` renders `[data-slot="model-page-break"]` elements.
  - Rust `refuses_bad_print_settings`: `pageSize: "A5"`, `orientation: "up"` and `margin: 60` are each refused with the messages above. `{pageSize:"Letter", orientation:"portrait", margin:0, showPageBreaks:true}` round-trips.
- [ ] **Step 2: Run the tests and see them fail.** Run: `node_modules/.bin/vitest run src/__tests__/model-export.test.ts`; `cd rust/cube && cargo test -p cubeserver --lib print_settings`. Expected: FAIL.
- [ ] **Step 3: Add the dependencies** (`yarn add html-to-image@1.11.11 jspdf@4.2.1`) and implement. The PDF button runs at once with the model's print settings, as Redgate's does. The image button opens the dialog.
- [ ] **Step 4: Run the tests and see them pass,** then the server lib suite and the vitest suite. Confirm that `vite build` puts `jspdf` and `html-to-image` in their own chunks, apart from the main one.
- [ ] **Step 5: Commit** `feat(models): export to PNG, SVG and PDF, print settings and page breaks`.

### Task 5: Generate documentation

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/export/documentation.ts`, pure.
- Modify: `ModelToolbar.tsx`, to enable `Generate model documentation` (`exports.onDocs`).
- Modify: `ModelEditor.tsx`, the wiring:
  - render the whole diagram to SVG with Task 4's `renderDiagram`;
  - call `documentationHtml`;
  - `download` a `text/html` Blob.
- Test: `src/__tests__/model-docs.test.ts` (new).

**Interfaces — Produces:**
- `documentationHtml(input: { name: string; description?: string; def: ModelDefinition; columnsOf: (t: ModelTable) => { name: string; type: string }[]; diagram: string | null; colors: Record<string, string>; generatedAt: string }): string`. It returns one self-contained HTML5 document with no external resource:
  - `<title>` is the model name;
  - the sections, in order, with these ids:
    - `#diagram`: an `<img>` holding the SVG data URL; left out when `diagram` is null;
    - `#tables`: one subsection per table, with its comment and a columns table of Name, Type, Nullable, PK and Comment, then its keys, indexes and checks;
    - `#references`: From, To, Cardinality and Mandatory;
    - `#views`: comment, columns and SQL in a `<pre>`;
    - `#cubes`: source, dimensions, measures and segments, then the cube joins;
    - `#notes`;
  - a table of contents linking to them;
  - CSS inline, using the color values passed in `colors` (the resolved tokens `--background`, `--foreground`, `--muted`, `--border`);
  - every piece of user text goes through one `escapeHtml` (`& < > " '`).

- [ ] **Step 1: Write the failing tests:**
  - `documentation lists the model's objects`: with the Task 3 fixture, the output contains every id in order, a row for each column, the reference `orders.customer_id → customers.id`, the view SQL inside `<pre>`, and the cube's dimension and measure names.
  - `documentation escapes user text`: a comment `<script>alert(1)</script> & "x"` appears as `&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;x&quot;`, and the output contains no `<script`.
  - `documentation without a diagram has no #diagram section`.
  - `the toolbar button downloads {modelName}-documentation.html`: with `renderDiagram` mocked, check the anchor's `download` attribute.
- [ ] **Step 2: Run the tests and see them fail.** Run: `node_modules/.bin/vitest run src/__tests__/model-docs.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests and see them pass,** then the full vitest, tsc and oxlint.
- [ ] **Step 5: Commit** `feat(models): generate model documentation (HTML)`.

### Task 6: Share

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/ShareDialog.tsx`.
- Modify: `ModelToolbar.tsx`, adding a `Share` tool (lucide `Share2`) at the right end of the file group, with the `share: { onOpen(): void }` prop group.
- Modify: `ModelEditor.tsx`, the wiring. It reads `platformAdmin` from the RBAC session (`useRbac()`), as `api.ts` reads `owned`.
- Test: `model-editor.test.ts`.

**Interfaces — Produces:**
- `ShareDialog({ open, onOpenChange, orgName, platformAdmin }: { open: boolean; onOpenChange(open: boolean): void; orgName: string; platformAdmin: boolean })`.
- The copy is exact:
  - title: `Share model`;
  - body: `Models are shared through their organization: every owner of {orgName} can open and edit this model. One person edits at a time; the others see it read-only.`;
  - for platform admins, a link button `Manage owners` that goes to `/admin/organizations` with react-router's `useHistory().push`;
  - for others, the line `Ask a platform admin to add or remove owners.`;
  - a `Close` button.

- [ ] **Step 1: Write the failing test** `share dialog explains the organization's access`:
  - `Share` opens the dialog with the org name in the body;
  - `Manage owners` shows only when the mocked session is a platform admin, and clicking it pushes `/admin/organizations`.
- [ ] **Step 2: Run it and see it fail.** Run: `node_modules/.bin/vitest run src/__tests__/model-editor.test.ts -t "share dialog"`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run it and see it pass,** then the full vitest, tsc and oxlint.
- [ ] **Step 5: Commit** `feat(models): Share explains and links the organization's access`.

### Task 7: Build, install, browser check

- [ ] **Step 1: Run the full suites.**
  - Rust lib and `model_api` / `model_store`, with the env from Task 1 Step 3.
  - Playground: vitest, tsc and oxlint.
- [ ] **Step 2: Deploy as `deploy-flow`, with the backup tag `redgate5`.**
  - Build `cube-server` in release, copy it as `.new`, then `mv -f`.
  - Build with `vite build` and install with `install-playground.sh`.
  - Run `./start-all.sh restart`, then check that `/readyz` answers 200 and that the log shows no migration error for `cube_model_locks`.
- [ ] **Step 3: Check in the browser with playwright, mocked API only** (no real model changed, no real Build, no Create in database), in light and in dark:
  - the explorer runs `SELECT * FROM columns` and "Find in diagram" moves the canvas;
  - PNG and SVG download;
  - PDF downloads, and the page breaks draw;
  - the documentation downloads;
  - a mocked `lockModel` `{held:false, holder:'bob'}` shows the banner, and Take control clears it;
  - Share opens.
- [ ] **Step 4: Commit** any fix found, with a test.

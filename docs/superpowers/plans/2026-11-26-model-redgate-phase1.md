# Model estilo Redgate — Fase 1 (base do editor) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the model editor into the Redgate Data Modeler editing shell:
- toolbar groups, the floating tool palette and the left and right panes;
- undo/redo, multi-selection, copy/paste/cut, paste as shortcut;
- Size/Align/Distribution/References menus, zoom, search and the minimap;
- tables and crow's-foot references drawn like Redgate, in our colors.

**Architecture:**
- Pure functions, each tested on its own, change the definition: `modelGraph.ts` plus new `arrange.ts`, `clipboard.ts`, `history.ts`, `referencePath.ts` and `search.ts`.
- `ModelEditor.tsx` keeps one definition state behind an undo history and wires new presentational components: toolbar, palette, structure pane, properties pane, search and reference edge.
- The Rust validator accepts the new optional definition fields. Nothing else on the server changes in this phase.

**Tech Stack:** React 18, `@xyflow/react` 12.3.6 (`NodeResizer`, `MiniMap`, `BaseEdge`, `EdgeLabelRenderer`), Tailwind `tw:` + shadcn, lucide icons, vitest + jsdom; Rust (serde) in `cubeserver`.

**Spec:** `docs/superpowers/specs/2026-11-26-model-redgate-design.md` (Fase 1 + "Definição do model (versão 2)" + "Migração ao longo das fases").

## Global Constraints

- Colors only from CSS tokens (`var(--card)`, `var(--border)`, `var(--model-<color>)`, `ring`…), light and dark; no hex.
- UI text in English, using Redgate's names: "Model structure", "Model explorer", "Issues", "Model properties", "Table properties", "Reference properties", "Size", "Align", "Distribution", "Order", "References", "Adjust to text", "Same width", "Same height", "Justify", "Show all references", "Add reference point", "Reduce reference point", "Swap reference orientation". Server error messages stay Portuguese.
- Shortcuts: Ctrl+Z undo; Ctrl+Y and Ctrl+Shift+Z redo; Ctrl+S save; Ctrl+C/V/X; Del; Ctrl+A; Ctrl+F search; Ctrl+Alt+N/P/B toggle left pane/right pane/top bar; 1–8 palette tools. Shortcuts are ignored while focus is in `input`, `textarea`, `select` or `[contenteditable]`. Ctrl also means Meta on macOS.
- Multi-select is Ctrl+click (React Flow `multiSelectionKeyCode={['Control','Meta']}`). No right-click menu.
- History: at most 100 steps; a drag is one step; history is not persisted.
- Paste: offset 20px; names `<name>_1`, `<name>_2`… (first free).
- Zoom choices: 25, 50, 75, 100, 200, 400 %.
- Phase-1 placeholders render disabled with hint "Coming soon": Export image, Export PDF, Generate SQL, Generate documentation, Paste-format buttons beyond those listed, tools View (5), Note (6), Area (7), Cube (8), sections "Model explorer" and "Issues", and the Order menu (Redgate disables it for tables, and phase 1 has only tables).
- Existing behaviour keeps working: drag tables from the connection, new/edit draft table, colors, collapse table, link by drag or by clicking column names, Arrange, Create in database, Build cube, Save with `version`, Delete model, unsaved-changes warning.
- Definition: `formatVersion: 2` is written on save; a definition without it still loads. `primary`, `primaryKey`, `joins` keep their meaning (build unchanged).

## Review Focus

1. **Shortcuts in text fields:** Ctrl+Z, Del or Ctrl+A typed in the properties pane or the search box must edit the text, not the canvas. Test: "ignores canvas shortcuts while typing in a field" (Task 9).
2. **Undo after a server change:** undo after "Create in database" must not bring a created table back as a draft. History resets on load and after create. Test: "clears history after create in database" (Task 9).
3. **Pasting a draft whose `schema.table` already exists:** it must not produce two drafts with the same `schema.table`, which the server refuses. Test: "pastes a draft under a free table name" (Task 3).
4. **Removing a table that has shortcuts:** removing the original removes its shortcuts. Removing a shortcut keeps the table. Tests in Task 3.
5. **Saving a model that uses the new fields to a server still on the old code:** cannot happen, because Task 1 ships first. A definition with an unknown `formatVersion` (e.g. 3) is refused with a clear message. Test in Task 1.

---

### Task 1: Server accepts definition v2 fields

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/cube_model.rs` (structs at 13-60, `validate` at ~193)
- Test: same file, `mod tests`

**Interfaces:**
- Produces (JSON, camelCase; all optional, omitted when absent):
  - `Definition`: `formatVersion?: 2`, `description?: string`.
  - `ModelTable`: `comment?: string`, `width?: number`, `height?: number`, `shortcuts?: [{ id: string, x: number, y: number }]`.
  - `ModelJoin`: `points?: [{ x: number, y: number }]`, `swapped?: bool`, `hidden?: bool`.

- [ ] **Step 1: Write failing tests**
  - `accepts_v2_fields`: a definition with all the fields above deserializes, `validate` is `Ok`, and serializing it gives back the same JSON.
  - `refuses_bad_v2_fields`: `validate` returns `Err` containing each of:
    - `"formatVersion inválido: 3"`;
    - `"Tamanho inválido"` for `width: 0` or a negative `height`;
    - `"O atalho s1 se repete"` for two shortcuts with id `s1` in the model;
    - `"Ponto inválido"` for a non-finite coordinate.
  - `v1_definition_still_valid`: the existing `orders_customers()` fixture serializes with no new keys.
- [ ] **Step 2:** Run `cd rust/cube && cargo test -p cubeserver --lib rbac::cube_model` and check that the new tests fail.
- [ ] **Step 3: Implement the fields.**
  - Each new field is `#[serde(default, skip_serializing_if = ...)]`; the `bool`s skip when false.
  - `format_version: Option<u32>`; `validate` refuses anything but `None`/`Some(2)`.
  - Shortcut ids are unique across the whole model.
- [ ] **Step 4:** Run the same command. Every test passes, the old ones included.
- [ ] **Step 5:** Commit `feat(models): accept the v2 definition fields`.

### Task 2: Definition types and table/join helpers

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/models/modelGraph.ts`
- Test: `packages/cubejs-playground/src/__tests__/model-graph.test.ts`

**Interfaces:**
- Produces:
  - Types:
    - `type Point = { x: number; y: number }`;
    - `type Shortcut = { id: string; x: number; y: number }`;
    - `ModelTable` gains `comment?`, `width?`, `height?`, `shortcuts?: Shortcut[]`;
    - `ModelJoin` gains `points?: Point[]`, `swapped?: boolean`, `hidden?: boolean`;
    - `ModelDefinition` gains `formatVersion?: 2`, `description?: string`.
  - `renameTable(def, alias, next): { def; error?: string }`:
    - renames the alias everywhere (`primary`, join ends);
    - refuses a bad identifier with `Invalid name: <next>` and a taken one with `<next> is already in this model`.
  - `setTableComment(def, alias, comment): ModelDefinition`.
  - `placeTables(def, places: Map<string, { x?: number; y?: number; width?: number | null; height?: number | null }>): ModelDefinition`:
    - the keys are table aliases or shortcut node ids `alias~shortcutId`;
    - `null` deletes the size (Adjust to text);
    - positions are rounded.
  - `setJoin(def, index, patch: Partial<Pick<ModelJoin,'relationship'|'join'|'points'|'swapped'|'hidden'>>): ModelDefinition` replaces `updateJoin`, keeping its callers.
  - `showAllJoins(def): ModelDefinition`.
  - `withFormat(def): ModelDefinition` sets `formatVersion: 2`.

- [ ] **Step 1: Write failing tests**
  - `renameTable moves primary and join ends, refuses bad and taken names`.
  - `placeTables moves tables and shortcuts, sets and clears sizes`: `width: null` removes the key.
  - `setJoin patches one join; showAllJoins clears hidden`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-graph.test.ts` (in `packages/cubejs-playground`) and check that they fail.
- [ ] **Step 3:** Implement. `removeTable` stays as is; shortcuts live inside the table, so they go with it.
- [ ] **Step 4:** Run the same command and check that it passes.
- [ ] **Step 5:** Commit `feat(models): v2 definition fields and table helpers`.

### Task 3: Clipboard and shortcuts

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/clipboard.ts`
- Test: `packages/cubejs-playground/src/__tests__/model-clipboard.test.ts`

**Interfaces:**
- Consumes: the `ModelDefinition` types (Task 2).
- Produces:
  - `type Clip = { tables: ModelTable[]; joins: ModelJoin[] }`.
  - `copy(def, aliases: string[]): Clip`: the tables and the joins whose two ends are both copied. A shortcut node id counts as its table.
  - `paste(def, clip, offset = 20): { def; aliases: string[] }`:
    - new aliases are `<alias>_1`, `<alias>_2`… (first free);
    - a draft's `table` is renamed the same way while its `schema.table` is taken;
    - positions shift by `offset`;
    - joins are remapped to the new aliases; shortcuts are not copied.
  - `cut(def, aliases): { def; clip }`: `copy`, then remove each table. A shortcut id removes only that shortcut.
  - `addShortcut(def, alias, x, y): { def; id: string }`: the id is `s<n>`, with `n` the first number free in the model.
  - `removeShortcut(def, alias, id): ModelDefinition`.
  - `shortcutNodeId(alias, id) => \`${alias}~${id}\`` and `parseNodeId(nodeId) => { alias, shortcut?: string }`.

- [ ] **Step 1: Write failing tests**
  - `copies the selected tables and only the joins between them`.
  - `pastes with _1 names, 20px offset and remapped joins`: paste `orders` (x 10) twice and get `orders_1` (x 30), then `orders_2`.
  - `pastes a draft under a free table name`: a draft `public.items` pasted gives the table `items_1`.
  - `cut removes the tables and returns the clip`.
  - `a shortcut gets the first free id, and removing it keeps the table`.
  - `removing a table removes its shortcuts`.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-clipboard.test.ts` and check that it fails.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same command and check that it passes.
- [ ] **Step 5:** Commit `feat(models): copy, paste, cut and table shortcuts`.

### Task 4: Undo/redo history

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/history.ts`
- Test: `packages/cubejs-playground/src/__tests__/model-history.test.ts`

**Interfaces:**
- Produces:
  - `type History<T> = { past: T[]; present: T; future: T[] }`.
  - `start<T>(value): History<T>`.
  - `record<T>(h, next, limit = 100): History<T>`:
    - pushes `present` and clears `future`;
    - drops the oldest past entries beyond `limit`;
    - returns `h` unchanged when `next === present`.
  - `undo<T>(h): History<T>` and `redo<T>(h): History<T>` (no-ops at the ends).
  - `useHistory<T>(initial): { value: T; set(next: T): void; undo(): void; redo(): void; reset(value: T): void; canUndo: boolean; canRedo: boolean }`: a React hook over the functions above.

- [ ] **Step 1: Write failing tests**
  - `record, undo and redo walk the states`.
  - `a new record after undo drops the future`.
  - `keeps at most 100 past states`: after 105 records, `past.length === 100` and `past[0]` is state 5.
  - `recording the same value is a no-op`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): undo history`.

### Task 5: Arrange geometry (Size, Align, Distribution)

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/arrange.ts`
- Test: `packages/cubejs-playground/src/__tests__/model-arrange.test.ts`

**Interfaces:**
- Produces:
  - `type Box = { id: string; x: number; y: number; width: number; height: number }`.
  - `type AlignMode = 'top' | 'middle' | 'bottom' | 'left' | 'center' | 'right'`.
  - `type DistributeMode = 'v-justify' | 'top' | 'middle' | 'bottom' | 'h-justify' | 'left' | 'center' | 'right'`.
  - `align(boxes, mode): Map<string, { x: number; y: number }>`:
    - aligns to the extreme of the selection (`top` = min y, `bottom` = max bottom, `middle` = the mean of the selection's vertical bounds, and the same horizontally);
    - needs at least 2 boxes, else returns an empty map.
  - `distribute(boxes, mode): Map<string, { x: number; y: number }>`:
    - sorts by the mode's axis and keeps the first and last box where they are;
    - `*-justify` makes equal gaps between the boxes; the others make equal steps between the named edges or centers;
    - needs at least 3 boxes.
  - `sameSize(boxes, 'width' | 'height'): Map<string, { width?: number; height?: number }>`: uses the first box's size.

- [ ] **Step 1: Write failing tests**, with the exact numbers:
  - `align left/center/right/top/middle/bottom`: boxes `a{0,0,100,50}`, `b{200,100,50,30}`. `left` puts `b.x` at 0; `right` puts `a.x` at 150; `center` gives `a.x = 75` and `b.x = 100` (center 125).
  - `distribute h-justify leaves equal gaps`: widths 100, 50, 100 at x 0, 130, 400 give `b.x = 225`.
  - `distribute left steps the left edges evenly`: x 0, 50, 400 give `b.x = 200`.
  - `align needs 2 and distribute needs 3`: both return empty maps.
  - `sameSize copies the first box's width`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): align, distribute and size geometry`.

### Task 6: Reference geometry and crow's-foot ends

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/referencePath.ts`
- Test: `packages/cubejs-playground/src/__tests__/model-reference-path.test.ts`

**Interfaces:**
- Consumes: `Point`, `ModelJoin` (Task 2).
- Produces:
  - `type EndMark = { many: boolean; optional: boolean }`.
  - `endMarks(j: ModelJoin): { from: EndMark; to: EndMark }`:
    - the "many" side is `from` for `many_to_one`, `to` for `one_to_many`, and neither for `one_to_one`;
    - the many side (or the `to` side of a `one_to_one`) is optional (0..* / 0..1);
    - the one side is optional when `join === 'left'` and mandatory when `'inner'`.
  - `orthogonalPath(source: Point, sourceSide: 'left'|'right', target: Point, targetSide: 'left'|'right', points: Point[] = []): { d: string; mid: Point; segments: [Point, Point][] }`:
    - leaves each end horizontally for 20px, joins consecutive points with a horizontal-then-vertical elbow, and returns the SVG path;
    - `mid` is the middle of the longest segment.
  - `addPoint(points, segments): Point[]` inserts the midpoint of the longest segment in its place; `reducePoint(points): Point[]` drops the last.
  - `markPath(at: Point, side: 'left'|'right', mark: EndMark): string`: the SVG path of the end symbol, 12px deep (one bar for mandatory, a circle for optional, a three-line crow's foot for many).

- [ ] **Step 1: Write failing tests**
  - `endMarks for many_to_one left`: from `{many:true, optional:true}`, to `{many:false, optional:true}`.
  - `endMarks for inner makes the one side mandatory`.
  - `orthogonalPath without points has only horizontal and vertical segments`: each segment has equal x or equal y.
  - `orthogonalPath passes through each reference point`.
  - `addPoint splits the longest segment and reducePoint drops the last point`.
  - `swapped sides start the path on the left`: `d` starts at the source and its next x is `source.x - 20`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): rectilinear reference routing and crow's-foot marks`.

### Task 7: Model search

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/search.ts`
- Test: `packages/cubejs-playground/src/__tests__/model-search.test.ts`

**Interfaces:**
- Produces:
  - `type Hit = { kind: 'table' | 'column' | 'reference'; label: string; nodeId: string; edgeIndex?: number }`.
  - `searchModel(def, columnsOf: (t: ModelTable) => {name:string;type:string}[], query: string, limit = 50): Hit[]`:
    - case-insensitive substring match;
    - tables match by alias and `schema.table`;
    - columns are labelled `alias.column`;
    - a reference is labelled `from.alias.column → to.alias.column`;
    - an empty query returns `[]`.

- [ ] **Step 1: Write failing tests**
  - `finds tables, columns and references, case-insensitive`.
  - `empty query finds nothing`.
  - `caps the hits at the limit`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass.
- [ ] **Step 5:** Commit `feat(models): model search`.

### Task 8: Redgate-style table node and reference edge

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/models/TableNode.tsx`
- Create: `packages/cubejs-playground/src/pages/Schema/models/ReferenceEdge.tsx`
- Test: `packages/cubejs-playground/src/__tests__/model-editor.test.ts` (the `table node` describe block); `model-reference-path.test.ts` already covers the geometry.

**Interfaces:**
- Consumes: `endMarks`, `orthogonalPath`, `markPath` (Task 6).
- Produces:
  - `TableFlowNode.data` gains:
    - `shortcut?: string` (the shortcut id, absent on the original);
    - `fixedSize: boolean`;
    - `fks: string[]` (the columns that are the `from` end of a `many_to_one`, or the `to` end of a `one_to_many`).
  - `type ReferenceFlowEdge = Edge<{ index: number; join: ModelJoin }, 'reference'>`.
  - `ReferenceEdge: EdgeProps<ReferenceFlowEdge>`:
    - draws with `BaseEdge`, `orthogonalPath` and `markPath` at both ends;
    - the stroke is `var(--foreground)`, or `var(--ring)` and 2px wide when selected;
    - the label goes through `EdgeLabelRenderer` at `mid`;
    - when selected, each point is a draggable 8px square that calls `data.onMovePoint(index, pointIndex, flowPoint)` (the edge reads the screen→flow conversion from `useReactFlow`).

- [ ] **Step 1: Write failing tests** (component level, through the existing mock)
  - `draws a table like Redgate`: the header has the name; each column row has three cells, name, type and marks (`PK`, `FK`, `PK FK`). Assert `data-slot="model-column-marks"` reads `PK` for `id` and `FK` for `customer_id` in the fixture.
  - `shows a shortcut icon on a shortcut node`: `aria-label="shortcut"`.
  - Keep the existing tests: picked column, collapse, color.
- [ ] **Step 2:** Run `node_modules/.bin/vitest run src/__tests__/model-editor.test.ts -t "table node"` and check that it fails.
- [ ] **Step 3: Restyle `TableNode`.**
  - Square corners and a thin `tw:border-foreground/70` border, no shadow.
  - Header: centered bold name, background `var(--model-<color>)`, or `var(--muted)` without a color. A `tw:ring-2 tw:ring-ring` when selected.
  - Columns in a grid `name | type | marks`, at `text-xs`.
  - Keep: the collapse toggle, the remove button, the "new" and "principal" badges (as small text after the name), the column handles and the click-to-link behaviour.
  - Add `NodeResizer` (`isVisible={selected}`, `minWidth={160}`, colored by `var(--ring)`). Its `onResizeEnd` calls `data.onResize(nodeId, {x, y, width, height})`.
- [ ] **Step 4:** Run the tests and check that they pass. Run `node_modules/.bin/tsc --noEmit -p .` and check that it exits 0.
- [ ] **Step 5:** Commit `feat(models): Redgate-style tables and crow's-foot references`.

### Task 9: Editor shell — history, selection, shortcuts, clipboard

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/models/useEditorShortcuts.ts`
- Modify: `packages/cubejs-playground/src/pages/Schema/models/ModelEditor.tsx`
- Test: `packages/cubejs-playground/src/__tests__/model-editor.test.ts` (new `describe('editing')`)

**Interfaces:**
- Consumes: `useHistory` (Task 4); `copy`/`paste`/`cut`/`addShortcut`/`removeShortcut`/`parseNodeId`/`shortcutNodeId` (Task 3); `placeTables`, `withFormat` (Task 2).
- Produces:
  - `useEditorShortcuts(map: Record<string, (e: KeyboardEvent) => void>, enabled: boolean)`:
    - keys are normalized as `mod+z`, `mod+shift+z`, `mod+alt+n`, `delete`, `1`…;
    - `mod` is Ctrl or Meta;
    - events from text fields are skipped, and a matched key gets `preventDefault`.
  - Editor state:
    - `history = useHistory<ModelDefinition>`, with `def = history.value`;
    - every current `change(next)` becomes `history.set(next)` plus `edited()`;
    - `reset` runs on load and after "Create in database".
  - Selection:
    - `selectedNodes: string[]` (node ids) and `selectedEdge: number | null`;
    - the React Flow nodes carry `selected`, and `onSelectionChange` sets both.
  - Shortcut nodes: one extra node per `table.shortcuts[]` entry, id `shortcutNodeId`, same data, plus `shortcut`.
  - `edges` use `type: 'reference'`; hidden joins are left out; edges connect to the original table only (decided: a shortcut shows the same FK marks, but no lines, so lines are not drawn twice).

- [ ] **Step 1: Write failing tests**
  - `undoes and redoes a table move with Ctrl+Z and Ctrl+Y`.
  - `ignores canvas shortcuts while typing in a field`: Ctrl+Z in the model name input does not undo, and Delete in it removes nothing.
  - `Ctrl+A selects every table, Delete removes the selection`.
  - `copies and pastes the selection with Ctrl+C / Ctrl+V`: `orders_1` appears.
  - `pastes as a shortcut from the toolbar and removes only the shortcut`.
  - `clears history after create in database`: after a successful create, the undo button is disabled.
  - `saves with Ctrl+S and writes formatVersion 2`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
  - Pass `multiSelectionKeyCode={['Control','Meta']}`.
  - A drag records one history step: `onNodeDragStop` → `placeTables`.
  - The ReactFlow `onNodesChange` keeps visual moves without recording.
  - The clipboard is a `useRef<Clip | null>`.
  - "Paste as shortcut" uses the first copied table that is not a draft, else the first copied table. It places the shortcut at the copied table + 20px.
- [ ] **Step 4:** Run the whole file. Old tests that relied on the header fields move to the properties pane in Task 11, so they may fail until then. Mark them `it.todo` and restore them in Task 11.
- [ ] **Step 5:** Commit `feat(models): undo, multi-select, keyboard shortcuts and clipboard`.

### Task 10: Toolbar, tool palette, zoom, search, minimap

**Files:**
- Rewrite: `packages/cubejs-playground/src/pages/Schema/models/ModelToolbar.tsx`
- Create:
  - `packages/cubejs-playground/src/pages/Schema/models/ToolPalette.tsx`
  - `packages/cubejs-playground/src/pages/Schema/models/SearchBox.tsx`
- Modify: `ModelEditor.tsx`
- Test: `model-editor.test.ts` (`describe('toolbar')`, rewritten). Extend the `@xyflow/react` mock with `NodeResizer`, `MiniMap`, `BaseEdge`, `EdgeLabelRenderer` (render nothing) and `useReactFlow().zoomTo`, `setCenter`, `getZoom`, and `useOnViewportChange`.

**Interfaces:**
- Consumes: `align`/`distribute`/`sameSize` (Task 5); `setJoin`, `showAllJoins` (Task 2); `addPoint`/`reducePoint` (Task 6); `searchModel` (Task 7).
- Produces:
  - `ModelToolbar` props. Each group is one prop object:
    - `history: { canUndo, canRedo, onUndo, onRedo }`;
    - `file: { dirty, busy, onSave, onBuild }`; the exports and Generate SQL are disabled placeholders;
    - `edit: { canCopy, canPaste, canDelete, onCopy, onPaste, onPasteShortcut, onCut, onDelete }`;
    - `arrange: { count, onSize(kind: 'fit'|'width'|'height'), onAlign(m: AlignMode), onDistribute(m: DistributeMode) }`;
    - `references: { edgeSelected, onShowAll, onAddPoint, onReducePoint, onSwap }`;
    - `zoom: { percent, onZoomIn, onZoomOut, onOneToOne, onZoomTo(p) }`;
    - `panes: { left, right, top, onToggleLeft, onToggleRight, onToggleTop }`;
    - `search: ReactNode`.
  - Each group is separated by `Separator`. Dropdown menus use the shadcn `DropdownMenu`, with the items in Redgate's order: Align is "Top, Center, Bottom | Left, Center, Right" and Distribution is "Justify, Top, Center, Bottom | Justify, Left, Center, Right". The two Centers are told apart by aria-labels "Align middle" and "Align center"; the Distribution items have aria-labels "Distribute <mode>".
  - Enablement:
    - Size needs 1+ tables selected;
    - Align needs 2+;
    - Distribution needs 3+;
    - Order is always disabled in phase 1;
    - the References items need a selected edge, except "Show all references", which needs a hidden join.
  - `ToolPalette` props: `{ tool: Tool; onTool(t: Tool) }` with `type Tool = 'select' | 'area-select' | 'table' | 'reference' | 'view' | 'note' | 'area' | 'cube'`.
    - It sits at top-left of the canvas, absolute, as a row.
    - Hints follow Redgate: "(1) Select", "(2) Select area", "(3) Add new table", "(4) Add new reference", "(5) Add new view", "(6) Add new note", "(7) Add new area", "(8) Add new cube".
    - 5–8 are disabled with "Coming soon".
    - `table` opens the new-table dialog and returns to `select`.
    - `area-select` sets ReactFlow `selectionOnDrag` and `panOnDrag={false}`; `select` pans on drag.
    - `reference` starts a click-click link: the first table's first PK (or first column), then the second table's matching column, through the existing `link()`.
  - `SearchBox` props: `{ hits: Hit[]; query; onQuery; onPick(h: Hit) }`.
    - Ctrl+F focuses it; Enter picks the first hit.
    - Picking selects the node or edge and calls `setCenter` on it at the current zoom.
  - Minimap: `<MiniMap pannable zoomable nodeColor="var(--muted)" maskColor="color-mix(in oklch, var(--background) 60%, transparent)" />` in the bottom-right, with a button that collapses it (`aria-label="Toggle minimap"`).

- [ ] **Step 1: Write failing tests**
  - `has Redgate's groups and enables them by the selection`: Align is disabled with 1 table and enabled with 2; Distribution is enabled with 3; Order is disabled.
  - `aligns the selected tables to the left`: the saved definition has equal `x`.
  - `Same width copies the first selected table's width`.
  - `adds and removes a reference point and swaps the orientation`: the definition join gets `points` with length 1, then 0, then `swapped: true`.
  - `zooms to a chosen percentage and back to 1:1`: `zoomTo` is called with 0.5, then 1.
  - `search lists hits and centers the picked table`.
  - `the palette's table tool opens the new-table dialog; tools 5-8 are disabled`.
  - `toggles the panes with Ctrl+Alt+N / P / B`.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests and check that they pass. Run `tsc` and check that it is clean.
- [ ] **Step 5:** Commit `feat(models): Redgate toolbar, tool palette, zoom, search and minimap`.

### Task 11: Structure pane and properties pane

**Files:**
- Create:
  - `packages/cubejs-playground/src/pages/Schema/models/StructurePanel.tsx`
  - `packages/cubejs-playground/src/pages/Schema/models/PropertiesPanel.tsx`
- Modify: `ModelEditor.tsx`. Move `TablePanel`/`TableItem` into `StructurePanel.tsx` as the "Database tables" section. The top header row goes away.
- Test: `model-editor.test.ts`. Restore the `it.todo`s from Task 9 against the new places.

**Interfaces:**
- Consumes: `renameTable`, `setTableComment`, `setColor`, `setPrimary`, `setJoin`, `removeJoin` (Task 2 and earlier).
- Produces:
  - `StructurePanel` props: `{ def; columnsOf; connectionTables; locked; selected: string[]; onPick(nodeId: string | {edge: number}) }`.
    - A shadcn `Accordion` with "Model structure", "Database tables", "Model explorer" (disabled, "Coming soon") and "Issues" (disabled, "Coming soon").
    - The "Model structure" tree: Model › Tables (each expandable to its columns), References, Text notes, Views, Subject areas and Cubes. The last four are empty groups in phase 1.
    - Clicking an item selects it and centers on it.
  - `PropertiesPanel` props: `{ def; name; version; selection: { kind: 'model' } | { kind: 'table'; alias } | { kind: 'reference'; index }; columnsOf; onChange(next: ModelDefinition); onName(n); busy; onDelete }`.
    - The title is "Model properties", "Table properties" or "Reference properties", and the sections are `Collapsible`s with + and −.
    - **Model / General:** Name, Description, Connection, Principal and Primary key (the two selects move here unchanged, with the same ids `model-principal` and `model-primary-key`), Size ("N tables"), plus "Delete model" at the bottom.
    - **Table / General:** Name (calls `renameTable`; an error shows under the field), Comment, Color (the palette swatches from today's toolbar move here).
    - **Table / Columns:** a read-only list of name and type for database tables. A draft gets an "Edit columns" button that opens `TableDialog`.
    - **Reference / General:** From and To (alias.column, read-only), Relationship and Join (the `JoinDialog` selects, inline), Hidden (a checkbox), Delete.
    - Clicking an edge selects it and shows this pane; it no longer opens `JoinDialog`. A new link still opens `JoinDialog`.

- [ ] **Step 1: Write failing tests**
  - `the properties pane edits the model's principal and primary key`: the old header tests, retargeted.
  - `renames a table from its properties and keeps its links`.
  - `edits a reference's relationship from its properties and deletes it`: replaces "changes and deletes a link from its edge".
  - `the structure tree lists tables and their columns and selects on click`.
  - `drops a table from the database tables section`: the existing drag test, retargeted.
- [ ] **Step 2:** Run the tests and check that they fail.
- [ ] **Step 3:** Implement.
  - Panes are resizable with the existing `useResizable`/`ResizeHandle`. Their widths are kept under `cubePlayground.modelLeftWidth` and `cubePlayground.modelRightWidth`; their visibility under `cubePlayground.modelPanes`.
- [ ] **Step 4:** Run the full suite: `node_modules/.bin/vitest run`, `node_modules/.bin/tsc --noEmit -p .`, `../../node_modules/.bin/oxlint src/pages/Schema/models src/__tests__`. All must be green.
- [ ] **Step 5:** Commit `feat(models): model structure and properties panes`.

### Task 12: Build, install, browser check, deploy

**Files:** none new.

- [ ] **Step 1:** Server.
  - Back up `/root/cube/rust/cube/target/release/cube-server` to `/root/cube-rust/cube-server.bak-20261126-redgate1`.
  - Run `cargo build --release -p cubeserver --bin cube-server` in `rust/cube`, copy the binary over, then run `/root/cube-rust/start-all.sh restart`.
- [ ] **Step 2:** Playground.
  - Back up `/root/cube-rust/playground` to `playground.bak-20261126-redgate1`.
  - Run `node_modules/.bin/vite build` in `packages/cubejs-playground`, then `/root/cube-rust/install-playground.sh <that>/build`.
- [ ] **Step 3: Browser check with playwright.**
  - Log in as `e2e_owner` and open a model. Expected, with no console errors:
    - toolbar groups, palette, left and right panes and minimap are visible;
    - Ctrl+click selects 3 tables; Align left and Distribution work;
    - Ctrl+Z undoes; Ctrl+C/V creates `_1`; paste as shortcut shows the shortcut icon;
    - selecting a reference shows its properties; adding a point bends it;
    - search finds a column and centers it;
    - Ctrl+S saves, and a reload keeps sizes, points and shortcuts.
  - Take screenshots in light and dark.
- [ ] **Step 4:** Commit any fixes. Push only when the user asks.

import '@xyflow/react/dist/style.css';

import {
  CSSProperties, DragEvent, useCallback, useEffect, useMemo, useRef, useState,
} from 'react';
import {
  Background, ConnectionMode, MiniMap, ReactFlow, ReactFlowProvider, applyNodeChanges, useReactFlow,
} from '@xyflow/react';
import type {
  Connection, EdgeMouseHandler, NodeChange, NodeMouseHandler, OnConnectEnd, OnNodeDrag,
  OnSelectionChangeParams,
} from '@xyflow/react';
import { CircleAlert, Map as MapIcon } from 'lucide-react';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Spinner, notify } from '@/components/kit';

import { ApiError, CubeModelView, OrgNode, modelApi } from '../api';
import { problemLines } from '../editor';
import type { OrgDetail } from '../tree';
import { JoinChoice, JoinDialog } from './JoinDialog';
import {
  End, ModelDefinition, Place, Point, addDraftTable, addTable, canConnect, connect, drafts,
  emptyDefinition, joinLabel, layout, placeTables, removeJoin, removeTable, setJoin, showAllJoins, unlinked,
  updateDraftTable, updateJoin, withFormat,
} from './modelGraph';
import { TableFlowNode, TableNode, columnAt } from './TableNode';
import {
  Clip, addShortcut, copy, cut, parseNodeId, paste, removeShortcut, shortcutNodeId,
} from './clipboard';
import { useHistory } from './history';
import { useEditorShortcuts } from './useEditorShortcuts';
import { ReferenceEdge, ReferenceFlowEdge } from './ReferenceEdge';
import {
  AlignMode, Box as Bounds, DistributeMode, align, distribute, sameSize,
} from './arrange';
import { addPoint, endSides, orthogonalPath, reducePoint } from './referencePath';
import { Hit, searchModel } from './search';
import { SearchBox } from './SearchBox';
import { TOOLS, Tool, ToolPalette } from './ToolPalette';
import { StructurePanel, TABLE_MIME } from './StructurePanel';
import { PropertiesPanel, Selected } from './PropertiesPanel';
import { ModelToolbar } from './ModelToolbar';
import { TableDialog, TableDraft } from './TableDialog';
import { CreateTablesDialog } from './CreateTablesDialog';

export { TABLE_MIME };

// The server's 409 when the cube's file changed since the last build: a
// build with `force` overwrites it. Its other 409 (the path is another
// file's) is not retried.
const EDITED = 'O cubo foi editado desde o último build';

const NODE_TYPES = { table: TableNode };
const EDGE_TYPES = { reference: ReferenceEdge };
// Where a paste lands from what was copied.
const PASTE_OFFSET = 20;
const sameIds = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);
const NEW_JOIN: JoinChoice = { relationship: 'many_to_one', join: 'left' };
// A model of one or two tables would otherwise open at 2x.
const FIT_VIEW = { maxZoom: 1 };
// Where a new table goes when the canvas has no size to centre it in.
const NEW_TABLE_AT = { x: 40, y: 40 };

// React Flow's own colors, from the theme's tokens.
const FLOW_STYLE = {
  '--xy-background-color': 'var(--background)',
  '--xy-edge-label-background-color': 'var(--card)',
  '--xy-edge-label-color': 'var(--card-foreground)',
  '--xy-controls-button-background-color': 'var(--card)',
  '--xy-controls-button-color': 'var(--card-foreground)',
  '--xy-controls-button-border-color': 'var(--border)',
} as CSSProperties;

type ModelApi = ReturnType<typeof modelApi>;
type Columns = { name: string; type: string }[];

export type ModelEditorProps = {
  api: ModelApi;
  org: OrgNode;
  modelId: string;
  tables: OrgDetail['tables'];
  // After a build: the cube's name and the organization's refreshed detail.
  onBuilt(cubeName: string, detail: OrgDetail): void;
  // After new tables were created in the database: the organization's
  // refreshed detail.
  onTablesCreated?(detail: OrgDetail): void;
  // After a save: the tree shows the model's name.
  onSaved?(saved: CubeModelView): void;
  // After the model is deleted.
  onDeleted?(): void;
  // Tells the page when there are changes not saved, so leaving can warn.
  onDirtyChange?(dirty: boolean): void;
};

type JoinEdit = { kind: 'new'; from: End; to: End } | { kind: 'edit'; index: number };
type TableEdit = { kind: 'new' } | { kind: 'edit'; alias: string };
// The statements of a dry run, the version they were read at and the
// error of a run.
type CreateAsk = {
  statements: string[];
  version: number;
  error: string | null;
  created?: string[];
};
type Ask = 'overwrite' | 'delete';

const STALE = 'Changed elsewhere: reopen';
// The server's 409 for a version that is not the stored one. Its other 409
// (a duplicate name) says its own reason.
const STALE_SERVER = 'O model mudou desde que você abriu; abra de novo';
const conflict = (e: any): string => (
  e instanceof ApiError && e.status === 409 && e.message === STALE_SERVER ? STALE : e.message
);

// The theme the app shows (`data-theme` on <html>), followed as it changes.
// It only reads the attribute: the app's own theme hook writes it.
function useDocumentTheme(): 'light' | 'dark' {
  const read = () => (document.documentElement.getAttribute('data-theme') === 'dark' ? 'dark' : 'light');
  const [theme, setTheme] = useState<'light' | 'dark'>(read);
  useEffect(() => {
    const observer = new MutationObserver(() => setTheme(read()));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    setTheme(read());
    return () => observer.disconnect();
  }, []);
  return theme;
}

function definitionOf(value: unknown): ModelDefinition {
  const def = (value && typeof value === 'object' ? value : {}) as Partial<ModelDefinition>;
  return {
    ...emptyDefinition(),
    ...def,
    tables: Array.isArray(def.tables) ? def.tables : [],
    joins: Array.isArray(def.joins) ? def.joins : [],
  };
}

type Panes = { left: boolean; right: boolean; top: boolean };
const PANES_KEY = 'cubePlayground.modelPanes';

// Which panes show, remembered in this browser.
function readPanes(): Panes {
  const all = { left: true, right: true, top: true };
  try {
    const raw = JSON.parse(window.localStorage.getItem(PANES_KEY) ?? '{}');
    return {
      left: raw.left !== false, right: raw.right !== false, top: raw.top !== false,
    };
  } catch {
    return all;
  }
}

function readAliases(key: string): string[] {
  try {
    const raw = JSON.parse(window.localStorage.getItem(key) ?? '[]');
    return Array.isArray(raw) ? raw.filter((a): a is string => typeof a === 'string') : [];
  } catch {
    return [];
  }
}

// The tables of a model shown collapsed, remembered in this browser: a view
// setting of the viewer, not part of the model.
function useCollapsedTables(modelId: string): [string[], (alias: string) => void] {
  const key = `cubePlayground.modelCollapsed.${modelId}`;
  const [aliases, setAliases] = useState(() => readAliases(key));
  useEffect(() => setAliases(readAliases(key)), [key]);
  const toggle = useCallback((alias: string) => {
    setAliases((was) => {
      const next = was.includes(alias) ? was.filter((a) => a !== alias) : [...was, alias];
      try {
        window.localStorage.setItem(key, JSON.stringify(next));
      } catch {
        // Kept for this visit only.
      }
      return next;
    });
  }, [key]);
  return [aliases, toggle];
}

function Editor({
  api, modelId, tables, onBuilt, onTablesCreated, onSaved, onDeleted, onDirtyChange,
}: ModelEditorProps) {
  const theme = useDocumentTheme();
  // Bumped on every edit: a save clears `dirty` only when nothing changed
  // while it was on its way.
  const revision = useRef(0);
  const {
    screenToFlowPosition, zoomIn, zoomOut, zoomTo, fitView, setCenter, getZoom, getInternalNode,
  } = useReactFlow();
  const [tool, setTool] = useState<Tool>('select');
  const [zoomPercent, setZoomPercent] = useState(100);
  const [panes, setPanes] = useState(readPanes);
  const [showMap, setShowMap] = useState(true);
  const [query, setQuery] = useState('');
  const searchInput = useRef<HTMLInputElement>(null);
  // The table clicked first with the reference tool: the parent.
  const [refFrom, setRefFrom] = useState<string | null>(null);
  const togglePane = (pane: keyof Panes) => setPanes((was) => {
    const next = { ...was, [pane]: !was[pane] };
    try {
      window.localStorage.setItem(PANES_KEY, JSON.stringify(next));
    } catch {
      // Kept for this visit only.
    }
    return next;
  });
  const canvas = useRef<HTMLDivElement>(null);

  const [loadError, setLoadError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [name, setName] = useState('');
  const [version, setVersion] = useState(0);
  const history = useHistory<ModelDefinition>(emptyDefinition);
  const def = history.value;
  const setDef = history.set;
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState<'save' | 'build' | 'delete' | 'create' | null>(null);
  const [joinEdit, setJoinEdit] = useState<JoinEdit | null>(null);
  const [tableEdit, setTableEdit] = useState<TableEdit | null>(null);
  const [createAsk, setCreateAsk] = useState<CreateAsk | null>(null);
  // The nodes selected on the canvas (tables and shortcuts) and the
  // selected reference, by index.
  const [selection, setSelection] = useState<string[]>([]);
  const [selectedEdge, setSelectedEdge] = useState<number | null>(null);
  const selected = selection.length ? parseNodeId(selection[0]).alias : null;
  const [clip, setClip] = useState<Clip | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  // The reasons the ModelWriter gave for a change that does not compile.
  const [problems, setProblems] = useState<string[]>([]);
  const lastAsk = useRef<Ask>('overwrite');
  if (ask) {
    lastAsk.current = ask;
  }
  const [nodes, setNodes] = useState<TableFlowNode[]>([]);

  useEffect(() => {
    let cancelled = false;
    setLoaded(false);
    setLoadError(null);
    api.model(modelId).then(
      (m) => {
        if (cancelled) {
          return;
        }
        setName(m.name);
        setVersion(m.version);
        history.reset(definitionOf(m.definition));
        setDirty(false);
        setLoaded(true);
      },
      (e: Error) => !cancelled && setLoadError(e.message)
    );
    return () => {
      cancelled = true;
    };
    // history.reset is stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, modelId]);

  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  // Closing or reloading the tab with changes not saved asks first.
  useEffect(() => {
    if (!dirty) {
      return undefined;
    }
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = '';
    };
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  }, [dirty]);

  const edited = () => {
    revision.current += 1;
    setDirty(true);
  };

  const change = (next: ModelDefinition) => {
    setDef(next);
    edited();
  };

  const columnsOf = useCallback(
    (schema: string, table: string): Columns | undefined => tables?.[def.dataSource]?.[schema]?.[table],
    [tables, def.dataSource]
  );
  const missing = def.tables
    .filter((t) => !t.draft && !columnsOf(t.schema, t.table))
    .map((t) => `${t.schema}.${t.table}`);

  const [collapsedTables, toggleTable] = useCollapsedTables(modelId);
  // The column picked by a click to start a link; a click on a column of
  // another table ends it.
  const [picked, setPicked] = useState<{ alias: string; column: string } | null>(null);
  const pickRef = useRef<(alias: string, column: string) => void>(() => {});
  const onPick = useCallback((alias: string, column: string) => pickRef.current(alias, column), []);

  useEffect(() => {
    if (!picked) {
      return undefined;
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setPicked(null);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [picked]);

  const onRemove = useCallback((nodeId: string) => {
    const { alias, shortcut } = parseNodeId(nodeId);
    setDef((d) => (shortcut ? removeShortcut(d, alias, shortcut) : removeTable(d, alias)));
    revision.current += 1;
    setDirty(true);
  }, []);

  const onResize = useCallback((nodeId: string, box: { x: number; y: number; width: number; height: number }) => {
    const { alias, shortcut } = parseNodeId(nodeId);
    const place: Place = shortcut ? { x: box.x, y: box.y } : box;
    setDef((d) => placeTables(d, new Map([[shortcut ? nodeId : alias, place]])));
    revision.current += 1;
    setDirty(true);
  }, []);

  // The canvas' nodes follow the definition, keeping what React Flow
  // measured and selected on them.
  useEffect(() => {
    setNodes((previous) => def.tables.flatMap((t) => {
      const columns = t.draft
        ? t.draft.columns.map((c) => ({ name: c.name, type: c.type }))
        : columnsOf(t.schema, t.table);
      const principal = def.primary === t.alias;
      const keys = [
        ...(principal && def.primaryKey ? [def.primaryKey] : []),
        ...(t.draft?.columns.filter((c) => c.primaryKey).map((c) => c.name) ?? []),
      ];
      const linked = def.joins.flatMap((j) => [j.from, j.to])
        .filter((e) => e.alias === t.alias)
        .map((e) => e.column);
      const fks = def.joins.flatMap((j) => {
        if (j.relationship === 'many_to_one') {
          return [j.from];
        }
        return j.relationship === 'one_to_many' ? [j.to] : [];
      }).filter((e) => e.alias === t.alias).map((e) => e.column);
      const data = {
        alias: t.alias,
        schema: t.schema,
        table: t.table,
        columns: columns ?? [],
        principal,
        missing: !columns,
        draft: Boolean(t.draft),
        color: t.color,
        keys,
        linked,
        fks,
        fixedSize: t.width !== undefined || t.height !== undefined,
        onResize,
        onRemove,
        collapsed: collapsedTables.includes(t.alias),
        onToggle: toggleTable,
        picked: picked?.alias === t.alias ? picked.column : null,
        onPick,
      };
      // Undefined, not absent: the old node may still carry a size.
      const size = { width: t.width, height: t.height };
      const at = (id: string, x: number, y: number, shortcut?: string): TableFlowNode => ({
        ...previous.find((n) => n.id === id),
        id,
        type: 'table' as const,
        position: { x, y },
        ...size,
        selected: selection.includes(id),
        data: shortcut ? { ...data, shortcut } : data,
      });
      return [
        at(t.alias, t.x, t.y),
        ...(t.shortcuts ?? []).map((sc) => at(shortcutNodeId(t.alias, sc.id), sc.x, sc.y, sc.id)),
      ];
    }));
  }, [def, columnsOf, onRemove, onResize, collapsedTables, toggleTable, picked, onPick, selection]);

  const onMovePoint = useCallback((index: number, pointIndex: number, to: Point) => {
    setDef((d) => {
      const points = [...(d.joins[index]?.points ?? [])];
      points[pointIndex] = to;
      return setJoin(d, index, { points });
    });
    revision.current += 1;
    setDirty(true);
  }, [setDef]);

  const edges: ReferenceFlowEdge[] = useMemo(() => def.joins.flatMap((j, index) => (j.hidden ? [] : [{
    id: `join-${index}`,
    source: j.from.alias,
    sourceHandle: j.from.column,
    target: j.to.alias,
    targetHandle: j.to.column,
    label: joinLabel(j),
    type: 'reference' as const,
    selected: selectedEdge === index,
    data: {
      index, join: j, label: joinLabel(j), onMovePoint,
    },
  }])), [def.joins, selectedEdge, onMovePoint]);

  const onNodesChange = useCallback((changes: NodeChange<TableFlowNode>[]) => {
    // Tables leave through their own button, which updates the definition.
    const kept = changes.filter((c) => c.type !== 'remove');
    setNodes((ns) => applyNodeChanges(kept, ns));
  }, []);

  // React Flow asks for a stable handler.
  const onSelectionChange = useCallback(({ nodes: chosen, edges: lines }: OnSelectionChangeParams) => {
    const ids = chosen.map((n) => n.id);
    setSelection((was) => (sameIds(was, ids) ? was : ids));
    // A picked node wins: React Flow cannot unselect our controlled edges,
    // so it may still report the reference picked before.
    const edge = lines[0]?.data as { index?: number } | undefined;
    setSelectedEdge(ids.length === 0 && typeof edge?.index === 'number' ? edge.index : null);
  }, []);

  const onNodeDoubleClick: NodeMouseHandler<TableFlowNode> = (_event, node) => {
    if (def.tables.some((t) => t.alias === node.id && t.draft)) {
      setTableEdit({ kind: 'edit', alias: node.id });
    }
  };

  // One step of history per drag, whatever it moved: tables and shortcuts.
  const onNodeDragStop: OnNodeDrag<TableFlowNode> = (_event, _node, dragged) => {
    change(placeTables(def, new Map(dragged.map((n) => [n.id, { x: n.position.x, y: n.position.y }]))));
  };

  const copySelection = () => {
    if (selection.length) {
      setClip(copy(def, selection));
    }
  };

  const pasteClip = () => {
    if (!clip?.tables.length) {
      return;
    }
    const result = paste(def, clip, PASTE_OFFSET);
    change(result.def);
    setSelection(result.aliases);
  };

  const pasteShortcut = () => {
    const tablesCopied = clip?.tables ?? [];
    const source = tablesCopied.find((t) => !t.draft) ?? tablesCopied[0];
    if (!source || !def.tables.some((t) => t.alias === source.alias)) {
      return;
    }
    const result = addShortcut(def, source.alias, source.x + PASTE_OFFSET, source.y + PASTE_OFFSET);
    change(result.def);
    setSelection([shortcutNodeId(source.alias, result.id)]);
  };

  const cutSelection = () => {
    if (!selection.length) {
      return;
    }
    const result = cut(def, selection);
    setClip(result.clip);
    change(result.def);
    setSelection([]);
  };

  const deleteSelection = () => {
    if (selection.length) {
      change(cut(def, selection).def);
      setSelection([]);
      setSelectedEdge(null);
    } else if (selectedEdge !== null && def.joins[selectedEdge]) {
      change(removeJoin(def, selectedEdge));
      setSelectedEdge(null);
    }
  };

  // References are selected by their place, which undo and redo may give
  // to another one.
  const undo = () => {
    if (history.canUndo) {
      history.undo();
      setSelectedEdge(null);
      edited();
    }
  };

  const redo = () => {
    if (history.canRedo) {
      history.redo();
      setSelectedEdge(null);
      edited();
    }
  };

  const link = (from: { alias: string; column: string }, to: { alias: string; column: string }) => {
    const error = canConnect(def, from, to);
    if (error) {
      notify.error(error);
      return;
    }
    setJoinEdit({ kind: 'new', from, to });
  };

  pickRef.current = (alias, column) => {
    if (!picked || picked.alias === alias || !def.tables.some((t) => t.alias === picked.alias)) {
      setPicked(picked?.alias === alias && picked.column === column ? null : { alias, column });
      return;
    }
    setPicked(null);
    link(picked, { alias, column });
  };

  const onConnect = (c: Connection) => {
    if (c.source && c.target && c.sourceHandle && c.targetHandle) {
      link(
        { alias: parseNodeId(c.source).alias, column: c.sourceHandle },
        { alias: parseNodeId(c.target).alias, column: c.targetHandle }
      );
    }
  };

  // A link dropped on a column's row, away from its handle.
  const onConnectEnd: OnConnectEnd = (event, state) => {
    if (state.isValid || !state.fromNode || !state.fromHandle?.id) {
      return;
    }
    const point = 'changedTouches' in event ? event.changedTouches[0] : event;
    const at = point && columnAt(document.elementFromPoint(point.clientX, point.clientY));
    const from = parseNodeId(state.fromNode.id).alias;
    const to = at && { ...at, alias: parseNodeId(at.alias).alias };
    if (to && to.alias !== from) {
      link({ alias: from, column: state.fromHandle.id }, to);
    }
  };

  const onEdgeClick: EdgeMouseHandler = (_event, edge) => {
    const index = Number((edge.data as { index?: number } | undefined)?.index);
    if (def.joins[index]) {
      setSelection([]);
      setSelectedEdge(index);
    }
  };

  const onDragOver = (e: DragEvent) => {
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = 'move';
    }
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    const raw = e.dataTransfer?.getData(TABLE_MIME);
    if (!raw) {
      return;
    }
    let ref: { ds: string; schema: string; table: string };
    try {
      ref = JSON.parse(raw);
    } catch {
      return;
    }
    const p = screenToFlowPosition({ x: e.clientX, y: e.clientY });
    const result = addTable(def, ref.ds, ref.schema, ref.table, Math.round(p.x), Math.round(p.y));
    if (result.error) {
      notify.error(result.error);
      return;
    }
    change(result.def);
  };

  // The saved model, or null when the save failed.
  const save = async (): Promise<CubeModelView | null> => {
    setBusy('save');
    setProblems([]);
    const sent = revision.current;
    try {
      const saved = await api.saveModel(modelId, { name, definition: withFormat(def), version });
      setVersion(saved.version);
      if (revision.current === sent) {
        setDirty(false);
      }
      onSaved?.(saved);
      return saved;
    } catch (e: any) {
      notify.error(conflict(e));
      return null;
    } finally {
      setBusy(null);
    }
  };

  const latest = useRef({ dirty, busy, save });
  latest.current = { dirty, busy, save };

  // Saves what is not saved, then builds; a cube edited by hand asks before
  // `force` overwrites it.
  const runBuild = async (force = false) => {
    if (dirty && !(await save())) {
      return;
    }
    setBusy('build');
    setProblems([]);
    try {
      const result = force ? await api.buildModel(modelId, true) : await api.buildModel(modelId);
      onBuilt(result.cubeName, result.detail);
    } catch (e: any) {
      if (e instanceof ApiError && e.status === 409 && e.message === EDITED) {
        setAsk('overwrite');
      } else {
        if (e instanceof ApiError && e.status === 400 && e.problems?.length) {
          setProblems(problemLines(e.problems));
        }
        notify.error(e.message);
      }
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    setBusy('delete');
    try {
      await api.deleteModel(modelId, version);
    } catch (e: any) {
      notify.error(conflict(e));
      setBusy(null);
      return;
    }
    setDirty(false);
    setBusy(null);
    onDeleted?.();
  };

  // A new table goes to the middle of what the canvas shows.
  const newTablePosition = () => {
    const box = canvas.current?.getBoundingClientRect();
    if (!box || box.width === 0 || box.height === 0) {
      return NEW_TABLE_AT;
    }
    return screenToFlowPosition({ x: box.left + box.width / 2, y: box.top + box.height / 2 });
  };

  const saveTable = (value: TableDraft): string | null => {
    if (tableEdit?.kind === 'edit') {
      const result = updateDraftTable(def, tableEdit.alias, value.schema, value.table, value.columns);
      if (result.error) {
        return result.error;
      }
      change(result.def);
    } else {
      const p = newTablePosition();
      const result = addDraftTable(
        def, value.ds, value.schema, value.table, value.columns, Math.round(p.x), Math.round(p.y)
      );
      if (result.error) {
        return result.error;
      }
      change(result.def);
    }
    setTableEdit(null);
    return null;
  };

  // Arrange fits the view once the nodes carry the new positions: a fit
  // asked for at once would measure the old ones and leave tables outside.
  const fitPending = useRef(false);
  useEffect(() => {
    if (fitPending.current) {
      fitPending.current = false;
      window.requestAnimationFrame(() => fitView(FIT_VIEW));
    }
  }, [nodes, fitView]);

  const arrange = () => {
    fitPending.current = true;
    change(layout(def));
  };

  // Saves what is not saved, then asks the server what it would run.
  const openCreate = async () => {
    let current = version;
    if (dirty) {
      const saved = await save();
      if (!saved) {
        return;
      }
      current = saved.version;
    }
    setBusy('create');
    try {
      const result = await api.createTables(modelId, current, true);
      setCreateAsk({ statements: result.statements ?? [], version: current, error: null });
    } catch (e: any) {
      notify.error(conflict(e));
    } finally {
      setBusy(null);
    }
  };

  // Runs the statements; the server saves the model without the drafts it
  // created, even when a later one failed.
  const runCreate = async () => {
    if (!createAsk) {
      return;
    }
    setBusy('create');
    try {
      const result = await api.createTables(modelId, createAsk.version, false);
      if (result.model) {
        // What the server created is no draft any more: undoing past it
        // would offer to create it again.
        history.reset(definitionOf(result.model.definition));
        setVersion(result.model.version);
        setDirty(false);
      }
      if (result.detail) {
        onTablesCreated?.(result.detail);
      }
      if (result.created?.length) {
        notify.success(`Created: ${result.created.join(', ')}`);
      }
      if (result.error) {
        const next = result.model ? definitionOf(result.model.definition) : null;
        if (next && drafts(next).length === 0) {
          setCreateAsk(null);
          notify.error(result.error);
        } else {
          // What is still to create, at the version the run saved.
          const nextVersion = result.model?.version ?? createAsk.version;
          let { statements } = createAsk;
          if (result.model) {
            try {
              statements = (await api.createTables(modelId, nextVersion, true)).statements ?? [];
            } catch (e: any) {
              statements = [];
              setCreateAsk({
                statements, version: nextVersion, error: `${result.error}\n${conflict(e)}`, created: result.created,
              });
              return;
            }
          }
          setCreateAsk({
            statements, version: nextVersion, error: result.error, created: result.created,
          });
        }
      } else {
        setCreateAsk(null);
      }
    } catch (e: any) {
      setCreateAsk({ ...createAsk, error: conflict(e) });
    } finally {
      setBusy(null);
    }
  };

  const loose = unlinked(def);
  const build = () => {
    if (loose.length) {
      setProblems([]);
      notify.error(`Link every table to the principal before building: ${loose.join(', ')}`);
    } else {
      runBuild();
    }
  };

  // The selected nodes as boxes on the canvas, in selection order.
  const boxes = (): Bounds[] => selection.flatMap((id) => {
    const n = nodes.find((x) => x.id === id);
    if (!n) {
      return [];
    }
    return [{
      id,
      x: n.position.x,
      y: n.position.y,
      width: n.width ?? n.measured?.width ?? 0,
      height: n.height ?? n.measured?.height ?? 0,
    }];
  });

  const moveTo = (places: Map<string, { x: number; y: number }>) => {
    if (places.size) {
      change(placeTables(def, places));
    }
  };

  const onSize = (kind: 'fit' | 'width' | 'height') => {
    const sizes = new Map<string, Place>();
    if (kind === 'fit') {
      selection.forEach((id) => sizes.set(parseNodeId(id).alias, { width: null, height: null }));
    } else {
      sameSize(boxes(), kind).forEach((size, id) => sizes.set(parseNodeId(id).alias, size));
    }
    if (sizes.size) {
      change(placeTables(def, sizes));
    }
  };

  // Where a reference is drawn now, as its edge draws it.
  const referencePath = (index: number) => {
    const j = def.joins[index];
    const end = (alias: string, column: string) => {
      const n = getInternalNode?.(alias);
      const t = def.tables.find((x) => x.alias === alias);
      const x = n?.internals.positionAbsolute.x ?? t?.x ?? 0;
      const y = n?.internals.positionAbsolute.y ?? t?.y ?? 0;
      const width = n?.measured.width ?? t?.width ?? 0;
      const handle = n?.internals.handleBounds?.source?.find((h) => h.id === column);
      return { x, width, y: handle ? y + handle.y + handle.height / 2 : y };
    };
    const a = end(j.from.alias, j.from.column);
    const b = end(j.to.alias, j.to.column);
    const [sa, sb] = endSides(a, b, Boolean(j.swapped));
    return orthogonalPath(
      { x: sa === 'right' ? a.x + a.width : a.x, y: a.y },
      sa,
      { x: sb === 'right' ? b.x + b.width : b.x, y: b.y },
      sb,
      j.points ?? []
    );
  };

  const onReference = (what: 'add' | 'reduce' | 'swap') => {
    const j = selectedEdge === null ? undefined : def.joins[selectedEdge];
    if (!j || selectedEdge === null) {
      return;
    }
    if (what === 'swap') {
      change(setJoin(def, selectedEdge, { swapped: !j.swapped }));
    } else if (what === 'add') {
      change(setJoin(def, selectedEdge, { points: addPoint(j.points ?? [], referencePath(selectedEdge)) }));
    } else {
      change(setJoin(def, selectedEdge, { points: reducePoint(j.points ?? []) }));
    }
  };

  const columnsOfTable = (t: ModelDefinition['tables'][number]) => (t.draft
    ? t.draft.columns.map((c) => ({ name: c.name, type: c.type }))
    : columnsOf(t.schema, t.table) ?? []);
  const hits = searchModel(def, columnsOfTable, query);

  const centerOn = (nodeId: string) => {
    const n = nodes.find((x) => x.id === nodeId);
    if (n) {
      const width = n.width ?? n.measured?.width ?? 0;
      const height = n.height ?? n.measured?.height ?? 0;
      setCenter(n.position.x + width / 2, n.position.y + height / 2, { zoom: getZoom(), duration: 300 });
    }
  };

  const pickNode = (nodeId: string) => {
    setSelection([nodeId]);
    setSelectedEdge(null);
    centerOn(nodeId);
  };

  const pickEdge = (index: number) => {
    setSelection([]);
    setSelectedEdge(index);
    const j = def.joins[index];
    if (j) {
      centerOn(j.from.alias);
    }
  };

  const pickHit = (hit: Hit) => (hit.edgeIndex !== undefined ? pickEdge(hit.edgeIndex) : pickNode(hit.nodeId));

  const chooseTool = (next: Tool) => {
    setRefFrom(null);
    if (next === 'table') {
      setTableEdit({ kind: 'new' });
      setTool('select');
      return;
    }
    setTool(next);
  };

  // The reference tool: a click on the parent, then on the child. The
  // child's column is `<parent>_<key>`, or the key's own name, or its first.
  const onNodeClick: NodeMouseHandler<TableFlowNode> = (_event, node) => {
    if (tool !== 'reference') {
      return;
    }
    const { alias } = parseNodeId(node.id);
    if (!refFrom || refFrom === alias) {
      setRefFrom(alias);
      return;
    }
    const parent = nodes.find((n) => n.id === refFrom)?.data;
    const child = node.data;
    const key = parent?.keys[0] ?? parent?.columns[0]?.name;
    const names = child.columns.map((c) => c.name);
    const column = [`${refFrom}_${key}`, key].find((c) => c && names.includes(c)) ?? names[0];
    setRefFrom(null);
    setTool('select');
    if (key && column) {
      link({ alias, column }, { alias: refFrom, column: key });
    }
  };

  const toolKeys = Object.fromEntries(TOOLS.filter((t) => !t.soon).map((t, i) => [
    String(i + 1), () => chooseTool(t.tool),
  ]));

  useEditorShortcuts({
    ...toolKeys,
    'mod+f': () => searchInput.current?.focus(),
    'mod+alt+n': () => togglePane('left'),
    'mod+alt+p': () => togglePane('right'),
    'mod+alt+b': () => togglePane('top'),
    'mod+z': undo,
    'mod+y': redo,
    'mod+shift+z': redo,
    // A field that commits on blur commits first, then the save reads it.
    'mod+s': () => {
      (document.activeElement as HTMLElement | null)?.blur?.();
      window.setTimeout(() => {
        const now = latest.current;
        if (now.dirty && now.busy === null) {
          now.save();
        }
      }, 0);
    },
    'mod+c': copySelection,
    'mod+v': pasteClip,
    'mod+x': cutSelection,
    'mod+a': () => setSelection(nodes.map((n) => n.id)),
    delete: deleteSelection,
  }, loaded && !loadError && !joinEdit && !tableEdit && !createAsk && !ask);

  if (loadError) {
    return (
      <Alert variant="destructive">
        <CircleAlert />
        <AlertTitle>Could not open the model</AlertTitle>
        <AlertDescription>{loadError}</AlertDescription>
      </Alert>
    );
  }
  if (!loaded) {
    return <Spinner />;
  }

  // What the confirm asks; the last question stays while it closes.
  const asked = ask ?? lastAsk.current;
  const question = {
    overwrite: {
      title: 'Overwrite the cube?',
      text: 'The cube was edited by hand since the last build. Overwrite?',
      ok: 'Overwrite',
    },
    delete: {
      title: 'Delete model?',
      text: `Delete model ${name}? The cube it built stays in Cubes.`,
      ok: 'Delete',
    },
  }[asked];

  let propertiesOf: Selected = { kind: 'model' };
  if (selectedEdge !== null && def.joins[selectedEdge]) {
    propertiesOf = { kind: 'reference', index: selectedEdge };
  } else if (selected && def.tables.some((t) => t.alias === selected)) {
    propertiesOf = { kind: 'table', alias: selected };
  }
  const editing = joinEdit?.kind === 'edit' ? def.joins[joinEdit.index] : undefined;
  const editedTable = tableEdit?.kind === 'edit' ? def.tables.find((t) => t.alias === tableEdit.alias) : undefined;

  return (
    <div className="tw:flex tw:h-full tw:min-h-[560px] tw:flex-col tw:gap-3">
      {panes.top ? (
        <div data-slot="model-top-bar" className="tw:flex tw:items-center tw:gap-3">
          <h2 className="tw:m-0 tw:truncate tw:text-base tw:font-semibold">{name || 'Untitled model'}</h2>
          {dirty ? <Badge variant="secondary">Unsaved changes</Badge> : null}
          {busy === 'save' || busy === 'build' ? <Spinner /> : null}
        </div>
      ) : null}

      {missing.length ? (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertTitle>Tables not found in the connection</AlertTitle>
          <AlertDescription>
            {`${missing.join(', ')}: remove them or the build fails`}
          </AlertDescription>
        </Alert>
      ) : null}
      {problems.length ? (
        <Alert variant="destructive" data-slot="model-problems">
          <CircleAlert />
          <AlertTitle>The change does not compile</AlertTitle>
          <AlertDescription>
            <ul className="tw:m-0 tw:list-disc tw:pl-4">
              {problems.map((p) => <li key={p}>{p}</li>)}
            </ul>
          </AlertDescription>
        </Alert>
      ) : null}
      {loose.length ? (
        <p className="tw:m-0 tw:text-sm tw:text-muted-foreground">
          {`Not linked to the principal yet: ${loose.join(', ')}`}
        </p>
      ) : null}

      <ModelToolbar
        history={{
          canUndo: history.canUndo, canRedo: history.canRedo, onUndo: undo, onRedo: redo,
        }}
        file={{
          dirty,
          busy: busy !== null,
          hasDrafts: drafts(def).length > 0,
          canBuild: def.tables.length > 0,
          onSave: () => save(),
          onCreate: openCreate,
          onBuild: build,
        }}
        edit={{
          canCopy: selection.length > 0,
          canPaste: Boolean(clip?.tables.length),
          canDelete: selection.length > 0 || selectedEdge !== null,
          onCopy: copySelection,
          onPaste: pasteClip,
          onPasteShortcut: pasteShortcut,
          onCut: cutSelection,
          onDelete: deleteSelection,
        }}
        arrange={{
          count: selection.length,
          onSize,
          onAlign: (mode: AlignMode) => moveTo(align(boxes(), mode)),
          onDistribute: (mode: DistributeMode) => moveTo(distribute(boxes(), mode)),
          onArrange: arrange,
        }}
        references={{
          edgeSelected: selectedEdge !== null && Boolean(def.joins[selectedEdge]),
          anyHidden: def.joins.some((j) => j.hidden),
          onShowAll: () => change(showAllJoins(def)),
          onAddPoint: () => onReference('add'),
          onReducePoint: () => onReference('reduce'),
          onSwap: () => onReference('swap'),
        }}
        zoom={{
          percent: zoomPercent,
          onZoomIn: () => zoomIn(),
          onZoomOut: () => zoomOut(),
          onOneToOne: () => zoomTo(1),
          onZoomTo: (p: number) => zoomTo(p / 100),
          onFit: () => fitView(FIT_VIEW),
        }}
        panes={{
          ...panes,
          onToggleLeft: () => togglePane('left'),
          onToggleRight: () => togglePane('right'),
          onToggleTop: () => togglePane('top'),
        }}
        search={(
          <SearchBox query={query} hits={hits} onQuery={setQuery} onPick={pickHit} inputRef={searchInput} />
        )}
      />

      <div className="tw:flex tw:min-h-0 tw:flex-1 tw:overflow-hidden tw:rounded-md tw:border">
        {panes.left ? (
          <StructurePanel
            def={def}
            columnsOf={columnsOfTable}
            selection={selection}
            selectedEdge={selectedEdge}
            onPickNode={pickNode}
            onPickEdge={pickEdge}
            tables={tables}
            dataSource={def.dataSource}
            locked={def.tables.length > 0}
          />
        ) : null}
        <div
          ref={canvas}
          className="tw:relative tw:min-w-0 tw:flex-1"
          data-slot="model-canvas"
          onDragOver={onDragOver}
          onDrop={onDrop}
        >
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={NODE_TYPES}
            edgeTypes={EDGE_TYPES}
            multiSelectionKeyCode={['Control', 'Meta']}
            colorMode={theme}
            style={FLOW_STYLE}
            onNodesChange={onNodesChange}
            onNodeDragStop={onNodeDragStop}
            onConnect={onConnect}
            onConnectEnd={onConnectEnd}
            onPaneClick={() => {
              setPicked(null);
              setSelectedEdge(null);
            }}
            minZoom={0.25}
            maxZoom={4}
            connectionMode={ConnectionMode.Loose}
            connectionRadius={30}
            onEdgeClick={onEdgeClick}
            onSelectionChange={onSelectionChange}
            onNodeDoubleClick={onNodeDoubleClick}
            onNodeClick={onNodeClick}
            onMove={(_event, viewport) => setZoomPercent(Math.round(viewport.zoom * 100))}
            panOnDrag={tool !== 'area-select'}
            selectionOnDrag={tool === 'area-select'}
            zoomOnDoubleClick={false}
            deleteKeyCode={null}
            fitView
            fitViewOptions={FIT_VIEW}
          >
            <Background />
            {showMap ? (
              <MiniMap
                pannable
                zoomable
                position="bottom-right"
                nodeColor="var(--muted)"
                nodeStrokeColor="var(--foreground)"
                maskColor="color-mix(in oklch, var(--background) 60%, transparent)"
                style={{ background: 'var(--card)', border: '1px solid var(--border)' }}
              />
            ) : null}
          </ReactFlow>
          <ToolPalette tool={tool} onTool={chooseTool} />
          <button
            type="button"
            aria-label="Toggle minimap"
            aria-pressed={showMap}
            title={showMap ? 'Hide the minimap' : 'Show the minimap'}
            className="tw:absolute tw:right-2 tw:bottom-2 tw:z-10 tw:flex tw:size-6 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border tw:bg-card tw:p-0 tw:text-muted-foreground tw:hover:text-foreground"
            onClick={() => setShowMap((was) => !was)}
          >
            <MapIcon className="tw:size-3.5" />
          </button>
          {picked ? (
            <div
              role="status"
              data-slot="model-link-hint"
              className="tw:pointer-events-none tw:absolute tw:top-2 tw:left-1/2 tw:z-10 tw:-translate-x-1/2 tw:rounded-md tw:border tw:bg-popover tw:px-3 tw:py-1.5 tw:text-xs tw:text-popover-foreground tw:shadow-md"
            >
              {`Linking ${picked.alias}.${picked.column}: click a column of another table (Esc cancels)`}
            </div>
          ) : null}
        </div>
        {panes.right ? (
          <PropertiesPanel
            def={def}
            name={name}
            selected={propertiesOf}
            columnsOf={columnsOfTable}
            onChange={change}
            onName={(next) => {
              setName(next);
              edited();
            }}
            onEditColumns={(alias) => setTableEdit({ kind: 'edit', alias })}
            onRenamed={(from, to) => setSelection((was) => was.map((id) => {
              const { alias, shortcut } = parseNodeId(id);
              if (alias !== from) {
                return id;
              }
              return shortcut ? shortcutNodeId(to, shortcut) : to;
            }))}
            onDeselect={() => setSelectedEdge(null)}
            onDeleteModel={() => setAsk('delete')}
            deleting={busy === 'delete'}
            busy={busy !== null}
          />
        ) : null}
      </div>

      <JoinDialog
        open={joinEdit !== null}
        from={joinEdit?.kind === 'new' ? joinEdit.from : editing?.from}
        to={joinEdit?.kind === 'new' ? joinEdit.to : editing?.to}
        initial={editing ? { relationship: editing.relationship, join: editing.join } : NEW_JOIN}
        onClose={() => setJoinEdit(null)}
        onSave={(choice) => {
          if (joinEdit?.kind === 'new') {
            change(connect(def, joinEdit.from, joinEdit.to, choice.relationship, choice.join));
          } else if (joinEdit?.kind === 'edit') {
            change(updateJoin(def, joinEdit.index, choice));
          }
          setJoinEdit(null);
        }}
        onDelete={joinEdit?.kind === 'edit' ? () => {
          change(removeJoin(def, joinEdit.index));
          setJoinEdit(null);
        } : undefined}
      />

      <TableDialog
        open={tableEdit !== null}
        initial={editedTable?.draft
          ? { schema: editedTable.schema, table: editedTable.table, columns: editedTable.draft.columns }
          : undefined}
        dataSource={def.dataSource}
        connections={Object.keys(tables || {})}
        schemasOf={(ds) => Object.keys(tables?.[ds] ?? {})}
        onSave={saveTable}
        onClose={() => setTableEdit(null)}
      />

      <CreateTablesDialog
        open={createAsk !== null}
        dataSource={def.dataSource}
        statements={createAsk?.statements ?? []}
        error={createAsk?.error}
        created={createAsk?.created}
        running={busy === 'create'}
        onCreate={runCreate}
        onClose={() => setCreateAsk(null)}
      />

      <AlertDialog open={ask !== null} onOpenChange={(open) => !open && setAsk(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{question.title}</AlertDialogTitle>
            <AlertDialogDescription>{question.text}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                setAsk(null);
                if (asked === 'delete') {
                  remove();
                } else {
                  runBuild(asked === 'overwrite');
                }
              }}
            >
              {question.ok}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

export function ModelEditor(props: ModelEditorProps) {
  return (
    <ReactFlowProvider>
      <Editor {...props} />
    </ReactFlowProvider>
  );
}

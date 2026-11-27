import '@xyflow/react/dist/style.css';

import {
  CSSProperties, DragEvent, useCallback, useEffect, useMemo, useRef, useState,
} from 'react';
import {
  Background, ConnectionMode, MiniMap, ReactFlow, ReactFlowProvider, applyNodeChanges, useReactFlow,
} from '@xyflow/react';
import type {
  Connection, EdgeMouseHandler, NodeChange, NodeMouseHandler, OnConnectEnd, OnNodeDrag,
  EdgeChange, NodeSelectionChange,
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
import { useRbac } from '../../../hooks';
import { problemLines } from '../editor';
import type { OrgDetail } from '../tree';
import { JoinChoice, JoinDialog } from './JoinDialog';
import {
  End, ModelDefinition, Place, Point, REL_LABEL, addDraftTable, addTable, canConnect, connect, drafts,
  emptyDefinition, joinLabel, layout, noteFormat, removeJoin, removeTable, setJoin, showAllJoins,
  updateDraftTable, updateJoin, withFormat,
} from './modelGraph';
import { TableFlowNode, TableNode, columnAt } from './TableNode';
import { NoteFlowNode, NoteNode } from './NoteNode';
import { AreaFlowNode, AreaNode } from './AreaNode';
import { ViewFlowNode, ViewNode } from './ViewNode';
import {
  CubeJoinEdge, CubeJoinFlowEdge, CubeSourceEdge, CubeSourceFlowEdge,
} from './CubeJoinEdge';
import {
  CUBE_SOURCE, CubeFlowNode, CubeNode, cubeSourceEdges,
} from './CubeNode';
import { CubeJoinDialog } from './CubeJoinDialog';
import { DependencyEdge, DependencyFlowEdge } from './DependencyEdge';
import {
  addArea, addNote, addView, bringToFront, insideArea, nodeIdOf, nodeIdsOf, objectOf, placeObjects, previewIdOf,
  sendToBack,
} from './objects';
import {
  addCube, addCubeJoin, autoMembers, cubeKeyOf, dimensionNames, memberTitles, removeCubeJoin, setCubeJoin,
  setCubeSourceLine,
} from './cubes';
import {
  Clip, addShortcut, copy, cut, parseNodeId, paste, removeShortcut, shortcutNodeId,
} from './clipboard';
import { useHistory } from './history';
import { Dialect, convertDrafts } from './dialect';
import { addReference, ensureIds } from './physical';
import { issues } from './validate';
import { ShareDialog } from './ShareDialog';
import { SqlDialog, SqlOptions } from './SqlDialog';
import { ExplorerDialog } from './explorer/ExplorerDialog';
import { explorerTables } from './explorer/explorerTables';
import { ExportImageDialog } from './export/ExportImageDialog';
import { PageBreaks } from './export/PageBreaks';
import { exportPdf } from './export/exportPdf';
import { documentationHtml } from './export/documentation';
import { download } from './export/download';
import { renderDiagram } from './export/renderDiagram';
import { diagramBounds, pageGrid } from './export/pages';
import { useEditorShortcuts } from './useEditorShortcuts';
import { ReferenceEdge, ReferenceFlowEdge } from './ReferenceEdge';
import {
  AlignMode, Box as Bounds, DistributeMode, align, distribute, sameSize,
} from './arrange';
import {
  Side, addPoint, endSides, orthogonalPath, reducePoint, swappedSides,
} from './referencePath';
import { Hit, searchModel } from './search';
import { SearchBox } from './SearchBox';
import { TOOLS, Tool, ToolPalette, VIEW_TOOLS } from './ToolPalette';
import { StructurePanel, TABLE_MIME } from './StructurePanel';
import { PropertiesPanel, Selected } from './PropertiesPanel';
import { ModelToolbar } from './ModelToolbar';
import { TableDialog, TableDraft } from './TableDialog';
import { CreateTablesDialog } from './CreateTablesDialog';
import { useEditLock } from './useEditLock';
import { LockErrorBanner, ReadOnlyBanner } from './ReadOnlyBanner';
import { ReadOnlyContext } from './PanelParts';

export { TABLE_MIME };

// The server's 409 when the cube's file changed since the last build: a
// build with `force` overwrites it. Its other 409 (the path is another
// file's) is not retried.
const EDITED = 'O cubo foi editado desde o último build';

const NODE_TYPES = {
  table: TableNode, note: NoteNode, area: AreaNode, view: ViewNode, cube: CubeNode,
};
const EDGE_TYPES = {
  reference: ReferenceEdge, dependency: DependencyEdge, cubejoin: CubeJoinEdge, cubesource: CubeSourceEdge,
};
// Areas are always behind everything else.
const AREA_Z = -1000;
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
type CanvasNode = TableFlowNode | NoteFlowNode | AreaFlowNode | ViewFlowNode | CubeFlowNode;
// A dragged area and what lies entirely within it, where each started.
type AreaDrag = { area: string; from: Point; members: { id: string; from: Point }[] };
type Columns = { name: string; type: string }[];

// What a build wrote: the cubes by name and the files it removed, by base name.
export type Built = { cubes: string[]; removed: string[] };

const baseName = (path: string) => path.slice(path.lastIndexOf('/') + 1);

// The toast after a build: `Built cubes a, b; removed x.yml`, or `Removed
// x.yml` when no cube was built; empty when the build did nothing.
export function builtMessage({ cubes, removed }: Built): string {
  const files = removed.join(', ');
  if (!cubes.length) {
    return files ? `Removed ${files}` : '';
  }
  return `Built cubes ${cubes.join(', ')}${files ? `; removed ${files}` : ''}`;
}

// The edge id of a cube join, kept in `selectedEdge` beside reference ids.
const CUBE_JOIN = 'cubejoin:';
const cubeJoinEdgeId = (id: string) => `${CUBE_JOIN}${id}`;

export type ModelEditorProps = {
  api: ModelApi;
  org: OrgNode;
  modelId: string;
  tables: OrgDetail['tables'];
  // After a build: what it built and removed, and the organization's
  // refreshed detail.
  onBuilt(built: Built, detail: OrgDetail): void;
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
// A cube join being made, between two cube ids.
// A join to make between two cubes, or (`id`) one to change.
type CubeJoinAsk = { from: string; to: string; id?: string };
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
// The server's 423 for a write while someone else holds the edit lock.
const LOCKED_BY = 'editado por ';
// What read-only drops from the shortcuts: the combos that change the model.
const EDIT_KEYS = ['mod+z', 'mod+y', 'mod+shift+z', 'mod+s', 'mod+v', 'mod+x', 'delete'];
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
  api, org, modelId, tables, onBuilt, onTablesCreated, onSaved, onDeleted, onDirtyChange,
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
  // The dialect each connection designs tables in, from the server.
  const [dialects, setDialects] = useState<Record<string, Dialect>>({});
  // Bumped by a double click on a draft: its first column takes the focus.
  const [focusKey, setFocusKey] = useState(0);
  // The Generate SQL dialog: open with its limits, options and answer.
  const [sqlAsk, setSqlAsk] = useState<{ only?: string[] } | null>(null);
  const [explorerOpen, setExplorerOpen] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const platformAdmin = Boolean(useRbac().session?.user.platformAdmin);
  const [imageOpen, setImageOpen] = useState(false);
  const [pdfBusy, setPdfBusy] = useState(false);
  const [sqlOptions, setSqlOptions] = useState<SqlOptions>({ drop: false, onlyNew: false });
  const [sqlScript, setSqlScript] = useState<{ script: string | null; error: string | null; loading: boolean }>(
    { script: null, error: null, loading: false }
  );
  const [version, setVersion] = useState(0);
  const history = useHistory<ModelDefinition>(emptyDefinition);
  const def = history.value;
  // While someone else holds the edit lock, or before the claim answers,
  // every change is ignored; a reload goes through `history.reset`.
  const readOnlyRef = useRef(true);
  const { set: record } = history;
  const setDef = useCallback((next: Parameters<typeof record>[0]) => {
    if (!readOnlyRef.current) {
      record(next);
    }
  }, [record]);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState<'save' | 'build' | 'delete' | 'create' | null>(null);
  const [joinEdit, setJoinEdit] = useState<JoinEdit | null>(null);
  const [cubeJoinAsk, setCubeJoinAsk] = useState<CubeJoinAsk | null>(null);
  // The cubes the last build wrote: a model without cubes still builds to
  // remove their files.
  const [builtNames, setBuiltNames] = useState<string[]>([]);
  const [tableEdit, setTableEdit] = useState<TableEdit | null>(null);
  const [createAsk, setCreateAsk] = useState<CreateAsk | null>(null);
  // The nodes selected on the canvas (tables and shortcuts) and the
  // selected reference, by index.
  const [selection, setSelection] = useState<string[]>([]);
  // The selected reference, by id: undo and deletes move places, not ids.
  const [selectedEdge, setSelectedEdge] = useState<string | null>(null);
  const joinsRef = useRef<ModelDefinition['joins']>([]);
  joinsRef.current = def.joins;
  const foundEdge = selectedEdge === null ? -1 : def.joins.findIndex((j) => j.id === selectedEdge);
  const edgeAt: number | null = foundEdge < 0 ? null : foundEdge;
  // The selected cube join, by id, while it exists.
  const cubeJoinAt = selectedEdge?.startsWith(CUBE_JOIN)
    ? (def.cubeJoins ?? []).find((j) => cubeJoinEdgeId(j.id) === selectedEdge)?.id ?? null
    : null;
  const first = selection.length ? objectOf(selection[0]) : null;
  // The selected table, for what only tables have.
  const selected = first?.kind === 'table' ? first.id : null;
  const [clip, setClip] = useState<Clip | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  // The reasons the ModelWriter gave for a change that does not compile.
  const [problems, setProblems] = useState<string[]>([]);
  const lastAsk = useRef<Ask>('overwrite');
  if (ask) {
    lastAsk.current = ask;
  }
  const [nodes, setNodes] = useState<CanvasNode[]>([]);
  // Where the PDF's pages break, over the whole diagram, when asked.
  const print = def.settings?.print;
  const pageBreaks = useMemo(
    () => (print?.showPageBreaks ? pageGrid(diagramBounds(nodes, 'all')!, print) : null),
    [nodes, print]
  );

  // The model as the server has it, replacing what is on the canvas and its
  // history.
  const showModel = (m: CubeModelView) => {
    setName(m.name);
    setVersion(m.version);
    setBuiltNames(m.cubeNames ?? []);
    const known = m.dialects ?? {};
    setDialects(known);
    const read = definitionOf(m.definition);
    // Old models' portable types as the connection's dialect writes them;
    // the next save stores them.
    history.reset(ensureIds(convertDrafts(read, known[read.dataSource] ?? null)));
    setDirty(false);
    setLoaded(true);
  };
  const reload = () => api.model(modelId).then(showModel, (e: Error) => notify.error(e.message));

  // Someone took the lock: what was not saved goes, the model as they left
  // it comes.
  const onLost = (holder: string) => {
    setJoinEdit(null);
    setCubeJoinAsk(null);
    setTableEdit(null);
    setCreateAsk(null);
    setAsk(null);
    setTool('select');
    setRefFrom(null);
    // A reload that fails says why; the changes are still on the canvas.
    api.model(modelId).then((m) => {
      showModel(m);
      notify.info(`${holder} took control. Your unsaved changes were discarded.`);
    }, (e: Error) => notify.error(e.message));
  };
  // Nothing is claimed for a model that could not be opened.
  const lock = useEditLock(api, modelId, onLost, {
    enabled: loaded && !loadError,
    onFreed: () => notify.info('You can edit this model now.'),
  });
  // Back in control after reading (Take control, or an expired lock
  // claimed): the model may have changed meanwhile, so it stays read-only
  // until the reload lands. Set while rendering, so no render unlocks first.
  const [lastStatus, setLastStatus] = useState(lock.status);
  const [regaining, setRegaining] = useState(false);
  if (lock.status !== lastStatus) {
    setLastStatus(lock.status);
    if (lock.status === 'editing' && (lastStatus === 'readOnly' || lastStatus === 'error')) {
      setRegaining(true);
    }
  }
  const readOnly = lock.status !== 'editing' || regaining;
  readOnlyRef.current = readOnly;
  // A 423 from a write: the lock is someone else's now.
  const refused = (e: unknown): boolean => {
    if (!(e instanceof ApiError && e.status === 423)) {
      return false;
    }
    const at = e.message.indexOf(LOCKED_BY);
    lock.lost(at >= 0 ? e.message.slice(at + LOCKED_BY.length) : lock.holder ?? '');
    return true;
  };
  useEffect(() => {
    if (regaining) {
      reload().finally(() => setRegaining(false));
    }
    // reload reads only stable setters and the api.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [regaining]);
  const take = () => lock.take().catch((e: Error) => notify.error(e.message));

  useEffect(() => {
    let cancelled = false;
    setLoaded(false);
    setLoadError(null);
    api.model(modelId).then(
      (m) => !cancelled && showModel(m),
      (e: Error) => !cancelled && setLoadError(e.message)
    );
    return () => {
      cancelled = true;
    };
    // showModel reads only stable setters.
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

  const edited = useCallback(() => {
    if (!readOnlyRef.current) {
      revision.current += 1;
      setDirty(true);
    }
  }, []);

  // Every reference keeps an id, so new links get one as they are made.
  const change = (next: ModelDefinition) => {
    setDef(ensureIds(next));
    edited();
  };

  const columnsOf = useCallback(
    (schema: string, table: string): Columns | undefined => tables?.[def.dataSource]?.[schema]?.[table],
    [tables, def.dataSource]
  );
  const missing = def.tables
    .filter((t) => !t.draft && !columnsOf(t.schema, t.table))
    .map((t) => `${t.schema}.${t.table}`);

  const columnsOfTable = (t: ModelDefinition['tables'][number]) => (t.draft
    ? t.draft.columns.map((c) => ({ name: c.name, type: c.type }))
    : columnsOf(t.schema, t.table) ?? []);
  const dialect: Dialect = dialects[def.dataSource] ?? null;
  // The same definition, the same problems: validation runs once per edit.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const problemsFound = useMemo(() => issues(def, columnsOfTable, dialect), [def, dialect, tables]);

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
    edited();
  }, [setDef, edited]);

  const onResize = useCallback((nodeId: string, box: { x: number; y: number; width: number; height: number }) => {
    const { shortcut } = objectOf(nodeId);
    const place: Place = shortcut ? { x: box.x, y: box.y } : box;
    setDef((d) => placeObjects(d, new Map([[nodeId, place]])));
    edited();
  }, [setDef, edited]);

  // The canvas' nodes follow the definition, keeping what React Flow
  // measured and selected on them.
  useEffect(() => {
    setNodes((previous) => {
      const kept = (id: string) => previous.find((n) => n.id === id);
      const showTypes = def.settings?.format?.showTypes ?? true;
      const areas = (def.areas ?? []).map((a): AreaFlowNode => {
        const id = nodeIdOf('area', a.id);
        return {
          ...(kept(id) as AreaFlowNode | undefined),
          id,
          type: 'area',
          position: { x: a.x, y: a.y },
          width: a.width,
          height: a.height,
          zIndex: AREA_Z + (a.z ?? 0),
          selected: selection.includes(id),
          data: {
            id: a.id,
            name: a.name,
            fill: a.fill,
            border: a.border,
            fillTransparency: a.fillTransparency,
            borderTransparency: a.borderTransparency,
            onResize,
            readOnly,
          },
        };
      });
      const views = (def.views ?? []).map((v): ViewFlowNode => {
        const id = nodeIdOf('view', v.id);
        return {
          ...(kept(id) as ViewFlowNode | undefined),
          id,
          type: 'view',
          position: { x: v.x, y: v.y },
          width: v.width,
          height: v.height,
          zIndex: v.z ?? 0,
          selected: selection.includes(id),
          data: {
            id: v.id,
            name: v.name,
            columns: v.columns,
            color: v.color,
            colorTransparency: v.colorTransparency,
            showTypes,
            fixedSize: v.width !== undefined || v.height !== undefined,
            onResize,
            readOnly,
          },
        };
      });
      const notes = (def.notes ?? []).map((n): NoteFlowNode => {
        const id = nodeIdOf('note', n.id);
        return {
          ...(kept(id) as NoteFlowNode | undefined),
          id,
          type: 'note',
          position: { x: n.x, y: n.y },
          width: n.width,
          height: n.height,
          zIndex: n.z ?? 0,
          selected: selection.includes(id),
          data: {
            id: n.id, text: n.text, color: n.color, format: noteFormat(n), onResize, readOnly,
          },
        };
      });
      const tableNodes = def.tables.flatMap((t) => {
        const columns = t.draft
          ? t.draft.columns.map((c) => ({ name: c.name, type: c.type }))
          : columnsOf(t.schema, t.table);
        const cubeKey = cubeKeyOf(def, t.alias);
        const keys = [
          ...(cubeKey ? [cubeKey] : []),
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
          missing: !columns,
          draft: Boolean(t.draft),
          color: t.color,
          bodyColor: t.bodyColor,
          colorTransparency: t.colorTransparency,
          bodyTransparency: t.bodyTransparency,
          keys,
          linked,
          fks,
          showTypes,
          showNullable: def.settings?.format?.showNullable ?? false,
          nullable: t.draft?.columns.filter((c) => c.nullable && !c.primaryKey).map((c) => c.name) ?? [],
          fixedSize: t.width !== undefined || t.height !== undefined,
          onResize,
          onRemove,
          readOnly,
          collapsed: collapsedTables.includes(t.alias),
          onToggle: toggleTable,
          picked: picked?.alias === t.alias ? picked.column : null,
          onPick,
        };
        // Undefined, not absent: the old node may still carry a size.
        const size = { width: t.width, height: t.height };
        const at = (id: string, x: number, y: number, shortcut?: string): TableFlowNode => ({
          ...(kept(id) as TableFlowNode | undefined),
          id,
          type: 'table' as const,
          position: { x, y },
          ...size,
          zIndex: 0,
          selected: selection.includes(id),
          data: shortcut ? { ...data, shortcut } : data,
        });
        return [
          at(t.alias, t.x, t.y),
          ...(t.shortcuts ?? []).map((sc) => at(shortcutNodeId(t.alias, sc.id), sc.x, sc.y, sc.id)),
        ];
      });
      const cubes = (def.cubes ?? []).map((c): CubeFlowNode => {
        const id = nodeIdOf('cube', c.id);
        const auto = c.dimensions.length === 0 && c.measures.length === 0;
        const shown = auto ? autoMembers(def, c, columnsOf) : c;
        return {
          ...(kept(id) as CubeFlowNode | undefined),
          id,
          type: 'cube',
          position: { x: c.x, y: c.y },
          width: c.width,
          height: c.height,
          zIndex: c.z ?? 0,
          selected: selection.includes(id),
          data: {
            cube: c,
            dimensions: shown.dimensions.map((d) => d.name),
            measures: shown.measures.map((m) => m.name),
            titles: memberTitles(c),
            issues: problemsFound.filter((i) => i.target.kind === 'cube' && i.target.id === c.id).length,
            fixedSize: c.width !== undefined || c.height !== undefined,
            onResize,
            readOnly,
          },
        };
      });
      return [...areas, ...tableNodes, ...views, ...cubes, ...notes];
    });
  }, [
    def, columnsOf, onRemove, onResize, collapsedTables, toggleTable, picked, onPick, selection, problemsFound,
    readOnly,
  ]);

  const onSetSide = useCallback((index: number, end: 'from' | 'to', side: Side) => {
    setDef((d) => setJoin(d, index, end === 'from' ? { fromSide: side } : { toSide: side }));
    edited();
  }, [setDef, edited]);

  const onSetPoints = useCallback((index: number, points: Point[]) => {
    setDef((d) => setJoin(d, index, { points }));
    edited();
  }, [setDef, edited]);

  const references: ReferenceFlowEdge[] = useMemo(() => def.joins.flatMap((j, index) => (j.hidden ? [] : [{
    id: `join-${index}`,
    source: j.from.alias,
    sourceHandle: j.from.column,
    target: j.to.alias,
    targetHandle: j.to.column,
    label: joinLabel(j),
    type: 'reference' as const,
    selected: Boolean(j.id) && selectedEdge === j.id,
    data: {
      index, join: j, label: joinLabel(j), onSetPoints, onSetSide,
    },
  }])), [def.joins, selectedEdge, onSetPoints, onSetSide]);

  // A line from each view to every table or view it depends on that exists.
  const dependencies: DependencyFlowEdge[] = useMemo(() => {
    const views = def.views ?? [];
    return views.flatMap((v) => [
      ...v.dependsOnTables.filter((alias) => def.tables.some((t) => t.alias === alias)),
      ...v.dependsOnViews.filter((id) => views.some((x) => x.id === id)).map((id) => nodeIdOf('view', id)),
    ].map((target) => ({
      id: `dep-${v.id}-${target}`,
      source: nodeIdOf('view', v.id),
      sourceHandle: 'node',
      target,
      targetHandle: 'node',
      type: 'dependency' as const,
      selectable: false,
      focusable: false,
      data: {},
    })));
  }, [def.views, def.tables]);
  const onSetCubeJoinSide = useCallback((id: string, end: 'from' | 'to', side: Side) => {
    setDef((d) => setCubeJoin(d, id, end === 'from' ? { fromSide: side } : { toSide: side }));
    edited();
  }, [setDef, edited]);

  const onSetCubeJoinPoints = useCallback((id: string, points: Point[]) => {
    setDef((d) => setCubeJoin(d, id, { points }));
    edited();
  }, [setDef, edited]);

  const onSetCubeSourceSide = useCallback((cubeId: string, table: string, end: 'from' | 'to', side: Side) => {
    setDef((d) => setCubeSourceLine(d, cubeId, table, end === 'from' ? { fromSide: side } : { toSide: side }));
    edited();
  }, [setDef, edited]);

  const onSetCubeSourcePoints = useCallback((cubeId: string, table: string, points: Point[]) => {
    setDef((d) => setCubeSourceLine(d, cubeId, table, { points }));
    edited();
  }, [setDef, edited]);

  const cubeLines: (CubeJoinFlowEdge | CubeSourceFlowEdge)[] = useMemo(() => [
    ...cubeSourceEdges(def, selectedEdge, { onSetPoints: onSetCubeSourcePoints, onSetSide: onSetCubeSourceSide }),
    ...(def.cubeJoins ?? []).filter((j) => !j.hidden).map((j): CubeJoinFlowEdge => ({
      id: cubeJoinEdgeId(j.id),
      source: nodeIdOf('cube', j.from),
      sourceHandle: 'join',
      target: nodeIdOf('cube', j.to),
      targetHandle: 'join',
      label: REL_LABEL[j.relationship],
      type: 'cubejoin',
      selected: selectedEdge === cubeJoinEdgeId(j.id),
      data: { join: j, onSetPoints: onSetCubeJoinPoints, onSetSide: onSetCubeJoinSide },
    })),
    // cubeSourceEdges reads only the tables and the cubes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [def.cubes, def.cubeJoins, def.tables, def.joins, selectedEdge, onSetCubeJoinPoints, onSetCubeJoinSide,
    onSetCubeSourcePoints, onSetCubeSourceSide]);
  const edges: (ReferenceFlowEdge | DependencyFlowEdge | CubeJoinFlowEdge | CubeSourceFlowEdge)[] = useMemo(
    () => [...references, ...dependencies, ...cubeLines],
    [references, dependencies, cubeLines]
  );

  // The selection follows only what the user does (React Flow's `select`
  // changes): listening to onSelectionChange too would echo the selection
  // the editor itself hands React Flow, and the two chase each other.
  const onNodesChange = useCallback((changes: NodeChange<CanvasNode>[]) => {
    const picks = changes.filter((c): c is NodeSelectionChange => c.type === 'select');
    if (picks.length) {
      setSelection((was) => {
        const next = was.filter((id) => !picks.some((c) => c.id === id && !c.selected));
        picks.forEach((c) => {
          if (c.selected && !next.includes(c.id)) {
            next.push(c.id);
          }
        });
        return sameIds(was, next) ? was : next;
      });
      if (picks.some((c) => c.selected)) {
        setSelectedEdge(null);
      }
    }
    // Tables leave through their own button, which updates the definition;
    // selection comes from `selection`. Read-only, what a resizer or a drag
    // would change stays off the canvas too: only measurements land.
    const kept = changes.filter((c) => c.type !== 'remove' && c.type !== 'select' && !(readOnlyRef.current && (
      c.type === 'position' || (c.type === 'dimensions' && (c.resizing !== undefined || c.setAttributes))
    )));
    if (kept.length) {
      setNodes((ns) => applyNodeChanges(kept, ns));
    }
  }, []);

  const onEdgesChange = useCallback((changes: EdgeChange[]) => {
    changes.forEach((c) => {
      if (c.type === 'select' && (c.id.startsWith(CUBE_JOIN) || c.id.startsWith(CUBE_SOURCE))) {
        if (c.selected) {
          setSelection([]);
          setSelectedEdge(c.id);
        } else {
          setSelectedEdge((was) => (was === c.id ? null : was));
        }
        return;
      }
      // View dependency lines are not selectable: references, cube joins and
      // cube source lines are.
      if (c.type !== 'select' || !c.id.startsWith('join-')) {
        return;
      }
      const index = Number(c.id.replace('join-', ''));
      const id = joinsRef.current[index]?.id ?? null;
      if (c.selected && id) {
        setSelection([]);
        setSelectedEdge(id);
      } else if (!c.selected) {
        setSelectedEdge((was) => (was === id ? null : was));
      }
    });
  }, []);

  const onNodeDoubleClick: NodeMouseHandler<CanvasNode> = (_event, node) => {
    const { kind } = objectOf(node.id);
    if (kind === 'note' || kind === 'view' || def.tables.some((t) => t.alias === node.id && t.draft)) {
      setSelection([node.id]);
      setSelectedEdge(null);
      setFocusKey((k) => k + 1);
    }
  };

  // The box of every node on the canvas, as React Flow sized it.
  const boxOf = (n: CanvasNode): Bounds => ({
    id: n.id,
    x: n.position.x,
    y: n.position.y,
    width: n.width ?? n.measured?.width ?? 0,
    height: n.height ?? n.measured?.height ?? 0,
  });

  // A dragged area takes along what lies entirely within it; what is dragged
  // itself already moves, and moves once.
  const areaDrags = useRef<AreaDrag[]>([]);
  const onNodeDragStart: OnNodeDrag<CanvasNode> = (_event, _node, dragged) => {
    const all = nodes.map(boxOf);
    const taken = new Set(dragged.map((n) => n.id));
    areaDrags.current = dragged.filter((n) => objectOf(n.id).kind === 'area').flatMap((n) => {
      const area = all.find((b) => b.id === n.id);
      if (!area) {
        return [];
      }
      const members = insideArea(area, all).filter((id) => !taken.has(id));
      members.forEach((id) => taken.add(id));
      return [{
        area: n.id,
        from: { x: area.x, y: area.y },
        members: members.map((id) => {
          const b = all.find((x) => x.id === id) as Bounds;
          return { id, from: { x: b.x, y: b.y } };
        }),
      }];
    });
  };

  // Where the members of the dragged areas are now.
  const membersAt = (dragged: CanvasNode[]): Map<string, Point> => {
    const places = new Map<string, Point>();
    areaDrags.current.forEach((d) => {
      const now = dragged.find((n) => n.id === d.area)?.position;
      if (!now) {
        return;
      }
      d.members.forEach((m) => places.set(m.id, {
        x: m.from.x + now.x - d.from.x, y: m.from.y + now.y - d.from.y,
      }));
    });
    return places;
  };

  const onNodeDrag: OnNodeDrag<CanvasNode> = (_event, _node, dragged) => {
    const places = membersAt(dragged);
    if (places.size) {
      setNodes((ns) => ns.map((n) => {
        const at = places.get(n.id);
        return at ? { ...n, position: at } : n;
      }));
    }
  };

  // One step of history per drag, whatever it moved, an area's contents too.
  const onNodeDragStop: OnNodeDrag<CanvasNode> = (_event, _node, dragged) => {
    const places = new Map<string, Place>(dragged.map((n) => [n.id, { x: n.position.x, y: n.position.y }]));
    membersAt(dragged).forEach((at, id) => places.set(id, at));
    areaDrags.current = [];
    change(placeObjects(def, places));
  };

  const copySelection = () => {
    if (selection.length) {
      setClip(copy(def, selection));
    }
  };

  const canPaste = Boolean(clip && (
    clip.tables.length || clip.notes.length || clip.areas.length || clip.views.length || clip.cubes.length
  ));
  const pasteClip = () => {
    if (!clip || !canPaste) {
      return;
    }
    const result = paste(def, clip, PASTE_OFFSET);
    change(result.def);
    setSelection(result.nodeIds);
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
    } else if (edgeAt !== null) {
      change(removeJoin(def, edgeAt));
      setSelectedEdge(null);
    } else if (cubeJoinAt !== null) {
      change(removeCubeJoin(def, cubeJoinAt));
      setSelectedEdge(null);
    }
  };

  // A selection whose reference undo or redo takes away selects nothing:
  // `edgeAt` finds no reference with its id.
  const undo = () => {
    if (history.canUndo && !readOnly) {
      history.undo();
      edited();
    }
  };

  const redo = () => {
    if (history.canRedo && !readOnly) {
      history.redo();
      edited();
    }
  };

  const link = (from: { alias: string; column: string }, to: { alias: string; column: string }) => {
    if (readOnly) {
      return;
    }
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

  // Two cubes join through the dialog; a cube and a table do not link.
  const onConnect = (c: Connection) => {
    if (readOnly) {
      return;
    }
    const ends = [c.source, c.target].map((id) => objectOf(id ?? ''));
    if (ends.some((e) => e.kind === 'cube')) {
      if (ends[0].kind === 'cube' && ends[1].kind === 'cube' && ends[0].id !== ends[1].id) {
        // Two cubes already joined, either way: their join, not a second one.
        const pair = [ends[0].id, ends[1].id];
        const existing = (def.cubeJoins ?? []).find((j) => pair.includes(j.from) && pair.includes(j.to));
        if (existing) {
          setSelection([]);
          setSelectedEdge(cubeJoinEdgeId(existing.id));
          setCubeJoinAsk({ from: existing.from, to: existing.to, id: existing.id });
        } else {
          setCubeJoinAsk({ from: ends[0].id, to: ends[1].id });
        }
      }
      return;
    }
    if (c.source && c.target && c.sourceHandle && c.targetHandle) {
      link(
        { alias: parseNodeId(c.source).alias, column: c.sourceHandle },
        { alias: parseNodeId(c.target).alias, column: c.targetHandle }
      );
    }
  };

  // A link dropped on a column's row, away from its handle.
  const onConnectEnd: OnConnectEnd = (event, state) => {
    if (state.isValid || !state.fromNode || !state.fromHandle?.id || objectOf(state.fromNode.id).kind === 'cube') {
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

  // Double-click a cube join to change it in its dialog.
  const onEdgeDoubleClick: EdgeMouseHandler = (_event, edge) => {
    const join = edge.id.startsWith(CUBE_JOIN)
      ? (def.cubeJoins ?? []).find((j) => cubeJoinEdgeId(j.id) === edge.id)
      : undefined;
    if (join && !readOnly) {
      setSelection([]);
      setSelectedEdge(edge.id);
      setCubeJoinAsk({ from: join.from, to: join.to, id: join.id });
    }
  };

  const onEdgeClick: EdgeMouseHandler = (_event, edge) => {
    if (edge.id.startsWith(CUBE_JOIN) || edge.id.startsWith(CUBE_SOURCE)) {
      setSelection([]);
      setSelectedEdge(edge.id);
      return;
    }
    if (!edge.id.startsWith('join-')) {
      return;
    }
    const index = Number((edge.data as { index?: number } | undefined)?.index);
    if (def.joins[index]) {
      setSelection([]);
      setSelectedEdge(def.joins[index].id ?? null);
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
      const saved = await api.saveModel(modelId, { name, definition: withFormat(def), version }, lock.token);
      setVersion(saved.version);
      if (revision.current === sent) {
        setDirty(false);
      }
      setBuiltNames(saved.cubeNames ?? []);
      onSaved?.(saved);
      return saved;
    } catch (e: any) {
      if (!refused(e)) {
        notify.error(conflict(e));
      }
      return null;
    } finally {
      setBusy(null);
    }
  };

  // The script of what is on the canvas, saved or not, asked again when the
  // options change.
  const sqlDef = useRef(def);
  sqlDef.current = def;
  useEffect(() => {
    if (!sqlAsk) {
      return undefined;
    }
    let cancelled = false;
    setSqlScript((was) => ({ ...was, loading: true, error: null }));
    api.modelSql(modelId, withFormat(sqlDef.current), {
      ...sqlOptions,
      // A table's preview shows that table, new or not.
      ...(sqlAsk.only ? { onlyNew: false, only: sqlAsk.only } : {}),
    }).then(
      (r) => !cancelled && setSqlScript({ script: r.script, error: null, loading: false }),
      (e: Error) => !cancelled && setSqlScript({ script: null, error: e.message, loading: false })
    );
    return () => {
      cancelled = true;
    };
  }, [api, modelId, sqlAsk, sqlOptions]);

  // Each opening starts from the model's SQL generation settings.
  const openSql = (limits: { only?: string[] }) => {
    setSqlOptions({ drop: def.settings?.sqlGeneration?.drop ?? false, onlyNew: false });
    setSqlAsk(limits);
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
      const result = await api.buildModel(modelId, force, lock.token);
      const names = result.cubes.map((c) => c.cubeName);
      setBuiltNames(names);
      onBuilt({ cubes: names, removed: (result.removed ?? []).map(baseName) }, result.detail);
    } catch (e: any) {
      if (refused(e)) {
        return;
      }
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
      await api.deleteModel(modelId, version, lock.token);
    } catch (e: any) {
      if (!refused(e)) {
        notify.error(conflict(e));
      }
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
      const result = await api.createTables(modelId, current, true, lock.token);
      setCreateAsk({ statements: result.statements ?? [], version: current, error: null });
    } catch (e: any) {
      if (!refused(e)) {
        notify.error(conflict(e));
      }
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
      const result = await api.createTables(modelId, createAsk.version, false, lock.token);
      if (result.model) {
        // What the server created is no draft any more: undoing past it
        // would offer to create it again.
        history.reset(ensureIds(definitionOf(result.model.definition)));
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
              statements = (await api.createTables(modelId, nextVersion, true, lock.token)).statements ?? [];
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
      if (refused(e)) {
        return;
      }
      setCreateAsk({ ...createAsk, error: conflict(e) });
    } finally {
      setBusy(null);
    }
  };

  // The selected nodes as boxes on the canvas, in selection order.
  const boxes = (): Bounds[] => selection.flatMap((id) => {
    const n = nodes.find((x) => x.id === id);
    return n ? [boxOf(n)] : [];
  });

  const moveTo = (places: Map<string, { x: number; y: number }>) => {
    if (places.size) {
      change(placeObjects(def, places));
    }
  };

  // A shortcut is sized as its table.
  const sizeKey = (nodeId: string) => (objectOf(nodeId).kind === 'table' ? parseNodeId(nodeId).alias : nodeId);
  const onSize = (kind: 'fit' | 'width' | 'height') => {
    const sizes = new Map<string, Place>();
    if (kind === 'fit') {
      selection.forEach((id) => sizes.set(sizeKey(id), { width: null, height: null }));
    } else {
      sameSize(boxes(), kind).forEach((size, id) => sizes.set(sizeKey(id), size));
    }
    if (sizes.size) {
      change(placeObjects(def, sizes));
    }
  };

  const canOrder = selection.some((id) => ['note', 'area'].includes(objectOf(id).kind));

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
    const [sa, sb] = endSides(a, b, Boolean(j.swapped), { from: j.fromSide, to: j.toSide });
    return orthogonalPath(
      { x: sa === 'right' ? a.x + a.width : a.x, y: a.y },
      sa,
      { x: sb === 'right' ? b.x + b.width : b.x, y: b.y },
      sb,
      j.points ?? []
    );
  };

  const onReference = (what: 'add' | 'reduce' | 'swap') => {
    const j = edgeAt === null ? undefined : def.joins[edgeAt];
    if (!j || edgeAt === null) {
      return;
    }
    if (what === 'swap') {
      change(setJoin(def, edgeAt, { swapped: !j.swapped, ...swappedSides(j) }));
    } else if (what === 'add') {
      change(setJoin(def, edgeAt, { points: addPoint(j.points ?? [], referencePath(edgeAt)) }));
    } else {
      change(setJoin(def, edgeAt, { points: reducePoint(j.points ?? []) }));
    }
  };

  const hits = searchModel(def, columnsOfTable, query);

  // The PDF goes at once, with the model's print settings, as Redgate's.
  const printPdf = async () => {
    if (!canvas.current) {
      return;
    }
    setPdfBusy(true);
    try {
      await exportPdf(canvas.current, diagramBounds(nodes, 'all')!, print, `${name || 'model'}.pdf`);
    } catch (e: any) {
      notify.error(e?.message ?? 'The PDF could not be made');
    } finally {
      setPdfBusy(false);
    }
  };
  // The documentation: one HTML file, with the diagram when it can be drawn.
  const generateDocs = async () => {
    setPdfBusy(true);
    try {
      let diagram: string | null = null;
      const bounds = diagramBounds(nodes, 'all');
      if (canvas.current && bounds) {
        try {
          diagram = await renderDiagram(canvas.current, bounds, 'svg');
        } catch {
          diagram = null;
        }
      }
      const style = getComputedStyle(document.documentElement);
      const colors = Object.fromEntries(
        ['--background', '--foreground', '--muted', '--border'].map((k) => [k, style.getPropertyValue(k).trim()])
      );
      const html = documentationHtml({
        name: name || 'model',
        description: def.description,
        def,
        columnsOf: columnsOfTable,
        diagram,
        colors,
        generatedAt: new Date().toISOString(),
      });
      download(new Blob([html], { type: 'text/html' }), `${name || 'model'}-documentation.html`);
    } catch (e: any) {
      notify.error(e?.message ?? 'The documentation could not be made');
    } finally {
      setPdfBusy(false);
    }
  };
  // The dimensions a cube join may pair, generated ones in auto mode.
  // The join the cube join dialog changes, while it still exists.
  const editedCubeJoin = cubeJoinAsk?.id ? (def.cubeJoins ?? []).find((j) => j.id === cubeJoinAsk.id) : undefined;
  const cubeDimensions = (id: string) => {
    const c = (def.cubes ?? []).find((x) => x.id === id);
    return c ? dimensionNames(def, c, columnsOf) : [];
  };

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
    setSelectedEdge(def.joins[index]?.id ?? null);
    const j = def.joins[index];
    if (j) {
      centerOn(j.from.alias);
    }
  };

  const pickCubeJoin = (id: string) => {
    const j = (def.cubeJoins ?? []).find((x) => x.id === id);
    if (j) {
      setSelection([]);
      setSelectedEdge(cubeJoinEdgeId(id));
      centerOn(nodeIdOf('cube', j.from));
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

  // The reference tool, as Redgate's: a click on the parent, then on the
  // child; the reference is made at once, a new child getting the column.
  const onNodeClick: NodeMouseHandler<CanvasNode> = (_event, node) => {
    if (tool !== 'reference' || objectOf(node.id).kind !== 'table') {
      return;
    }
    const { alias } = parseNodeId(node.id);
    if (!refFrom || refFrom === alias) {
      setRefFrom(alias);
      return;
    }
    const result = addReference(def, refFrom, alias, columnsOfTable, dialect);
    setRefFrom(null);
    setTool('select');
    if (result.id) {
      change(result.def);
      setSelection([]);
      setSelectedEdge(result.id);
    }
  };

  // Each tool keeps its own number, whatever is not ready yet; read-only
  // keeps the ones that only look.
  const toolKeys = Object.fromEntries(TOOLS.filter((t) => (
    !t.soon && (!readOnly || VIEW_TOOLS.includes(t.tool))
  )).map((t) => [
    String(TOOLS.indexOf(t) + 1), () => chooseTool(t.tool),
  ]));

  // The view, note and area tools place theirs where the canvas is clicked,
  // select it and go back to Select.
  const onPaneClick = (e: { clientX: number; clientY: number }) => {
    setPicked(null);
    setSelectedEdge(null);
    if (readOnly || (tool !== 'note' && tool !== 'area' && tool !== 'view' && tool !== 'cube')) {
      return;
    }
    const p = screenToFlowPosition({ x: e.clientX, y: e.clientY });
    let result: { def: ModelDefinition; id: string };
    if (tool === 'note') {
      result = addNote(def, p.x, p.y);
    } else if (tool === 'area') {
      result = addArea(def, p.x, p.y);
    } else if (tool === 'cube') {
      result = addCube(def, p.x, p.y);
    } else {
      result = addView(def, p.x, p.y, def.tables[0]?.schema ?? 'public');
    }
    change(result.def);
    setSelection([nodeIdOf(tool, result.id)]);
    setTool('select');
  };

  const shortcuts: Record<string, (e: KeyboardEvent) => void> = {
    ...toolKeys,
    'mod+f': () => searchInput.current?.focus(),
    'mod+g': () => dialect && openSql({}),
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
    'mod+a': () => setSelection(nodeIdsOf(def)),
    delete: deleteSelection,
  };
  if (readOnly) {
    EDIT_KEYS.forEach((combo) => delete shortcuts[combo]);
  }
  useEditorShortcuts(
    shortcuts,
    loaded && !loadError && !joinEdit && !cubeJoinAsk && !tableEdit && !createAsk && !ask && !sqlAsk
  );

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
      text: `Delete model ${name}? The cubes it built stay in Cubes.`,
      ok: 'Delete',
    },
  }[asked];

  let propertiesOf: Selected = { kind: 'model' };
  if (edgeAt !== null) {
    propertiesOf = { kind: 'reference', index: edgeAt };
  } else if (cubeJoinAt !== null) {
    propertiesOf = { kind: 'cubeJoin', id: cubeJoinAt };
  } else if (selected && def.tables.some((t) => t.alias === selected)) {
    propertiesOf = { kind: 'table', alias: selected };
  } else if (first && first.kind !== 'table' && nodeIdsOf(def).includes(selection[0])) {
    propertiesOf = { kind: first.kind, id: first.id };
  }
  const editing = joinEdit?.kind === 'edit' ? def.joins[joinEdit.index] : undefined;
  const editedTable = tableEdit?.kind === 'edit' ? def.tables.find((t) => t.alias === tableEdit.alias) : undefined;

  return (
    <div className="tw:flex tw:h-[calc(100vh-6rem)] tw:min-h-[560px] tw:flex-col tw:gap-3">
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

      <ModelToolbar
        readOnly={readOnly}
        history={{
          canUndo: history.canUndo, canRedo: history.canRedo, onUndo: undo, onRedo: redo,
        }}
        file={{
          dirty,
          busy: busy !== null,
          hasDrafts: drafts(def).length > 0,
          canBuild: (def.cubes?.length ?? 0) > 0 || builtNames.length > 0,
          onSave: () => save(),
          onCreate: openCreate,
          onBuild: () => runBuild(),
          canSql: dialect !== null,
          onSql: () => openSql({}),
        }}
        edit={{
          canCopy: selection.length > 0,
          canPaste,
          // A cube's line to its table goes with the cube, not on its own.
          canDelete: selection.length > 0 || (selectedEdge !== null && !selectedEdge.startsWith(CUBE_SOURCE)),
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
          order: {
            canOrder,
            onFront: () => change(bringToFront(def, selection)),
            onBack: () => change(sendToBack(def, selection)),
          },
        }}
        references={{
          edgeSelected: edgeAt !== null,
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
        explorer={{ onOpen: () => setExplorerOpen(true) }}
        share={{ onOpen: () => setShareOpen(true) }}
        exports={{ onImage: () => setImageOpen(true), onPdf: printPdf, onDocs: generateDocs, busy: pdfBusy }}
        search={(
          <SearchBox query={query} hits={hits} onQuery={setQuery} onPick={pickHit} inputRef={searchInput} />
        )}
      />

      {lock.status === 'readOnly' ? <ReadOnlyBanner holder={lock.holder} onTake={take} /> : null}
      {lock.status === 'error' ? <LockErrorBanner onRetry={() => lock.retry()} /> : null}

      <div className="tw:flex tw:min-h-0 tw:flex-1 tw:overflow-hidden tw:rounded-md tw:border">
        {panes.left ? (
          <StructurePanel
            def={def}
            columnsOf={columnsOfTable}
            selection={selection}
            selectedEdge={edgeAt}
            onPickNode={pickNode}
            onPickEdge={pickEdge}
            selectedCubeJoin={cubeJoinAt}
            onPickCubeJoin={pickCubeJoin}
            issues={problemsFound}
            onPickIssue={(issue) => {
              if (issue.target.kind === 'table') {
                pickNode(issue.target.alias);
              } else if (issue.target.kind === 'view') {
                pickNode(nodeIdOf('view', issue.target.id));
              } else if (issue.target.kind === 'cube') {
                pickNode(nodeIdOf('cube', issue.target.id));
              } else if (issue.target.kind === 'cubeJoin') {
                pickCubeJoin(issue.target.id);
              } else if (issue.target.kind === 'reference') {
                const { id } = issue.target;
                const at = def.joins.findIndex((j) => j.id === id);
                if (at >= 0) {
                  pickEdge(at);
                }
              }
            }}
            tables={tables}
            dataSource={def.dataSource}
            locked={def.tables.length > 0}
            readOnly={readOnly}
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
            // Selection never reorders, as in Redgate: a selected area stays
            // behind the tables, whatever its z.
            elevateNodesOnSelect={false}
            colorMode={theme}
            style={FLOW_STYLE}
            onNodesChange={onNodesChange}
            onNodeDragStart={onNodeDragStart}
            onNodeDrag={onNodeDrag}
            onNodeDragStop={onNodeDragStop}
            onConnect={onConnect}
            onConnectEnd={onConnectEnd}
            nodesDraggable={!readOnly}
            nodesConnectable={!readOnly}
            onPaneClick={onPaneClick}
            minZoom={0.25}
            maxZoom={4}
            connectionMode={ConnectionMode.Loose}
            connectionRadius={30}
            onEdgeClick={onEdgeClick}
            onEdgeDoubleClick={onEdgeDoubleClick}
            onEdgesChange={onEdgesChange}
            onNodeDoubleClick={onNodeDoubleClick}
            onNodeClick={onNodeClick}
            onMove={(_event, viewport) => setZoomPercent(Math.round(viewport.zoom * 100))}
            panOnDrag={tool !== 'area-select'}
            // The reference tool picks tables itself: a selection by React Flow
            // on the same click would fight the reference it selects.
            elementsSelectable={tool !== 'reference'}
            selectionOnDrag={tool === 'area-select'}
            zoomOnDoubleClick={false}
            deleteKeyCode={null}
            fitView
            fitViewOptions={FIT_VIEW}
          >
            <Background />
            {pageBreaks ? <PageBreaks pages={pageBreaks} /> : null}
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
          <ToolPalette tool={tool} onTool={chooseTool} readOnly={readOnly} />
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
          // Read-only: every field of the pane is off; its sections still
          // open, and SQL preview still shows.
          <ReadOnlyContext.Provider value={readOnly}>
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
              dialect={dialect}
              issues={problemsFound}
              focusKey={focusKey}
              // A table by its alias, a view by its id.
              onSqlPreview={dialect ? (nodeId) => {
                const only = previewIdOf(nodeId);
                if (only) {
                  openSql({ only: [only] });
                }
              } : undefined}
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
          </ReadOnlyContext.Provider>
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

      <CubeJoinDialog
        open={cubeJoinAsk !== null}
        from={(def.cubes ?? []).find((c) => c.id === cubeJoinAsk?.from)?.name ?? ''}
        to={(def.cubes ?? []).find((c) => c.id === cubeJoinAsk?.to)?.name ?? ''}
        fromDimensions={cubeJoinAsk ? cubeDimensions(cubeJoinAsk.from) : []}
        toDimensions={cubeJoinAsk ? cubeDimensions(cubeJoinAsk.to) : []}
        initial={editedCubeJoin ? { relationship: editedCubeJoin.relationship, on: editedCubeJoin.on } : undefined}
        onClose={() => setCubeJoinAsk(null)}
        onSave={(choice) => {
          if (editedCubeJoin) {
            change(setCubeJoin(def, editedCubeJoin.id, choice));
          } else if (cubeJoinAsk) {
            // Joined meanwhile (an undo, a redo): keep the one join.
            const pair = [cubeJoinAsk.from, cubeJoinAsk.to];
            const existing = (def.cubeJoins ?? []).find((j) => pair.includes(j.from) && pair.includes(j.to));
            let joinId = existing?.id;
            if (!joinId) {
              const result = addCubeJoin(def, cubeJoinAsk.from, cubeJoinAsk.to, choice.relationship, choice.on);
              change(result.def);
              joinId = result.id;
            }
            setSelection([]);
            setSelectedEdge(cubeJoinEdgeId(joinId));
          }
          setCubeJoinAsk(null);
        }}
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

      <ShareDialog
        open={shareOpen}
        onOpenChange={setShareOpen}
        orgName={org.name}
        platformAdmin={platformAdmin}
      />

      <SqlDialog
        open={sqlAsk !== null}
        name={name}
        only={sqlAsk?.only}
        script={sqlScript.script}
        error={sqlScript.error}
        loading={sqlScript.loading}
        options={sqlOptions}
        onOptions={setSqlOptions}
        onClose={() => setSqlAsk(null)}
      />

      <ExplorerDialog
        open={explorerOpen}
        name={name}
        tables={() => explorerTables(def, columnsOfTable)}
        onFind={pickNode}
        onClose={() => setExplorerOpen(false)}
      />

      <ExportImageDialog
        open={imageOpen}
        name={name}
        nodes={nodes}
        target={() => canvas.current}
        onClose={() => setImageOpen(false)}
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

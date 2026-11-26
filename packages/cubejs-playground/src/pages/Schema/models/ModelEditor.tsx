import '@xyflow/react/dist/style.css';

import {
  CSSProperties, DragEvent, useCallback, useEffect, useMemo, useRef, useState,
} from 'react';
import {
  Background, Controls, ReactFlow, ReactFlowProvider, applyNodeChanges, useReactFlow,
} from '@xyflow/react';
import type {
  Connection, Edge, EdgeMouseHandler, NodeChange, OnNodeDrag,
} from '@xyflow/react';
import { ChevronDown, ChevronRight, CircleAlert } from 'lucide-react';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  EmptyState, Field, SearchInput, SimpleSelect, Spinner, notify,
} from '@/components/kit';

import { ApiError, CubeModelView, OrgNode, modelApi } from '../api';
import { problemLines } from '../editor';
import type { OrgDetail } from '../tree';
import { JoinChoice, JoinDialog } from './JoinDialog';
import {
  End, ModelDefinition, addTable, canConnect, connect, emptyDefinition, joinLabel, removeJoin,
  removeTable, setPrimary, unlinked, updateJoin,
} from './modelGraph';
import { TableFlowNode, TableNode } from './TableNode';
import { PanelToggle } from '../PanelToggle';
import { useCollapsed } from '../useCollapsed';
import { useResizable } from '../useResizable';
import { ResizeHandle } from '../ResizeHandle';

// What a table of the panel carries while dragged to the canvas.
export const TABLE_MIME = 'application/x-cube-table';

// The server's 409 when the cube's file changed since the last build: a
// build with `force` overwrites it. Its other 409 (the path is another
// file's) is not retried.
const EDITED = 'O cubo foi editado desde o último build';

const NODE_TYPES = { table: TableNode };
const NEW_JOIN: JoinChoice = { relationship: 'many_to_one', join: 'left' };

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
  // After a save: the tree shows the model's name.
  onSaved?(saved: CubeModelView): void;
  // After the model is deleted.
  onDeleted?(): void;
  // Tells the page when there are changes not saved, so leaving can warn.
  onDirtyChange?(dirty: boolean): void;
};

type JoinEdit = { kind: 'new'; from: End; to: End } | { kind: 'edit'; index: number };
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

// The tables panel's width in px: where it starts and how far its edge drags.
const PANEL_WIDTH = { initial: 256, min: 180, max: 520 };

// A table of the panel: dragged to the canvas, its columns shown on demand.
function TableItem({
  ds, schema, table, columns, disabled, dataSource,
}: {
  ds: string;
  schema: string;
  table: string;
  columns: { name: string; type: string }[];
  disabled: boolean;
  dataSource: string;
}) {
  const [open, setOpen] = useState(false);
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <li>
      <div className="tw:flex tw:items-center">
        <button
          type="button"
          data-slot="model-table-expand"
          aria-label={`${open ? 'Hide' : 'Show'} the columns of ${table}`}
          aria-expanded={open}
          disabled={columns.length === 0}
          className="tw:flex tw:size-5 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:disabled:invisible"
          onClick={() => setOpen((was) => !was)}
        >
          <Chevron className="tw:size-3.5" />
        </button>
        <div
          data-slot="model-table-item"
          draggable={!disabled}
          aria-disabled={disabled || undefined}
          title={disabled
            ? `Tables of one connection only: this model reads ${dataSource}`
            : `${ds}.${schema}.${table}`}
          className={disabled
            ? 'tw:min-w-0 tw:flex-1 tw:cursor-not-allowed tw:truncate tw:rounded tw:px-1 tw:py-0.5 tw:text-muted-foreground tw:opacity-50'
            : 'tw:min-w-0 tw:flex-1 tw:cursor-grab tw:truncate tw:rounded tw:px-1 tw:py-0.5 tw:hover:bg-accent'}
          onDragStart={(e) => {
            if (disabled) {
              e.preventDefault();
              return;
            }
            e.dataTransfer.setData(TABLE_MIME, JSON.stringify({ ds, schema, table }));
            e.dataTransfer.effectAllowed = 'move';
          }}
        >
          {table}
        </div>
      </div>
      {open ? (
        <ul data-slot="model-table-columns" className="tw:m-0 tw:list-none tw:p-0 tw:pl-6 tw:text-xs">
          {columns.map((c) => (
            <li key={c.name} className="tw:flex tw:gap-2 tw:py-0.5">
              <span className="tw:min-w-0 tw:flex-1 tw:truncate">{c.name}</span>
              <span className="tw:shrink-0 tw:text-muted-foreground">{c.type}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </li>
  );
}

// The tables of the panel, ds › schema › table, matching the search.
function TablePanel({
  tables, dataSource, locked,
}: {
  tables: OrgDetail['tables'];
  dataSource: string;
  // The model has tables: only its connection's can join it.
  locked: boolean;
}) {
  const [search, setSearch] = useState('');
  const [collapsed, toggle] = useCollapsed('cubePlayground.modelTablesCollapsed');
  const [width, setWidth] = useResizable('cubePlayground.modelTablesWidth', PANEL_WIDTH.initial, PANEL_WIDTH.min, PANEL_WIDTH.max);
  const needle = search.trim().toLowerCase();
  const tree = Object.entries(tables || {}).map(([ds, schemas]) => ({
    ds,
    schemas: Object.entries(schemas).map(([schema, list]) => ({
      schema,
      tables: Object.entries(list).filter(([t]) => (
        !needle || [ds, schema, t].some((part) => part.toLowerCase().includes(needle))
      )).map(([t, columns]) => ({ name: t, columns: columns || [] })),
    })).filter((s) => s.tables.length > 0),
  })).filter((d) => d.schemas.length > 0);

  if (collapsed) {
    return (
      <aside
        data-slot="model-table-panel"
        className="tw:flex tw:w-12 tw:shrink-0 tw:flex-col tw:items-center tw:border-r tw:py-3"
      >
        <PanelToggle collapsed onToggle={toggle} label="tables" />
      </aside>
    );
  }

  return (
    <aside
      data-slot="model-table-panel"
      style={{ width }}
      className="tw:relative tw:flex tw:shrink-0 tw:flex-col tw:gap-2 tw:border-r tw:p-3"
    >
      <ResizeHandle width={width} min={PANEL_WIDTH.min} max={PANEL_WIDTH.max} label="tables" onResize={setWidth} />
      <div className="tw:flex tw:items-center tw:gap-1">
        <div className="tw:min-w-0 tw:flex-1">
          <SearchInput value={search} onChange={setSearch} placeholder="Search tables" />
        </div>
        <PanelToggle collapsed={false} onToggle={toggle} label="tables" />
      </div>
      <p className="tw:m-0 tw:text-xs tw:text-muted-foreground">Drag a table onto the canvas</p>
      <div className="tw:min-h-0 tw:flex-1 tw:overflow-y-auto">
        {tree.length === 0 ? <EmptyState>No table matches</EmptyState> : null}
        <ul className="tw:m-0 tw:list-none tw:p-0 tw:text-sm">
          {tree.map(({ ds, schemas }) => {
            const disabled = locked && ds !== dataSource;
            return (
              <li key={ds} className="tw:mb-2">
                <div className="tw:font-medium">{ds}</div>
                <ul className="tw:m-0 tw:list-none tw:p-0 tw:pl-3">
                  {schemas.map(({ schema, tables: names }) => (
                    <li key={schema}>
                      <div className="tw:text-muted-foreground">{schema}</div>
                      <ul className="tw:m-0 tw:list-none tw:p-0 tw:pl-3">
                        {names.map(({ name: table, columns }) => (
                          <TableItem
                            key={table}
                            ds={ds}
                            schema={schema}
                            table={table}
                            columns={columns}
                            disabled={disabled}
                            dataSource={dataSource}
                          />
                        ))}
                      </ul>
                    </li>
                  ))}
                </ul>
              </li>
            );
          })}
        </ul>
      </div>
    </aside>
  );
}

function Editor({
  api, modelId, tables, onBuilt, onSaved, onDeleted, onDirtyChange,
}: ModelEditorProps) {
  const theme = useDocumentTheme();
  // Bumped on every edit: a save clears `dirty` only when nothing changed
  // while it was on its way.
  const revision = useRef(0);
  const { screenToFlowPosition } = useReactFlow();

  const [loadError, setLoadError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [name, setName] = useState('');
  const [version, setVersion] = useState(0);
  const [def, setDef] = useState<ModelDefinition>(emptyDefinition);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState<'save' | 'build' | 'delete' | null>(null);
  const [joinEdit, setJoinEdit] = useState<JoinEdit | null>(null);
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
        setDef(definitionOf(m.definition));
        setDirty(false);
        setLoaded(true);
      },
      (e: Error) => !cancelled && setLoadError(e.message)
    );
    return () => {
      cancelled = true;
    };
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
  const missing = def.tables.filter((t) => !columnsOf(t.schema, t.table)).map((t) => `${t.schema}.${t.table}`);

  const onRemove = useCallback((alias: string) => {
    setDef((d) => removeTable(d, alias));
    revision.current += 1;
    setDirty(true);
  }, []);

  // The canvas' nodes follow the definition, keeping what React Flow
  // measured and selected on them.
  useEffect(() => {
    setNodes((previous) => def.tables.map((t) => {
      const old = previous.find((n) => n.id === t.alias);
      const columns = columnsOf(t.schema, t.table);
      return {
        ...old,
        id: t.alias,
        type: 'table' as const,
        position: { x: t.x, y: t.y },
        data: {
          alias: t.alias,
          schema: t.schema,
          table: t.table,
          columns: columns ?? [],
          principal: def.primary === t.alias,
          missing: !columns,
          onRemove,
        },
      };
    }));
  }, [def, columnsOf, onRemove]);

  const edges: Edge[] = useMemo(() => def.joins.map((j, index) => ({
    id: `join-${index}`,
    source: j.from.alias,
    sourceHandle: j.from.column,
    target: j.to.alias,
    targetHandle: j.to.column,
    label: joinLabel(j),
    data: { index },
  })), [def.joins]);

  const onNodesChange = useCallback((changes: NodeChange<TableFlowNode>[]) => {
    // Tables leave through their own button, which updates the definition.
    const kept = changes.filter((c) => c.type !== 'remove');
    setNodes((ns) => applyNodeChanges(kept, ns));
  }, []);

  const onNodeDragStop: OnNodeDrag<TableFlowNode> = (_event, _node, dragged) => {
    const moved = new Map(dragged.map((n) => [n.id, n.position]));
    change({
      ...def,
      tables: def.tables.map((t) => {
        const p = moved.get(t.alias);
        return p ? { ...t, x: Math.round(p.x), y: Math.round(p.y) } : t;
      }),
    });
  };

  const onConnect = (c: Connection) => {
    if (!c.source || !c.target || !c.sourceHandle || !c.targetHandle) {
      return;
    }
    const from = { alias: c.source, column: c.sourceHandle };
    const to = { alias: c.target, column: c.targetHandle };
    const error = canConnect(def, from, to);
    if (error) {
      notify.error(error);
      return;
    }
    setJoinEdit({ kind: 'new', from, to });
  };

  const onEdgeClick: EdgeMouseHandler = (_event, edge) => {
    const index = Number((edge.data as { index?: number } | undefined)?.index);
    if (def.joins[index]) {
      setJoinEdit({ kind: 'edit', index });
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

  const save = async (): Promise<boolean> => {
    setBusy('save');
    setProblems([]);
    const sent = revision.current;
    try {
      const saved = await api.saveModel(modelId, { name, definition: def, version });
      setVersion(saved.version);
      if (revision.current === sent) {
        setDirty(false);
      }
      onSaved?.(saved);
      return true;
    } catch (e: any) {
      notify.error(conflict(e));
      return false;
    } finally {
      setBusy(null);
    }
  };

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

  const loose = unlinked(def);
  const build = () => {
    if (loose.length) {
      setProblems([]);
      notify.error(`Link every table to the principal before building: ${loose.join(', ')}`);
    } else {
      runBuild();
    }
  };

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

  const principal = def.tables.find((t) => t.alias === def.primary);
  const principalColumns = principal ? columnsOf(principal.schema, principal.table) ?? [] : [];
  const editing = joinEdit?.kind === 'edit' ? def.joins[joinEdit.index] : undefined;

  return (
    <div className="tw:flex tw:h-full tw:min-h-[560px] tw:flex-col tw:gap-3">
      <div className="tw:flex tw:flex-wrap tw:items-end tw:gap-3">
        <Field label="Name" htmlFor="model-name" className="tw:w-56">
          <Input
            id="model-name"
            aria-label="Model name"
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              edited();
            }}
          />
        </Field>
        <div className="tw:flex tw:flex-col tw:gap-2 tw:text-sm">
          <span className="tw:font-medium">Connection</span>
          <span className="tw:flex tw:h-9 tw:items-center tw:text-muted-foreground" data-slot="model-connection">
            {def.dataSource || 'Drop a table to choose it'}
          </span>
        </div>
        <Field label="Principal" htmlFor="model-principal" className="tw:w-44">
          <SimpleSelect
            id="model-principal"
            value={def.primary ?? undefined}
            placeholder="No table yet"
            disabled={def.tables.length === 0}
            options={def.tables.map((t) => ({ value: t.alias, label: t.alias }))}
            onChange={(v) => v && change(setPrimary(def, v))}
          />
        </Field>
        <Field label="Primary key" htmlFor="model-primary-key" className="tw:w-44">
          <SimpleSelect
            id="model-primary-key"
            value={def.primaryKey ?? undefined}
            placeholder="Choose a column"
            disabled={principalColumns.length === 0}
            options={principalColumns.map((c) => ({ value: c.name, label: c.name }))}
            onChange={(v) => change({ ...def, primaryKey: v ?? null })}
          />
        </Field>
        <div className="tw:ml-auto tw:flex tw:items-center tw:gap-2">
          {dirty ? <Badge variant="secondary">Unsaved changes</Badge> : null}
          <Button variant="outline" disabled={!dirty || busy !== null} onClick={() => save()}>
            {busy === 'save' ? <Spinner className="tw:text-current" /> : null}
            Save
          </Button>
          <Button disabled={busy !== null || def.tables.length === 0} onClick={build}>
            {busy === 'build' ? <Spinner className="tw:text-current" /> : null}
            Build cube
          </Button>
          <Button variant="destructive" disabled={busy !== null} onClick={() => setAsk('delete')}>
            {busy === 'delete' ? <Spinner className="tw:text-current" /> : null}
            Delete model
          </Button>
        </div>
      </div>

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

      <div className="tw:flex tw:min-h-0 tw:flex-1 tw:overflow-hidden tw:rounded-md tw:border">
        <TablePanel tables={tables} dataSource={def.dataSource} locked={def.tables.length > 0} />
        <div
          className="tw:min-w-0 tw:flex-1"
          data-slot="model-canvas"
          onDragOver={onDragOver}
          onDrop={onDrop}
        >
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={NODE_TYPES}
            colorMode={theme}
            style={FLOW_STYLE}
            onNodesChange={onNodesChange}
            onNodeDragStop={onNodeDragStop}
            onConnect={onConnect}
            onEdgeClick={onEdgeClick}
            deleteKeyCode={null}
            fitView
            // A model of one or two tables would otherwise open at 2x.
            fitViewOptions={{ maxZoom: 1 }}
          >
            <Background />
            <Controls showInteractive={false} />
          </ReactFlow>
        </div>
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

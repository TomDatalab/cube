import { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Layout, Modal, Empty, Typography, Input, Tooltip, Button, Spin, message } from 'antd';
import type { DataNode } from 'antd/lib/tree';
import { Prompt, RouterProps } from 'react-router-dom';
import { Plus, Workflow } from 'lucide-react';
import { toast } from 'sonner';

import { Button as UiButton } from '@/components/ui/button';
import { EmptyState } from '@/components/kit';

import { playgroundAction } from '../../events';
import { Menu, Tree } from '../../components';
import { Alert, CubeLoader } from '../../atoms';
import { AppContext } from '../../components/AppContext';
import { ButtonDropdown } from '../../QueryBuilder/ButtonDropdown';
import { useOrganizationScope, useRbac, useWorkspace } from '../../hooks';
import { authHeaders, onUnauthorized, switchScope } from '../../shared/auth';
import { SchemaFormat } from '../../types';
import { CubeIcon } from '../../shared/icons/CubeIcon';
import { FileIcon } from '../../shared/icons/FileIcon';
import { TableIcon } from '../../shared/icons/TableIcon';
import { CubeChange, CubeDetail } from './CubeDetail';
import { CubeList } from './CubeList';
import { FileList, fileListRows, TableList, tableListRows } from './OrgLists';
import { cubeEditBlock, cubeFileOf, cubeUsers } from './cubeEdit';
import { removeCube } from './cubeYaml';
import { loadRows, openScope, orgWorkspace, previewHeaders, PreviewQuery } from './preview';
import { FileDetail } from './FileDetail';
import { fileHint, offersJavaScript } from './editor';
import { FileContent, OrgNode, PLATFORM, modelApi } from './api';
import { useOpenBlock } from './useOpenBlock';
import { ModelEditor } from './models/ModelEditor';
import { emptyDefinition } from './models/modelGraph';
import { NameDialog } from '../Admin/NameDialog';
import {
  OrgDetail,
  OrgSummary,
  TreeCube,
  buildQueryUrl,
  detailNodes,
  filterTree,
  iconKind,
  loadedTreeKeys,
  orgNodes,
  parseKey,
  tablesFromSameOrg,
} from './tree';

const { Content, Sider } = Layout;

const PAGE_SIZE = 50;
const MORE_KEY = 'more:';

const TREE_ICONS = { cube: CubeIcon, table: TableIcon, file: FileIcon, model: Workflow };

// The full name a tooltip shows: a file's whole path, else the title.
function fullName(node: DataNode): string | undefined {
  const key = String(node.key);
  if (key.startsWith('file:')) {
    return parseKey(key).rest;
  }
  return typeof node.title === 'string' ? node.title : undefined;
}

// The node's name, after its kind's icon when it has one. A name too long for
// the sidebar ends in an ellipsis; the tooltip has all of it.
function TreeTitle({ node }: { node: DataNode }) {
  const kind = iconKind(String(node.key));
  const Icon = kind ? TREE_ICONS[kind] : null;
  return (
    <span className="schema-tree-title" title={fullName(node)}>
      {Icon ? (
        <span className={`schema-tree-icon schema-tree-icon-${kind}`}>
          <Icon />
        </span>
      ) : null}
      <span className="schema-tree-label">{node.title}</span>
    </span>
  );
}
const LEAVE_MODEL = 'This model has changes not saved. Leave without saving?';

// The right panel for an organization's "Models" node: its models, each
// opening its editor, and "New model". Models need an organization (an id).
function ModelList({
  orgName, models, canCreate, onOpen, onNew,
}: {
  orgName: string;
  models: NonNullable<OrgDetail['models']>;
  canCreate: boolean;
  onOpen: (id: string) => void;
  onNew: () => void;
}) {
  return (
    <div data-testid="model-list" className="tw:flex tw:flex-col tw:gap-3">
      <div className="tw:flex tw:items-center tw:justify-between tw:gap-3">
        <h4 className="tw:m-0 tw:text-lg tw:font-medium">{`Models of ${orgName} (${models.length})`}</h4>
        <UiButton
          disabled={!canCreate}
          title={canCreate ? undefined : 'Models belong to an organization'}
          onClick={onNew}
        >
          <Plus />
          New model
        </UiButton>
      </div>
      {models.length === 0 ? (
        <EmptyState>No models yet: a model joins tables into one cube</EmptyState>
      ) : (
        <ul className="tw:m-0 tw:list-none tw:divide-y tw:rounded-md tw:border tw:p-0">
          {models.map((m) => (
            <li key={m.id}>
              <button
                type="button"
                data-slot="model-list-item"
                className="tw:flex tw:w-full tw:cursor-pointer tw:items-center tw:justify-between tw:gap-3 tw:border-0 tw:bg-transparent tw:px-3 tw:py-2 tw:text-left tw:text-sm tw:text-foreground tw:hover:bg-accent"
                onClick={() => onOpen(m.id)}
              >
                <span className="tw:truncate">{m.name}</span>
                <span className="tw:text-muted-foreground">{m.cubeName ? `cube ${m.cubeName}` : 'not built yet'}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

const SEARCH_DEBOUNCE_MS = 300;
const SEARCH_MIN_CHARS = 2;

// A cube's panel: its members come with the model, and the button needs the
// organization's workspaces.
// Its members are edited in its file: read fresh, changed, saved with the
// version read (a concurrent change makes the save a 409, not a loss).
function CubePanel({
  api, org, owner, basePath, cube, cubes, errors, files, onOpen, onSaved, onDeleted,
}: {
  api: ReturnType<typeof modelApi>;
  org: OrgNode | undefined;
  owner: boolean;
  basePath: string;
  cube: TreeCube;
  // The organization's cubes and views: who uses this cube.
  cubes: TreeCube[];
  errors: OrgDetail['errors'];
  files: OrgDetail['files'];
  onOpen: () => void;
  onSaved: (detail: OrgDetail) => void;
  onDeleted: () => void;
}) {
  const open = useOpenBlock(api, org, owner, cube);
  // The preview queries with the user's own access: an admin in the cube's
  // organization, an owner in the workspace chosen in the header.
  // The header may be in another organization: an owner queries in a
  // workspace of the cube's own (the chosen one when it is of it).
  const chosen = useWorkspace();
  const organization = useOrganizationScope();
  const workspace = orgWorkspace(chosen, open.workspaces);
  const scope = org ? previewHeaders({ isAdmin: !owner, slug: org.slug, workspace }) : null;
  // "Open in Playground" runs in the header's scope: move it to the cube's
  // organization first, or the query reaches a model without the cube.
  const openHere = () => {
    const next = org && openScope({
      isAdmin: !owner, slug: org.slug, workspace: chosen, organization, workspaces: open.workspaces ?? [],
    });
    if (next) {
      switchScope(next, onOpen);
    } else {
      onOpen();
    }
  };
  let previewBlock: string | null = null;
  if (!scope) {
    previewBlock = 'Choose a workspace in the header to preview this cube';
  } else if (open.disabled && open.hint) {
    previewBlock = open.hint;
  }
  const loadUrl = `${basePath.replace(/\/$/, '')}/v1/load`;
  const runQuery = scope && !open.disabled
    ? (query: PreviewQuery) => loadRows(loadUrl, { ...authHeaders(), ...scope }, query, (url, init) => (
      fetch(url, init).then((r) => {
        onUnauthorized(r);
        return r;
      })
    ))
    : null;
  const file = cubeFileOf(files, cube);
  const routed = Boolean(org && (org.slug === PLATFORM || org.id));
  const editBlock = cubeEditBlock({ cube, file, routed });
  const onEdit = async (change: CubeChange) => {
    if (!org || !file?.id) {
      return;
    }
    const current = await api.file(org, file.id);
    const next = change(current.content);
    if (next === current.content) {
      return;
    }
    onSaved(await api.save(org, file.id, next, current.version ?? file.version ?? 0));
  };
  // The cube leaves its file, read fresh; the file goes when the cube is all
  // it defines. The server refuses what breaks the model.
  const onDelete = async () => {
    if (!org || !file?.id) {
      return;
    }
    const current = await api.file(org, file.id);
    const version = current.version ?? file.version ?? 0;
    const next = removeCube(current.content, cube.name);
    if (next === null) {
      await api.remove(org, file.id, version);
    } else {
      await api.save(org, file.id, next, version);
    }
    onDeleted();
  };
  return (
    <CubeDetail
      cube={cube}
      errors={errors}
      open={open}
      onOpen={openHere}
      editBlock={editBlock}
      onEdit={onEdit}
      onDelete={onDelete}
      users={cubeUsers(cube.name, cubes)}
      preview={{
        runQuery, block: previewBlock, hidden: !owner,
      }}
    />
  );
}

export function SchemaPage({ history }: RouterProps) {
  const { playgroundContext } = useContext(AppContext);
  const rbac = useRbac();
  const token = rbac.session?.token;
  const api = useMemo(
    () => modelApi(rbac, playgroundContext.basePath),
    [rbac.isAdmin, token, playgroundContext.basePath]
  );

  const [orgs, setOrgs] = useState<OrgSummary[]>([]);
  const [total, setTotal] = useState(0);
  // Organizations the server's `?q=` returned for the current query.
  const [found, setFound] = useState<OrgSummary[]>([]);
  const [foundTotal, setFoundTotal] = useState(0);
  const [searching, setSearching] = useState(false);
  const [orgsLoading, setOrgsLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [loadError, setLoadError] = useState<Error | null>(null);
  const [details, setDetails] = useState<Record<string, OrgDetail>>({});
  const [detailErrors, setDetailErrors] = useState<Record<string, string>>({});
  const [expandedKeys, setExpandedKeys] = useState<string[]>([]);
  const [checkedKeys, setCheckedKeys] = useState<string[]>([]);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [file, setFile] = useState<FileContent | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const [fileReload, setFileReload] = useState(0);
  const [search, setSearch] = useState('');
  const [query, setQuery] = useState('');
  const [shown, setShown] = useState(false);
  // The open model's editor has changes not saved.
  const [modelDirty, setModelDirty] = useState(false);
  // The organization "New model" asks a name for.
  const [newModelIn, setNewModelIn] = useState<string | null>(null);

  // The organizations the lists returned, by slug.
  const known = useRef<Record<string, OrgSummary>>({});
  const detailRequests = useRef<Record<string, Promise<void> | undefined>>({});
  const baseRequest = useRef(0);
  const foundRequest = useRef(0);
  const loadingMoreRef = useRef(false);

  const [major, minor] = playgroundContext?.coreServerVersion
    ? playgroundContext.coreServerVersion.split('.')
    : [];
  const isYamlFormatSupported: boolean = (Number(major) > 0) || (!minor || Number(minor) >= 31);
  const isDocker = playgroundContext?.isDocker;

  useEffect(() => {
    [...orgs, ...found].forEach((o) => {
      known.current[o.slug] = o;
    });
  }, [orgs, found]);

  // The query the server sees: only 2+ characters search there.
  useEffect(() => {
    const term = search.trim();
    const timer = setTimeout(
      () => setQuery(term.length >= SEARCH_MIN_CHARS ? term : ''),
      SEARCH_DEBOUNCE_MS
    );
    return () => clearTimeout(timer);
  }, [search]);

  const fetchPage = (q: string, offset: number) => api.list(q, offset);

  // The unfiltered list, once.
  useEffect(() => {
    const request = ++baseRequest.current;
    (async () => {
      try {
        const result = await fetchPage('', 0);
        if (request === baseRequest.current) {
          setOrgs(result.organizations || []);
          setTotal(result.total || 0);
        }
      } catch (e: any) {
        if (request === baseRequest.current) {
          setLoadError(e);
        }
      } finally {
        if (request === baseRequest.current) {
          setOrgsLoading(false);
        }
      }
    })();
  }, []);

  // What the server finds for a query of 2+ characters.
  useEffect(() => {
    const request = ++foundRequest.current;
    if (!query) {
      setFound([]);
      setFoundTotal(0);
      setSearching(false);
      return;
    }
    setSearching(true);
    (async () => {
      try {
        const result = await fetchPage(query, 0);
        if (request === foundRequest.current) {
          setFound(result.organizations || []);
          setFoundTotal(result.total || 0);
        }
      } catch (e: any) {
        if (request === foundRequest.current) {
          message.error(e.message);
        }
      } finally {
        if (request === foundRequest.current) {
          setSearching(false);
        }
      }
    })();
  }, [query]);

  // The organizations already listed (every loaded page) and the search
  // results, fetched again so their counts follow a change of the model.
  const refreshOrgs = async () => {
    const base = ++baseRequest.current;
    try {
      const fresh: OrgSummary[] = [];
      const seen = new Set<string>();
      let freshTotal = 0;
      const wanted = Math.max(orgs.length, 1);
      for (let offset = 0; offset < wanted; offset += PAGE_SIZE) {
        // eslint-disable-next-line no-await-in-loop
        const result = await fetchPage('', offset);
        const page: OrgSummary[] = result.organizations || [];
        freshTotal = result.total || 0;
        page.forEach((o) => {
          if (!seen.has(o.slug)) {
            seen.add(o.slug);
            fresh.push(o);
          }
        });
        if (!page.length) {
          break;
        }
      }
      if (base === baseRequest.current) {
        setOrgs(fresh);
        setTotal(freshTotal);
      }
    } catch (e: any) {
      if (base === baseRequest.current) {
        message.error(e.message);
      }
    }
    if (query) {
      const request = ++foundRequest.current;
      try {
        const result = await fetchPage(query, 0);
        if (request === foundRequest.current) {
          setFound(result.organizations || []);
          setFoundTotal(result.total || 0);
        }
      } catch (e: any) {
        if (request === foundRequest.current) {
          message.error(e.message);
        }
      }
    }
  };

  const loadDetail = useCallback((slug: string, force = false): Promise<void> => {
    const existing = detailRequests.current[slug];
    if (!force && existing) {
      return existing;
    }
    const request = (async () => {
      try {
        const detail = await api.detail(known.current[slug] ?? { slug, name: slug });
        setDetails((current) => ({ ...current, [slug]: detail }));
        setDetailErrors((current) => {
          const { [slug]: _gone, ...rest } = current;
          return rest;
        });
      } catch (e: any) {
        delete detailRequests.current[slug];
        // No longer the owner: the organization leaves the tree.
        if (!rbac.isAdmin && (e.status === 403 || e.status === 404)) {
          setOrgs((current) => current.filter((o) => o.slug !== slug));
          setFound((current) => current.filter((o) => o.slug !== slug));
          return;
        }
        setDetailErrors((current) => ({ ...current, [slug]: e.message }));
      }
    })();
    detailRequests.current[slug] = request;
    return request;
  }, [api, rbac.isAdmin]);

  const loadMore = async () => {
    if (loadingMoreRef.current) {
      return;
    }
    const searchingNow = !!query;
    const counter = searchingNow ? foundRequest : baseRequest;
    const request = counter.current;
    const offset = searchingNow ? found.length : orgs.length;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    try {
      const result = await fetchPage(query, offset);
      if (request !== counter.current) {
        return;
      }
      const append = (current: OrgSummary[]) => [
        ...current,
        ...(result.organizations || []).filter(
          (o: OrgSummary) => !current.some((c) => c.slug === o.slug)
        ),
      ];
      if (searchingNow) {
        setFound(append);
        setFoundTotal(result.total || 0);
      } else {
        setOrgs(append);
        setTotal(result.total || 0);
      }
    } catch (e: any) {
      message.error(e.message);
    } finally {
      loadingMoreRef.current = false;
      setLoadingMore(false);
    }
  };

  const selected = useMemo(
    () => (selectedKey && selectedKey !== MORE_KEY ? parseKey(selectedKey) : null),
    [selectedKey]
  );

  // The file's id (its path without RBAC) and the node it belongs to.
  const fileId = useMemo(() => {
    if (selected?.kind !== 'file' || !selected.rest) {
      return null;
    }
    return details[selected.slug]?.files.find((f) => f.fileName === selected.rest)?.id ?? selected.rest;
  }, [selected, details]);

  useEffect(() => {
    setFile(null);
    setFileError(null);
    if (!selected || !fileId) {
      return undefined;
    }
    let cancelled = false;
    api.file(known.current[selected.slug] ?? { slug: selected.slug, name: selected.slug }, fileId).then(
      (result) => !cancelled && setFile(result),
      (e: Error) => !cancelled && setFileError(e.message)
    );
    return () => {
      cancelled = true;
    };
  }, [api, selected?.slug, fileId, fileReload]);

  const treeData = useMemo(() => {
    // The loaded list, plus what the server found that is not in it yet.
    const all = [...orgs, ...found.filter((f) => !orgs.some((o) => o.slug === f.slug))];
    const nodes: DataNode[] = orgNodes(all).map((node, index) => {
      const org = all[index];
      const detail = details[org.slug];
      return {
        ...node,
        title: (org.errors ?? 0) > 0 ? `${org.name} (${org.errors} errors)` : node.title,
        ...(detail ? { children: detailNodes(org.slug, detail) } : {}),
      };
    });
    const filtered = filterTree(nodes, search);
    const foundSlugs = new Set(found.map((f) => f.slug));
    const visible: DataNode[] = [];
    nodes.forEach((node) => {
      const match = filtered.find((f) => f.key === node.key);
      if (match) {
        visible.push(match);
      } else if (foundSlugs.has(String(node.key).slice('org:'.length))) {
        visible.push(node);
      }
    });
    const more = query ? found.length < foundTotal : orgs.length < total;
    if (more) {
      visible.push({
        key: MORE_KEY,
        title: loadingMore ? 'Carregando…' : 'Carregar mais…',
        isLeaf: true,
        checkable: false,
      });
    }
    return visible;
  }, [orgs, found, foundTotal, total, details, search, query, loadingMore]);

  // table key -> [`<dataSource>.<schema>`, table], as `generate-schema` expects.
  const tableRefs = useMemo(() => {
    const refs: Record<string, [string, string]> = {};
    Object.entries(details).forEach(([slug, detail]) => {
      Object.entries(detail.tables || {}).forEach(([ds, schemas]) => {
        Object.entries(schemas).forEach(([schema, tables]) => {
          Object.keys(tables).forEach((table) => {
            refs[`table:${slug}:${ds}.${schema}.${table}`] = [`${ds}.${schema}`, table];
          });
        });
      });
    });
    return refs;
  }, [details]);

  const checkedSlug = checkedKeys.length ? tablesFromSameOrg(checkedKeys) : null;
  const mixedOrgs = checkedKeys.length > 0 && checkedSlug === null;

  const generateSchema = async (format: SchemaFormat = SchemaFormat.js) => {
    if (!checkedSlug) {
      return;
    }
    const options = { format };
    playgroundAction('Generate Schema', options);
    const org = known.current[checkedSlug] ?? { slug: checkedSlug, name: checkedSlug };
    const tables = checkedKeys.map((k) => tableRefs[k]).filter(Boolean);
    const tablesSchema: Record<string, unknown> = {};
    Object.entries(details[checkedSlug]?.tables || {}).forEach(([ds, schemas]) => {
      Object.entries(schemas).forEach(([schema, list]) => {
        tablesSchema[`${ds}.${schema}`] = list;
      });
    });
    try {
      await api.generate(org, tables, { format, tablesSchema });
    } catch (e: any) {
      playgroundAction('Generate Schema Fail', { error: String(e), ...options });
      message.error(e.message);
      return;
    }

    playgroundAction('Generate Schema Success', options);
    await loadDetail(checkedSlug, true);
    setCheckedKeys([]);
    // The new cubes show in the organizations' counts.
    refreshOrgs();
    Modal.success({
      title: 'Data model files successfully generated!',
      content:
        'You can start exploring your data model and building the charts',
      okText: 'Build',
      cancelText: 'Close',
      okCancel: true,
      onOk() {
        history.push('/build');
      },
    });
  };

  // Selects a node; leaving a model with changes not saved asks first. It
  // reads refs: the toast after a build calls it later.
  const dirtyRef = useRef(false);
  dirtyRef.current = modelDirty;
  const selectedRef = useRef<string | null>(null);
  selectedRef.current = selectedKey;
  const select = (key: string | null): boolean => {
    // eslint-disable-next-line no-alert
    if (dirtyRef.current && key !== selectedRef.current && !window.confirm(LEAVE_MODEL)) {
      return false;
    }
    if (key !== selectedRef.current) {
      setModelDirty(false);
    }
    setSelectedKey(key);
    return true;
  };

  const openUnder = (slug: string, group: 'cubes' | 'models', key: string) => {
    if (select(key)) {
      setExpandedKeys((keys) => Array.from(new Set([...keys, `org:${slug}`, `org:${slug}:${group}`])));
    }
  };

  const createModel = async (slug: string, name: string) => {
    const org = known.current[slug] ?? { slug, name: slug };
    const created = await api.createModel(org, name, emptyDefinition());
    await loadDetail(slug, true);
    openUnder(slug, 'models', `model:${slug}:${created.id}`);
  };

  const afterBuild = async (slug: string, cubeName: string) => {
    await loadDetail(slug, true);
    refreshOrgs();
    toast.success(`Cube ${cubeName} built`, {
      action: { label: 'Open cube', onClick: () => openUnder(slug, 'cubes', `cube:${slug}:${cubeName}`) },
    });
  };

  const afterFileChange = (slug: string, others: string[] = []) => {
    [slug, ...others].forEach((s) => {
      if (details[s]) {
        loadDetail(s, true);
      }
    });
    refreshOrgs();
  };

  const renderContent = () => {
    if (!selected) {
      return (
        <Empty
          style={{ marginTop: 50 }}
          description="Select tables to generate Cube data model"
        />
      );
    }

    const detail = details[selected.slug];

    if (!detail && detailErrors[selected.slug]) {
      return (
        <Alert
          type="error"
          message="Could not load this organization"
          description={(
            <>
              <div>{detailErrors[selected.slug]}</div>
              <Button style={{ marginTop: 8 }} onClick={() => loadDetail(selected.slug, true)}>
                Tentar de novo
              </Button>
            </>
          )}
        />
      );
    }

    if (selected.kind === 'tables' || selected.kind === 'files') {
      if (!detail) {
        return <CubeLoader />;
      }
      const orgName = known.current[selected.slug]?.name ?? selected.slug;
      if (selected.kind === 'tables') {
        const rows = tableListRows(selected.slug, detail.tables);
        return (
          <TableList
            orgName={orgName}
            rows={rows}
            checkedKeys={checkedKeys}
            onCheck={(keys) => {
              // This organization's tables as checked here; the others' stay.
              const here = new Set(rows.map((r) => r.key));
              setCheckedKeys([...checkedKeys.filter((k) => !here.has(k)), ...keys]);
            }}
          />
        );
      }
      return (
        <FileList
          orgName={orgName}
          rows={fileListRows(selected.slug, detail.files || [], detail.cubes, detail.errors || [])}
          onOpen={(key) => {
            setExpandedKeys((keys) => Array.from(new Set([...keys, `org:${selected.slug}`, `files:${selected.slug}`])));
            setSelectedKey(key);
          }}
        />
      );
    }

    if (selected.kind === 'cubes' || selected.kind === 'views') {
      if (!detail) {
        return <CubeLoader />;
      }
      const orgKey = `org:${selected.slug}`;
      const group = selected.kind;
      return (
        <CubeList
          orgName={known.current[selected.slug]?.name ?? selected.slug}
          cubes={detail.cubes}
          errors={detail.errors || []}
          type={group === 'views' ? 'view' : 'cube'}
          onOpen={(name) => {
            // The node shows selected under its open "Cubes" or "Views" node.
            setExpandedKeys((keys) => Array.from(new Set([...keys, orgKey, `${orgKey}:${group}`])));
            setSelectedKey(`cube:${selected.slug}:${name}`);
          }}
        />
      );
    }

    if (selected.kind === 'models') {
      if (!detail) {
        return <CubeLoader />;
      }
      const org = known.current[selected.slug];
      return (
        <ModelList
          orgName={org?.name ?? selected.slug}
          models={detail.models || []}
          canCreate={Boolean(org?.id)}
          onOpen={(id) => openUnder(selected.slug, 'models', `model:${selected.slug}:${id}`)}
          onNew={() => setNewModelIn(selected.slug)}
        />
      );
    }

    if (selected.kind === 'model') {
      const org = known.current[selected.slug];
      if (!detail || !org || !selected.rest) {
        return <CubeLoader />;
      }
      return (
        <ModelEditor
          key={selected.rest}
          api={api}
          org={org}
          modelId={selected.rest}
          tables={detail.tables || {}}
          onBuilt={(cubeName) => afterBuild(selected.slug, cubeName)}
          onSaved={() => loadDetail(selected.slug, true)}
          onDirtyChange={setModelDirty}
        />
      );
    }

    if (selected.kind === 'cube') {
      const cube = detail?.cubes.find((c) => c.name === selected.rest);
      if (!cube) {
        return <CubeLoader />;
      }
      return (
        <CubePanel
          api={api}
          org={known.current[selected.slug]}
          owner={!rbac.isAdmin}
          basePath={playgroundContext.basePath ?? '/cube'}
          cube={cube}
          cubes={detail.cubes}
          errors={detail.errors || []}
          files={detail.files || []}
          onOpen={() => history.push(buildQueryUrl(cube))}
          onDeleted={() => {
            setSelectedKey(null);
            afterFileChange(selected.slug);
          }}
          onSaved={(saved) => {
            setDetails((current) => ({ ...current, [selected.slug]: saved }));
            refreshOrgs();
          }}
        />
      );
    }

    if (selected.kind === 'file') {
      if (fileError) {
        return <Alert type="error" message="Could not load the file" description={fileError} />;
      }
      if (!file) {
        return <CubeLoader />;
      }
      const hint = fileHint(rbac.enabled, Boolean(isDocker));
      return (
        <>
          {hint === 'docker' && (
            <Alert
              message={(
                <span>
                  Data model files are located and can be edited in the mount
                  volume directory.{' '}
                  <Typography.Link
                    href="https://docs.cube.dev/docs/data-modeling/overview"
                    target="_blank"
                  >
                    Learn more about working with Cube data model in the docs
                  </Typography.Link>
                </span>
              )}
              type="info"
              style={{ paddingTop: 10, paddingBottom: 11 }}
            />
          )}
          {hint === 'path' && (
            <Alert
              message={(
                <span>
                  This file can be edited at&nbsp;
                  <b>{file.absPath}</b>
                </span>
              )}
              type="info"
              style={{ paddingTop: 10, paddingBottom: 11 }}
            />
          )}
          <FileDetail
            api={api}
            org={known.current[selected.slug] ?? { slug: selected.slug, name: selected.slug }}
            id={fileId!}
            file={file}
            isAdmin={rbac.isAdmin}
            orgs={orgs}
            onSaved={(saved) => {
              setDetails((current) => ({ ...current, [selected.slug]: saved }));
              setFileReload((n) => n + 1);
              refreshOrgs();
            }}
            onRemoved={() => {
              setSelectedKey(null);
              afterFileChange(selected.slug);
            }}
            onMoved={(destination) => {
              const dest = destination
                ? Object.values(known.current).find((o) => o.id === destination)?.slug
                : PLATFORM;
              setSelectedKey(null);
              afterFileChange(selected.slug, dest ? [dest] : []);
            }}
            onReload={() => setFileReload((n) => n + 1)}
          />
        </>
      );
    }

    if (selected.kind === 'org') {
      const org = orgs.find((o) => o.slug === selected.slug);
      if (org && (org.errors ?? 0) > 0 && !detail) {
        return <CubeLoader />;
      }
      if (detail?.errors?.length) {
        return (
          <Alert
            type="error"
            message={`${org?.name ?? selected.slug}: ${detail.errors.length} errors`}
            description={detail.errors.map((e) => (
              <div key={`${e.file}:${e.cube}:${e.message}`}>
                <b>{e.file}</b>
                {e.cube ? ` (${e.cube})` : ''}: {e.message}
              </div>
            ))}
          />
        );
      }
    }

    return (
      <Empty
        style={{ marginTop: 50 }}
        description="Select tables to generate Cube data model"
      />
    );
  };

  const renderTree = () => {
    if (loadError) {
      return (
        <Alert
          data-testid="schema-error"
          message="Error while loading the data model"
          description={loadError.toString()}
          type="error"
        />
      );
    }
    if (orgsLoading) {
      return <CubeLoader />;
    }
    if (!treeData.length) {
      return (
        <Alert
          message="Empty data model"
          description="No organization matches"
          type="warning"
        />
      );
    }
    return (
      <Tree
        checkable
        checkStrictly
        treeData={treeData}
        titleRender={(node: DataNode) => <TreeTitle node={node} />}
        loadedKeys={loadedTreeKeys(treeData, Object.keys(details))}
        loadData={async (node: DataNode) => {
          const key = String(node.key);
          if (key !== MORE_KEY && parseKey(key).kind === 'org') {
            await loadDetail(parseKey(key).slug);
          }
        }}
        expandedKeys={expandedKeys}
        onExpand={(keys: any) => {
          playgroundAction('Expand Tables');
          setExpandedKeys(keys);
        }}
        checkedKeys={checkedKeys}
        onCheck={(checked: any) => {
          playgroundAction('Check Tables');
          const keys: string[] = Array.isArray(checked) ? checked : checked.checked;
          setCheckedKeys(keys.filter((k) => k.startsWith('table:')));
        }}
        selectedKeys={selectedKey ? [selectedKey] : []}
        onSelect={(keys: any[]) => {
          const key = keys.length ? String(keys[0]) : null;
          if (key === MORE_KEY) {
            loadMore();
            return;
          }
          if (!select(key)) {
            return;
          }
          if (key && parseKey(key).kind === 'org') {
            loadDetail(parseKey(key).slug);
          }
        }}
      />
    );
  };

  return (
    <Layout style={{ height: '100%' }}>
      <Sider width={340} className="schema-sidebar">
        <div style={{ display: 'flex', gap: 8, padding: 8, alignItems: 'center' }}>
          <Input
            allowClear
            placeholder="Search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            suffix={searching ? <Spin size="small" /> : undefined}
          />
          <Tooltip title={mixedOrgs ? 'Escolha tabelas de uma única organização' : ''}>
            <span>
              <ButtonDropdown
                show={shown}
                disabled={!checkedKeys.length || mixedOrgs}
                type="primary"
                data-testid="chart-type-btn"
                overlay={(
                  <Menu data-testid="generate-schema">
                    <Menu.Item
                      title={
                        !isYamlFormatSupported
                          ? 'yaml schema format is supported by Cube 0.31.0 and later'
                          : ''
                      }
                      disabled={!isYamlFormatSupported}
                      onClick={() => generateSchema(SchemaFormat.yaml)}
                    >
                      YAML
                    </Menu.Item>
                    {offersJavaScript(rbac.enabled) && (
                      <Menu.Item onClick={() => generateSchema()}>
                        JavaScript
                      </Menu.Item>
                    )}
                  </Menu>
                )}
                style={{ border: 0 }}
                onOverlayOpen={() => setShown(true)}
                onOverlayClose={() => setShown(false)}
                onItemClick={() => setShown(false)}
              >
                Generate Data Model
              </ButtonDropdown>
            </span>
          </Tooltip>
        </div>
        {renderTree()}
      </Sider>

      <Content style={{ minHeight: 280, padding: 24 }}>
        {renderContent()}
      </Content>
      <Prompt when={modelDirty} message={LEAVE_MODEL} />
      <NameDialog
        title={newModelIn ? 'New model' : null}
        onSubmit={({ name }) => createModel(newModelIn!, name)}
        onClose={() => setNewModelIn(null)}
      />
    </Layout>
  );
}

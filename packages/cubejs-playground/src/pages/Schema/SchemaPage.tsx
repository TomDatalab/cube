import { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Layout, Modal, Empty, Typography, Input, Tooltip, Button, Spin, message } from 'antd';
import type { DataNode } from 'antd/lib/tree';
import { RouterProps } from 'react-router-dom';

import PrismCode from '../../PrismCode';
import { playgroundAction } from '../../events';
import { Menu, Tree } from '../../components';
import { Alert, CubeLoader } from '../../atoms';
import { playgroundFetch } from '../../shared/helpers';
import { AppContext } from '../../components/AppContext';
import { ButtonDropdown } from '../../QueryBuilder/ButtonDropdown';
import { useSecurityContext } from '../../hooks';
import { createCubeApi } from '../../shared/cube-api';
import { SchemaFormat } from '../../types';
import { CubeIcon } from '../../shared/icons/CubeIcon';
import { FileIcon } from '../../shared/icons/FileIcon';
import { TableIcon } from '../../shared/icons/TableIcon';
import { CubeDetail, MetaCube } from './CubeDetail';
import {
  OrgDetail,
  OrgSummary,
  buildQueryUrl,
  detailNodes,
  filterTree,
  iconKind,
  metaApiUrl,
  metaToken,
  orgNodes,
  parseKey,
  tablesFromSameOrg,
} from './tree';

const { Content, Sider } = Layout;

const PAGE_SIZE = 50;
const MORE_KEY = 'more:';

const TREE_ICONS = { cube: CubeIcon, table: TableIcon, file: FileIcon };

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
const SEARCH_DEBOUNCE_MS = 300;
const SEARCH_MIN_CHARS = 2;

type OpenFile = { fileName: string; content: string; absPath: string };

const encodePath = (path: string) => path.split('/').map(encodeURIComponent).join('/');

async function readJson(res: Response) {
  if (res.status !== 200) {
    const text = await res.text().catch(() => '');
    throw new Error(text || `Request failed (${res.status})`);
  }
  return res.json();
}

export function SchemaPage({ history }: RouterProps) {
  const { playgroundContext, apiUrl: contextApiUrl } = useContext(AppContext);
  const { currentToken } = useSecurityContext();
  // Only the query builder sets the context's address; opened directly, the
  // page builds it, or meta would never load.
  const apiUrl = metaApiUrl(contextApiUrl, playgroundContext.basePath, window.location.href);
  // No workspace header: a platform admin sees the cubes of every organization.
  const token = metaToken(
    currentToken,
    playgroundContext.cubejsToken,
    Boolean(playgroundContext.rbac || playgroundContext.authRequired)
  );
  const cubeApi = useMemo(
    () => (apiUrl && token ? createCubeApi(token, apiUrl, { workspace: false }) : null),
    [apiUrl, token]
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
  const [metaError, setMetaError] = useState<string | null>(null);
  const [expandedKeys, setExpandedKeys] = useState<string[]>([]);
  const [checkedKeys, setCheckedKeys] = useState<string[]>([]);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [file, setFile] = useState<OpenFile | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  const [query, setQuery] = useState('');
  const [metaCubes, setMetaCubes] = useState<Record<string, MetaCube> | null>(null);
  const [metaLoading, setMetaLoading] = useState(true);
  const [shown, setShown] = useState(false);

  const detailRequests = useRef<Record<string, Promise<void> | undefined>>({});
  const baseRequest = useRef(0);
  const foundRequest = useRef(0);
  const loadingMoreRef = useRef(false);

  const [major, minor] = playgroundContext?.coreServerVersion
    ? playgroundContext.coreServerVersion.split('.')
    : [];
  const isYamlFormatSupported: boolean = (Number(major) > 0) || (!minor || Number(minor) >= 31);
  const isDocker = playgroundContext?.isDocker;

  // The query the server sees: only 2+ characters search there.
  useEffect(() => {
    const term = search.trim();
    const timer = setTimeout(
      () => setQuery(term.length >= SEARCH_MIN_CHARS ? term : ''),
      SEARCH_DEBOUNCE_MS
    );
    return () => clearTimeout(timer);
  }, [search]);

  const fetchPage = async (q: string, offset: number) => {
    const params = new URLSearchParams({ limit: String(PAGE_SIZE), offset: String(offset) });
    if (q) {
      params.set('q', q);
    }
    return readJson(await playgroundFetch(`playground/tree?${params}`));
  };

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

  // Bumped after a Generate Data Model, so meta is fetched again.
  const [metaVersion, setMetaVersion] = useState(0);

  useEffect(() => {
    if (!cubeApi) {
      return undefined;
    }
    let cancelled = false;
    setMetaLoading(true);
    cubeApi
      .meta()
      .then((meta) => {
        if (cancelled) {
          return;
        }
        setMetaError(null);
        const map: Record<string, MetaCube> = {};
        (meta.meta.cubes as unknown as MetaCube[]).forEach((cube) => {
          map[cube.name] = cube;
        });
        setMetaCubes(map);
      })
      .catch((e: any) => {
        if (!cancelled) {
          setMetaCubes({});
          setMetaError(e?.message || String(e));
        }
      })
      .finally(() => !cancelled && setMetaLoading(false));
    return () => {
      cancelled = true;
    };
  }, [cubeApi, metaVersion]);

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
        const detail = await readJson(
          await playgroundFetch(`playground/tree/${encodeURIComponent(slug)}`)
        );
        setDetails((current) => ({ ...current, [slug]: detail }));
        setDetailErrors((current) => {
          const { [slug]: _gone, ...rest } = current;
          return rest;
        });
      } catch (e: any) {
        delete detailRequests.current[slug];
        setDetailErrors((current) => ({ ...current, [slug]: e.message }));
      }
    })();
    detailRequests.current[slug] = request;
    return request;
  }, []);

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

  useEffect(() => {
    setFile(null);
    setFileError(null);
    if (!selected || selected.kind !== 'file' || !selected.rest) {
      return undefined;
    }
    let cancelled = false;
    (async () => {
      try {
        const result = await readJson(
          await playgroundFetch(
            `playground/tree/${encodeURIComponent(selected.slug)}/files/${encodePath(selected.rest!)}`
          )
        );
        if (!cancelled) {
          setFile(result);
        }
      } catch (e: any) {
        if (!cancelled) {
          setFileError(e.message);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selected]);

  const treeData = useMemo(() => {
    // The loaded list, plus what the server found that is not in it yet.
    const all = [...orgs, ...found.filter((f) => !orgs.some((o) => o.slug === f.slug))];
    const nodes: DataNode[] = orgNodes(all).map((node, index) => {
      const org = all[index];
      const detail = details[org.slug];
      return {
        ...node,
        title: org.errors > 0 ? `${org.name} (${org.errors} errors)` : node.title,
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
    const detail = details[checkedSlug];
    const tablesSchema: Record<string, Record<string, unknown>> = {};
    Object.entries(detail.tables || {}).forEach(([ds, schemas]) => {
      Object.entries(schemas).forEach(([schema, tables]) => {
        tablesSchema[`${ds}.${schema}`] = tables;
      });
    });

    const options = { format };
    playgroundAction('Generate Schema', options);
    let res: Response;
    try {
      res = await playgroundFetch('playground/generate-schema', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          format,
          tables: checkedKeys.map((k) => tableRefs[k]).filter(Boolean),
          tablesSchema,
        }),
      });
    } catch (e: any) {
      playgroundAction('Generate Schema Fail', { error: String(e), ...options });
      message.error(`Could not generate the data model: ${e.message}`);
      return;
    }

    if (res.status === 200) {
      playgroundAction('Generate Schema Success', options);
      await loadDetail(checkedSlug, true);
      setCheckedKeys([]);
      // The new cubes show in meta and in the organizations' counts.
      setMetaVersion((v) => v + 1);
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
    } else {
      const error = await res.text();
      playgroundAction('Generate Schema Fail', { error, ...options });
      message.error(error);
    }
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

    if (selected.kind === 'cube') {
      const cube = detail?.cubes.find((c) => c.name === selected.rest);
      if (!cube) {
        return <CubeLoader />;
      }
      return (
        <CubeDetail
          cube={cube}
          metaCube={metaCubes?.[cube.name]}
          metaLoading={metaLoading}
          metaError={metaError}
          errors={detail.errors || []}
          onOpen={(metaCube) => history.push(buildQueryUrl(metaCube))}
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
      return (
        <>
          <Alert
            message={
              isDocker ? (
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
              ) : (
                <span>
                  This file can be edited at&nbsp;
                  <b>{file.absPath}</b>
                </span>
              )
            }
            type="info"
            style={{ paddingTop: 10, paddingBottom: 11 }}
          />
          <PrismCode code={file.content} style={{ padding: 0, marginTop: 24 }} />
        </>
      );
    }

    if (selected.kind === 'org') {
      const org = orgs.find((o) => o.slug === selected.slug);
      if (org && org.errors > 0 && !detail) {
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
        loadedKeys={Object.keys(details).map((slug) => `org:${slug}`)}
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
          setSelectedKey(key);
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
                    <Menu.Item onClick={() => generateSchema()}>
                      JavaScript
                    </Menu.Item>
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
    </Layout>
  );
}

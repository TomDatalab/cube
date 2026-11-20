import { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Layout, Modal, Empty, Typography, Input, Tooltip, message } from 'antd';
import type { DataNode } from 'antd/lib/tree';
import { RouterProps } from 'react-router-dom';

import PrismCode from '../../PrismCode';
import { playgroundAction } from '../../events';
import { Menu, Tree } from '../../components';
import { Alert, CubeLoader } from '../../atoms';
import { playgroundFetch } from '../../shared/helpers';
import { AppContext } from '../../components/AppContext';
import { ButtonDropdown } from '../../QueryBuilder/ButtonDropdown';
import { useCubejsApi, useSecurityContext } from '../../hooks';
import { SchemaFormat } from '../../types';
import { CubeDetail, MetaCube } from './CubeDetail';
import {
  OrgDetail,
  OrgSummary,
  buildQueryUrl,
  detailNodes,
  filterTree,
  orgNodes,
  parseKey,
  tablesFromSameOrg,
} from './tree';

const { Content, Sider } = Layout;

const PAGE_SIZE = 50;
const MORE_KEY = 'more:';
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
  const { playgroundContext, apiUrl } = useContext(AppContext);
  const { currentToken } = useSecurityContext();
  const cubeApi = useCubejsApi(apiUrl ?? null, currentToken ?? null);

  const [orgs, setOrgs] = useState<OrgSummary[]>([]);
  const [total, setTotal] = useState(0);
  const [orgsLoading, setOrgsLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [loadError, setLoadError] = useState<Error | null>(null);
  const [details, setDetails] = useState<Record<string, OrgDetail>>({});
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
  const listRequest = useRef(0);

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

  useEffect(() => {
    const request = ++listRequest.current;
    setOrgsLoading(true);
    setLoadError(null);

    (async () => {
      try {
        const params = new URLSearchParams({ limit: String(PAGE_SIZE), offset: '0' });
        if (query) {
          params.set('q', query);
        }
        const result = await readJson(await playgroundFetch(`playground/tree?${params}`));
        if (request === listRequest.current) {
          setOrgs(result.organizations || []);
          setTotal(result.total || 0);
        }
      } catch (e: any) {
        if (request === listRequest.current) {
          setLoadError(e);
        }
      } finally {
        if (request === listRequest.current) {
          setOrgsLoading(false);
        }
      }
    })();
  }, [query]);

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
        const map: Record<string, MetaCube> = {};
        (meta.meta.cubes as unknown as MetaCube[]).forEach((cube) => {
          map[cube.name] = cube;
        });
        setMetaCubes(map);
      })
      .catch(() => !cancelled && setMetaCubes({}))
      .finally(() => !cancelled && setMetaLoading(false));
    return () => {
      cancelled = true;
    };
  }, [cubeApi]);

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
      } catch (e: any) {
        delete detailRequests.current[slug];
        message.error(`Could not load ${slug}: ${e.message}`);
      }
    })();
    detailRequests.current[slug] = request;
    return request;
  }, []);

  const loadMore = async () => {
    setLoadingMore(true);
    try {
      const params = new URLSearchParams({
        limit: String(PAGE_SIZE),
        offset: String(orgs.length),
      });
      if (query) {
        params.set('q', query);
      }
      const result = await readJson(await playgroundFetch(`playground/tree?${params}`));
      setOrgs((current) => [...current, ...(result.organizations || [])]);
      setTotal(result.total || 0);
    } catch (e: any) {
      message.error(e.message);
    } finally {
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
    const nodes: DataNode[] = orgNodes(orgs).map((node, index) => {
      const org = orgs[index];
      const detail = details[org.slug];
      return {
        ...node,
        title: org.errors > 0 ? `${org.name} (${org.errors} errors)` : node.title,
        ...(detail ? { children: detailNodes(org.slug, detail) } : {}),
      };
    });
    const visible = query ? nodes : filterTree(nodes, search);
    if (orgs.length < total) {
      visible.push({
        key: MORE_KEY,
        title: loadingMore ? 'Carregando…' : 'Carregar mais…',
        isLeaf: true,
        checkable: false,
      });
    }
    return visible;
  }, [orgs, total, details, search, query, loadingMore]);

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
    const res = await playgroundFetch('playground/generate-schema', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        format,
        tables: checkedKeys.map((k) => tableRefs[k]).filter(Boolean),
        tablesSchema,
      }),
    });

    if (res.status === 200) {
      playgroundAction('Generate Schema Success', options);
      await loadDetail(checkedSlug, true);
      setCheckedKeys([]);
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

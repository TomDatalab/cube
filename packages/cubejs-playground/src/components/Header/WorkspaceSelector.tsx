import { Select, Tooltip, Typography } from 'antd';
import { useEffect, useRef, useState } from 'react';

import { useIsMounted, useWorkspace } from '../../hooks';
import { chooseWorkspace } from '../../shared/auth';
import { sessionFetchJson } from '../../shared/helpers';
import { nextChoice } from './workspaceChoice';

// The value of "Todos (admin)": no `X-Cube-Workspace` at all.
const ALL = '';
const PAGE_SIZE = 50;
const DEBOUNCE_MS = 300;

type Option = { value: string; label: string };

type Props = {
  basePath: string;
  isAdmin: boolean;
};

const label = (orgName: string, wsName: string) => `${orgName} / ${wsName}`;

// The first workspaces matching `q` that the caller can see.
async function searchWorkspaces(base: string, q: string): Promise<Option[]> {
  const params = new URLSearchParams({ limit: String(PAGE_SIZE) });
  if (q.trim()) {
    params.set('q', q.trim());
  }
  const { workspaces = [] } = await sessionFetchJson(`${base}/v1/workspaces?${params}`);
  return workspaces.map((ws) => ({
    value: String(ws.id),
    label: label(ws.organizationName, ws.name),
  }));
}

// The label of a workspace outside the first page, or null when the server
// says it is gone or not visible (404).
async function workspaceLabel(base: string, id: string): Promise<string | null> {
  try {
    const ws = await sessionFetchJson(`${base}/v1/workspaces/${encodeURIComponent(id)}`);
    const org = await sessionFetchJson(
      `${base}/v1/organizations/${encodeURIComponent(ws.organizationId)}`
    );
    return label(org.name, ws.name);
  } catch (e) {
    if (/not found|404/i.test(String((e as Error).message))) {
      return null;
    }
    throw e;
  }
}

// Picks the workspace every data request is scoped to. Changing it reloads
// the page, so the query builder, meta and GraphQL start over in the new one
// (or, with storage blocked, applies it live: see `chooseWorkspace`).
export function WorkspaceSelector({ basePath, isAdmin }: Props) {
  const isMounted = useIsMounted();
  const base = basePath.replace(/\/$/, '');
  const [options, setOptions] = useState<Option[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [currentLabel, setCurrentLabel] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const current = useWorkspace();
  const latest = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout>>();

  function choose(id: string | null) {
    chooseWorkspace(id);
  }

  function search(q: string) {
    const request = ++latest.current;
    setLoading(true);
    searchWorkspaces(base, q).then((found) => {
      if (isMounted() && request === latest.current) {
        setOptions(found);
        setLoading(false);
      }
    }, () => {
      if (isMounted() && request === latest.current) {
        setLoading(false);
      }
    });
  }

  useEffect(() => {
    (async () => {
      const firstPage = await searchWorkspaces(base, '');
      const known = current ? firstPage.find((o) => o.value === current) : undefined;
      let resolved = known?.label ?? null;
      if (current && !known) {
        resolved = await workspaceLabel(base, current);
      }
      if (!isMounted()) {
        return;
      }
      setOptions(firstPage);
      setCurrentLabel(resolved);
      const next = nextChoice({
        current, currentVisible: resolved != null, firstPage, isAdmin,
      });
      if (next) {
        choose(next.choose);
      }
    })().catch((e) => {
      if (isMounted()) {
        setError(e.message);
        setOptions([]);
      }
    });
    return () => {
      latest.current++;
      clearTimeout(timer.current);
    };
  }, [base, isAdmin]);

  if (error) {
    return (
      <Tooltip title={error}>
        <Typography.Text type="danger" style={{ color: '#ff7875' }}>
          Workspaces unavailable
        </Typography.Text>
      </Tooltip>
    );
  }

  const all: Option[] = isAdmin ? [{ value: ALL, label: 'Todos (admin)' }] : [];
  const found = options || [];
  // The chosen workspace stays selectable even when a search hides it.
  const pinned: Option[] = current && currentLabel && !found.some((o) => o.value === current)
    ? [{ value: current, label: currentLabel }]
    : [];
  let value: string | undefined;
  if (options != null) {
    value = current ?? (isAdmin ? ALL : undefined);
  }

  return (
    <Select<string>
      data-testid="workspace-selector"
      size="small"
      style={{ width: 240 }}
      loading={options == null || loading}
      disabled={options == null}
      placeholder={options?.length === 0 && !isAdmin ? 'No workspace' : 'Workspace'}
      value={value}
      options={[...all, ...pinned, ...found]}
      showSearch
      filterOption={false}
      dropdownMatchSelectWidth={false}
      onSearch={(q) => {
        clearTimeout(timer.current);
        timer.current = setTimeout(() => search(q), DEBOUNCE_MS);
      }}
      onDropdownVisibleChange={(open) => {
        // Back to the first workspaces once the search box is closed.
        if (!open) {
          clearTimeout(timer.current);
          search('');
        }
      }}
      onChange={(next) => choose(next === ALL ? null : next)}
    />
  );
}

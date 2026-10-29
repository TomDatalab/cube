import { Select, Tooltip, Typography } from 'antd';
import { useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import { getWorkspace, setWorkspace } from '../../shared/auth';
import { sessionFetchJson } from '../../shared/helpers';

// The value of "Todos (admin)": no `X-Cube-Workspace` at all.
const ALL = '';

type Option = { value: string; label: string };

type Props = {
  basePath: string;
  isAdmin: boolean;
};

async function loadOptions(basePath: string): Promise<Option[]> {
  const base = basePath.replace(/\/$/, '');
  const { organizations = [] } = await sessionFetchJson(`${base}/v1/organizations`);
  const perOrg = await Promise.all(
    organizations.map(async (org) => {
      const { workspaces = [] } = await sessionFetchJson(
        `${base}/v1/organizations/${encodeURIComponent(org.id)}/workspaces`
      );
      return workspaces.map((ws) => ({ value: String(ws.id), label: `${org.name} / ${ws.name}` }));
    })
  );
  return perOrg.flat().sort((a, b) => a.label.localeCompare(b.label));
}

// Picks the workspace every data request is scoped to. Changing it reloads
// the page, so the query builder, meta and GraphQL start over in the new one.
export function WorkspaceSelector({ basePath, isAdmin }: Props) {
  const isMounted = useIsMounted();
  const [options, setOptions] = useState<Option[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const current = getWorkspace();

  function choose(id: string | null) {
    setWorkspace(id);
    window.location.reload();
  }

  useEffect(() => {
    loadOptions(basePath)
      .then((loaded) => {
        if (!isMounted()) {
          return;
        }
        setOptions(loaded);
        if (current && loaded.some((o) => o.value === current)) {
          return;
        }
        // Nothing chosen yet, or a workspace this user no longer sees.
        if (!isAdmin && loaded.length) {
          choose(loaded[0].value);
        } else if (current) {
          choose(null);
        }
      })
      .catch((e) => {
        if (isMounted()) {
          setError(e.message);
          setOptions([]);
        }
      });
  }, [basePath, isAdmin]);

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
  let value: string | undefined;
  if (options != null) {
    value = current ?? (isAdmin ? ALL : undefined);
  }

  return (
    <Select<string>
      data-testid="workspace-selector"
      size="small"
      style={{ width: 240 }}
      loading={options == null}
      disabled={options == null || (!isAdmin && options.length === 0)}
      placeholder={options?.length === 0 ? 'No workspace' : 'Workspace'}
      value={value}
      options={[...all, ...(options || [])]}
      showSearch
      optionFilterProp="label"
      dropdownMatchSelectWidth={false}
      onChange={(next) => choose(next === ALL ? null : next)}
    />
  );
}

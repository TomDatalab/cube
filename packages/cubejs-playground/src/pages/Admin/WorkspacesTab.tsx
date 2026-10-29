import {
  Alert, Button, Input, message, Popconfirm, Space, Table, Typography,
} from 'antd';
import { useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, Connection, PAGE_SIZE, Workspace, WorkspaceListing,
} from './api';
import { NameDialog, NameValues } from './NameDialog';
import { organizationOptions, RemoteSelect } from './RemoteSelect';
import { useSearch } from './useSearch';
import { WorkspacePanel } from './WorkspacePanel';

type Props = {
  api: AdminApi;
  // The organization shown, or `null` for all of them.
  organization: string | null;
  onOrganizationChange: (org: string | null) => void;
};

// Every workspace of every organization, filtered by organization.
export function WorkspacesTab({ api, organization, onOrganizationChange }: Props) {
  const isMounted = useIsMounted();
  // `true`: a new workspace; a workspace: renaming it.
  const [editing, setEditing] = useState<Workspace | true | null>(null);
  // The name of the filtered organization, which may come in as an id only.
  const [organizationLabel, setOrganizationLabel] = useState<string | undefined>();
  const {
    q, setQ, page, setPage, result, loading, error, reload,
  } = useSearch(
    (term, p) => api.searchWorkspaces({ q: term, organizationId: organization, page: p }),
    [api, organization]
  );

  useEffect(() => {
    setOrganizationLabel(undefined);
    if (!organization) {
      return;
    }
    api.searchOrganizations(organization, 1).then((found) => {
      const match = found.items.find((o) => o.id === organization);
      if (!isMounted()) {
        return;
      }
      if (match) {
        setOrganizationLabel(match.name);
      } else {
        // Deleted (or no longer visible): back to all organizations.
        onOrganizationChange(null);
      }
    }, () => {});
  }, [api, organization]);

  async function save({ name, organization: org }: NameValues) {
    if (editing === true) {
      await api.createWorkspace(org!, name);
    } else {
      await api.renameWorkspace(editing!.id, name);
    }
    message.success(`${name} saved`);
    reload();
  }

  async function remove(ws: Workspace) {
    try {
      await api.deleteWorkspace(ws.id);
      message.success(`${ws.name} deleted`);
      reload();
    } catch (e: any) {
      message.error(e.message);
    }
  }

  let dialogTitle: string | null = null;
  if (editing === true) {
    dialogTitle = 'New workspace';
  } else if (editing) {
    dialogTitle = 'Rename workspace';
  }

  return (
    <>
      <Space style={{ marginBottom: 16 }}>
        <RemoteSelect
          allowClear
          placeholder="All organizations"
          style={{ width: 240 }}
          value={organization ?? undefined}
          valueLabel={organizationLabel}
          onChange={(v) => onOrganizationChange(v || null)}
          fetchOptions={organizationOptions(api)}
        />
        <Button type="primary" onClick={() => setEditing(true)}>
          New workspace
        </Button>
      </Space>

      <Input.Search
        allowClear
        placeholder="Search workspaces, or their organization"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        style={{ maxWidth: 400, display: 'block', marginBottom: 16 }}
      />
      {error ? <Alert type="error" message={error} showIcon style={{ marginBottom: 16 }} /> : null}

      <Table<WorkspaceListing>
        rowKey="id"
        size="small"
        loading={loading}
        dataSource={result?.items || []}
        pagination={{
          current: page,
          pageSize: PAGE_SIZE,
          total: result?.total ?? 0,
          onChange: setPage,
          showSizeChanger: false,
        }}
        expandable={{
          expandedRowRender: (ws) => <WorkspaceDetails api={api} workspace={ws} />,
        }}
        columns={[
          { title: 'Workspace', dataIndex: 'name' },
          { title: 'Organization', dataIndex: 'organizationName' },
          { title: 'Id', dataIndex: 'id', render: (v) => <Typography.Text type="secondary" copyable>{v}</Typography.Text> },
          {
            title: '',
            align: 'right',
            render: (_, ws) => (
              <Space>
                <Button size="small" onClick={() => setEditing(ws)}>Rename</Button>
                <Popconfirm
                  title={`Delete ${ws.name}, with its members, objects and connection releases?`}
                  onConfirm={() => remove(ws)}
                >
                  <Button size="small" danger>Delete</Button>
                </Popconfirm>
              </Space>
            ),
          },
        ]}
      />

      <NameDialog
        title={dialogTitle}
        initialName={editing && editing !== true ? editing.name : ''}
        choice={editing === true ? {
          field: 'organization',
          label: 'Organization',
          message: 'Pick an organization',
          fetchOptions: organizationOptions(api),
          initial: organization ?? undefined,
          initialLabel: organizationLabel,
        } : undefined}
        onSubmit={save}
        onClose={() => setEditing(null)}
      />
    </>
  );
}

type DetailsProps = {
  api: AdminApi;
  workspace: Workspace;
};

// The members and connection releases of one workspace, with its
// organization's connections to release from.
function WorkspaceDetails({ api, workspace }: DetailsProps) {
  const isMounted = useIsMounted();
  const [connections, setConnections] = useState<Connection[]>([]);

  async function loadConnections() {
    try {
      const loaded = await api.connections(workspace.organizationId);
      if (isMounted()) {
        setConnections(loaded);
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  useEffect(() => {
    loadConnections();
  }, [workspace.organizationId]);

  return (
    <WorkspacePanel
      api={api}
      workspace={workspace}
      connections={connections}
      reloadConnections={loadConnections}
    />
  );
}

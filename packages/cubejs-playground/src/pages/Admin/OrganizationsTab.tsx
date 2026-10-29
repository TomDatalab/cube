import {
  Button, Card, message, Popconfirm, Select, Space, Table, Tag, Typography,
} from 'antd';
import { useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, Connection, DataSource, Organization, User, Workspace,
} from './api';
import { NameDialog } from './NameDialog';
import { WorkspacePanel } from './WorkspacePanel';

type Props = {
  api: AdminApi;
  users: User[];
};

function userOptions(users: User[]) {
  return users
    .filter((u) => !u.disabled)
    .map((u) => ({ value: u.id, label: u.name ? `${u.id} (${u.name})` : u.id }));
}

export function OrganizationsTab({ api, users }: Props) {
  const isMounted = useIsMounted();
  const [organizations, setOrganizations] = useState<Organization[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [dialog, setDialog] = useState<'new' | 'rename' | null>(null);

  async function loadOrganizations() {
    try {
      const loaded = await api.organizations();
      if (isMounted()) {
        setOrganizations(loaded);
        if (selected && !loaded.some((o) => o.id === selected)) {
          setSelected(null);
        }
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  useEffect(() => {
    loadOrganizations();
  }, []);

  const org = organizations?.find((o) => o.id === selected) || null;

  async function save({ name, owner }: { name: string; owner?: string }) {
    if (dialog === 'new') {
      const created = await api.createOrganization(name, owner!);
      setSelected(created?.id ?? null);
    } else {
      await api.renameOrganization(org!.id, name);
    }
    message.success(`${name} saved`);
    await loadOrganizations();
  }

  return (
    <>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => setDialog('new')}>
          New organization
        </Button>
      </Space>

      <Table<Organization>
        rowKey="id"
        size="small"
        loading={organizations == null}
        dataSource={organizations || []}
        pagination={false}
        rowSelection={{
          type: 'radio',
          selectedRowKeys: selected ? [selected] : [],
          onChange: (keys) => setSelected((keys[0] as string) ?? null),
        }}
        onRow={(o) => ({ onClick: () => setSelected(o.id), style: { cursor: 'pointer' } })}
        columns={[
          { title: 'Organization', dataIndex: 'name' },
          { title: 'Created by', dataIndex: 'createdBy', render: (v) => v || '—' },
          { title: 'Created', dataIndex: 'createdAt', render: (v) => new Date(v).toLocaleString() },
        ]}
      />

      {org ? (
        <OrganizationCard
          key={org.id}
          api={api}
          org={org}
          organizations={(organizations || []).map((o) => o.id)}
          users={users}
          onRename={() => setDialog('rename')}
          onDeleted={loadOrganizations}
        />
      ) : null}

      <NameDialog
        title={dialog && (dialog === 'new' ? 'New organization' : 'Rename organization')}
        initialName={dialog === 'rename' ? org?.name : ''}
        ownerOptions={dialog === 'new' ? userOptions(users) : undefined}
        onSubmit={save}
        onClose={() => setDialog(null)}
      />
    </>
  );
}

type CardProps = {
  api: AdminApi;
  org: Organization;
  // Every organization's id: a data source one of them holds is not offered.
  organizations: string[];
  users: User[];
  onRename: () => void;
  onDeleted: () => Promise<void>;
};

function OrganizationCard({
  api, org, organizations, users, onRename, onDeleted,
}: CardProps) {
  const isMounted = useIsMounted();
  const [workspaces, setWorkspaces] = useState<Workspace[] | null>(null);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [dataSources, setDataSources] = useState<DataSource[]>([]);
  const [owner, setOwner] = useState<string | undefined>();
  const [dataSource, setDataSource] = useState<string | undefined>();
  // `true`: a new workspace; a workspace: renaming it.
  const [editing, setEditing] = useState<Workspace | true | null>(null);

  async function loadWorkspaces() {
    try {
      const loaded = await api.workspaces(org.id);
      if (isMounted()) {
        setWorkspaces(loaded);
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  async function loadConnections() {
    try {
      const [loaded, sources] = await Promise.all([
        api.connections(org.id),
        api.assignableDataSources(organizations),
      ]);
      if (isMounted()) {
        setConnections(loaded);
        setDataSources(sources);
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  useEffect(() => {
    loadWorkspaces();
    loadConnections();
  }, [org.id]);

  async function saveWorkspace({ name }: { name: string }) {
    if (editing === true) {
      await api.createWorkspace(org.id, name);
    } else {
      await api.renameWorkspace(editing!.id, name);
    }
    message.success(`${name} saved`);
    await loadWorkspaces();
  }

  async function act(action: () => Promise<unknown>, done: string, reload?: () => Promise<void>) {
    try {
      await action();
      message.success(done);
      await reload?.();
      return true;
    } catch (e: any) {
      message.error(e.message);
      return false;
    }
  }

  let workspaceDialogTitle: string | null = null;
  if (editing === true) {
    workspaceDialogTitle = 'New workspace';
  } else if (editing) {
    workspaceDialogTitle = 'Rename workspace';
  }

  return (
    <Card
      size="small"
      style={{ marginTop: 16 }}
      title={org.name}
      extra={(
        <Space>
          <Button size="small" onClick={onRename}>Rename</Button>
          <Popconfirm
            title={`Delete ${org.name}? It must have no workspaces or connections left.`}
            onConfirm={() => act(() => api.deleteOrganization(org.id), `${org.name} deleted`, onDeleted)}
          >
            <Button size="small" danger>Delete</Button>
          </Popconfirm>
        </Space>
      )}
    >
      <Typography.Text strong>Owners</Typography.Text>
      <div>
        <Typography.Text type="secondary">
          Owners administer the organization: its workspaces, members and connections.
        </Typography.Text>
      </div>
      <Space style={{ marginTop: 8, marginBottom: 16 }}>
        <Select
          size="small"
          showSearch
          placeholder="User"
          style={{ width: 200 }}
          value={owner}
          onChange={setOwner}
          options={userOptions(users)}
        />
        <Button
          size="small"
          disabled={!owner}
          onClick={() => act(() => api.addOwner(org.id, owner!), `${owner} is an owner of ${org.name}`)}
        >
          Make owner
        </Button>
        <Popconfirm
          title={`Remove ${owner} as an owner of ${org.name}?`}
          disabled={!owner}
          onConfirm={() => act(() => api.removeOwner(org.id, owner!), `${owner} is no longer an owner`)}
        >
          <Button size="small" danger disabled={!owner}>Remove owner</Button>
        </Popconfirm>
      </Space>

      <Typography.Text strong>Connections</Typography.Text>
      <div>
        {connections.length ? connections.map((c) => (
          <Space key={c.name} style={{ marginRight: 16 }}>
            <Typography.Text code>{c.name}</Typography.Text>
            {c.origin === 'assigned' ? (
              <>
                <Tag title="Assigned by a platform admin: the organization uses and releases it, only platform admins edit or delete it">
                  assigned
                </Tag>
                <Popconfirm
                  title={`Unassign ${c.name} from ${org.name}? Its releases to the organization's workspaces are withdrawn; the data source stays.`}
                  onConfirm={() => act(
                    () => api.unassignConnection(c.name),
                    `${c.name} no longer belongs to ${org.name}`,
                    loadConnections
                  )}
                >
                  <Button size="small" danger>Unassign</Button>
                </Popconfirm>
              </>
            ) : (
              <Tag title="Created by the organization: its owners edit and delete it">created</Tag>
            )}
          </Space>
        )) : <Typography.Text type="secondary">None yet.</Typography.Text>}
      </div>
      <Space style={{ marginTop: 8, marginBottom: 16 }}>
        <Select
          size="small"
          placeholder="Data source"
          style={{ width: 200 }}
          value={dataSource}
          onChange={setDataSource}
          options={dataSources
            .map((ds) => ({ value: ds.name, label: ds.type ? `${ds.name} (${ds.type})` : ds.name }))}
        />
        <Popconfirm
          title={`Assign ${dataSource} to ${org.name}? Its owners can release it to their workspaces, but only platform admins edit or delete it.`}
          disabled={!dataSource}
          onConfirm={async () => {
            if (await act(
              () => api.assignConnection(dataSource!, org.id),
              `${dataSource} belongs to ${org.name}`,
              loadConnections
            )) {
              setDataSource(undefined);
            }
          }}
        >
          <Button size="small" disabled={!dataSource}>Add to organization</Button>
        </Popconfirm>
      </Space>

      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <Typography.Text strong>Workspaces</Typography.Text>
        <Button size="small" type="primary" onClick={() => setEditing(true)}>New workspace</Button>
      </div>
      <Table<Workspace>
        rowKey="id"
        size="small"
        loading={workspaces == null}
        dataSource={workspaces || []}
        pagination={false}
        style={{ marginTop: 8 }}
        expandable={{
          expandedRowRender: (ws) => (
            <WorkspacePanel
              api={api}
              workspace={ws}
              users={users}
              connections={connections}
              reloadConnections={loadConnections}
            />
          ),
        }}
        columns={[
          { title: 'Workspace', dataIndex: 'name' },
          { title: 'Id', dataIndex: 'id', render: (v) => <Typography.Text type="secondary" copyable>{v}</Typography.Text> },
          {
            title: '',
            align: 'right',
            render: (_, ws) => (
              <Space>
                <Button size="small" onClick={() => setEditing(ws)}>Rename</Button>
                <Popconfirm
                  title={`Delete ${ws.name}, with its members, objects and connection releases?`}
                  onConfirm={() => act(() => api.deleteWorkspace(ws.id), `${ws.name} deleted`, loadWorkspaces)}
                >
                  <Button size="small" danger>Delete</Button>
                </Popconfirm>
              </Space>
            ),
          },
        ]}
      />

      <NameDialog
        title={workspaceDialogTitle}
        initialName={editing && editing !== true ? editing.name : ''}
        onSubmit={saveWorkspace}
        onClose={() => setEditing(null)}
      />
    </Card>
  );
}

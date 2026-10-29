import {
  Button, message, Popconfirm, Select, Space, Table, Typography,
} from 'antd';
import { useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, Connection, Organization, User, Workspace,
} from './api';
import { NameDialog, NameValues } from './NameDialog';
import { WorkspacePanel } from './WorkspacePanel';

type Props = {
  api: AdminApi;
  users: User[];
  // The organization shown, or `null` for all of them.
  organization: string | null;
  onOrganizationChange: (org: string | null) => void;
};

const ALL = '';

// Every workspace of every organization, filtered by organization.
export function WorkspacesTab({
  api, users, organization, onOrganizationChange,
}: Props) {
  const isMounted = useIsMounted();
  const [organizations, setOrganizations] = useState<Organization[] | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[] | null>(null);
  // `true`: a new workspace; a workspace: renaming it.
  const [editing, setEditing] = useState<Workspace | true | null>(null);

  async function load() {
    try {
      const orgs = await api.organizations();
      const all = await Promise.all(orgs.map((o) => api.workspaces(o.id)));
      if (isMounted()) {
        setOrganizations(orgs);
        setWorkspaces(all.flat());
        if (organization && !orgs.some((o) => o.id === organization)) {
          onOrganizationChange(null);
        }
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  useEffect(() => {
    load();
  }, []);

  const orgName = new Map((organizations || []).map((o) => [o.id, o.name]));
  const orgOptions = (organizations || []).map((o) => ({ value: o.id, label: o.name }));
  const shown = (workspaces || [])
    .filter((ws) => !organization || ws.organizationId === organization);

  async function save({ name, organization: org }: NameValues) {
    if (editing === true) {
      await api.createWorkspace(org!, name);
    } else {
      await api.renameWorkspace(editing!.id, name);
    }
    message.success(`${name} saved`);
    await load();
  }

  async function remove(ws: Workspace) {
    try {
      await api.deleteWorkspace(ws.id);
      message.success(`${ws.name} deleted`);
      await load();
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
        <Select
          style={{ width: 240 }}
          value={organization ?? ALL}
          onChange={(v: string) => onOrganizationChange(v || null)}
          options={[{ value: ALL, label: 'All organizations' }, ...orgOptions]}
        />
        <Button
          type="primary"
          disabled={!orgOptions.length}
          title={orgOptions.length ? undefined : 'Create an organization first'}
          onClick={() => setEditing(true)}
        >
          New workspace
        </Button>
      </Space>

      <Table<Workspace>
        rowKey="id"
        size="small"
        loading={workspaces == null}
        dataSource={shown}
        pagination={false}
        expandable={{
          expandedRowRender: (ws) => <WorkspaceDetails api={api} workspace={ws} users={users} />,
        }}
        columns={[
          { title: 'Workspace', dataIndex: 'name' },
          {
            title: 'Organization',
            dataIndex: 'organizationId',
            render: (v) => orgName.get(v) || v,
          },
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
          options: orgOptions,
          initial: organization ?? undefined,
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
  users: User[];
};

// The members and connection releases of one workspace, with its
// organization's connections to release from.
function WorkspaceDetails({ api, workspace, users }: DetailsProps) {
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
      users={users}
      connections={connections}
      reloadConnections={loadConnections}
    />
  );
}

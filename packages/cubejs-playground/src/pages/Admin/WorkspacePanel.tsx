import {
  Button, Checkbox, Divider, message, Popconfirm, Select, Space, Table, Typography,
} from 'antd';
import { useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, Connection, Member, Role, ROLES, Workspace,
} from './api';
import { RemoteSelect, userOptions } from './RemoteSelect';

type Props = {
  api: AdminApi;
  workspace: Workspace;
  connections: Connection[];
  reloadConnections: () => Promise<void>;
};

// The members of one workspace and the organization connections released to it.
export function WorkspacePanel({
  api, workspace, connections, reloadConnections,
}: Props) {
  const isMounted = useIsMounted();
  const [members, setMembers] = useState<Member[] | null>(null);
  const [newMember, setNewMember] = useState<string | undefined>();
  const [newRole, setNewRole] = useState<Role>('viewer');

  async function loadMembers() {
    try {
      const loaded = await api.members(workspace.id);
      if (isMounted()) {
        setMembers(loaded);
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  useEffect(() => {
    loadMembers();
  }, [workspace.id]);

  async function act(action: () => Promise<unknown>, done: string, reload: () => Promise<void>) {
    try {
      await action();
      message.success(done);
      await reload();
    } catch (e: any) {
      message.error(e.message);
    }
  }

  const roleOptions = ROLES.map((r) => ({ value: r, label: r }));
  // Only users not in the workspace yet: adding sets the role, so picking an
  // existing member here would change theirs.
  const memberIds = new Set((members || []).map((m) => m.user));
  const newMemberOptions = async (q: string) => (
    (await userOptions(api)(q)).filter((o) => !memberIds.has(o.value))
  );

  return (
    <div style={{ padding: '8px 0' }}>
      <Typography.Text strong>Members</Typography.Text>
      <Table<Member>
        rowKey="user"
        size="small"
        loading={members == null}
        dataSource={members || []}
        pagination={false}
        style={{ marginTop: 8 }}
        columns={[
          { title: 'User', dataIndex: 'user' },
          {
            title: 'Role',
            render: (_, m) => (
              <Select
                size="small"
                value={m.role}
                options={roleOptions}
                style={{ width: 120 }}
                onChange={(role: Role) => act(
                  () => api.setMember(workspace.id, m.user, role),
                  `${m.user} is now ${role}`,
                  loadMembers
                )}
              />
            ),
          },
          {
            title: '',
            align: 'right',
            render: (_, m) => (
              <Popconfirm
                title={`Remove ${m.user} from ${workspace.name}?`}
                onConfirm={() => act(
                  () => api.removeMember(workspace.id, m.user),
                  `${m.user} removed`,
                  loadMembers
                )}
              >
                <Button size="small" danger>Remove</Button>
              </Popconfirm>
            ),
          },
        ]}
      />
      <Space style={{ marginTop: 8 }}>
        <RemoteSelect
          // Loads its options again whenever the members change.
          key={[...memberIds].join(',')}
          size="small"
          placeholder="User"
          style={{ width: 200 }}
          value={newMember}
          onChange={setNewMember}
          fetchOptions={newMemberOptions}
        />
        <Select
          size="small"
          value={newRole}
          options={roleOptions}
          style={{ width: 120 }}
          onChange={setNewRole}
        />
        <Button
          size="small"
          type="primary"
          disabled={!newMember}
          onClick={() => act(
            () => api.setMember(workspace.id, newMember!, newRole),
            `${newMember} added as ${newRole}`,
            async () => {
              setNewMember(undefined);
              await loadMembers();
            }
          )}
        >
          Add member
        </Button>
      </Space>

      <Divider style={{ margin: '16px 0 8px' }} />

      <Typography.Text strong>Connections released to this workspace</Typography.Text>
      <div style={{ marginTop: 8 }}>
        {connections.length ? connections.map((c) => (
          <div key={c.name}>
            <Checkbox
              checked={c.workspaces.includes(workspace.id)}
              onChange={(e) => act(
                () => (e.target.checked
                  ? api.release(workspace.id, c.name)
                  : api.withdraw(workspace.id, c.name)),
                `${c.name} ${e.target.checked ? 'released to' : 'withdrawn from'} ${workspace.name}`,
                reloadConnections
              )}
            >
              {c.name}
              {c.type ? <Typography.Text type="secondary">{` (${c.type})`}</Typography.Text> : null}
            </Checkbox>
          </div>
        )) : (
          <Typography.Text type="secondary">
            The organization has no connections yet.
          </Typography.Text>
        )}
      </div>
    </div>
  );
}

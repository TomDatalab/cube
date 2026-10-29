import { CaretDownOutlined, CaretRightOutlined, CloseOutlined } from '@ant-design/icons';
import {
  Button, Empty, message, Popconfirm, Select, Spin, Typography,
} from 'antd';
import { DragEvent, useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, Member, Organization, Role, ROLES, Workspace,
} from './api';

// The drag payload of a user row: its id. A custom type, so text dragged in
// from elsewhere is not taken for a user.
export const USER_DRAG_TYPE = 'application/x-cube-user';

type Props = {
  api: AdminApi;
};

type Tree = {
  organizations: Organization[];
  workspaces: Workspace[];
  members: Record<string, Member[]>;
};

const roleOptions = ROLES.map((r) => ({ value: r, label: r }));

// Organization → workspace → members. A user dropped on a workspace joins it
// as a viewer; roles change and members leave from here.
export function MembershipTree({ api }: Props) {
  const isMounted = useIsMounted();
  const [tree, setTree] = useState<Tree | null>(null);
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [target, setTarget] = useState<string | null>(null);

  async function load() {
    try {
      const organizations = await api.organizations();
      const workspaces = (await Promise.all(organizations.map((o) => api.workspaces(o.id)))).flat();
      const lists = await Promise.all(workspaces.map((ws) => api.members(ws.id)));
      if (isMounted()) {
        setTree({
          organizations,
          workspaces,
          members: Object.fromEntries(workspaces.map((ws, i) => [ws.id, lists[i]])),
        });
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  useEffect(() => {
    load();
  }, []);

  async function act(action: () => Promise<unknown>, done: string) {
    try {
      await action();
      message.success(done);
    } catch (e: any) {
      message.error(e.message);
    }
    await load();
  }

  function toggle(ws: string) {
    const next = new Set(open);
    if (next.has(ws)) {
      next.delete(ws);
    } else {
      next.add(ws);
    }
    setOpen(next);
  }

  function dragOver(e: DragEvent, ws: string) {
    if (e.dataTransfer.types.includes(USER_DRAG_TYPE)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
      setTarget(ws);
    }
  }

  function drop(e: DragEvent, ws: Workspace) {
    e.preventDefault();
    setTarget(null);
    const user = e.dataTransfer.getData(USER_DRAG_TYPE);
    if (!user) {
      return;
    }
    setOpen(new Set(open).add(ws.id));
    if (tree?.members[ws.id]?.some((m) => m.user === user)) {
      message.info(`${user} is already in ${ws.name}`);
      return;
    }
    act(() => api.setMember(ws.id, user, 'viewer'), `${user} added to ${ws.name} as viewer`);
  }

  if (tree == null) {
    return <Spin />;
  }
  if (!tree.workspaces.length) {
    return (
      <Empty
        image={Empty.PRESENTED_IMAGE_SIMPLE}
        description="No workspaces yet: create them in the Workspaces tab"
      />
    );
  }

  return (
    <div>
      {tree.organizations.map((org) => {
        const workspaces = tree.workspaces.filter((ws) => ws.organizationId === org.id);
        return (
          <div key={org.id} style={{ marginBottom: 16 }}>
            <Typography.Text strong>{org.name}</Typography.Text>
            {workspaces.length ? null : (
              <div style={{ paddingLeft: 16 }}>
                <Typography.Text type="secondary">No workspaces</Typography.Text>
              </div>
            )}
            {workspaces.map((ws) => {
              const members = tree.members[ws.id] || [];
              const expanded = open.has(ws.id);
              const highlighted = target === ws.id;
              return (
                <div
                  key={ws.id}
                  data-workspace={ws.id}
                  onDragOver={(e) => dragOver(e, ws.id)}
                  onDragLeave={() => setTarget((t) => (t === ws.id ? null : t))}
                  onDrop={(e) => drop(e, ws)}
                  style={{
                    margin: '4px 0 4px 16px',
                    padding: '4px 8px',
                    borderRadius: 4,
                    border: `1px dashed ${highlighted ? '#7a77ff' : 'transparent'}`,
                    background: highlighted ? 'rgba(122, 119, 255, 0.08)' : undefined,
                  }}
                >
                  <div
                    role="button"
                    tabIndex={0}
                    onClick={() => toggle(ws.id)}
                    onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && toggle(ws.id)}
                    style={{ cursor: 'pointer', userSelect: 'none' }}
                  >
                    {expanded ? <CaretDownOutlined /> : <CaretRightOutlined />}
                    {' '}
                    <Typography.Text>{ws.name}</Typography.Text>
                    {' '}
                    <Typography.Text type="secondary">
                      {members.length === 1 ? '(1 member)' : `(${members.length} members)`}
                    </Typography.Text>
                  </div>
                  {expanded ? (
                    <div style={{ paddingLeft: 20 }}>
                      {members.length ? members.map((m) => (
                        <div
                          key={m.user}
                          style={{
                            display: 'flex', alignItems: 'center', gap: 8, marginTop: 4,
                          }}
                        >
                          <Typography.Text style={{ minWidth: 120 }}>{m.user}</Typography.Text>
                          <Select
                            size="small"
                            value={m.role}
                            options={roleOptions}
                            style={{ width: 100 }}
                            onChange={(role: Role) => act(
                              () => api.setMember(ws.id, m.user, role),
                              `${m.user} is now ${role} in ${ws.name}`
                            )}
                          />
                          <Popconfirm
                            title={`Remove ${m.user} from ${ws.name}?`}
                            onConfirm={() => act(
                              () => api.removeMember(ws.id, m.user),
                              `${m.user} removed from ${ws.name}`
                            )}
                          >
                            <Button
                              size="small"
                              type="text"
                              danger
                              icon={<CloseOutlined />}
                              aria-label={`Remove ${m.user}`}
                            />
                          </Popconfirm>
                        </div>
                      )) : (
                        <Typography.Text type="secondary">
                          Drag users here
                        </Typography.Text>
                      )}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}

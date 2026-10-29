import { CaretDownOutlined, CaretRightOutlined, CloseOutlined } from '@ant-design/icons';
import {
  Alert, Button, Empty, Input, message, Popconfirm, Select, Space, Spin, Typography,
} from 'antd';
import { DragEvent, useEffect, useRef, useState } from 'react';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, Member, PAGE_SIZE, Role, ROLES, WorkspaceListing,
} from './api';
import { groupByOrganization } from './membershipGroups';
import { useSearch } from './useSearch';

// The drag payload of a user row: its id. A custom type, so text dragged in
// from elsewhere is not taken for a user.
export const USER_DRAG_TYPE = 'application/x-cube-user';

type Props = {
  api: AdminApi;
};

const roleOptions = ROLES.map((r) => ({ value: r, label: r }));

// Organization → workspace → members, one searchable page of workspaces at a
// time; a workspace's members load when it opens or a user is dropped on it.
// A user dropped on a workspace joins it as a viewer; roles change and
// members leave from here.
export function MembershipTree({ api }: Props) {
  const isMounted = useIsMounted();
  const {
    q, setQ, page, setPage, result, loading, error,
  } = useSearch((term, p) => api.searchWorkspaces({ q: term, page: p }), []);
  const [members, setMembers] = useState<Record<string, Member[]>>({});
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [target, setTarget] = useState<string | null>(null);
  const openRef = useRef(open);
  openRef.current = open;

  async function loadMembers(ws: string): Promise<Member[] | null> {
    try {
      const list = await api.members(ws);
      if (isMounted()) {
        setMembers((m) => ({ ...m, [ws]: list }));
      }
      return list;
    } catch (e: any) {
      message.error(e.message);
      return null;
    }
  }

  // Members are kept per workspace until the next search; workspaces that
  // stay open across it load again.
  useEffect(() => {
    setMembers({});
    (result?.items || []).forEach((ws) => {
      if (openRef.current.has(ws.id)) {
        loadMembers(ws.id);
      }
    });
  }, [result]);

  async function act(ws: string, action: () => Promise<unknown>, done: string) {
    try {
      await action();
      message.success(done);
    } catch (e: any) {
      message.error(e.message);
    }
    await loadMembers(ws);
  }

  function toggle(ws: string) {
    const next = new Set(open);
    if (next.has(ws)) {
      next.delete(ws);
    } else {
      next.add(ws);
      if (!members[ws]) {
        loadMembers(ws);
      }
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

  async function drop(e: DragEvent, ws: WorkspaceListing) {
    e.preventDefault();
    setTarget(null);
    const user = e.dataTransfer.getData(USER_DRAG_TYPE);
    if (!user) {
      return;
    }
    setOpen(new Set(open).add(ws.id));
    const current = members[ws.id] || await loadMembers(ws.id);
    if (current?.some((m) => m.user === user)) {
      message.info(`${user} is already in ${ws.name}`);
      return;
    }
    if (current) {
      act(ws.id, () => api.setMember(ws.id, user, 'viewer'), `${user} added to ${ws.name} as viewer`);
    }
  }

  const groups = groupByOrganization(result?.items || []);
  const pages = Math.max(1, Math.ceil((result?.total || 0) / PAGE_SIZE));

  return (
    <div>
      <Input.Search
        allowClear
        placeholder="Search workspaces, or their organization"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        style={{ maxWidth: 400, display: 'block', marginBottom: 16 }}
      />
      {error ? <Alert type="error" message={error} showIcon style={{ marginBottom: 16 }} /> : null}
      {result == null ? <Spin /> : null}
      {result && !groups.length && !loading ? (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={q ? 'No workspaces match' : 'No workspaces yet: create them in the Workspaces tab'}
        />
      ) : null}
      {groups.map((group) => {
        const { workspaces } = group;
        return (
          <div key={group.organizationId} style={{ marginBottom: 16 }}>
            <Typography.Text strong>{group.organizationName}</Typography.Text>
            {workspaces.map((ws) => {
              const list = members[ws.id];
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
                    {list ? (
                      <Typography.Text type="secondary">
                        {list.length === 1 ? '(1 member)' : `(${list.length} members)`}
                      </Typography.Text>
                    ) : null}
                  </div>
                  {expanded ? (
                    <div style={{ paddingLeft: 20 }}>
                      {list?.length ? list.map((m) => (
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
                              ws.id,
                              () => api.setMember(ws.id, m.user, role),
                              `${m.user} is now ${role} in ${ws.name}`
                            )}
                          />
                          <Popconfirm
                            title={`Remove ${m.user} from ${ws.name}?`}
                            onConfirm={() => act(
                              ws.id,
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
                      )) : null}
                      {list && !list.length ? (
                        <Typography.Text type="secondary">
                          Drag users here
                        </Typography.Text>
                      ) : null}
                      {list ? null : <Spin size="small" />}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        );
      })}
      {result && result.total > PAGE_SIZE ? (
        <Space>
          <Button size="small" disabled={page <= 1 || loading} onClick={() => setPage(page - 1)}>
            Previous
          </Button>
          <Typography.Text type="secondary">{`page ${page} of ${pages}`}</Typography.Text>
          <Button size="small" disabled={page >= pages || loading} onClick={() => setPage(page + 1)}>
            Next
          </Button>
        </Space>
      ) : null}
    </div>
  );
}

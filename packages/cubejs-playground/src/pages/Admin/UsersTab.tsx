import { HolderOutlined } from '@ant-design/icons';
import {
  Alert, Button, Col, Form, Input, message, Modal, Row, Space, Switch, Table, Tag, Typography,
} from 'antd';
import { useState } from 'react';

import { AccessModal } from './access/AccessModal';
import { AdminApi, PAGE_SIZE, User } from './api';
import { MembershipTree, USER_DRAG_TYPE } from './MembershipTree';
import { useSearch } from './useSearch';

type Props = {
  api: AdminApi;
  currentUser: string | null;
};

// `MIN_PASSWORD_LENGTH` in `cubeserver/src/rbac/identity.rs`.
const MIN_PASSWORD = 10;

const passwordRules = [
  { required: true, message: 'Enter a password' },
  { min: MIN_PASSWORD, message: `At least ${MIN_PASSWORD} characters` },
];

export function UsersTab({ api, currentUser }: Props) {
  const {
    q, setQ, page, setPage, result, loading, error: searchError, reload,
  } = useSearch((term, p) => api.searchUsers(term, p), [api]);
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<User | null>(null);
  const [resetting, setResetting] = useState<User | null>(null);
  const [accessing, setAccessing] = useState<User | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [form] = Form.useForm();

  function open(set: (u: any) => void, value: any, initial: Record<string, any> = {}) {
    form.resetFields();
    form.setFieldsValue(initial);
    setError(null);
    set(value);
  }

  function close() {
    setCreating(false);
    setEditing(null);
    setResetting(null);
  }

  async function run(action: () => Promise<unknown>, done: string) {
    setSaving(true);
    setError(null);
    try {
      await action();
      close();
      message.success(done);
      reload();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  async function toggle(user: User, enabled: boolean) {
    try {
      await api.updateUser(user.id, { disabled: !enabled });
      message.success(`${user.id} ${enabled ? 'enabled' : 'disabled'}`);
      reload();
    } catch (e: any) {
      message.error(e.message);
    }
  }

  // Empty fields are left out, so they keep their value.
  function optional(values: Record<string, string | undefined>) {
    return Object.fromEntries(Object.entries(values).filter(([, v]) => v));
  }

  const modalOpen = creating || editing != null || resetting != null;
  let title = 'New user';
  if (editing) {
    title = `Edit ${editing.id}`;
  } else if (resetting) {
    title = `Reset the password of ${resetting.id}`;
  }

  return (
    <Row gutter={24}>
      <Col xs={24} xl={14}>
        <Space style={{ marginBottom: 16 }}>
          <Button type="primary" onClick={() => open(setCreating, true)}>
            New user
          </Button>
        </Space>

        <Input.Search
          allowClear
          placeholder="Search users by username, name or email"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          style={{ maxWidth: 400, display: 'block', marginBottom: 16 }}
        />
        {searchError ? (
          <Alert type="error" message={searchError} showIcon style={{ marginBottom: 16 }} />
        ) : null}

        <Table<User>
          rowKey="id"
          size="small"
          // the tag and action buttons can outgrow the column; scroll inside it
          scroll={{ x: 'max-content' }}
          loading={loading}
          dataSource={result?.items || []}
          pagination={{
            current: page,
            pageSize: PAGE_SIZE,
            total: result?.total ?? 0,
            onChange: setPage,
            showSizeChanger: false,
          }}
          // Enabled users drag onto a workspace of the tree to join it.
          onRow={(user) => (user.disabled ? {} : {
            draggable: true,
            onDragStart: (e) => {
              e.dataTransfer.setData(USER_DRAG_TYPE, user.id);
              e.dataTransfer.effectAllowed = 'copy';
            },
            style: { cursor: 'grab' },
          })}
          columns={[
            {
              key: 'drag',
              width: 32,
              render: (_, user) => (user.disabled ? null : (
                <HolderOutlined title="Drag onto a workspace" style={{ color: 'var(--pg-muted, #a1a1b5)' }} />
              )),
            },
            { title: 'Username', dataIndex: 'id' },
            { title: 'Name', dataIndex: 'name', render: (v) => v || '—' },
            { title: 'Email', dataIndex: 'email', render: (v) => v || '—' },
            {
              title: 'Enabled',
              render: (_, user) => (
                <Switch
                  size="small"
                  checked={!user.disabled}
                  // A platform admin cannot lock themselves out.
                  disabled={user.id === currentUser}
                  onChange={(enabled) => toggle(user, enabled)}
                />
              ),
            },
            {
              title: '',
              align: 'right',
              render: (_, user) => (
                <Space>
                  {user.id === currentUser ? <Tag>you</Tag> : null}
                  <Button
                    size="small"
                    onClick={() => open(setEditing, user, {
                      name: user.name || '',
                      email: user.email || '',
                    })}
                  >
                    Edit
                  </Button>
                  <Button size="small" onClick={() => setAccessing(user)}>
                    Access
                  </Button>
                  <Button size="small" onClick={() => open(setResetting, user)}>
                    Reset password
                  </Button>
                </Space>
              ),
            },
          ]}
        />

        <Modal
          title={title}
          visible={modalOpen}
          onCancel={close}
          onOk={() => form.submit()}
          okText="Save"
          confirmLoading={saving}
          destroyOnClose
        >
          <Form
            form={form}
            layout="vertical"
            requiredMark={false}
            onFinish={(values) => {
              if (creating) {
                const user = {
                  id: values.id,
                  password: values.password,
                  ...optional({ name: values.name, email: values.email }),
                };
                return run(() => api.createUser(user), `User ${values.id} created`);
              }
              if (editing) {
                return run(
                  () => api.updateUser(editing.id, optional({ name: values.name, email: values.email })),
                  `${editing.id} saved`
                );
              }
              return run(
                () => api.setPassword(resetting!.id, values.password),
                `New password set for ${resetting!.id}; their sessions ended`
              );
            }}
          >
            {creating ? (
              <Form.Item
                label="Username"
                name="id"
                rules={[{ required: true, message: 'Enter a username' }]}
              >
                <Input autoComplete="off" />
              </Form.Item>
            ) : null}

            {resetting == null ? (
              <>
                <Form.Item label="Name" name="name">
                  <Input autoComplete="off" />
                </Form.Item>
                <Form.Item label="Email" name="email">
                  <Input type="email" autoComplete="off" />
                </Form.Item>
              </>
            ) : null}

            {creating || resetting ? (
              <Form.Item label="Password" name="password" rules={passwordRules}>
                <Input.Password autoComplete="new-password" />
              </Form.Item>
            ) : null}

            {error ? <Alert type="error" message={error} showIcon /> : null}
          </Form>
        </Modal>
        <AccessModal api={api} user={accessing} onClose={() => setAccessing(null)} />
      </Col>
      <Col xs={24} xl={10}>
        <Typography.Title level={5}>Workspaces</Typography.Title>
        <Typography.Paragraph type="secondary">
          Drag a user onto a workspace to add them as a viewer.
        </Typography.Paragraph>
        <MembershipTree api={api} />
      </Col>
    </Row>
  );
}

import {
  Alert, Button, Form, Input, message, Modal, Space, Switch, Table, Tag,
} from 'antd';
import { useState } from 'react';

import { AdminApi, User } from './api';

type Props = {
  api: AdminApi;
  users: User[] | null;
  currentUser: string | null;
  reload: () => Promise<void>;
};

// `MIN_PASSWORD_LENGTH` in `cubeserver/src/rbac/identity.rs`.
const MIN_PASSWORD = 10;

const passwordRules = [
  { required: true, message: 'Enter a password' },
  { min: MIN_PASSWORD, message: `At least ${MIN_PASSWORD} characters` },
];

export function UsersTab({ api, users, currentUser, reload }: Props) {
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<User | null>(null);
  const [resetting, setResetting] = useState<User | null>(null);
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
      await reload();
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
      await reload();
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
    <>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => open(setCreating, true)}>
          New user
        </Button>
      </Space>

      <Table<User>
        rowKey="id"
        size="small"
        loading={users == null}
        dataSource={users || []}
        pagination={false}
        columns={[
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
    </>
  );
}

import { DeleteOutlined, EditOutlined } from '@ant-design/icons';
import {
  Alert, Button, Checkbox, Empty, Form, Input, message, Modal, Popconfirm, Space, Table, Tag, Tooltip,
  Typography,
} from 'antd';
import { useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import { IdText } from './IdText';
import {
  AdminApi, ModelCube, OrgRole, Permission, PERMISSION_LABELS, PERMISSIONS, restrictionSummary, withImplied,
} from './api';
import { organizationOptions, RemoteSelect } from './RemoteSelect';
import {
  RestrictionsFields, RestrictionsFormValues, toFormRestrictions, toRestrictions,
} from './RestrictionsFields';
import { useLatestCall } from './useLatestCall';

type Props = {
  api: AdminApi;
};

// `MAX_NAME` in `cubeserver/src/rbac/handlers/roles.rs`.
const MAX_NAME = 100;

// The built-in roles, as an organization's role would grant them; shown for
// reference, not editable.
const BUILTIN: { name: string; permissions: Permission[] }[] = [
  { name: 'owner', permissions: [...PERMISSIONS] },
  { name: 'editor', permissions: ['view', 'query', 'read_objects', 'write_objects'] },
  { name: 'viewer', permissions: ['view', 'query', 'read_objects'] },
];

type FormValues = { name: string; permissions: Permission[]; restrictions?: RestrictionsFormValues };

function PermissionTags({ permissions }: { permissions: readonly string[] }) {
  return (
    <Space size={[4, 4]} wrap>
      {permissions.map((p) => <Tag key={p} style={{ marginRight: 0 }}>{p}</Tag>)}
    </Space>
  );
}

// The roles of one organization: a name and the workspace permissions its
// rules give their members. Editing a role changes the access of every rule
// that uses it.
export function RolesTab({ api }: Props) {
  const isMounted = useIsMounted();
  const [organization, setOrganization] = useState<string | null>(null);
  const [roles, setRoles] = useState<OrgRole[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  // `true`: a new role; a role: editing it.
  const [editing, setEditing] = useState<OrgRole | true | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cubes, setCubes] = useState<ModelCube[]>([]);
  const [cubesError, setCubesError] = useState<string | null>(null);
  const [form] = Form.useForm<FormValues>();
  const beginMembers = useLatestCall();

  async function load(org: string) {
    setLoading(true);
    setLoadError(null);
    try {
      const loaded = await api.roles(org);
      if (isMounted()) {
        setRoles(loaded);
      }
    } catch (e: any) {
      if (isMounted()) {
        setLoadError(e.message);
      }
    } finally {
      if (isMounted()) {
        setLoading(false);
      }
    }
  }

  useEffect(() => {
    setRoles(null);
    // The members of the organization left behind are not this one's.
    beginMembers();
    setCubes([]);
    if (organization) {
      load(organization);
    }
  }, [api, organization]);

  // The members the restrictions can name; the form stays usable without them.
  // Only the latest load counts: one for another organization may answer
  // after it.
  async function loadMembers(org: string) {
    const current = beginMembers();
    setCubes([]);
    setCubesError(null);
    try {
      const loaded = await api.modelMembers(org);
      if (isMounted() && current()) {
        setCubes(loaded);
      }
    } catch (e: any) {
      if (isMounted() && current()) {
        setCubesError(e.message);
      }
    }
  }

  function open(role: OrgRole | true) {
    form.resetFields();
    form.setFieldsValue(role === true
      ? { permissions: ['view'], restrictions: { rowFilters: [], blockedMembers: [] } }
      : {
        name: role.name,
        permissions: role.permissions,
        restrictions: toFormRestrictions(role.restrictions),
      });
    setError(null);
    setEditing(role);
    loadMembers(organization!);
  }

  async function save(values: FormValues) {
    setSaving(true);
    setError(null);
    try {
      const role = {
        name: values.name.trim(),
        permissions: withImplied(values.permissions),
        restrictions: toRestrictions(values.restrictions),
      };
      if (editing === true) {
        await api.createRole(organization!, role);
      } else if (editing) {
        await api.updateRole(editing.id, role);
      }
      message.success(`${role.name} saved`);
      setEditing(null);
      load(organization!);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  async function remove(role: OrgRole) {
    try {
      await api.deleteRole(role.id);
      message.success(`${role.name} deleted`);
      load(organization!);
    } catch (e: any) {
      message.error(e.message);
    }
  }

  const editingRules = editing && editing !== true ? editing.rules : 0;

  return (
    <>
      <Space style={{ marginBottom: 16 }}>
        <RemoteSelect
          placeholder="Pick an organization"
          style={{ width: 240 }}
          value={organization ?? undefined}
          onChange={(v) => setOrganization(v || null)}
          fetchOptions={organizationOptions(api)}
        />
        <Button type="primary" disabled={!organization} onClick={() => open(true)}>
          New role
        </Button>
      </Space>

      {loadError ? <Alert type="error" message={loadError} showIcon style={{ marginBottom: 16 }} /> : null}

      {organization ? (
        <Table<OrgRole>
          rowKey="id"
          size="small"
          loading={loading}
          dataSource={roles || []}
          pagination={false}
          locale={{ emptyText: 'No roles yet: rules use the built-in ones below' }}
          columns={[
            { title: 'Role', dataIndex: 'name' },
            { title: 'Id', dataIndex: 'id', render: (v) => <IdText id={v} /> },
            {
              title: 'Permissions',
              dataIndex: 'permissions',
              render: (permissions: Permission[]) => <PermissionTags permissions={permissions} />,
            },
            {
              title: 'Restrictions',
              dataIndex: 'restrictions',
              render: (r: OrgRole['restrictions']) => restrictionSummary(r) || <Typography.Text type="secondary">None</Typography.Text>,
            },
            {
              title: 'Rules',
              dataIndex: 'rules',
              render: (n: number) => (n === 1 ? '1 rule' : `${n} rules`),
            },
            {
              title: '',
              key: 'actions',
              align: 'right',
              render: (_, role) => (
                <Space size={4}>
                  <Tooltip title="Edit">
                    <Button
                      size="small"
                      aria-label={`Edit ${role.name}`}
                      icon={<EditOutlined />}
                      onClick={() => open(role)}
                    />
                  </Tooltip>
                  {role.rules ? (
                    <Tooltip title="In use: give its rules another role first">
                      <Button size="small" danger disabled aria-label={`Delete ${role.name}`} icon={<DeleteOutlined />} />
                    </Tooltip>
                  ) : (
                    <Popconfirm title={`Delete ${role.name}?`} onConfirm={() => remove(role)}>
                      <Tooltip title="Delete">
                        <Button size="small" danger aria-label={`Delete ${role.name}`} icon={<DeleteOutlined />} />
                      </Tooltip>
                    </Popconfirm>
                  )}
                </Space>
              ),
            },
          ]}
        />
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="Pick an organization to see its roles" />
      )}

      <Typography.Title level={5} style={{ marginTop: 24 }}>Built-in roles</Typography.Title>
      <Typography.Paragraph type="secondary">
        Every organization&apos;s rules can also use these; they cannot be edited.
      </Typography.Paragraph>
      <Table
        rowKey="name"
        size="small"
        pagination={false}
        dataSource={BUILTIN}
        columns={[
          { title: 'Role', dataIndex: 'name' },
          // A built-in role has no UUID: rules name it by this key.
          { title: 'Id', dataIndex: 'name', render: (v) => <IdText id={v} /> },
          {
            title: 'Permissions',
            dataIndex: 'permissions',
            render: (permissions: Permission[]) => <PermissionTags permissions={permissions} />,
          },
        ]}
      />

      <Modal
        title={editing === true ? 'New role' : 'Edit role'}
        visible={editing != null}
        onCancel={() => setEditing(null)}
        onOk={() => form.submit()}
        okText="Save"
        confirmLoading={saving}
        width={640}
        destroyOnClose
      >
        <Form
          form={form}
          layout="vertical"
          requiredMark={false}
          onFinish={save}
          // Checking a permission checks what it implies.
          onValuesChange={(changed) => {
            if (changed.permissions) {
              form.setFieldsValue({ permissions: withImplied(changed.permissions) });
            }
          }}
        >
          <Form.Item
            label="Name"
            name="name"
            rules={[
              { required: true, whitespace: true, message: 'Name the role' },
              { max: MAX_NAME, message: `At most ${MAX_NAME} characters` },
            ]}
          >
            <Input maxLength={MAX_NAME} autoComplete="off" />
          </Form.Item>
          <Form.Item
            label="Permissions in the rule's workspace"
            name="permissions"
            extra="Manage implies every permission; writing objects implies reading them; any implies viewing."
            rules={[{ required: true, type: 'array', min: 1, message: 'Pick at least one permission' }]}
          >
            <Checkbox.Group style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {PERMISSIONS.map((p) => (
                <Checkbox key={p} value={p} style={{ marginLeft: 0 }}>
                  {PERMISSION_LABELS[p]}
                  {' '}
                  <Typography.Text type="secondary">{`(${p})`}</Typography.Text>
                </Checkbox>
              ))}
            </Checkbox.Group>
          </Form.Item>
          {cubesError ? (
            <Alert
              type="warning"
              showIcon
              style={{ marginBottom: 16 }}
              message="Could not load the model members, so the pickers below are empty"
              description={cubesError}
            />
          ) : null}
          <RestrictionsFields cubes={cubes} />
          {editingRules ? (
            <Alert
              type="info"
              showIcon
              style={{ marginBottom: 16 }}
              message={`${editingRules === 1 ? '1 rule uses' : `${editingRules} rules use`} this role: their members get the new permissions at once.`}
            />
          ) : null}
          {error ? <Alert type="error" message={error} showIcon /> : null}
        </Form>
      </Modal>
    </>
  );
}

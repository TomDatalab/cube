import { DeleteOutlined, EditOutlined, HolderOutlined } from '@ant-design/icons';
import {
  Alert, AutoComplete, Button, Col, Form, Input, message, Modal, Popconfirm, Row, Select, Space, Table, Tag,
  Tooltip, Typography,
} from 'antd';
import { useMemo, useRef, useState } from 'react';

import { useIsMounted } from '../../hooks';
import { IdText } from './IdText';
import {
  AdminApi, OrgRole, PAGE_SIZE, Rule,
} from './api';
import { organizationOptions, RemoteSelect, workspaceOptions } from './RemoteSelect';
import { RULE_DRAG_TYPE, ruleDragData, RuleTree } from './RuleTree';
import { useSearch } from './useSearch';

type Props = {
  api: AdminApi;
};

// `MAX_DESCRIPTION` and `MAX_TYPE` in `cubeserver/src/rbac/handlers/rules.rs`.
const MAX_DESCRIPTION = 500;
const MAX_TYPE = 100;

// The form's role: the id of one of the organization's roles.
type FormValues = { description: string; type: string; role: string; workspaceId: string };

// Every rule (a named role in one workspace) of every organization. Rows
// drag onto a workspace of the tree to move there; users dragged from the
// Users section onto a rule join it.
export function RulesTab({ api }: Props) {
  const [organization, setOrganization] = useState<string | null>(null);
  const {
    q, setQ, page, setPage, result, loading, error: searchError, reload,
  } = useSearch(
    (term, p) => api.searchRules({ q: term, organizationId: organization, page: p }),
    [api, organization]
  );
  // `true`: a new rule; a rule: editing it.
  const [editing, setEditing] = useState<Rule | true | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Bumped when the table changed rules, so the tree loads them again.
  const [treeVersion, setTreeVersion] = useState(0);
  const [form] = Form.useForm<FormValues>();
  const isMounted = useIsMounted();
  // The roles of the organization of the workspace picked in the form.
  const [orgRoles, setOrgRoles] = useState<OrgRole[]>([]);
  const rolesRequest = useRef(0);

  async function loadRoles(org: string | null) {
    const request = ++rolesRequest.current;
    setOrgRoles([]);
    if (!org) {
      return;
    }
    try {
      const loaded = await api.roles(org);
      if (isMounted() && request === rolesRequest.current) {
        setOrgRoles(loaded);
        // A role of another organization does not carry over.
        const role = form.getFieldValue('role');
        if (!role || !loaded.some((r) => r.id === role)) {
          // A new rule starts as the organization's default viewer, if it has one.
          form.setFieldsValue({ role: loaded.find((r) => r.builtin === 'viewer')?.id });
        }
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  async function workspaceChanged(ws: string | undefined) {
    if (!ws) {
      loadRoles(null);
      return;
    }
    try {
      loadRoles((await api.workspace(ws)).organizationId);
    } catch (e: any) {
      message.error(e.message);
    }
  }

  const roleOptions = orgRoles.map((r) => ({
    value: r.id,
    label: `${r.name} — ${r.permissions.join(', ')}`,
  }));
  // The types already in use on this page, offered while typing a new one.
  const typeOptions = useMemo(
    () => [...new Set((result?.items || []).map((r) => r.type))].sort().map((t) => ({ value: t })),
    [result]
  );

  function changed() {
    reload();
    setTreeVersion((v) => v + 1);
  }

  function open(rule: Rule | true) {
    form.resetFields();
    form.setFieldsValue(rule === true
      ? {}
      : {
        description: rule.description,
        type: rule.type,
        role: rule.roleId ?? undefined,
        workspaceId: rule.workspaceId,
      });
    loadRoles(rule === true ? null : rule.organizationId);
    setError(null);
    setEditing(rule);
  }

  async function save(values: FormValues) {
    setSaving(true);
    setError(null);
    try {
      const description = values.description.trim();
      const type = values.type.trim();
      if (editing === true) {
        await api.createRule(values.workspaceId, { description, type, roleId: values.role });
      } else if (editing) {
        await api.updateRule(editing.id, {
          description,
          type,
          roleId: values.role,
          workspaceId: values.workspaceId,
        });
      }
      message.success(`${description} saved`);
      setEditing(null);
      changed();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  async function remove(rule: Rule) {
    try {
      await api.deleteRule(rule.id);
      message.success(`${rule.description} deleted`);
      changed();
    } catch (e: any) {
      message.error(e.message);
    }
  }

  return (
    <Row gutter={24}>
      <Col xs={24} xl={15}>
        <Space style={{ marginBottom: 16 }}>
          <RemoteSelect
            allowClear
            placeholder="All organizations"
            style={{ width: 240 }}
            value={organization ?? undefined}
            onChange={(v) => setOrganization(v || null)}
            fetchOptions={organizationOptions(api)}
          />
          <Button type="primary" onClick={() => open(true)}>
            New rule
          </Button>
        </Space>

        <Input.Search
          allowClear
          placeholder="Search rules by description, type, role, workspace or organization"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          style={{ maxWidth: 400, display: 'block', marginBottom: 16 }}
        />
        {searchError ? (
          <Alert type="error" message={searchError} showIcon style={{ marginBottom: 16 }} />
        ) : null}

        <Table<Rule>
          rowKey="id"
          size="small"
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
          // A rule drags onto a workspace of the tree to move there.
          onRow={(rule) => ({
            draggable: true,
            onDragStart: (e) => {
              e.dataTransfer.setData(RULE_DRAG_TYPE, ruleDragData(rule));
              e.dataTransfer.effectAllowed = 'move';
            },
            style: { cursor: 'grab' },
          })}
          columns={[
            {
              key: 'drag',
              width: 32,
              render: () => (
                <HolderOutlined title="Drag onto a workspace" style={{ color: 'var(--pg-muted, #6c7393)' }} />
              ),
            },
            {
              title: 'Description',
              dataIndex: 'description',
              render: (v) => (
                <Typography.Text ellipsis={{ tooltip: v }} style={{ maxWidth: 240 }}>
                  {v}
                </Typography.Text>
              ),
            },
            { title: 'Id', dataIndex: 'id', render: (v) => <IdText id={v} /> },
            { title: 'Type', dataIndex: 'type' },
            {
              title: 'Role',
              dataIndex: 'roleName',
              render: (v: string, rule) => (
                <Tooltip title={rule.permissions.join(', ')}>
                  <Tag>{v}</Tag>
                </Tooltip>
              ),
            },
            { title: 'Workspace', dataIndex: 'workspaceName' },
            { title: 'Organization', dataIndex: 'organizationName' },
            {
              title: '',
              key: 'actions',
              align: 'right',
              fixed: 'right',
              render: (_, rule) => (
                <Space size={4}>
                  <Tooltip title="Edit">
                    <Button
                      size="small"
                      aria-label={`Edit ${rule.description}`}
                      icon={<EditOutlined />}
                      onClick={() => open(rule)}
                    />
                  </Tooltip>
                  <Popconfirm
                    title={`Delete ${rule.description}? Its members lose the ${rule.roleName} role it gives.`}
                    onConfirm={() => remove(rule)}
                  >
                    <Tooltip title="Delete">
                      <Button
                        size="small"
                        danger
                        aria-label={`Delete ${rule.description}`}
                        icon={<DeleteOutlined />}
                      />
                    </Tooltip>
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />

        <Modal
          title={editing === true ? 'New rule' : 'Edit rule'}
          visible={editing != null}
          onCancel={() => setEditing(null)}
          onOk={() => form.submit()}
          okText="Save"
          confirmLoading={saving}
          destroyOnClose
        >
          <Form
            form={form}
            layout="vertical"
            requiredMark={false}
            onFinish={save}
            onValuesChange={(values) => {
              if ('workspaceId' in values) {
                workspaceChanged(values.workspaceId);
              }
            }}
          >
            <Form.Item
              label="Description"
              name="description"
              rules={[
                { required: true, whitespace: true, message: 'Describe the rule' },
                { max: MAX_DESCRIPTION, message: `At most ${MAX_DESCRIPTION} characters` },
              ]}
            >
              <Input.TextArea autoSize={{ minRows: 2, maxRows: 6 }} showCount maxLength={MAX_DESCRIPTION} />
            </Form.Item>
            <Form.Item
              label="Type"
              name="type"
              extra="Any label, e.g. finance or audit."
              rules={[
                { required: true, whitespace: true, message: 'Type a type' },
                { max: MAX_TYPE, message: `At most ${MAX_TYPE} characters` },
              ]}
            >
              <AutoComplete
                options={typeOptions}
                filterOption={(input, option) => (option?.value as string)
                  .toLowerCase().includes(input.toLowerCase())}
              >
                <Input maxLength={MAX_TYPE} autoComplete="off" />
              </AutoComplete>
            </Form.Item>
            <Form.Item
              label="Workspace"
              name="workspaceId"
              extra="The organization is the workspace's."
              rules={[{ required: true, message: 'Pick a workspace' }]}
            >
              <RemoteSelect
                placeholder="Organization / workspace"
                fetchOptions={workspaceOptions(api)}
                valueLabel={editing && editing !== true
                  ? `${editing.organizationName} / ${editing.workspaceName}`
                  : undefined}
              />
            </Form.Item>
            <Form.Item
              label="Role"
              name="role"
              extra="What the rule's members can do in the workspace: one of the organization's roles (Admin → Roles)."
              rules={[{ required: true, message: 'Pick a role' }]}
            >
              <Select options={roleOptions} />
            </Form.Item>
            {error ? <Alert type="error" message={error} showIcon /> : null}
          </Form>
        </Modal>
      </Col>
      <Col xs={24} xl={9}>
        <Typography.Title level={5}>Workspaces</Typography.Title>
        <Typography.Paragraph type="secondary">
          Drag a rule onto a workspace to move it there, with its members. Users join a rule by
          being dragged onto it in Users.
        </Typography.Paragraph>
        <RuleTree api={api} version={treeVersion} onChanged={reload} />
      </Col>
    </Row>
  );
}

import {
  Alert,
  Button,
  Card,
  Col,
  Form,
  Image,
  Input,
  Layout,
  message,
  Popconfirm,
  Row,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
} from 'antd';
import {
  ArrowLeftOutlined,
  CheckCircleFilled,
  DatabaseOutlined,
  DeleteOutlined,
  EditOutlined,
  PlusOutlined,
} from '@ant-design/icons';
import { useCallback, useEffect, useMemo, useState } from 'react';

import envVarsDbMap from '../../shared/env-vars-db-map';
import Base64Upload from '../ConnectionWizard/components/Base64Upload';
import { Content, Header } from '../components/Ui';

const { Title, Text, Paragraph } = Typography;

type Setting = {
  env: string;
  title?: string;
  type?: string;
  uploadTarget?: string;
  extractField?: { jsonField: string; formField: string };
};

type Database = {
  title: string;
  driver: string;
  logo?: string;
  instructions?: string;
  settings: Setting[];
};

type DataSource = {
  name: string;
  type: string;
  host?: string | null;
  port?: number | null;
  database?: string | null;
  user?: string | null;
  hasPassword?: boolean;
  active: boolean;
};

type TestState = { loading?: boolean; ok?: boolean; error?: string };

// Settings for a connector the wizard's map does not describe (SQLite,
// DuckDB, Trino, ...): the fields every server-based database takes.
const GENERIC_SETTINGS: Setting[] = [
  { env: 'CUBEJS_DB_HOST', title: 'Hostname' },
  { env: 'CUBEJS_DB_PORT', title: 'Port' },
  { env: 'CUBEJS_DB_NAME', title: 'Database' },
  { env: 'CUBEJS_DB_USER', title: 'Username' },
  { env: 'CUBEJS_DB_PASS', title: 'Password' },
];

const NAME_PATTERN = /^[a-z][a-z0-9_]{0,62}$/;

async function request(url: string, options: RequestInit = {}) {
  const response = await fetch(url, {
    ...options,
    headers: { 'Content-Type': 'application/json', ...(options.headers || {}) },
  });
  let body: any = null;
  try {
    body = await response.json();
  } catch {
    // An empty or non-JSON body; the status says enough.
  }
  if (!response.ok) {
    throw new Error(body?.error || `${response.status} ${response.statusText}`);
  }
  return body;
}

function databasesFor(connectors: string[]): Database[] {
  const known: Database[] = envVarsDbMap.flatMap((group) => group.databases.map((db) => ({ ...db, settings: group.settings as Setting[] })));
  const list = known.filter((db) => connectors.includes(db.driver));
  connectors
    .filter((driver) => !known.some((db) => db.driver === driver))
    .forEach((driver) => list.push({ title: driver, driver, settings: GENERIC_SETTINGS }));
  return list;
}

function DatabaseLogo({ db, size = 24 }: { db?: Database; size?: number }) {
  return db?.logo ? (
    <Image src={db.logo} preview={false} width={size} height={size} />
  ) : (
    <DatabaseOutlined style={{ fontSize: size * 0.8 }} />
  );
}

// Form.Item hands `value` and `onChange` to its direct child, so they must
// reach the input.
function SettingControl({ setting, ...props }: { setting: Setting; [prop: string]: any }) {
  if (/PASS|SECRET/.test(setting.env)) {
    return <Input.Password autoComplete="new-password" {...props} />;
  }
  return <Input {...props} />;
}

export function DataSourcesPage() {
  const [dataSources, setDataSources] = useState<DataSource[]>([]);
  const [connectors, setConnectors] = useState<string[]>([]);
  const [editable, setEditable] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tests, setTests] = useState<Record<string, TestState>>({});

  const [adding, setAdding] = useState(false);
  // The data source being edited; `adding` is set too, for the form.
  const [editing, setEditing] = useState<{ name: string; hasPassword: boolean } | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [initialValues, setInitialValues] = useState<Record<string, string>>({});
  const [selected, setSelected] = useState<Database | null>(null);
  const [formTest, setFormTest] = useState<TestState>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [restarting, setRestarting] = useState<string | null>(null);
  const [form] = Form.useForm();

  const databases = useMemo(() => databasesFor(connectors), [connectors]);
  const databaseOf = (driver: string) => databases.find((db) => db.driver === driver);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const body = await request('playground/data-sources');
      setDataSources(body.dataSources || []);
      setConnectors(body.connectors || []);
      setEditable(!!body.editable);
      setLoadError(null);
    } catch (e: any) {
      setLoadError(e.message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  async function testExisting(name: string) {
    setTests((t) => ({ ...t, [name]: { loading: true } }));
    try {
      await request('playground/data-sources/test', {
        method: 'POST',
        body: JSON.stringify({ dataSource: name }),
      });
      setTests((t) => ({ ...t, [name]: { ok: true } }));
    } catch (e: any) {
      setTests((t) => ({ ...t, [name]: { ok: false, error: e.message } }));
    }
  }

  // When editing, every field the form shows is sent, empty ones too: the
  // server clears an emptied setting, and keeps the password if it is empty.
  function variables(values: Record<string, string>) {
    const out: Record<string, string> = editing ? {} : { CUBEJS_DB_TYPE: selected!.driver };
    selected!.settings
      .filter((s) => s.type !== 'base64upload')
      .forEach((s) => {
        if (values[s.env]) {
          out[s.env] = String(values[s.env]);
        } else if (editing) {
          out[s.env] = '';
        }
      });
    return out;
  }

  async function testForm() {
    const values = form.getFieldsValue();
    setFormTest({ loading: true });
    try {
      await request('playground/data-sources/test', {
        method: 'POST',
        body: JSON.stringify(
          editing
            ? { dataSource: editing.name, variables: variables(values) }
            : { variables: variables(values) }
        ),
      });
      setFormTest({ ok: true });
    } catch (e: any) {
      setFormTest({ ok: false, error: e.message });
    }
  }

  // The server replaces itself to load the new source; wait until it
  // answers again, then show the list it now serves.
  async function waitForRestart() {
    await new Promise((resolve) => setTimeout(resolve, 2000));
    for (let i = 0; i < 60; i++) {
      try {
        await request('playground/context');
        break;
      } catch {
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
    }
  }

  async function save(values: Record<string, string>) {
    setSaving(true);
    setSaveError(null);
    try {
      const body = editing
        ? await request(`playground/data-sources/${encodeURIComponent(editing.name)}`, {
          method: 'PUT',
          body: JSON.stringify({ variables: variables(values) }),
        })
        : await request('playground/data-sources', {
          method: 'POST',
          body: JSON.stringify({ name: values.name, variables: variables(values) }),
        });
      const verb = editing ? 'updated' : 'saved';
      closeForm();
      await afterChange(body, verb);
    } catch (e: any) {
      setSaveError(e.message);
    } finally {
      setSaving(false);
    }
  }

  async function afterChange(body: { name: string; restarting: boolean }, verb: string) {
    setTests({});
    if (body.restarting) {
      setRestarting(`Data source ${body.name} ${verb}. Restarting the server to load it…`);
      await waitForRestart();
      setRestarting(null);
    }
    message.success(`Data source ${body.name} ${verb}`);
    await load();
  }

  async function startEdit(ds: DataSource) {
    try {
      const body = await request(`playground/data-sources/${encodeURIComponent(ds.name)}`);
      const db = databaseOf(body.type) || { title: body.type, driver: body.type, settings: GENERIC_SETTINGS };
      closeForm();
      setEditing({ name: ds.name, hasPassword: !!body.hasPassword });
      setSelected(db);
      setInitialValues({ name: ds.name, ...body.variables });
      setAdding(true);
    } catch (e: any) {
      message.error(e.message);
    }
  }

  async function remove(name: string) {
    setDeleting(name);
    try {
      const body = await request(`playground/data-sources/${encodeURIComponent(name)}`, {
        method: 'DELETE',
      });
      await afterChange(body, 'deleted');
    } catch (e: any) {
      message.error({ content: e.message, duration: 8 });
    } finally {
      setDeleting(null);
    }
  }

  function closeForm() {
    setEditing(null);
    setInitialValues({});
    setAdding(false);
    setSelected(null);
    setFormTest({});
    setSaveError(null);
    form.resetFields();
  }

  const columns = [
    {
      title: 'Name',
      dataIndex: 'name',
      key: 'name',
      render: (name: string) => <Text strong>{name}</Text>,
    },
    {
      title: 'Type',
      dataIndex: 'type',
      key: 'type',
      render: (type: string) => (
        <Space>
          <DatabaseLogo db={databaseOf(type)} size={20} />
          {databaseOf(type)?.title || type}
        </Space>
      ),
    },
    {
      title: 'Host',
      key: 'host',
      render: (_: unknown, ds: DataSource) => (ds.host ? `${ds.host}${ds.port ? `:${ds.port}` : ''}` : '—'),
    },
    {
      title: 'Database',
      dataIndex: 'database',
      key: 'database',
      render: (v?: string | null) => v || '—',
    },
    {
      title: 'User',
      dataIndex: 'user',
      key: 'user',
      render: (v?: string | null) => v || '—',
    },
    {
      title: 'Status',
      key: 'status',
      render: (_: unknown, ds: DataSource) => {
        if (!ds.active) {
          return <Tag color="orange">Restart pending</Tag>;
        }
        const test = tests[ds.name];
        if (test?.ok) {
          return <Tag color="green">Connected</Tag>;
        }
        if (test?.error) {
          return (
            <Tooltip title={test.error}>
              <Tag color="red">Failed</Tag>
            </Tooltip>
          );
        }
        return <Tag>Not tested</Tag>;
      },
    },
    {
      title: '',
      key: 'actions',
      align: 'right' as const,
      render: (_: unknown, ds: DataSource) => (
        <Space>
          <Button
            size="small"
            disabled={!ds.active}
            loading={tests[ds.name]?.loading}
            onClick={() => testExisting(ds.name)}
          >
            Test connection
          </Button>
          {editable ? (
            <>
              <Button
                size="small"
                icon={<EditOutlined />}
                data-testid={`data-source-edit-${ds.name}`}
                disabled={!!restarting}
                onClick={() => startEdit(ds)}
              >
                Edit
              </Button>
              {ds.name === 'default' ? (
                <Tooltip title="The default data source cannot be deleted">
                  <Button size="small" danger icon={<DeleteOutlined />} disabled>
                    Delete
                  </Button>
                </Tooltip>
              ) : (
                <Popconfirm
                  title={`Delete data source ${ds.name}? Its settings are removed from cube.yml and .env.`}
                  okText="Delete"
                  okButtonProps={{ danger: true }}
                  onConfirm={() => remove(ds.name)}
                >
                  <Button
                    size="small"
                    danger
                    icon={<DeleteOutlined />}
                    data-testid={`data-source-delete-${ds.name}`}
                    loading={deleting === ds.name}
                    disabled={!!restarting}
                  >
                    Delete
                  </Button>
                </Popconfirm>
              )}
            </>
          ) : null}
        </Space>
      ),
    },
  ];

  function renderPicker() {
    return (
      <>
        <Paragraph>Choose the database to connect to.</Paragraph>
        <Row gutter={[16, 16]}>
          {databases.map((db) => (
            <Col key={db.driver} xs={24} sm={12} md={8} lg={6}>
              <Card
                hoverable
                size="small"
                data-testid={`data-source-db-${db.driver}`}
                onClick={() => {
                  setSelected(db);
                  setFormTest({});
                }}
              >
                <Space>
                  <DatabaseLogo db={db} size={28} />
                  <Text strong>{db.title}</Text>
                </Space>
              </Card>
            </Col>
          ))}
        </Row>
      </>
    );
  }

  function renderForm(db: Database) {
    return (
      <Row gutter={48}>
        <Col xs={24} lg={12}>
          <Space style={{ marginBottom: 16 }}>
            {!editing ? (
              <Button icon={<ArrowLeftOutlined />} onClick={() => setSelected(null)}>
                Change database
              </Button>
            ) : null}
            <DatabaseLogo db={db} size={28} />
            <Text strong>{db.title}</Text>
          </Space>

          {db.instructions ? (
            // The wizard's own copy, with its links.
            // eslint-disable-next-line react/no-danger
            <Paragraph><span dangerouslySetInnerHTML={{ __html: db.instructions }} /></Paragraph>
          ) : null}

          <Form
            key={editing?.name || 'new'}
            form={form}
            initialValues={initialValues}
            layout="vertical"
            onFinish={save}
            onValuesChange={() => setFormTest({})}
          >
            <Form.Item
              label="Name"
              name="name"
              extra={editing
                ? 'The name cannot change: the data model refers to it.'
                : 'Used as data_source: in the data model. Lower-case letters, digits and _.'}
              rules={editing ? [] : [
                { required: true, message: 'Give the data source a name' },
                { pattern: NAME_PATTERN, message: 'Start with a letter; use only a-z, 0-9 and _' },
                {
                  validator: (_, value) => (dataSources.some((ds) => ds.name === value)
                    ? Promise.reject(new Error('A data source with this name exists'))
                    : Promise.resolve()),
                },
              ]}
            >
              <Input placeholder="sales" disabled={!!editing} />
            </Form.Item>

            {db.settings.map((setting) => (setting.type === 'base64upload' ? (
              <Form.Item key={setting.env} label={setting.title}>
                <Base64Upload
                  onInput={({ raw, encoded }) => {
                    if (setting.uploadTarget) {
                      form.setFieldsValue({ [setting.uploadTarget]: encoded });
                    }
                    if (setting.extractField) {
                      form.setFieldsValue({
                        [setting.extractField.formField]: raw[setting.extractField.jsonField],
                      });
                    }
                  }}
                />
              </Form.Item>
            ) : (
              <Form.Item key={setting.env} label={setting.title || setting.env} name={setting.env}>
                <SettingControl
                  setting={setting}
                  placeholder={editing?.hasPassword && setting.env === 'CUBEJS_DB_PASS'
                    ? 'Leave empty to keep the current password'
                    : undefined}
                />
              </Form.Item>
            )))}

            {formTest.ok ? (
              <Paragraph>
                <Text type="success">
                  <CheckCircleFilled /> Connected
                </Text>
              </Paragraph>
            ) : null}
            {formTest.error ? (
              <Alert type="error" showIcon message={formTest.error} style={{ marginBottom: 16 }} />
            ) : null}
            {saveError ? (
              <Alert type="error" showIcon message={saveError} style={{ marginBottom: 16 }} />
            ) : null}

            <Space>
              <Button onClick={testForm} loading={formTest.loading}>
                Test connection
              </Button>
              <Button type="primary" htmlType="submit" loading={saving} data-testid="data-source-save">
                Save
              </Button>
            </Space>
          </Form>
        </Col>
        <Col xs={24} lg={12}>
          <Alert
            type="info"
            showIcon
            message="What saving does"
            description={editing ? (
              <>
                The connection is tested with the new settings first and saved only if it
                works. Each setting is changed where it is read from: the variable{' '}
                <Text code>cube.yml</Text> names for it, or <Text code>.env</Text>. The server
                restarts to load it, which takes a few seconds.
              </>
            ) : (
              <>
                The connection is tested first and saved only if it works. The data source is
                added to <Text code>cube.yml</Text>, its settings (password included) go to{' '}
                <Text code>.env</Text>, and the server restarts to load it, which takes a few
                seconds. Then point cubes at it with <Text code>data_source: &lt;name&gt;</Text>,
                or generate them from the Data Model page.
              </>
            )}
          />
        </Col>
      </Row>
    );
  }

  return (
    <Layout>
      <Header>
        <Row justify="space-between" align="middle">
          <Title>Data Source</Title>
          {!adding && editable ? (
            <Button
              type="primary"
              icon={<PlusOutlined />}
              data-testid="data-source-add"
              onClick={() => setAdding(true)}
              disabled={!!restarting}
            >
              Add data source
            </Button>
          ) : null}
        </Row>
      </Header>

      <Content>
        {restarting ? (
          <Alert
            type="info"
            showIcon
            style={{ marginBottom: 24 }}
            message={restarting}
          />
        ) : null}
        {loadError ? (
          <Alert type="error" showIcon style={{ marginBottom: 24 }} message="Cannot read the data sources" description={loadError} />
        ) : null}
        {!loading && !editable && !loadError ? (
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 24 }}
            message="This deployment has no cube.yml, so data sources cannot be added here."
          />
        ) : null}

        {adding ? (
          <Card
            title={editing ? `Edit data source ${editing.name}` : 'New data source'}
            extra={<Button type="text" onClick={closeForm}>Cancel</Button>}
            style={{ marginBottom: 24 }}
          >
            {selected ? renderForm(selected) : renderPicker()}
          </Card>
        ) : null}

        <Table
          rowKey="name"
          loading={loading}
          dataSource={dataSources}
          columns={columns}
          pagination={false}
        />
      </Content>
    </Layout>
  );
}

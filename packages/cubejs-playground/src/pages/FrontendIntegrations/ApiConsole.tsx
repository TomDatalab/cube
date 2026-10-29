import { Alert, Button, Card, Col, Input, Row, Select, Space, Table, Tag, Typography } from 'antd';
import { SendOutlined } from '@ant-design/icons';
import { useEffect, useMemo, useState } from 'react';

import { usePlaygroundContext, useToken } from '../../hooks';
import { getWorkspace, WORKSPACE_HEADER } from '../../shared/auth';
import { apiFetch } from '../../shared/helpers';

const { Text, Paragraph } = Typography;

type Method = 'GET' | 'POST' | 'PUT' | 'DELETE';

export type Endpoint = {
  key: string;
  group: string;
  name: string;
  method: Method;
  // Relative to the API prefix (`basePath`), e.g. `/v1/meta`.
  path: string;
  scope: string;
  body?: unknown;
};

// The data source endpoints write `cube.yml` and `.env`; the example adds
// a SQLite file so it can be tried without a database server.
const NEW_SOURCE = {
  name: 'example',
  variables: {
    CUBEJS_DB_TYPE: 'sqlite',
    CUBEJS_DB_NAME: '/tmp/cube-example.db',
  },
};

export function endpoints(measure: string): Endpoint[] {
  const query = { query: { measures: [measure] } };
  return [
    { key: 'meta', group: 'Data', name: 'Data model', method: 'GET', path: '/v1/meta', scope: 'meta' },
    { key: 'load', group: 'Data', name: 'Run a query', method: 'POST', path: '/v1/load', scope: 'data', body: query },
    { key: 'sql', group: 'Data', name: 'SQL of a query', method: 'POST', path: '/v1/sql', scope: 'data', body: query },
    {
      key: 'graphql',
      group: 'Data',
      name: 'GraphQL',
      method: 'POST',
      path: '/graphql',
      scope: 'graphql',
      body: { query: '{ __schema { queryType { name } } }' },
    },
    { key: 'connectors', group: 'Data', name: 'Connectors', method: 'GET', path: '/v1/connectors', scope: 'meta' },
    { key: 'ds-list', group: 'Data Source', name: 'List data sources', method: 'GET', path: '/v1/data-sources', scope: 'data_sources' },
    { key: 'ds-get', group: 'Data Source', name: 'Read a data source', method: 'GET', path: '/v1/data-sources/default', scope: 'data_sources' },
    {
      key: 'ds-test',
      group: 'Data Source',
      name: 'Test a connection',
      method: 'POST',
      path: '/v1/data-sources/test',
      scope: 'data_sources',
      body: { dataSource: 'default' },
    },
    {
      key: 'ds-create',
      group: 'Data Source',
      name: 'Add a data source',
      method: 'POST',
      path: '/v1/data-sources',
      scope: 'data_sources',
      body: NEW_SOURCE,
    },
    {
      key: 'ds-update',
      group: 'Data Source',
      name: 'Edit a data source',
      method: 'PUT',
      path: `/v1/data-sources/${NEW_SOURCE.name}`,
      scope: 'data_sources',
      body: { variables: { CUBEJS_DB_NAME: '/tmp/cube-example-2.db' } },
    },
    {
      key: 'ds-execute',
      group: 'Data Source',
      name: 'Run a statement (tables, views...)',
      method: 'POST',
      path: '/v1/execute',
      scope: 'execute',
      body: {
        connection: 'default',
        database: 'analytics',
        query: 'CREATE TABLE IF NOT EXISTS exemplo (id serial PRIMARY KEY, nome text)',
      },
    },
    {
      key: 'ds-delete',
      group: 'Data Source',
      name: 'Delete a data source',
      method: 'DELETE',
      path: `/v1/data-sources/${NEW_SOURCE.name}`,
      scope: 'data_sources',
    },
  ];
}

const METHOD_COLORS: Record<Method, string> = {
  GET: 'blue',
  POST: 'green',
  PUT: 'orange',
  DELETE: 'red',
};

type Result = {
  status: number;
  statusText: string;
  ms: number;
  body: string;
};

function statusColor(status: number) {
  if (status < 300) {
    return 'green';
  }
  return status < 500 ? 'orange' : 'red';
}

export function ApiConsole() {
  const playgroundContext = usePlaygroundContext();
  // The token set on the Playground page (security context), or the one the
  // server minted for the Playground.
  const appToken = useToken() || playgroundContext?.cubejsToken || null;
  const basePath = (playgroundContext?.basePath || '').replace(/\/$/, '');
  const apiUrl = `${window.location.origin}${basePath}`;

  const [measure, setMeasure] = useState('Orders.count');
  const list = useMemo(() => endpoints(measure), [measure]);

  const [method, setMethod] = useState<Method>('GET');
  const [path, setPath] = useState('/v1/data-sources');
  const [body, setBody] = useState('');
  const [token, setToken] = useState('');
  const [sending, setSending] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Sent by `apiFetch` on every request; shown in the curl below.
  const workspace = getWorkspace();

  useEffect(() => {
    if (appToken && !token) {
      setToken(appToken);
    }
  }, [appToken]);

  // A real measure for the query examples, from the model this server serves.
  useEffect(() => {
    if (!appToken) {
      return;
    }
    apiFetch(`${basePath}/v1/meta`, { headers: { Authorization: appToken } })
      .then((r) => (r.ok ? r.json() : null))
      .then((meta) => {
        const first = meta?.cubes?.find((c) => c.measures?.length)?.measures[0]?.name;
        if (first) {
          setMeasure(first);
        }
      })
      .catch(() => undefined);
  }, [appToken, basePath]);

  function pick(endpoint: Endpoint) {
    setMethod(endpoint.method);
    setPath(endpoint.path);
    setBody(endpoint.body === undefined ? '' : JSON.stringify(endpoint.body, null, 2));
    setResult(null);
    setError(null);
  }

  async function send() {
    setError(null);
    let payload: string | undefined;
    if (method !== 'GET' && body.trim()) {
      try {
        payload = JSON.stringify(JSON.parse(body));
      } catch (e: any) {
        setError(`The body is not valid JSON: ${e.message}`);
        return;
      }
    }

    setSending(true);
    const started = performance.now();
    try {
      const response = await apiFetch(`${basePath}${path.startsWith('/') ? path : `/${path}`}`, {
        method,
        headers: {
          ...(token ? { Authorization: token } : {}),
          ...(payload ? { 'Content-Type': 'application/json' } : {}),
        },
        body: payload,
      });
      const text = await response.text();
      let pretty = text;
      try {
        pretty = JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        // Not JSON; shown as it came.
      }
      setResult({
        status: response.status,
        statusText: response.statusText,
        ms: Math.round(performance.now() - started),
        body: pretty,
      });
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSending(false);
    }
  }

  const curl = [
    `curl -X ${method}`,
    `  -H "Authorization: ${token ? '<token>' : ''}"`,
    ...(workspace ? [`  -H "${WORKSPACE_HEADER}: ${workspace}"`] : []),
    ...(method !== 'GET' && body.trim() ? ['  -H "Content-Type: application/json"', `  -d '${body.replace(/\s*\n\s*/g, ' ').replace(/'/g, "'\\''")}'`] : []),
    `  ${apiUrl}${path}`,
  ].join(' \\\n');

  const restarting = result && result.status === 200 && /"restarting":\s*true/.test(result.body);

  return (
    <Row gutter={48}>
      <Col xs={24} xl={10}>
        <Paragraph>
          Every endpoint of this server, relative to <Text code>{apiUrl}</Text>. Pick one to load
          it into the console. Each needs a token with the scope shown.
        </Paragraph>
        <Table<Endpoint>
          size="small"
          rowKey="key"
          pagination={false}
          dataSource={list}
          onRow={(endpoint) => ({ onClick: () => pick(endpoint), style: { cursor: 'pointer' } })}
          columns={[
            {
              title: 'Endpoint',
              key: 'endpoint',
              render: (_, e) => (
                <Space direction="vertical" size={0}>
                  <Text strong>{e.name}</Text>
                  <Space size={4}>
                    <Tag color={METHOD_COLORS[e.method]}>{e.method}</Tag>
                    <Text code>{e.path}</Text>
                  </Space>
                </Space>
              ),
            },
            { title: 'Group', dataIndex: 'group', key: 'group' },
            {
              title: 'Scope',
              dataIndex: 'scope',
              key: 'scope',
              render: (scope) => <Tag>{scope}</Tag>,
            },
          ]}
        />
        <Alert
          style={{ marginTop: 16 }}
          type="info"
          showIcon
          message="The data_sources and execute scopes"
          description={(
            <>
              The Data Source endpoints read and write connection settings, passwords included,
              and <Text code>execute</Text> runs any statement on the database, so no token has
              these scopes unless the server grants them: add{' '}
              <Text code>data_sources</Text> or <Text code>execute</Text> to{' '}
              <Text code>CUBEJS_DEFAULT_API_SCOPES</Text>.
              A data source added, edited or deleted here shows on the Data Source page once
              the server has restarted, a few seconds later.
            </>
          )}
        />
      </Col>

      <Col xs={24} xl={14}>
        <Card title="Request" size="small">
          <Space direction="vertical" style={{ width: '100%' }}>
            <Input.Group compact style={{ display: 'flex' }}>
              <Select<Method>
                value={method}
                onChange={setMethod}
                style={{ width: 110 }}
                data-testid="api-console-method"
                options={(['GET', 'POST', 'PUT', 'DELETE'] as Method[]).map((m) => ({ label: m, value: m }))}
              />
              <Input
                addonBefore={basePath}
                value={path}
                onChange={(e) => setPath(e.target.value)}
                onPressEnter={send}
                style={{ flex: 1 }}
                data-testid="api-console-path"
              />
              <Button type="primary" icon={<SendOutlined />} loading={sending} onClick={send} data-testid="api-console-send">
                Send
              </Button>
            </Input.Group>

            <Input.Password
              addonBefore="Authorization"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="A JWT signed with the API secret"
            />

            {method !== 'GET' ? (
              <Input.TextArea
                value={body}
                onChange={(e) => setBody(e.target.value)}
                autoSize={{ minRows: 4, maxRows: 16 }}
                placeholder="JSON body"
                style={{ fontFamily: 'monospace' }}
                data-testid="api-console-body"
              />
            ) : null}

            <Text type="secondary">Equivalent command</Text>
            <pre style={{ background: '#f6f6f8', padding: 12, margin: 0, whiteSpace: 'pre-wrap', fontSize: 12 }}>
              {curl}
            </pre>
          </Space>
        </Card>

        {error ? <Alert type="error" showIcon message={error} style={{ marginTop: 16 }} /> : null}

        {result ? (
          <Card
            size="small"
            style={{ marginTop: 16 }}
            title={(
              <Space data-testid="api-console-status">
                <Tag color={statusColor(result.status)}>
                  {result.status} {result.statusText}
                </Tag>
                <Text type="secondary">{result.ms} ms</Text>
              </Space>
            )}
          >
            {restarting ? (
              <Alert
                type="info"
                showIcon
                style={{ marginBottom: 12 }}
                message="The server is restarting to load the change; the Data Source page shows it in a few seconds."
              />
            ) : null}
            <pre
              data-testid="api-console-response"
              style={{ margin: 0, maxHeight: 480, overflow: 'auto', fontSize: 12, whiteSpace: 'pre-wrap' }}
            >
              {result.body || '(empty body)'}
            </pre>
          </Card>
        ) : null}
      </Col>
    </Row>
  );
}

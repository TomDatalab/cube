import { useEffect, useState } from 'react';
import { ReloadOutlined, TableOutlined } from '@ant-design/icons';
import {
  Button, Descriptions, Select, Space, Table, Typography,
} from 'antd';

import { Alert } from '../../atoms';
import { shortName } from './cubeEdit';
import {
  PREVIEW_LIMITS, PreviewQuery, previewQueries, Row,
} from './preview';
import type { TreeCube } from './tree';

export type CubePreviewProps = {
  cube: TreeCube;
  // Runs a query with the caller's access; null when the preview cannot run.
  runQuery: ((query: PreviewQuery) => Promise<Row[]>) | null;
  // Why the preview cannot run, when it cannot.
  block: string | null;
  // Whether members query users do not see are previewed too.
  hidden: boolean;
};

type Part<T> = { value: T | null; error: string | null };
const EMPTY = { value: null, error: null };

// A value as the API sends it: null shown as such, the rest as text.
function cell(value: unknown) {
  if (value === null || value === undefined) {
    return <Typography.Text type="secondary" italic>null</Typography.Text>;
  }
  return <span style={{ whiteSpace: 'nowrap' }}>{String(value)}</span>;
}

/**
 * The first rows of a cube (every dimension, ungrouped) and its measures
 * over all the data. Loaded on demand: opening a cube runs no query.
 */
export function CubePreview({
  cube, runQuery, block, hidden,
}: CubePreviewProps) {
  const [limit, setLimit] = useState<number>(PREVIEW_LIMITS[0]);
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [rows, setRows] = useState<Part<Row[]>>(EMPTY);
  const [totals, setTotals] = useState<Part<Row>>(EMPTY);

  // Another cube: the preview starts over.
  useEffect(() => {
    setLoaded(false);
    setRows(EMPTY);
    setTotals(EMPTY);
  }, [cube.name]);

  const { rows: rowsQuery, totals: totalsQuery } = previewQueries(cube, { limit, hidden });

  const load = async (rowLimit = limit) => {
    if (!runQuery) {
      return;
    }
    const queries = previewQueries(cube, { limit: rowLimit, hidden });
    setLoading(true);
    const settle = async <T,>(query: PreviewQuery | null, pick: (r: Row[]) => T): Promise<Part<T>> => {
      if (!query) {
        return EMPTY;
      }
      try {
        return { value: pick(await runQuery(query)), error: null };
      } catch (e: any) {
        return { value: null, error: e?.message || String(e) };
      }
    };
    const [r, t] = await Promise.all([
      settle(queries.rows, (data) => data),
      settle(queries.totals, (data) => data[0] ?? {}),
    ]);
    setRows(r);
    setTotals(t);
    setLoaded(true);
    setLoading(false);
  };

  const columns = (rowsQuery?.dimensions as string[] | undefined ?? []).map((name) => ({
    title: <span style={{ whiteSpace: 'nowrap' }}>{shortName(name)}</span>,
    dataIndex: name,
    key: name,
    render: cell,
  }));

  return (
    <div style={{ margin: '24px 0 8px' }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 8 }}>
        <Typography.Title level={5} style={{ margin: 0 }}>Preview</Typography.Title>
        <Space size={8}>
          <Select
            size="small"
            aria-label="Rows"
            value={limit}
            style={{ width: 110 }}
            onChange={(value: number) => {
              setLimit(value);
              if (loaded) {
                load(value);
              }
            }}
            options={PREVIEW_LIMITS.map((n) => ({ value: n, label: `${n} rows` }))}
          />
          <Button
            size="small"
            icon={loaded ? <ReloadOutlined /> : <TableOutlined />}
            loading={loading}
            disabled={!runQuery || (!rowsQuery && !totalsQuery)}
            onClick={() => load()}
          >
            {loaded ? 'Refresh' : 'Load preview'}
          </Button>
        </Space>
      </div>

      {block && <Typography.Paragraph type="secondary">{block}</Typography.Paragraph>}

      {loaded && totalsQuery && (
        totals.error
          ? <Alert type="error" message="Could not load the measures" description={totals.error} style={{ marginBottom: 12 }} />
          : (
            <Descriptions size="small" bordered column={{ xs: 1, sm: 2, md: 3 }} style={{ marginBottom: 12 }}>
              {(totalsQuery.measures as string[]).map((name) => (
                <Descriptions.Item key={name} label={shortName(name)}>{cell(totals.value?.[name])}</Descriptions.Item>
              ))}
            </Descriptions>
          )
      )}

      {loaded && rowsQuery && (
        rows.error
          ? <Alert type="error" message="Could not load the rows" description={rows.error} />
          : (
            // Scrolled by its box, not by the table: columns keep their
            // natural width (a fixed header would fix them and cut titles).
            <div style={{ maxHeight: 420, overflow: 'auto', border: '1px solid #f0f0f0', borderRadius: 2 }}>
              <Table
                size="small"
                rowKey={(_, i) => String(i)}
                pagination={false}
                dataSource={rows.value ?? []}
                columns={columns}
                locale={{ emptyText: 'No rows' }}
              />
            </div>
          )
      )}
    </div>
  );
}

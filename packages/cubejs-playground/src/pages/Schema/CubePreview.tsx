import { CSSProperties, useEffect, useState } from 'react';
import { ReloadOutlined, TableOutlined } from '@ant-design/icons';
import {
  Button, Select, Space, Spin, Tooltip, Typography,
} from 'antd';

import { Alert } from '../../atoms';
import { shortName } from './cubeEdit';
import {
  formatValue, isNumeric, PREVIEW_LIMITS, PreviewQuery, previewQueries, Row,
} from './preview';
import type { TreeCube, TreeMember } from './tree';

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

// Theme tokens (light and dark), with the light value as fallback.
const C = {
  surface: 'var(--pg-surface, #fff)',
  subtle: 'var(--pg-subtle, #f7f8fb)',
  line: 'var(--pg-line, #e4e6ee)',
  lineSoft: 'var(--pg-line-soft, #eef0f5)',
  text: 'var(--pg-text, #1f2433)',
  muted: 'var(--pg-muted, #7a8199)',
};

const MONO = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';

const box: CSSProperties = {
  border: `1px solid ${C.line}`,
  borderRadius: 8,
  background: C.surface,
};

// A value, formatted for its type; a missing one is a muted dash.
function Value({ value, type }: { value: unknown; type?: string }) {
  const text = formatValue(value, type);
  if (text === null) {
    return (
      <Tooltip title="null">
        <span style={{ color: C.muted }}>—</span>
      </Tooltip>
    );
  }
  return <>{text}</>;
}

// The measures over all the data, one tile each.
function Totals({ names, types, values }: {
  names: string[];
  types: Map<string, string | undefined>;
  values: Row;
}) {
  return (
    <div style={{
      display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(160px, 1fr))', gap: 8, marginBottom: 12,
    }}
    >
      {names.map((name) => (
        <div key={name} style={{ ...box, padding: '10px 14px', background: C.subtle }}>
          <div
            title={shortName(name)}
            style={{
              fontSize: 11,
              letterSpacing: 0.4,
              textTransform: 'uppercase',
              color: C.muted,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {shortName(name)}
          </div>
          <div style={{
            fontSize: 22, fontWeight: 600, color: C.text, fontVariantNumeric: 'tabular-nums', marginTop: 2,
          }}
          >
            <Value value={values[name]} type={types.get(name)} />
          </div>
        </div>
      ))}
    </div>
  );
}

// The raw rows: a sticky header naming each dimension and its type, numbers
// right-aligned, alternate rows shaded; scrolled inside its box.
function Rows({ names, types, rows }: {
  names: string[];
  types: Map<string, string | undefined>;
  rows: Row[];
}) {
  const cellStyle = (name: string): CSSProperties => ({
    padding: '6px 12px',
    whiteSpace: 'nowrap',
    textAlign: isNumeric(types.get(name)) ? 'right' : 'left',
    fontVariantNumeric: 'tabular-nums',
    borderBottom: `1px solid ${C.lineSoft}`,
  });
  return (
    <div style={{ ...box, maxHeight: 420, overflow: 'auto' }}>
      <table style={{
        borderCollapse: 'separate', borderSpacing: 0, minWidth: '100%', fontSize: 13, color: C.text,
      }}
      >
        <thead>
          <tr>
            {names.map((name) => (
              <th
                key={name}
                style={{
                  ...cellStyle(name),
                  position: 'sticky',
                  top: 0,
                  zIndex: 1,
                  background: C.subtle,
                  borderBottom: `1px solid ${C.line}`,
                  fontWeight: 600,
                }}
              >
                <div>{shortName(name)}</div>
                <div style={{
                  fontSize: 11, fontWeight: 400, color: C.muted, fontFamily: MONO,
                }}
                >
                  {types.get(name) ?? ''}
                </div>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td colSpan={names.length} style={{ padding: 24, textAlign: 'center', color: C.muted }}>
                No rows
              </td>
            </tr>
          ) : rows.map((row, i) => (
            // Rows have no identity of their own: their place is their key.
            // eslint-disable-next-line react/no-array-index-key
            <tr key={i} style={{ background: i % 2 ? C.subtle : C.surface }}>
              {names.map((name) => (
                <td key={name} style={cellStyle(name)}>
                  <Value value={row[name]} type={types.get(name)} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
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
  const types = new Map<string, string | undefined>(
    [...(cube.dimensions ?? []), ...(cube.measures ?? [])].map((m: TreeMember) => [m.name, m.type])
  );
  const canRun = Boolean(runQuery) && Boolean(rowsQuery || totalsQuery);

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

  const dimensionNames = (rowsQuery?.dimensions as string[] | undefined) ?? [];
  const measureNames = (totalsQuery?.measures as string[] | undefined) ?? [];
  const rowCount = rows.value?.length ?? 0;
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

  let body;
  if (!loaded) {
    body = (
      <div style={{
        border: `1px dashed ${C.line}`,
        borderRadius: 8,
        padding: '28px 16px',
        textAlign: 'center',
        background: C.subtle,
      }}
      >
        <TableOutlined style={{ fontSize: 28, color: C.muted }} />
        <div style={{ margin: '8px 0 12px', color: C.muted }}>
          {block || `See the first rows of ${cube.name} and its measures over all the data.`}
        </div>
        <Button type="primary" icon={<TableOutlined />} loading={loading} disabled={!canRun} onClick={() => load()}>
          Load preview
        </Button>
      </div>
    );
  } else {
    body = (
      // While it reloads, what it showed fades under a spinner.
      <div style={{ position: 'relative' }} aria-busy={loading}>
        {loading && (
          <div style={{
            position: 'absolute', inset: 0, zIndex: 2, display: 'flex', alignItems: 'center', justifyContent: 'center',
          }}
          >
            <Spin />
          </div>
        )}
        <div style={{ opacity: loading ? 0.4 : 1, transition: 'opacity 0.2s' }}>
          {totalsQuery && (totals.error
            ? <Alert type="error" message="Could not load the measures" description={totals.error} style={{ marginBottom: 12 }} />
            : <Totals names={measureNames} types={types} values={totals.value ?? {}} />)}
          {rowsQuery && (rows.error
            ? <Alert type="error" message="Could not load the rows" description={rows.error} />
            : (
              <>
                <Rows names={dimensionNames} types={types} rows={rows.value ?? []} />
                <Typography.Text type="secondary" style={{ display: 'block', marginTop: 6, fontSize: 12 }}>
                  {rowCount === limit ? `First ${plural(rowCount, 'row')}` : `${plural(rowCount, 'row')} (all of them)`}
                  {' · '}
                  {plural(dimensionNames.length, 'dimension')}
                </Typography.Text>
              </>
            ))}
        </div>
      </div>
    );
  }

  return (
    <section style={{ margin: '24px 0 8px' }} aria-label="Preview">
      <div style={{
        display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 10,
      }}
      >
        <Typography.Title level={5} style={{ margin: 0 }}>Preview</Typography.Title>
        {loaded && (
          <Space size={8}>
            <Select
              size="small"
              aria-label="Rows"
              value={limit}
              style={{ width: 110 }}
              onChange={(value: number) => {
                setLimit(value);
                load(value);
              }}
              options={PREVIEW_LIMITS.map((n) => ({ value: n, label: `${n} rows` }))}
            />
            <Button size="small" icon={<ReloadOutlined />} loading={loading} disabled={!canRun} onClick={() => load()}>
              Refresh
            </Button>
          </Space>
        )}
      </div>
      {body}
    </section>
  );
}

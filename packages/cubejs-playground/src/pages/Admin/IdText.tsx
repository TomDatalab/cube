import { Tooltip, Typography } from 'antd';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** How an id is shown: a UUID as its first 8 characters, anything else whole. */
export function shortId(id: string): string {
  return UUID.test(id) ? `${id.slice(0, 8)}…` : id;
}

/**
 * An id in a table: muted and short, so a UUID does not widen the column; the
 * whole id is in the tooltip and is what the copy button copies.
 */
export function IdText({ id }: { id: string }) {
  const short = shortId(id);
  const text = (
    <Typography.Text type="secondary" copyable={{ text: id }} style={{ whiteSpace: 'nowrap' }}>
      {short}
    </Typography.Text>
  );
  return short === id ? text : <Tooltip title={id}>{text}</Tooltip>;
}

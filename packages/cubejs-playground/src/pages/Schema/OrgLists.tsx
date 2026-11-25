// The right panels for an organization's "Tables" and "Files" nodes: what
// the node holds, as a table. A table row checks the table like the tree
// does (for generating cubes); a file row opens the file's panel.

import { Button, Table, Tag, Typography } from 'antd';

import type { OrgDetail, TreeCube } from './tree';

export type TableListRow = {
  // The table's tree key, so checking a row checks the tree node.
  key: string;
  dataSource: string;
  schema: string;
  table: string;
  columns: number;
};

/** Every table of the organization, by data source, schema and name. */
export function tableListRows(slug: string, tables: OrgDetail['tables']): TableListRow[] {
  const rows: TableListRow[] = [];
  Object.entries(tables || {}).forEach(([dataSource, schemas]) => {
    Object.entries(schemas).forEach(([schema, byName]) => {
      Object.entries(byName).forEach(([table, columns]) => {
        rows.push({
          key: `table:${slug}:${dataSource}.${schema}.${table}`,
          dataSource,
          schema,
          table,
          columns: columns.length,
        });
      });
    });
  });
  return rows.sort((a, b) => a.key.localeCompare(b.key));
}

export type FileListRow = {
  key: string;
  fileName: string;
  // The name without the organization's folder, as the tree shows it.
  shown: string;
  cubes: string[];
  version?: number;
  readOnly: boolean;
  errors: number;
};

/** Every file of the organization by name: its cubes and views, its problems. */
export function fileListRows(
  slug: string,
  files: OrgDetail['files'],
  cubes: TreeCube[],
  errors: OrgDetail['errors']
): FileListRow[] {
  return [...files]
    .sort((a, b) => a.fileName.localeCompare(b.fileName))
    .map((f) => ({
      key: `file:${slug}:${f.fileName}`,
      fileName: f.fileName,
      shown: f.fileName.startsWith(`${slug}/`) ? f.fileName.slice(slug.length + 1) : f.fileName,
      cubes: cubes.filter((c) => c.file === f.fileName).map((c) => c.name),
      version: f.version,
      readOnly: f.editable === false,
      errors: errors.filter((e) => e.file === f.fileName).length,
    }));
}

const pagination = (count: number) => (count > 50 ? { pageSize: 50, showSizeChanger: false } : false as const);

export function TableList({ orgName, rows, checkedKeys, onCheck }: {
  orgName: string;
  rows: TableListRow[];
  checkedKeys: string[];
  onCheck: (keys: string[]) => void;
}) {
  return (
    <div data-testid="table-list">
      <Typography.Title level={4}>{`Tables of ${orgName} (${rows.length})`}</Typography.Title>
      <Typography.Paragraph type="secondary">
        Check tables to generate cubes from them, here or in the tree.
      </Typography.Paragraph>
      <Table<TableListRow>
        size="small"
        rowKey="key"
        pagination={pagination(rows.length)}
        dataSource={rows}
        locale={{ emptyText: 'No tables: release a connection to this organization' }}
        rowSelection={{
          selectedRowKeys: checkedKeys.filter((k) => rows.some((r) => r.key === k)),
          onChange: (keys) => onCheck(keys.map(String)),
        }}
        columns={[
          { title: 'Data source', dataIndex: 'dataSource', key: 'dataSource' },
          { title: 'Schema', dataIndex: 'schema', key: 'schema' },
          { title: 'Table', dataIndex: 'table', key: 'table' },
          { title: 'Columns', dataIndex: 'columns', key: 'columns', width: 90, align: 'right' },
        ]}
      />
    </div>
  );
}

export function FileList({ orgName, rows, onOpen }: {
  orgName: string;
  rows: FileListRow[];
  onOpen: (key: string) => void;
}) {
  return (
    <div data-testid="file-list">
      <Typography.Title level={4}>{`Files of ${orgName} (${rows.length})`}</Typography.Title>
      <Table<FileListRow>
        size="small"
        rowKey="key"
        pagination={pagination(rows.length)}
        dataSource={rows}
        locale={{ emptyText: 'No files yet: select tables to generate cubes' }}
        columns={[
          {
            title: 'File',
            key: 'file',
            render: (_: unknown, row) => (
              <>
                <Button type="link" size="small" style={{ padding: 0 }} onClick={() => onOpen(row.key)}>
                  {row.shown}
                </Button>
                {row.readOnly && <Tag style={{ marginLeft: 8 }}>read-only</Tag>}
                {row.errors > 0 && <Tag color="red" style={{ marginLeft: 8 }}>{`${row.errors} errors`}</Tag>}
              </>
            ),
          },
          {
            title: 'Cubes',
            key: 'cubes',
            render: (_: unknown, row) => row.cubes.join(', ') || <Typography.Text type="secondary">none</Typography.Text>,
          },
          { title: 'Version', dataIndex: 'version', key: 'version', width: 80, align: 'right' },
        ]}
      />
    </div>
  );
}

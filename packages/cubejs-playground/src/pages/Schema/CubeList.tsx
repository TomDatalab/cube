// The right panel for an organization's "Cubes" or "Views" node: its cubes
// (or views), each opening its own panel.

import { Button, Table, Tag, Typography } from 'antd';

import type { OrgDetail, TreeCube } from './tree';

export type CubeListRow = {
  key: string;
  name: string;
  title?: string;
  type: TreeCube['type'];
  dataSource: string;
  measures: number;
  dimensions: number;
  file: string;
  // A view's source cubes.
  includes: string[];
  errors: number;
};

/** The cubes (or the views) by name, with how many problems name each. */
export function cubeListRows(cubes: TreeCube[], errors: OrgDetail['errors'], type: TreeCube['type']): CubeListRow[] {
  return cubes
    .filter((c) => c.type === type)
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((c) => ({
      key: c.name,
      name: c.name,
      title: c.title,
      type: c.type,
      dataSource: c.dataSource,
      measures: c.members.measures,
      dimensions: c.members.dimensions,
      file: c.file,
      includes: c.includes ?? [],
      errors: errors.filter((e) => e.cube === c.name || (!e.cube && e.file === c.file)).length,
    }));
}

export function CubeList({ orgName, cubes, errors, type, onOpen }: {
  orgName: string;
  cubes: TreeCube[];
  errors: OrgDetail['errors'];
  type: TreeCube['type'];
  onOpen: (name: string) => void;
}) {
  const rows = cubeListRows(cubes, errors, type);
  const views = type === 'view';
  return (
    <div data-testid={views ? 'view-list' : 'cube-list'}>
      <Typography.Title level={4}>{`${views ? 'Views' : 'Cubes'} of ${orgName} (${rows.length})`}</Typography.Title>
      <Table<CubeListRow>
        size="small"
        rowKey="key"
        pagination={rows.length > 50 ? { pageSize: 50, showSizeChanger: false } : false}
        dataSource={rows}
        locale={{
          emptyText: views ? 'No views: define them in a file' : 'No cubes yet: select tables to generate them',
        }}
        columns={[
          {
            title: 'Name',
            key: 'name',
            render: (_: unknown, row) => (
              <>
                <Button type="link" size="small" style={{ padding: 0 }} onClick={() => onOpen(row.name)}>
                  {row.title || row.name}
                </Button>
                {row.title && row.title !== row.name && (
                  <Typography.Text type="secondary" style={{ marginLeft: 8 }}>{row.name}</Typography.Text>
                )}
                {row.errors > 0 && <Tag color="red" style={{ marginLeft: 8 }}>{`${row.errors} errors`}</Tag>}
              </>
            ),
          },
          views
            ? {
              title: 'Cubes',
              key: 'includes',
              render: (_: unknown, row) => row.includes.join(', '),
            }
            : { title: 'Data source', dataIndex: 'dataSource', key: 'dataSource' },
          { title: 'Measures', dataIndex: 'measures', key: 'measures', width: 90, align: 'right' },
          { title: 'Dimensions', dataIndex: 'dimensions', key: 'dimensions', width: 100, align: 'right' },
          { title: 'File', dataIndex: 'file', key: 'file', ellipsis: true },
        ]}
      />
    </div>
  );
}

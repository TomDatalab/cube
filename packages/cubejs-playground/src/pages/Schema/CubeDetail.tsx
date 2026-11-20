import { Button, Descriptions, Table, Typography } from 'antd';

import { Alert } from '../../atoms';
import type { TreeCube } from './tree';

export type MetaMember = {
  name: string;
  title?: string;
  shortTitle?: string;
  type?: string;
  primaryKey?: boolean;
};

// A cube as `/cube/v1/meta` describes it. `joins` is only there when the server
// sends it (the extended meta), the standard response leaves it out.
export type MetaCube = {
  name: string;
  title?: string;
  description?: string;
  measures: MetaMember[];
  dimensions: MetaMember[];
  segments: MetaMember[];
  joins?: { name?: string; relationship?: string; sql?: string }[];
};

export type CubeDetailProps = {
  cube: TreeCube;
  // undefined when the cube is not in the meta (rejected, or meta not loaded).
  metaCube?: MetaCube;
  metaLoading?: boolean;
  errors: { file: string; cube: string; message: string }[];
  onOpen: (cube: MetaCube) => void;
};

const memberColumns = (withPrimaryKey: boolean) => [
  { title: 'Name', dataIndex: 'name', key: 'name' },
  { title: 'Type', dataIndex: 'type', key: 'type' },
  ...(withPrimaryKey
    ? [{
      title: 'PK',
      dataIndex: 'primaryKey',
      key: 'primaryKey',
      render: (value?: boolean) => (value ? 'yes' : ''),
    }]
    : [{ title: 'Title', dataIndex: 'title', key: 'title' }]),
];

export function CubeDetail({
  cube, metaCube, metaLoading, errors, onOpen,
}: CubeDetailProps) {
  const rejections = errors.filter((e) => e.cube === cube.name || e.file === cube.file);

  return (
    <div data-testid="cube-detail">
      <Typography.Title level={4}>{metaCube?.title || cube.title || cube.name}</Typography.Title>
      {metaCube?.description && <Typography.Paragraph>{metaCube.description}</Typography.Paragraph>}

      <Descriptions size="small" column={1} style={{ marginBottom: 16 }}>
        <Descriptions.Item label="Data source">{cube.dataSource}</Descriptions.Item>
        <Descriptions.Item label="File">{cube.file}</Descriptions.Item>
      </Descriptions>

      {!metaCube && !metaLoading && (
        <Alert
          type="error"
          message="Cube not available"
          description={rejections.length
            ? rejections.map((e) => <div key={`${e.file}:${e.message}`}>{e.message}</div>)
            : 'This cube is not in the API meta.'}
          style={{ marginBottom: 16 }}
        />
      )}

      <Button
        type="primary"
        disabled={!metaCube}
        onClick={() => metaCube && onOpen(metaCube)}
        style={{ marginBottom: 24 }}
      >
        Abrir no Playground
      </Button>

      {metaCube && (
        <>
          <Typography.Title level={5}>Measures ({metaCube.measures.length})</Typography.Title>
          <Table
            size="small"
            rowKey="name"
            pagination={false}
            dataSource={metaCube.measures}
            columns={memberColumns(false)}
          />

          <Typography.Title level={5} style={{ marginTop: 24 }}>
            Dimensions ({metaCube.dimensions.length})
          </Typography.Title>
          <Table
            size="small"
            rowKey="name"
            pagination={false}
            dataSource={metaCube.dimensions}
            columns={memberColumns(true)}
          />

          <Typography.Title level={5} style={{ marginTop: 24 }}>
            Segments ({metaCube.segments.length})
          </Typography.Title>
          <Table
            size="small"
            rowKey="name"
            pagination={false}
            dataSource={metaCube.segments}
            columns={[
              { title: 'Name', dataIndex: 'name', key: 'name' },
              { title: 'Title', dataIndex: 'title', key: 'title' },
            ]}
          />

          {metaCube.joins && metaCube.joins.length > 0 && (
            <>
              <Typography.Title level={5} style={{ marginTop: 24 }}>
                Joins ({metaCube.joins.length})
              </Typography.Title>
              <Table
                size="small"
                rowKey={(join) => join.name || join.sql || ''}
                pagination={false}
                dataSource={metaCube.joins}
                columns={[
                  { title: 'Name', dataIndex: 'name', key: 'name' },
                  { title: 'Relationship', dataIndex: 'relationship', key: 'relationship' },
                  { title: 'SQL', dataIndex: 'sql', key: 'sql' },
                ]}
              />
            </>
          )}
        </>
      )}
    </div>
  );
}

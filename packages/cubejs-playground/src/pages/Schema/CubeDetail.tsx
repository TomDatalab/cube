import { Button, Descriptions, Table, Tag, Tooltip, Typography } from 'antd';

import { Alert } from '../../atoms';
import type { OpenBlock } from './api';
import type { TreeCube, TreeMember } from './tree';

export type CubeDetailProps = {
  cube: TreeCube;
  errors: { file: string; cube: string; message: string }[];
  open: OpenBlock;
  onOpen: () => void;
};

// A member the query users do not see carries a tag.
const nameColumn = {
  title: 'Name',
  dataIndex: 'name',
  key: 'name',
  render: (name: string, member: TreeMember) => (
    <>
      {name}
      {member.public === false && (
        <Tooltip title="Os usuários de consulta não veem este membro">
          <Tag style={{ marginLeft: 8 }}>oculto</Tag>
        </Tooltip>
      )}
    </>
  ),
};

const memberColumns = (withPrimaryKey: boolean) => [
  nameColumn,
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
  cube, errors, open, onOpen,
}: CubeDetailProps) {
  const rejections = errors.filter((e) => e.cube === cube.name || e.file === cube.file);
  const measures = cube.measures ?? [];
  const dimensions = cube.dimensions ?? [];
  const segments = cube.segments ?? [];

  return (
    <div data-testid="cube-detail">
      <Typography.Title level={4}>{cube.title || cube.name}</Typography.Title>

      <Descriptions size="small" column={1} style={{ marginBottom: 16 }}>
        <Descriptions.Item label="Data source">{cube.dataSource}</Descriptions.Item>
        <Descriptions.Item label="File">{cube.file}</Descriptions.Item>
      </Descriptions>

      {rejections.length > 0 && (
        <Alert
          type="error"
          message="Cube not available"
          description={rejections.map((e) => <div key={`${e.file}:${e.message}`}>{e.message}</div>)}
          style={{ marginBottom: 16 }}
        />
      )}

      {open.warning && (
        <Alert type="warning" message={open.warning} style={{ marginBottom: 16 }} />
      )}

      <Tooltip title={open.disabled ? open.hint : ''}>
        <span>
          <Button
            type="primary"
            disabled={open.disabled}
            onClick={onOpen}
            style={{ marginBottom: 24 }}
          >
            Abrir no Playground
          </Button>
        </span>
      </Tooltip>

      {cube.joins && cube.joins.length > 0 && (
        <>
          <Typography.Title level={5}>Joins ({cube.joins.length})</Typography.Title>
          <Table
            size="small"
            rowKey="name"
            pagination={false}
            dataSource={cube.joins}
            columns={[
              { title: 'Cube', dataIndex: 'name', key: 'name' },
              { title: 'Relationship', dataIndex: 'relationship', key: 'relationship' },
            ]}
            style={{ marginBottom: 24 }}
          />
        </>
      )}

      {cube.includes && cube.includes.length > 0 && (
        <>
          <Typography.Title level={5}>Includes ({cube.includes.length})</Typography.Title>
          <Typography.Paragraph>{cube.includes.join(', ')}</Typography.Paragraph>
        </>
      )}

      <Typography.Title level={5}>Measures ({measures.length})</Typography.Title>
      <Table
        size="small"
        rowKey="name"
        pagination={false}
        dataSource={measures}
        columns={memberColumns(false)}
      />

      <Typography.Title level={5} style={{ marginTop: 24 }}>
        Dimensions ({dimensions.length})
      </Typography.Title>
      <Table
        size="small"
        rowKey="name"
        pagination={false}
        dataSource={dimensions}
        columns={memberColumns(true)}
      />

      <Typography.Title level={5} style={{ marginTop: 24 }}>
        Segments ({segments.length})
      </Typography.Title>
      <Table
        size="small"
        rowKey="name"
        pagination={false}
        dataSource={segments}
        columns={[nameColumn, { title: 'Title', dataIndex: 'title', key: 'title' }]}
      />
    </div>
  );
}

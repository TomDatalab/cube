import { ReactNode, useState } from 'react';
import { EditOutlined, PlusOutlined } from '@ant-design/icons';
import {
  Button, Descriptions, Form, Input, Modal, Select, Space, Table, Tag, Tooltip, Typography, message,
} from 'antd';

import { Alert } from '../../atoms';
import type { OpenBlock } from './api';
import { editFailure, shortName } from './cubeEdit';
import {
  addMeasure,
  DIMENSION_TYPES,
  DimensionType,
  MEASURE_TYPES,
  MeasureType,
  measureNeedsSql,
  renameDimension,
  setDimensionType,
  setMeasureType,
} from './cubeYaml';
import type { TreeCube, TreeMember } from './tree';

/** A change to the cube's file: its text in, the new text out. */
export type CubeChange = (text: string) => string;

export type CubeDetailProps = {
  cube: TreeCube;
  errors: { file: string; cube: string; message: string }[];
  open: OpenBlock;
  onOpen: () => void;
  // Why the members cannot be edited here; null when they can.
  editBlock?: string | null;
  // Applies a change to the cube's file and saves it; rejects on failure.
  onEdit?: (change: CubeChange) => Promise<void>;
};

type Dialog = { kind: 'measure'; member: TreeMember }
  | { kind: 'new-measure' }
  | { kind: 'dimension'; member: TreeMember };

type Fields = { name: string; type: string; sql: string; title: string };

// The change a dialog's fields make; the edit throws what the file refuses.
function changeOf(cube: string, dialog: Dialog, f: Fields): CubeChange {
  if (dialog.kind === 'measure') {
    const name = shortName(dialog.member.name);
    return (text) => setMeasureType(text, cube, name, f.type as MeasureType, f.sql || undefined);
  }
  if (dialog.kind === 'new-measure') {
    return (text) => addMeasure(text, cube, {
      name: f.name.trim(), type: f.type as MeasureType, sql: f.sql, title: f.title,
    });
  }
  const from = shortName(dialog.member.name);
  const to = f.name.trim();
  return (text) => {
    let next = to && to !== from ? renameDimension(text, cube, from, to) : text;
    if (f.type !== dialog.member.type) {
      next = setDimensionType(next, cube, to || from, f.type as DimensionType);
    }
    return next;
  };
}

function dialogTitle(cube: string, dialog: Dialog): string {
  if (dialog.kind === 'new-measure') {
    return `Add measure to ${cube}`;
  }
  const kind = dialog.kind === 'measure' ? 'measure' : 'dimension';
  return `Edit ${kind} ${shortName(dialog.member.name)}`;
}

function MemberDialog({
  cube, dialog, onEdit, onClose,
}: {
  cube: string;
  dialog: Dialog;
  onEdit: (change: CubeChange) => Promise<void>;
  onClose: () => void;
}) {
  const member = dialog.kind === 'new-measure' ? null : dialog.member;
  const [fields, setFields] = useState<Fields>({
    name: member ? shortName(member.name) : '',
    type: member?.type ?? 'count',
    sql: '',
    title: '',
  });
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<{ message: string; problems: string[] } | null>(null);
  const set = (key: keyof Fields) => (value: string) => setFields((f) => ({ ...f, [key]: value }));

  const isMeasure = dialog.kind !== 'dimension';
  const geo = dialog.kind === 'dimension' && member?.type === 'geo';
  const askSql = isMeasure && measureNeedsSql(fields.type as MeasureType);

  const save = async () => {
    setSaving(true);
    setFailure(null);
    try {
      await onEdit(changeOf(cube, dialog, fields));
      message.success(`${cube} saved`);
      onClose();
    } catch (e) {
      setFailure(editFailure(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      visible
      title={dialogTitle(cube, dialog)}
      okText="Save"
      cancelText="Cancel"
      confirmLoading={saving}
      onOk={save}
      onCancel={onClose}
      destroyOnClose
    >
      <Form layout="vertical" requiredMark={false} onFinish={save}>
        {dialog.kind !== 'measure' && (
          <Form.Item
            label="Name"
            extra={dialog.kind === 'dimension'
              ? 'References to it in this cube are renamed too. Other cubes, views and saved queries that use the old name must be updated.'
              : undefined}
          >
            <Input aria-label="Name" value={fields.name} onChange={(e) => set('name')(e.target.value)} />
          </Form.Item>
        )}
        <Form.Item
          label="Type"
          extra={geo ? 'A geo dimension takes latitude and longitude: change its type in the file.' : undefined}
        >
          <Select
            aria-label="Type"
            value={fields.type}
            disabled={geo}
            onChange={set('type')}
            options={(isMeasure ? MEASURE_TYPES : DIMENSION_TYPES).map((t) => ({ value: t, label: t }))}
          />
        </Form.Item>
        {askSql && (
          <Form.Item
            label="SQL"
            extra={dialog.kind === 'measure'
              ? 'Only needed when the measure has no SQL yet.'
              : 'The column or expression to aggregate, e.g. {CUBE}.amount.'}
          >
            <Input
              aria-label="SQL"
              placeholder="{CUBE}.column"
              value={fields.sql}
              onChange={(e) => set('sql')(e.target.value)}
            />
          </Form.Item>
        )}
        {dialog.kind === 'new-measure' && (
          <Form.Item label="Title" extra="Optional.">
            <Input aria-label="Title" value={fields.title} onChange={(e) => set('title')(e.target.value)} />
          </Form.Item>
        )}
      </Form>
      {failure && (
        <Alert
          type="error"
          message={failure.message}
          description={failure.problems.length ? failure.problems.map((p) => <div key={p}>{p}</div>) : undefined}
        />
      )}
    </Modal>
  );
}

// A member the query users do not see carries a tag.
const nameColumn = {
  title: 'Name',
  dataIndex: 'name',
  key: 'name',
  render: (name: string, member: TreeMember) => (
    <>
      {name}
      {member.public === false && (
        <Tooltip title="Query users do not see this member">
          <Tag style={{ marginLeft: 8 }}>hidden</Tag>
        </Tooltip>
      )}
    </>
  ),
};

// The row's edit button: a pencil at the end of the row.
const editColumn = (edit: ((member: TreeMember) => void) | null) => (edit
  ? [{
    title: '',
    key: 'edit',
    width: 48,
    align: 'right' as const,
    render: (_: unknown, member: TreeMember) => (
      <Tooltip title="Edit">
        <Button
          type="text"
          size="small"
          icon={<EditOutlined />}
          aria-label={`Edit ${shortName(member.name)}`}
          onClick={() => edit(member)}
        />
      </Tooltip>
    ),
  }]
  : []);

const memberColumns = (withPrimaryKey: boolean) => [
  nameColumn,
  { title: 'Type', dataIndex: 'type', key: 'type', width: 140 },
  ...(withPrimaryKey
    ? [{
      title: 'PK',
      dataIndex: 'primaryKey',
      key: 'primaryKey',
      width: 60,
      render: (value?: boolean) => (value ? 'yes' : ''),
    }]
    : [{ title: 'Title', dataIndex: 'title', key: 'title' }]),
];

// A section's heading: its name and count, and its action on the right.
function SectionHeader({ title, count, action }: { title: string; count: number; action?: ReactNode }) {
  return (
    <div style={{
      display: 'flex', alignItems: 'center', justifyContent: 'space-between', margin: '24px 0 8px',
    }}
    >
      <Space size={8} align="center">
        <Typography.Title level={5} style={{ margin: 0 }}>{title}</Typography.Title>
        <Tag style={{ margin: 0 }}>{count}</Tag>
      </Space>
      {action}
    </div>
  );
}

export function CubeDetail({
  cube, errors, open, onOpen, editBlock = null, onEdit,
}: CubeDetailProps) {
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const canEdit = Boolean(onEdit) && !editBlock;
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
          <Button type="primary" disabled={open.disabled} onClick={onOpen}>
            Open in Playground
          </Button>
        </span>
      </Tooltip>

      {onEdit && editBlock && cube.type === 'cube' && (
        <Typography.Paragraph type="secondary" style={{ marginTop: 16, marginBottom: 0 }}>
          {editBlock}
        </Typography.Paragraph>
      )}

      {cube.joins && cube.joins.length > 0 && (
        <>
          <SectionHeader title="Joins" count={cube.joins.length} />
          <Table
            size="small"
            rowKey="name"
            pagination={false}
            dataSource={cube.joins}
            columns={[
              { title: 'Cube', dataIndex: 'name', key: 'name' },
              { title: 'Relationship', dataIndex: 'relationship', key: 'relationship' },
            ]}
          />
        </>
      )}

      {cube.includes && cube.includes.length > 0 && (
        <>
          <SectionHeader title="Includes" count={cube.includes.length} />
          <Typography.Paragraph>{cube.includes.join(', ')}</Typography.Paragraph>
        </>
      )}

      <SectionHeader
        title="Measures"
        count={measures.length}
        action={canEdit ? (
          <Button size="small" icon={<PlusOutlined />} onClick={() => setDialog({ kind: 'new-measure' })}>
            Add measure
          </Button>
        ) : undefined}
      />
      <Table
        size="small"
        rowKey="name"
        pagination={false}
        dataSource={measures}
        columns={[
          ...memberColumns(false),
          ...editColumn(canEdit ? (member) => setDialog({ kind: 'measure', member }) : null),
        ]}
      />

      <SectionHeader title="Dimensions" count={dimensions.length} />
      <Table
        size="small"
        rowKey="name"
        pagination={false}
        dataSource={dimensions}
        columns={[
          ...memberColumns(true),
          ...editColumn(canEdit ? (member) => setDialog({ kind: 'dimension', member }) : null),
        ]}
      />

      <SectionHeader title="Segments" count={segments.length} />
      <Table
        size="small"
        rowKey="name"
        pagination={false}
        dataSource={segments}
        columns={[nameColumn, { title: 'Title', dataIndex: 'title', key: 'title' }]}
      />

      {dialog && onEdit && (
        <MemberDialog cube={cube.name} dialog={dialog} onEdit={onEdit} onClose={() => setDialog(null)} />
      )}
    </div>
  );
}

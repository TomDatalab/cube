import { useState } from 'react';
import { Button, Descriptions, Input, Modal, Select, Table, Tag, Tooltip, Typography, message } from 'antd';

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

// The change a dialog's fields make, or an error thrown by the edit.
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
  let title = `Nova measure em ${cube}`;
  if (dialog.kind === 'measure') {
    title = `Measure ${fields.name}`;
  } else if (dialog.kind === 'dimension') {
    title = `Dimension ${member ? shortName(member.name) : ''}`;
  }

  const save = async () => {
    setSaving(true);
    setFailure(null);
    try {
      await onEdit(changeOf(cube, dialog, fields));
      message.success('Cubo salvo');
      onClose();
    } catch (e) {
      setFailure(editFailure(e));
    } finally {
      setSaving(false);
    }
  };

  const label = (text: string) => <Typography.Text strong style={{ display: 'block', margin: '12px 0 4px' }}>{text}</Typography.Text>;

  return (
    <Modal
      visible
      title={title}
      okText="Salvar"
      cancelText="Cancelar"
      confirmLoading={saving}
      onOk={save}
      onCancel={onClose}
      destroyOnClose
    >
      {dialog.kind !== 'measure' && (
        <>
          {label('Nome')}
          <Input aria-label="Nome" value={fields.name} onChange={(e) => set('name')(e.target.value)} />
          {dialog.kind === 'dimension' && (
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              As referências a ela neste cubo são renomeadas junto. Outros cubos, views e consultas salvas
              que usam o nome antigo precisam ser ajustados.
            </Typography.Text>
          )}
        </>
      )}
      {label('Tipo')}
      <Select
        aria-label="Tipo"
        style={{ width: '100%' }}
        value={fields.type}
        disabled={geo}
        onChange={set('type')}
        options={(isMeasure ? MEASURE_TYPES : DIMENSION_TYPES).map((t) => ({ value: t, label: t }))}
      />
      {geo && (
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          Uma dimension geo tem latitude e longitude: o tipo dela muda no arquivo.
        </Typography.Text>
      )}
      {askSql && (
        <>
          {label(dialog.kind === 'measure' ? 'SQL (se a measure ainda não tiver)' : 'SQL')}
          <Input
            aria-label="SQL"
            placeholder="{CUBE}.coluna"
            value={fields.sql}
            onChange={(e) => set('sql')(e.target.value)}
          />
        </>
      )}
      {dialog.kind === 'new-measure' && (
        <>
          {label('Título (opcional)')}
          <Input aria-label="Título" value={fields.title} onChange={(e) => set('title')(e.target.value)} />
        </>
      )}
      {failure && (
        <Alert
          type="error"
          message={failure.message}
          description={failure.problems.length ? failure.problems.map((p) => <div key={p}>{p}</div>) : undefined}
          style={{ marginTop: 16 }}
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
        <Tooltip title="Os usuários de consulta não veem este membro">
          <Tag style={{ marginLeft: 8 }}>oculto</Tag>
        </Tooltip>
      )}
    </>
  ),
};

const editColumn = (edit: ((member: TreeMember) => void) | null) => (edit
  ? [{
    title: '',
    key: 'edit',
    width: 80,
    render: (_: unknown, member: TreeMember) => (
      <Button size="small" type="link" onClick={() => edit(member)}>Editar</Button>
    ),
  }]
  : []);

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

      {onEdit && editBlock && cube.type === 'cube' && (
        <Typography.Paragraph type="secondary">{editBlock}</Typography.Paragraph>
      )}

      <Typography.Title level={5}>
        Measures ({measures.length})
        {canEdit && (
          <Button size="small" style={{ marginLeft: 12 }} onClick={() => setDialog({ kind: 'new-measure' })}>
            Nova measure
          </Button>
        )}
      </Typography.Title>
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

      <Typography.Title level={5} style={{ marginTop: 24 }}>
        Dimensions ({dimensions.length})
      </Typography.Title>
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

      {dialog && onEdit && (
        <MemberDialog cube={cube.name} dialog={dialog} onEdit={onEdit} onClose={() => setDialog(null)} />
      )}
    </div>
  );
}

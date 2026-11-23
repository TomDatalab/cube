import { useEffect, useReducer } from 'react';
import { Button, Input, Modal, Select, Space, Typography, message } from 'antd';

import PrismCode from '../../PrismCode';
import { Alert } from '../../atoms';
import { FileContent, OrgNode, PLATFORM, modelApi } from './api';
import { initialEditor, problemLines, readOnlyReason, reduce } from './editor';
import type { OrgDetail, OrgSummary } from './tree';

type Props = {
  api: ReturnType<typeof modelApi>;
  org: OrgNode;
  // The file's id (its path without RBAC).
  id: string;
  file: FileContent;
  isAdmin: boolean;
  // The organizations an admin can move the file to.
  orgs: OrgSummary[];
  onSaved: (detail: OrgDetail) => void;
  onRemoved: () => void;
  onMoved: (organizationId: string | null) => void;
  onReload: () => void;
};

const MONO = { fontFamily: 'monospace' } as const;

export function FileDetail({
  api, org, id, file, isAdmin, orgs, onSaved, onRemoved, onMoved, onReload,
}: Props) {
  const [state, dispatch] = useReducer(reduce, initialEditor(file.version ?? 0));

  // Another file, or a fresh copy of this one: start over.
  useEffect(() => {
    dispatch({ type: 'saved', version: file.version ?? 0, content: file.content });
  }, [id, file.version]);

  const reason = file.editable === false ? readOnlyReason(file) : null;
  const editing = state.mode !== 'view';
  const path = file.path ?? id;

  const save = async () => {
    dispatch({ type: 'save' });
    try {
      const detail = await api.save(org, id, state.draft, state.base);
      const version = detail.files.find((f) => f.id === id)?.version ?? state.base + 1;
      dispatch({ type: 'saved', version, content: state.draft });
      onSaved(detail);
    } catch (e: any) {
      dispatch({
        type: 'failed',
        status: e.status ?? 0,
        body: { error: e.message, problems: e.problems },
      });
    }
  };

  const reload = () => {
    Modal.confirm({
      title: 'Recarregar o arquivo?',
      content: 'Suas alterações não salvas serão descartadas.',
      okText: 'Recarregar',
      cancelText: 'Cancelar',
      onOk: () => {
        dispatch({ type: 'cancel' });
        onReload();
      },
    });
  };

  const remove = () => {
    Modal.confirm({
      title: `Apagar ${path}?`,
      content: 'O arquivo será removido do modelo.',
      okText: 'Apagar',
      okButtonProps: { danger: true },
      cancelText: 'Cancelar',
      onOk: async () => {
        try {
          await api.remove(org, id, state.base);
          onRemoved();
        } catch (e: any) {
          message.error(e.message);
        }
      },
    });
  };

  const move = () => {
    let chosen: string | null | undefined;
    Modal.confirm({
      title: `Mover ${path} para…`,
      content: (
        <Select
          style={{ width: '100%' }}
          placeholder="Escolha o destino"
          options={[
            ...(org.slug === PLATFORM ? [] : [{ value: '', label: 'Plataforma' }]),
            ...orgs.filter((o) => o.id && o.slug !== org.slug).map((o) => ({ value: o.id!, label: o.name })),
          ]}
          onChange={(value: string) => {
            chosen = value || null;
          }}
        />
      ),
      okText: 'Mover',
      cancelText: 'Cancelar',
      onOk: async () => {
        if (chosen === undefined) {
          message.warning('Escolha o destino');
          throw new Error('no target');
        }
        try {
          await api.move(id, chosen);
          onMoved(chosen);
        } catch (e: any) {
          message.error(e.message);
          throw e;
        }
      },
    });
  };

  return (
    <>
      {reason && <Alert type="info" message={reason} style={{ marginBottom: 16 }} />}
      {!editing && (
        <Space style={{ marginBottom: 8 }}>
          {file.editable && (
            <Button
              onClick={() => {
                dispatch({ type: 'edit' });
                dispatch({ type: 'change', text: file.content });
              }}
            >
              Editar
            </Button>
          )}
          {file.editable && <Button danger onClick={remove}>Apagar</Button>}
          {isAdmin && file.id && <Button onClick={move}>Mover para…</Button>}
        </Space>
      )}
      {editing ? (
        <>
          {state.error && (
            <Alert
              type="error"
              style={{ marginBottom: 8 }}
              message={state.stale ? 'O arquivo mudou desde que você abriu' : state.error}
              description={(
                <>
                  {state.stale && <div>{state.error}</div>}
                  {problemLines(state.problems).map((line) => <div key={line}>{line}</div>)}
                  {state.stale && (
                    <Button style={{ marginTop: 8 }} onClick={reload}>Recarregar</Button>
                  )}
                </>
              )}
            />
          )}
          <Input.TextArea
            rows={24}
            style={MONO}
            value={state.draft}
            disabled={state.mode === 'saving'}
            onChange={(e) => dispatch({ type: 'change', text: e.target.value })}
          />
          <Space style={{ marginTop: 8 }}>
            <Button type="primary" loading={state.mode === 'saving'} onClick={save}>Salvar</Button>
            <Button disabled={state.mode === 'saving'} onClick={() => dispatch({ type: 'cancel' })}>
              Cancelar
            </Button>
          </Space>
        </>
      ) : (
        <PrismCode code={file.content} style={{ padding: 0, marginTop: 16 }} />
      )}
      {file.updatedBy && file.updatedAt && (
        <div>
          <Typography.Text type="secondary">
            Alterado por {file.updatedBy} em {new Date(file.updatedAt).toLocaleString('pt-BR')}
          </Typography.Text>
        </div>
      )}
    </>
  );
}

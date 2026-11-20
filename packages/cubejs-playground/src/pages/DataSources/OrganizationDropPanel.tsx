import { Empty, Input, message, Modal, Tag, Typography } from 'antd';
import { DragEvent, useMemo, useState } from 'react';

import { AdminApi, Organization } from '../Admin/api';
import { DATA_SOURCE_DRAG_TYPE, dropAction, sourcesByOrganization } from './organizationDrop';

type Owned = { name: string; organization?: { id: string; name: string } | null };

type Props = {
  api: AdminApi;
  organizations: Organization[];
  dataSources: Owned[];
  // Called after a drop here changed a data source's organization.
  onChanged: () => Promise<void> | void;
};

// The organizations, each a drop target with the data sources it owns. A
// Platform data source dropped on one is assigned to it; one of another
// organization moves there, after a confirmation, since its releases to the
// old organization's workspaces are withdrawn.
export function OrganizationDropPanel({
  api, organizations, dataSources, onChanged,
}: Props) {
  const [q, setQ] = useState('');
  const [target, setTarget] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const owned = useMemo(() => sourcesByOrganization(dataSources), [dataSources]);
  const shown = useMemo(() => {
    const term = q.trim().toLowerCase();
    return [...organizations]
      .sort((a, b) => a.name.localeCompare(b.name))
      .filter((o) => !term || o.name.toLowerCase().includes(term));
  }, [organizations, q]);

  function dragOver(e: DragEvent, org: string) {
    if (e.dataTransfer.types.includes(DATA_SOURCE_DRAG_TYPE)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      setTarget(org);
    }
  }

  async function change(name: string, org: Organization, verb: 'assign' | 'move') {
    setBusy(org.id);
    try {
      await (verb === 'assign' ? api.assignConnection(name, org.id) : api.moveConnection(name, org.id));
      message.success(`${name} belongs to ${org.name}`);
      await onChanged();
    } catch (e: any) {
      message.error(e.message);
    } finally {
      setBusy(null);
    }
  }

  // The workspaces of `from` the data source is released to, for the
  // confirmation; `null` when they cannot be read.
  async function releasedTo(name: string, from: string): Promise<number | null> {
    try {
      const list = await api.connections(from);
      return list.find((c) => c.name === name)?.workspaces.length ?? 0;
    } catch {
      return null;
    }
  }

  async function drop(e: DragEvent, org: Organization) {
    e.preventDefault();
    setTarget(null);
    const name = e.dataTransfer.getData(DATA_SOURCE_DRAG_TYPE);
    const ds = dataSources.find((d) => d.name === name);
    if (!ds) {
      return;
    }
    const action = dropAction(ds, org.id);
    if (action === 'forbidden') {
      message.warning('The default data source stays Platform: it cannot belong to an organization');
    } else if (action === 'none') {
      message.info(`${name} already belongs to ${org.name}`);
    } else if (action === 'assign') {
      await change(name, org, 'assign');
    } else {
      const from = ds.organization!;
      const count = await releasedTo(name, from.id);
      let releases = `Its releases to ${from.name}'s workspaces are withdrawn.`;
      if (count === 0) {
        releases = `It is not released to any of ${from.name}'s workspaces.`;
      } else if (count != null) {
        releases = `Its releases to ${count === 1 ? '1 workspace' : `${count} workspaces`} of ${from.name} are withdrawn.`;
      }
      Modal.confirm({
        title: `Move ${name} from ${from.name} to ${org.name}?`,
        content: `${releases} Only platform admins edit or delete it afterwards.`,
        okText: 'Move',
        onOk: () => change(name, org, 'move'),
      });
    }
  }

  return (
    <div>
      <Typography.Paragraph type="secondary">
        Drag a data source onto an organization to give it to that organization.
      </Typography.Paragraph>
      <Input.Search
        allowClear
        placeholder="Search organizations"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        style={{ display: 'block', marginBottom: 12 }}
      />
      {!shown.length ? (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={q ? 'No organizations match' : 'No organizations yet: create them in Admin → Organizations'}
        />
      ) : null}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        {shown.map((org) => {
          const list = owned[org.id] || [];
          const highlighted = target === org.id;
          return (
            <div
              key={org.id}
              data-organization={org.id}
              data-testid={`data-source-org-${org.id}`}
              onDragOver={(e) => dragOver(e, org.id)}
              onDragLeave={() => setTarget((t) => (t === org.id ? null : t))}
              onDrop={(e) => drop(e, org)}
              style={{
                padding: '6px 10px',
                borderRadius: 4,
                border: `1px dashed ${highlighted ? 'var(--pg-accent, #7a77ff)' : 'var(--pg-line, #e8e8ec)'}`,
                background: highlighted ? 'rgba(var(--purple-color-rgb), 0.08)' : undefined,
                opacity: busy === org.id ? 0.6 : 1,
              }}
            >
              <Typography.Text strong>{org.name}</Typography.Text>
              {' '}
              <Typography.Text type="secondary">
                {list.length === 1 ? '(1 data source)' : `(${list.length} data sources)`}
              </Typography.Text>
              <div style={{ marginTop: 4 }}>
                {list.length ? list.map((ds) => (
                  <Tag key={ds.name} style={{ marginBottom: 4 }}>{ds.name}</Tag>
                )) : (
                  <Typography.Text type="secondary">Drag data sources here</Typography.Text>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

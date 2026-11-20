import { Empty, Input, message, Modal, Tag, Typography } from 'antd';
import { CSSProperties, DragEvent, ReactNode, useMemo, useState } from 'react';

import { AdminApi, Organization } from '../Admin/api';
import {
  canDrag, DATA_SOURCE_DRAG_TYPE, dropAction, leavesZone, sourcesByOrganization,
} from './organizationDrop';

type Owned = { name: string; organization?: { id: string; name: string } | null };

type Props = {
  api: AdminApi;
  organizations: Organization[];
  dataSources: Owned[];
  // Called after a drop here changed a data source's organization.
  onChanged: () => Promise<void> | void;
  // Nothing can be dropped or removed meanwhile (the server restarts).
  disabled?: boolean;
};

// The drop target that stands for "no organization".
const PLATFORM = '__platform__';

const zoneStyle = (highlighted: boolean, busy: boolean): CSSProperties => ({
  padding: '6px 10px',
  borderRadius: 4,
  border: `1px dashed ${highlighted ? 'var(--pg-accent, #7a77ff)' : 'var(--pg-line, #e8e8ec)'}`,
  background: highlighted ? 'rgba(var(--purple-color-rgb), 0.08)' : undefined,
  opacity: busy ? 0.6 : 1,
});

function startDrag(e: DragEvent, name: string) {
  e.dataTransfer.setData(DATA_SOURCE_DRAG_TYPE, name);
  e.dataTransfer.effectAllowed = 'move';
}

// Platform, then the organizations, each a drop target with its data
// sources. A Platform data source dropped on an organization is assigned to
// it; one of another organization moves there; one dropped on Platform (or
// removed with its ×) leaves its organization. Moving and leaving ask first,
// since the old organization's workspace releases are withdrawn.
export function OrganizationDropPanel({
  api, organizations, dataSources, onChanged, disabled = false,
}: Props) {
  const [q, setQ] = useState('');
  const [target, setTarget] = useState<string | null>(null);
  const [saving, setBusy] = useState<string | null>(null);
  // A zone's key while its drop is saved; any truthy value blocks drags.
  const busy = saving || (disabled ? 'disabled' : null);

  const owned = useMemo(() => sourcesByOrganization(dataSources), [dataSources]);
  const platform = useMemo(
    () => dataSources
      .filter((ds) => !ds.organization)
      .sort((a, b) => a.name.localeCompare(b.name)),
    [dataSources]
  );
  const shown = useMemo(() => {
    const term = q.trim().toLowerCase();
    return [...organizations]
      .sort((a, b) => a.name.localeCompare(b.name))
      .filter((o) => !term || o.name.toLowerCase().includes(term));
  }, [organizations, q]);

  function dragOver(e: DragEvent, zone: string) {
    if (!busy && e.dataTransfer.types.includes(DATA_SOURCE_DRAG_TYPE)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      setTarget(zone);
    }
  }

  async function run(zone: string, action: () => Promise<unknown>, done: string) {
    setBusy(zone);
    try {
      await action();
      message.success(done);
      await onChanged();
    } catch (e: any) {
      message.error(e.message);
    } finally {
      setBusy(null);
    }
  }

  // What leaving `from` withdraws, for the confirmation.
  async function releasesNote(name: string, from: { id: string; name: string }): Promise<string> {
    try {
      const list = await api.connections(from.id);
      const count = list.find((c) => c.name === name)?.workspaces.length ?? 0;
      if (count === 0) {
        return `It is not released to any of ${from.name}'s workspaces.`;
      }
      return `Its releases to ${count === 1 ? '1 workspace' : `${count} workspaces`} of ${from.name} are withdrawn.`;
    } catch {
      return `Its releases to ${from.name}'s workspaces are withdrawn.`;
    }
  }

  async function confirmUnassign(ds: Owned) {
    const from = ds.organization!;
    const note = await releasesNote(ds.name, from);
    Modal.confirm({
      title: `Remove ${ds.name} from ${from.name}?`,
      content: `${note} It goes back to Platform, where only platform admins use it, and can be given to another organization.`,
      okText: 'Remove',
      okButtonProps: { danger: true },
      onOk: () => run(PLATFORM, () => api.unassignConnection(ds.name), `${ds.name} is back on Platform`),
    });
  }

  async function drop(e: DragEvent, org: Organization | null) {
    e.preventDefault();
    setTarget(null);
    if (busy) {
      return;
    }
    const name = e.dataTransfer.getData(DATA_SOURCE_DRAG_TYPE);
    const ds = dataSources.find((d) => d.name === name);
    if (!ds) {
      return;
    }
    const action = dropAction(ds, org ? org.id : null);
    if (action === 'forbidden') {
      message.warning('The default data source stays Platform: it cannot belong to an organization');
    } else if (action === 'none') {
      message.info(org ? `${name} already belongs to ${org.name}` : `${name} is already on Platform`);
    } else if (action === 'unassign') {
      await confirmUnassign(ds);
    } else if (action === 'assign') {
      await run(org!.id, () => api.assignConnection(name, org!.id), `${name} belongs to ${org!.name}`);
    } else {
      const from = ds.organization!;
      const note = await releasesNote(name, from);
      Modal.confirm({
        title: `Move ${name} from ${from.name} to ${org!.name}?`,
        content: `${note} Only platform admins edit or delete it afterwards.`,
        okText: 'Move',
        onOk: () => run(org!.id, () => api.moveConnection(name, org!.id), `${name} belongs to ${org!.name}`),
      });
    }
  }

  function zone(key: string, org: Organization | null, title: ReactNode, list: Owned[], empty: string) {
    return (
      <div
        key={key}
        data-organization={org ? org.id : 'platform'}
        data-testid={`data-source-org-${org ? org.id : 'platform'}`}
        onDragOver={(e) => dragOver(e, key)}
        onDragLeave={(e) => {
          if (leavesZone(e.currentTarget, e.relatedTarget as Node | null)) {
            setTarget((t) => (t === key ? null : t));
          }
        }}
        onDrop={(e) => drop(e, org)}
        style={zoneStyle(target === key, busy === key)}
      >
        {title}
        {' '}
        <Typography.Text type="secondary">
          {list.length === 1 ? '(1 data source)' : `(${list.length} data sources)`}
        </Typography.Text>
        <div style={{ marginTop: 4 }}>
          {list.length ? list.map((ds) => (
            <Tag
              key={ds.name}
              draggable={canDrag(ds, !!busy)}
              onDragStart={(e) => startDrag(e, ds.name)}
              closable={!!org && !busy}
              onClose={(e) => {
                // The tag stays until the server agrees.
                e.preventDefault();
                confirmUnassign(ds);
              }}
              style={{ marginBottom: 4, cursor: canDrag(ds, !!busy) ? 'grab' : undefined }}
            >
              {ds.name}
            </Tag>
          )) : (
            <Typography.Text type="secondary">{empty}</Typography.Text>
          )}
        </div>
      </div>
    );
  }

  return (
    <div>
      <Typography.Paragraph type="secondary">
        Drag a data source onto an organization to give it to that organization, or onto
        Platform to take it back.
      </Typography.Paragraph>
      <Input.Search
        allowClear
        placeholder="Search organizations"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        style={{ display: 'block', marginBottom: 12 }}
      />
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        {zone(
          PLATFORM,
          null,
          <Typography.Text strong title="Without an organization: for platform admins only">Platform</Typography.Text>,
          platform,
          'Drag data sources here to take them out of their organization',
        )}
        {!shown.length ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={q ? 'No organizations match' : 'No organizations yet: create them in Admin → Organizations'}
          />
        ) : null}
        {shown.map((org) => zone(
          org.id,
          org,
          <Typography.Text strong>{org.name}</Typography.Text>,
          owned[org.id] || [],
          'Drag data sources here',
        ))}
      </div>
    </div>
  );
}

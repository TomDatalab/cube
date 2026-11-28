import { useEffect, useState } from 'react';
import { CircleAlert } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import {
  Card, CardAction, CardContent, CardHeader, CardTitle,
} from '@/components/ui/card';
import {
  Column, Confirm, DataTable, notify, SearchInput, SimpleSelect, Tag,
} from '@/components/kit';

import { useIsMounted } from '../../hooks';
import { IdText } from './IdText';
import {
  AdminApi, Connection, DataSource, moveTargets, Organization, PAGE_SIZE, Workspace,
} from './api';
import { NameDialog, NameValues } from './NameDialog';
import { RemoteSelect, userOptions } from './RemoteSelect';
import { useSearch } from './useSearch';

type Props = {
  api: AdminApi;
  // Opens the Workspaces tab on one organization's workspaces.
  onShowWorkspaces: (org: string) => void;
};

export function OrganizationsTab({ api, onShowWorkspaces }: Props) {
  const [selected, setSelected] = useState<Organization | null>(null);
  const [dialog, setDialog] = useState<'new' | 'rename' | null>(null);
  const {
    q, setQ, page, setPage, result, loading, error, reload,
  } = useSearch((term, p) => api.searchOrganizations(term, p), [api]);

  // The listing's copy, so a rename shows; the chosen one when it is on
  // another page.
  const org = result?.items.find((o) => o.id === selected?.id) || selected;

  async function save({ name, owner }: NameValues) {
    if (dialog === 'new') {
      const created = await api.createOrganization(name, owner!);
      setSelected(created ?? null);
    } else {
      await api.renameOrganization(org!.id, name);
    }
    notify.success(`${name} saved`);
    reload();
  }

  const columns: Column<Organization>[] = [
    {
      key: 'select',
      title: '',
      width: 32,
      render: (_, o) => (
        <input
          type="radio"
          name="organization"
          aria-label={`Select ${o.name}`}
          className="tw:cursor-pointer tw:accent-primary"
          checked={selected?.id === o.id}
          onChange={() => setSelected(o)}
          onClick={(e) => e.stopPropagation()}
        />
      ),
    },
    { key: 'name', title: 'Organization', dataIndex: 'name' },
    { key: 'id', title: 'Id', dataIndex: 'id', render: (v) => <IdText id={v} /> },
    { key: 'createdBy', title: 'Created by', dataIndex: 'createdBy', render: (v) => v || '—' },
    { key: 'createdAt', title: 'Created', dataIndex: 'createdAt', render: (v) => new Date(v).toLocaleString() },
  ];

  return (
    <>
      <div className="tw:mb-4 tw:flex tw:items-center tw:gap-2">
        <Button onClick={() => setDialog('new')}>
          New organization
        </Button>
      </div>

      <SearchInput
        className="tw:mb-4"
        placeholder="Search organizations by name or id"
        value={q}
        onChange={setQ}
      />
      {error ? (
        <Alert variant="destructive" className="tw:mb-4">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      <DataTable<Organization>
        rowKey="id"
        loading={loading}
        rows={result?.items || []}
        pagination={{
          current: page,
          pageSize: PAGE_SIZE,
          total: result?.total ?? 0,
          onChange: setPage,
        }}
        selectedKey={selected?.id ?? null}
        onRowClick={setSelected}
        columns={columns}
      />

      {org ? (
        <OrganizationCard
          key={org.id}
          api={api}
          org={org}
          onRename={() => setDialog('rename')}
          onShowWorkspaces={() => onShowWorkspaces(org.id)}
          onDeleted={async () => {
            setSelected(null);
            reload();
          }}
        />
      ) : null}

      <NameDialog
        title={dialog && (dialog === 'new' ? 'New organization' : 'Rename organization')}
        initialName={dialog === 'rename' ? org?.name : ''}
        choice={dialog === 'new' ? {
          field: 'owner',
          label: 'Owner',
          message: 'Pick the first owner',
          fetchOptions: userOptions(api),
        } : undefined}
        onSubmit={save}
        onClose={() => setDialog(null)}
      />
    </>
  );
}

type CardProps = {
  api: AdminApi;
  org: Organization;
  onRename: () => void;
  onShowWorkspaces: () => void;
  onDeleted: () => Promise<void>;
};

function OrganizationCard({
  api, org, onRename, onShowWorkspaces, onDeleted,
}: CardProps) {
  const isMounted = useIsMounted();
  const [workspaces, setWorkspaces] = useState<Workspace[] | null>(null);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [dataSources, setDataSources] = useState<DataSource[]>([]);
  const [owner, setOwner] = useState<string | undefined>();
  const [ownerName, setOwnerName] = useState<string | undefined>();
  const [dataSource, setDataSource] = useState<string | undefined>();
  const [organizations, setOrganizations] = useState<Organization[]>([]);
  // The organization each connection is about to move to.
  const [moveTo, setMoveTo] = useState<Record<string, string | undefined>>({});

  async function loadWorkspaces() {
    try {
      const loaded = await api.workspaces(org.id);
      if (isMounted()) {
        setWorkspaces(loaded);
      }
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  async function loadConnections() {
    try {
      const [loaded, sources, orgs] = await Promise.all([
        api.connections(org.id),
        api.assignableDataSources(),
        api.organizations(),
      ]);
      if (isMounted()) {
        setConnections(loaded);
        setDataSources(sources);
        setOrganizations(orgs);
        setMoveTo({});
      }
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  useEffect(() => {
    loadWorkspaces();
    loadConnections();
  }, [org.id]);

  async function act(action: () => Promise<unknown>, done: string, reload?: () => Promise<void>) {
    try {
      await action();
      notify.success(done);
      await reload?.();
      return true;
    } catch (e: any) {
      notify.error(e.message);
      return false;
    }
  }

  return (
    <Card className="tw:mt-4 tw:gap-4 tw:py-4">
      <CardHeader className="tw:px-4">
        <CardTitle>{org.name}</CardTitle>
        <CardAction className="tw:flex tw:items-center tw:gap-2">
          <Button size="sm" variant="outline" onClick={onRename}>Rename</Button>
          <Confirm
            title={`Delete ${org.name}? It must have no workspaces or connections left.`}
            onConfirm={() => act(() => api.deleteOrganization(org.id), `${org.name} deleted`, onDeleted)}
          >
            <Button size="sm" variant="outline" className="tw:text-destructive">Delete</Button>
          </Confirm>
        </CardAction>
      </CardHeader>
      <CardContent className="tw:px-4 tw:text-sm">
        <strong className="tw:font-semibold">Owners</strong>
        <div className="tw:text-muted-foreground">
          Owners administer the organization: its workspaces, members and connections.
        </div>
        <div className="tw:mt-2 tw:mb-4 tw:flex tw:flex-wrap tw:items-center tw:gap-2">
          <RemoteSelect
            size="small"
            placeholder="User"
            style={{ width: 200 }}
            value={owner}
            onChange={(v, label) => {
              setOwner(v);
              setOwnerName(label);
            }}
            fetchOptions={userOptions(api)}
          />
          <Button
            size="sm"
            variant="outline"
            disabled={!owner}
            onClick={() => act(() => api.addOwner(org.id, owner!), `${ownerName ?? owner} is an owner of ${org.name}`)}
          >
            Make owner
          </Button>
          <Confirm
            title={`Remove ${ownerName ?? owner} as an owner of ${org.name}?`}
            disabled={!owner}
            onConfirm={() => act(() => api.removeOwner(org.id, owner!), `${ownerName ?? owner} is no longer an owner`)}
          >
            <Button size="sm" variant="outline" className="tw:text-destructive" disabled={!owner}>Remove owner</Button>
          </Confirm>
        </div>

        <strong className="tw:font-semibold">Connections</strong>
        <div className="tw:flex tw:flex-wrap">
          {connections.length ? connections.map((c) => {
            const target = organizations.find((o) => o.id === moveTo[c.name]);
            return (
              <div key={c.name} className="tw:mr-4 tw:mb-2 tw:flex tw:items-center tw:gap-2">
                <code className="tw:rounded tw:border tw:bg-muted tw:px-1 tw:font-mono tw:text-xs">{c.name}</code>
                {c.origin === 'assigned' ? (
                  <Tag title="Assigned by a platform admin: the organization uses and releases it, only platform admins edit or delete it">
                    assigned
                  </Tag>
                ) : (
                  <Tag title="Created by the organization: its owners edit and delete it">created</Tag>
                )}
                <div className="tw:w-40">
                  {/* Names the select, as its aria-label did. */}
                  <label htmlFor={`move-to-${c.name}`} className="tw:sr-only">{`Move ${c.name} to`}</label>
                  <SimpleSelect
                    id={`move-to-${c.name}`}
                    size="sm"
                    placeholder="Move to…"
                    value={moveTo[c.name]}
                    onChange={(v) => setMoveTo({ ...moveTo, [c.name]: v })}
                    options={moveTargets(organizations, org.id).map((o) => ({ value: o.id, label: o.name }))}
                  />
                </div>
                <Confirm
                  title={`Move ${c.name} to ${target?.name}? Its releases to ${org.name}'s workspaces are withdrawn.`}
                  disabled={!target}
                  onConfirm={() => act(
                    () => api.moveConnection(c.name, target!.id),
                    `${c.name} belongs to ${target!.name}`,
                    loadConnections
                  )}
                >
                  <Button size="sm" variant="outline" disabled={!target}>Move</Button>
                </Confirm>
              </div>
            );
          }) : <span className="tw:text-muted-foreground">None yet.</span>}
        </div>
        <div className="tw:mt-2 tw:mb-4 tw:flex tw:flex-wrap tw:items-center tw:gap-2">
          <SimpleSelect
            size="sm"
            className="tw:w-[200px]"
            placeholder="Data source"
            value={dataSource}
            onChange={setDataSource}
            options={dataSources
              .map((ds) => ({ value: ds.name, label: ds.type ? `${ds.name} (${ds.type})` : ds.name }))}
          />
          <Confirm
            title={`Assign ${dataSource} to ${org.name}? Its owners can release it to their workspaces, but only platform admins edit or delete it.`}
            disabled={!dataSource}
            onConfirm={async () => {
              if (await act(
                () => api.assignConnection(dataSource!, org.id),
                `${dataSource} belongs to ${org.name}`,
                loadConnections
              )) {
                setDataSource(undefined);
              }
            }}
          >
            <Button size="sm" variant="outline" disabled={!dataSource}>Add to organization</Button>
          </Confirm>
        </div>

        <strong className="tw:font-semibold">Workspaces</strong>
        <div className="tw:mt-2">
          {workspaces == null ? (
            <span className="tw:text-muted-foreground">Loading...</span>
          ) : (
            <div className="tw:flex tw:items-center tw:gap-2">
              <span>
                {workspaces.length === 1 ? '1 workspace' : `${workspaces.length} workspaces`}
              </span>
              <Button size="sm" variant="link" className="tw:px-0" onClick={onShowWorkspaces}>
                Open in Workspaces
              </Button>
            </div>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

import { Fragment, useEffect, useState } from 'react';
import { ChevronDown, ChevronRight, CircleAlert } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@/components/ui/table';
import {
  Confirm, EmptyState, notify, Pager, SearchInput, Spinner,
} from '@/components/kit';

import { useIsMounted } from '../../hooks';
import { IdText } from './IdText';
import {
  AdminApi, Connection, PAGE_SIZE, Workspace,
} from './api';
import { NameDialog, NameValues } from './NameDialog';
import { organizationOptions, RemoteSelect } from './RemoteSelect';
import { useSearch } from './useSearch';
import { WorkspacePanel } from './WorkspacePanel';

type Props = {
  api: AdminApi;
  // The organization shown, or `null` for all of them.
  organization: string | null;
  onOrganizationChange: (org: string | null) => void;
};

// Every workspace of every organization, filtered by organization.
export function WorkspacesTab({ api, organization, onOrganizationChange }: Props) {
  const isMounted = useIsMounted();
  // The workspaces whose details are open under their row.
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  // `true`: a new workspace; a workspace: renaming it.
  const [editing, setEditing] = useState<Workspace | true | null>(null);
  // The name of the filtered organization, which may come in as an id only.
  const [organizationLabel, setOrganizationLabel] = useState<string | undefined>();
  const {
    q, setQ, page, setPage, result, loading, error, reload,
  } = useSearch(
    (term, p) => api.searchWorkspaces({ q: term, organizationId: organization, page: p }),
    [api, organization]
  );

  useEffect(() => {
    setOrganizationLabel(undefined);
    if (!organization) {
      return;
    }
    api.searchOrganizations(organization, 1).then((found) => {
      const match = found.items.find((o) => o.id === organization);
      if (!isMounted()) {
        return;
      }
      if (match) {
        setOrganizationLabel(match.name);
      } else {
        // Deleted (or no longer visible): back to all organizations.
        onOrganizationChange(null);
      }
    }, () => {});
  }, [api, organization]);

  async function save({ name, organization: org }: NameValues) {
    if (editing === true) {
      await api.createWorkspace(org!, name);
    } else {
      await api.renameWorkspace(editing!.id, name);
    }
    notify.success(`${name} saved`);
    reload();
  }

  async function remove(ws: Workspace) {
    try {
      await api.deleteWorkspace(ws.id);
      notify.success(`${ws.name} deleted`);
      reload();
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  let dialogTitle: string | null = null;
  if (editing === true) {
    dialogTitle = 'New workspace';
  } else if (editing) {
    dialogTitle = 'Rename workspace';
  }

  const rows = result?.items || [];
  const COLUMNS = 5;

  return (
    <>
      <div className="tw:mb-4 tw:flex tw:flex-wrap tw:items-center tw:gap-2">
        <RemoteSelect
          allowClear
          placeholder="All organizations"
          style={{ width: 240 }}
          value={organization ?? undefined}
          valueLabel={organizationLabel}
          onChange={(v) => onOrganizationChange(v || null)}
          fetchOptions={organizationOptions(api)}
        />
        <Button onClick={() => setEditing(true)}>
          New workspace
        </Button>
      </div>

      <SearchInput
        className="tw:mb-4"
        placeholder="Search workspaces, or their organization"
        value={q}
        onChange={setQ}
      />
      {error ? (
        <Alert variant="destructive" className="tw:mb-4">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      <div className="tw:relative">
        <div className="tw:rounded-md tw:border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="tw:w-8" />
                <TableHead>Workspace</TableHead>
                <TableHead>Organization</TableHead>
                <TableHead>Id</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={COLUMNS}>
                    <EmptyState className="tw:py-6">{loading ? <Spinner className="tw:mx-auto" /> : 'No data'}</EmptyState>
                  </TableCell>
                </TableRow>
              ) : rows.map((ws) => {
                const open = Boolean(expanded[ws.id]);
                return (
                  <Fragment key={ws.id}>
                    <TableRow>
                      <TableCell>
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          aria-label={open ? 'Collapse row' : 'Expand row'}
                          aria-expanded={open}
                          onClick={() => setExpanded((e) => ({ ...e, [ws.id]: !open }))}
                        >
                          {open ? <ChevronDown /> : <ChevronRight />}
                        </Button>
                      </TableCell>
                      <TableCell className="tw:whitespace-normal">{ws.name}</TableCell>
                      <TableCell className="tw:whitespace-normal">{ws.organizationName}</TableCell>
                      <TableCell><IdText id={ws.id} /></TableCell>
                      <TableCell className="tw:text-right">
                        <div className="tw:flex tw:items-center tw:justify-end tw:gap-2">
                          <Button size="sm" variant="outline" onClick={() => setEditing(ws)}>Rename</Button>
                          <Confirm
                            title={`Delete ${ws.name}, with its members, objects and connection releases?`}
                            onConfirm={() => remove(ws)}
                          >
                            <Button size="sm" variant="outline" className="tw:text-destructive">Delete</Button>
                          </Confirm>
                        </div>
                      </TableCell>
                    </TableRow>
                    {open ? (
                      <TableRow className="tw:hover:bg-transparent">
                        <TableCell colSpan={COLUMNS} className="tw:bg-muted/30 tw:whitespace-normal">
                          <WorkspaceDetails api={api} workspace={ws} />
                        </TableCell>
                      </TableRow>
                    ) : null}
                  </Fragment>
                );
              })}
            </TableBody>
          </Table>
        </div>
        {loading && rows.length > 0 ? (
          <div className="tw:absolute tw:inset-0 tw:flex tw:items-center tw:justify-center tw:bg-background/50">
            <Spinner className="tw:size-5" />
          </div>
        ) : null}
        <Pager current={page} pageSize={PAGE_SIZE} total={result?.total ?? 0} onChange={setPage} />
      </div>

      <NameDialog
        title={dialogTitle}
        initialName={editing && editing !== true ? editing.name : ''}
        choice={editing === true ? {
          field: 'organization',
          label: 'Organization',
          message: 'Pick an organization',
          fetchOptions: organizationOptions(api),
          initial: organization ?? undefined,
          initialLabel: organizationLabel,
        } : undefined}
        onSubmit={save}
        onClose={() => setEditing(null)}
      />
    </>
  );
}

type DetailsProps = {
  api: AdminApi;
  workspace: Workspace;
};

// The members and connection releases of one workspace, with its
// organization's connections to release from.
function WorkspaceDetails({ api, workspace }: DetailsProps) {
  const isMounted = useIsMounted();
  const [connections, setConnections] = useState<Connection[]>([]);

  async function loadConnections() {
    try {
      const loaded = await api.connections(workspace.organizationId);
      if (isMounted()) {
        setConnections(loaded);
      }
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  useEffect(() => {
    loadConnections();
  }, [workspace.organizationId]);

  return (
    <WorkspacePanel
      api={api}
      workspace={workspace}
      connections={connections}
      reloadConnections={loadConnections}
    />
  );
}

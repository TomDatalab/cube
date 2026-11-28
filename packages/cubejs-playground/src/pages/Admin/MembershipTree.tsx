import { DragEvent, useEffect, useRef, useState } from 'react';
import {
  ChevronDown, ChevronRight, CircleAlert, X,
} from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import {
  Confirm, EmptyState, notify, SearchInput, SimpleSelect, Spinner,
} from '@/components/kit';
import { cn } from '@/lib/utils';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, Member, memberName, PAGE_SIZE, Role, ROLES, WorkspaceListing,
} from './api';
import { groupByOrganization } from './membershipGroups';
import { useSearch } from './useSearch';

// The drag payload of a user row: its id. A custom type, so text dragged in
// from elsewhere is not taken for a user.
export const USER_DRAG_TYPE = 'application/x-cube-user';
// The dragged user's username, for the messages; the id is the call's.
export const USER_DRAG_NAME_TYPE = 'application/x-cube-username';

type Props = {
  api: AdminApi;
};

const roleOptions = ROLES.map((r) => ({ value: r, label: r }));

// Organization → workspace → members, one searchable page of workspaces at a
// time; a workspace's members load when it opens or a user is dropped on it.
// A user dropped on a workspace joins it as a viewer; roles change and
// members leave from here.
export function MembershipTree({ api }: Props) {
  const isMounted = useIsMounted();
  const {
    q, setQ, page, setPage, result, loading, error,
  } = useSearch((term, p) => api.searchWorkspaces({ q: term, page: p }), []);
  const [members, setMembers] = useState<Record<string, Member[]>>({});
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [target, setTarget] = useState<string | null>(null);
  const openRef = useRef(open);
  openRef.current = open;

  async function loadMembers(ws: string): Promise<Member[] | null> {
    try {
      const list = await api.members(ws);
      if (isMounted()) {
        setMembers((m) => ({ ...m, [ws]: list }));
      }
      return list;
    } catch (e: any) {
      notify.error(e.message);
      return null;
    }
  }

  // Members are kept per workspace until the next search; workspaces that
  // stay open across it load again.
  useEffect(() => {
    setMembers({});
    (result?.items || []).forEach((ws) => {
      if (openRef.current.has(ws.id)) {
        loadMembers(ws.id);
      }
    });
  }, [result]);

  async function act(ws: string, action: () => Promise<unknown>, done: string) {
    try {
      await action();
      notify.success(done);
    } catch (e: any) {
      notify.error(e.message);
    }
    await loadMembers(ws);
  }

  function toggle(ws: string) {
    const next = new Set(open);
    if (next.has(ws)) {
      next.delete(ws);
    } else {
      next.add(ws);
      if (!members[ws]) {
        loadMembers(ws);
      }
    }
    setOpen(next);
  }

  function dragOver(e: DragEvent, ws: string) {
    if (e.dataTransfer.types.includes(USER_DRAG_TYPE)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
      setTarget(ws);
    }
  }

  async function drop(e: DragEvent, ws: WorkspaceListing) {
    e.preventDefault();
    setTarget(null);
    const user = e.dataTransfer.getData(USER_DRAG_TYPE);
    if (!user) {
      return;
    }
    const username = e.dataTransfer.getData(USER_DRAG_NAME_TYPE) || user;
    setOpen(new Set(open).add(ws.id));
    const current = members[ws.id] || await loadMembers(ws.id);
    if (current?.some((m) => m.user === user)) {
      notify.info(`${username} is already in ${ws.name}`);
      return;
    }
    if (current) {
      act(ws.id, () => api.setMember(ws.id, user, 'viewer'), `${username} added to ${ws.name} as viewer`);
    }
  }

  const groups = groupByOrganization(result?.items || []);
  const pages = Math.max(1, Math.ceil((result?.total || 0) / PAGE_SIZE));

  return (
    <div className="tw:text-sm">
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
      {result == null && !error ? <Spinner /> : null}
      {result && !groups.length && !loading ? (
        <EmptyState>
          {q ? 'No workspaces match' : 'No workspaces yet: create them in the Workspaces tab'}
        </EmptyState>
      ) : null}
      {groups.map((group) => {
        const { workspaces } = group;
        return (
          <div key={group.organizationId} className="tw:mb-4">
            <span className="tw:font-semibold">{group.organizationName}</span>
            {workspaces.map((ws) => {
              const list = members[ws.id];
              const expanded = open.has(ws.id);
              const highlighted = target === ws.id;
              return (
                <div
                  key={ws.id}
                  data-workspace={ws.id}
                  onDragOver={(e) => dragOver(e, ws.id)}
                  onDragLeave={() => setTarget((t) => (t === ws.id ? null : t))}
                  onDrop={(e) => drop(e, ws)}
                  className={cn(
                    'tw:my-1 tw:ml-4 tw:rounded tw:border tw:border-dashed tw:px-2 tw:py-1',
                    highlighted ? 'tw:border-primary tw:bg-primary/10' : 'tw:border-transparent'
                  )}
                >
                  <div
                    role="button"
                    tabIndex={0}
                    onClick={() => toggle(ws.id)}
                    onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && toggle(ws.id)}
                    className="tw:flex tw:cursor-pointer tw:items-center tw:gap-1 tw:select-none"
                  >
                    {expanded ? <ChevronDown className="tw:size-3.5" /> : <ChevronRight className="tw:size-3.5" />}
                    <span>{ws.name}</span>
                    {list ? (
                      <span className="tw:text-muted-foreground">
                        {list.length === 1 ? '(1 member)' : `(${list.length} members)`}
                      </span>
                    ) : null}
                  </div>
                  {expanded ? (
                    <div className="tw:pl-5">
                      {list?.length ? list.map((m) => (
                        <div key={m.user} className="tw:mt-1 tw:flex tw:items-center tw:gap-2">
                          <span className="tw:min-w-[120px]">{memberName(m)}</span>
                          <SimpleSelect
                            size="sm"
                            className="tw:w-[100px]"
                            value={m.role}
                            options={roleOptions}
                            onChange={(role) => role && act(
                              ws.id,
                              () => api.setMember(ws.id, m.user, role as Role),
                              `${memberName(m)} is now ${role} in ${ws.name}`
                            )}
                          />
                          <Confirm
                            title={`Remove ${memberName(m)} from ${ws.name}?`}
                            onConfirm={() => act(
                              ws.id,
                              () => api.removeMember(ws.id, m.user),
                              `${memberName(m)} removed from ${ws.name}`
                            )}
                          >
                            <Button
                              size="icon-xs"
                              variant="ghost"
                              className="tw:text-destructive tw:hover:text-destructive"
                              aria-label={`Remove ${memberName(m)}`}
                            >
                              <X />
                            </Button>
                          </Confirm>
                        </div>
                      )) : null}
                      {list && !list.length ? (
                        <span className="tw:text-muted-foreground">
                          Drag users here
                        </span>
                      ) : null}
                      {list ? null : <Spinner />}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        );
      })}
      {result && result.total > PAGE_SIZE ? (
        <div className="tw:flex tw:items-center tw:gap-2">
          <Button size="sm" variant="outline" disabled={page <= 1 || loading} onClick={() => setPage(page - 1)}>
            Previous
          </Button>
          <span className="tw:text-muted-foreground">{`page ${page} of ${pages}`}</span>
          <Button size="sm" variant="outline" disabled={page >= pages || loading} onClick={() => setPage(page + 1)}>
            Next
          </Button>
        </div>
      ) : null}
    </div>
  );
}

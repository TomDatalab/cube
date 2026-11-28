import { useEffect, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Label } from '@/components/ui/label';
import { Separator } from '@/components/ui/separator';
import {
  Confirm, DataTable, notify, SimpleSelect,
} from '@/components/kit';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, Connection, Member, memberName, Role, ROLES, Workspace,
} from './api';
import { RemoteSelect, userOptions } from './RemoteSelect';

type Props = {
  api: AdminApi;
  workspace: Workspace;
  connections: Connection[];
  reloadConnections: () => Promise<void>;
};

// The members of one workspace and the organization connections released to it.
export function WorkspacePanel({
  api, workspace, connections, reloadConnections,
}: Props) {
  const isMounted = useIsMounted();
  const [members, setMembers] = useState<Member[] | null>(null);
  const [newMember, setNewMember] = useState<string | undefined>();
  const [newMemberName, setNewMemberName] = useState<string | undefined>();
  const [newRole, setNewRole] = useState<Role>('viewer');

  async function loadMembers() {
    try {
      const loaded = await api.members(workspace.id);
      if (isMounted()) {
        setMembers(loaded);
      }
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  useEffect(() => {
    loadMembers();
  }, [workspace.id]);

  async function act(action: () => Promise<unknown>, done: string, reload: () => Promise<void>) {
    try {
      await action();
      notify.success(done);
      await reload();
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  const roleOptions = ROLES.map((r) => ({ value: r, label: r }));
  // Only users not in the workspace yet: adding sets the role, so picking an
  // existing member here would change theirs.
  const memberIds = new Set((members || []).map((m) => m.user));
  const newMemberOptions = async (q: string) => (
    (await userOptions(api)(q)).filter((o) => !memberIds.has(o.value))
  );

  return (
    <div className="tw:py-2">
      <span className="tw:font-semibold">Members</span>
      <DataTable<Member>
        className="tw:mt-2"
        rowKey="user"
        loading={members == null}
        rows={members || []}
        columns={[
          { key: 'user', title: 'User', render: (_, m) => memberName(m) },
          {
            key: 'role',
            title: 'Role',
            render: (_, m) => (
              <SimpleSelect
                size="sm"
                className="tw:w-[120px]"
                value={m.role}
                options={roleOptions}
                onChange={(role) => role && act(
                  () => api.setMember(workspace.id, m.user, role as Role),
                  `${memberName(m)} is now ${role}`,
                  loadMembers
                )}
              />
            ),
          },
          {
            key: 'actions',
            title: '',
            align: 'right',
            render: (_, m) => (
              <Confirm
                title={`Remove ${memberName(m)} from ${workspace.name}?`}
                onConfirm={() => act(
                  () => api.removeMember(workspace.id, m.user),
                  `${memberName(m)} removed`,
                  loadMembers
                )}
              >
                <Button size="sm" variant="outline" className="tw:text-destructive">Remove</Button>
              </Confirm>
            ),
          },
        ]}
      />
      <div className="tw:mt-2 tw:flex tw:flex-wrap tw:items-center tw:gap-2">
        <RemoteSelect
          // Loads its options again whenever the members change.
          key={[...memberIds].join(',')}
          size="small"
          placeholder="User"
          style={{ width: 200 }}
          value={newMember}
          onChange={(v, label) => {
            setNewMember(v);
            setNewMemberName(label);
          }}
          fetchOptions={newMemberOptions}
        />
        <SimpleSelect
          size="sm"
          className="tw:w-[120px]"
          value={newRole}
          options={roleOptions}
          onChange={(role) => role && setNewRole(role as Role)}
        />
        <Button
          size="sm"
          disabled={!newMember}
          onClick={() => act(
            () => api.setMember(workspace.id, newMember!, newRole),
            `${newMemberName ?? newMember} added as ${newRole}`,
            async () => {
              setNewMember(undefined);
              setNewMemberName(undefined);
              await loadMembers();
            }
          )}
        >
          Add member
        </Button>
      </div>

      <Separator className="tw:mt-4 tw:mb-2" />

      <span className="tw:font-semibold">Connections released to this workspace</span>
      <div className="tw:mt-2 tw:flex tw:flex-col tw:gap-2">
        {connections.length ? connections.map((c) => {
          const id = `release-${workspace.id}-${c.name}`;
          return (
            <div key={c.name} className="tw:flex tw:items-center tw:gap-2">
              <Checkbox
                id={id}
                checked={c.workspaces.includes(workspace.id)}
                onCheckedChange={(next) => {
                  const checked = next === true;
                  act(
                    () => (checked
                      ? api.release(workspace.id, c.name)
                      : api.withdraw(workspace.id, c.name)),
                    `${c.name} ${checked ? 'released to' : 'withdrawn from'} ${workspace.name}`,
                    reloadConnections
                  );
                }}
              />
              <Label htmlFor={id} className="tw:font-normal">
                {c.name}
                {c.type ? <span className="tw:text-muted-foreground">{` (${c.type})`}</span> : null}
              </Label>
            </div>
          );
        }) : (
          <span className="tw:text-muted-foreground">
            The organization has no connections yet.
          </span>
        )}
      </div>
    </div>
  );
}

import { ReactNode, useEffect, useState } from 'react';
import { CircleAlert, X } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import {
  Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle,
} from '@/components/ui/card';
import {
  Column, Confirm, DataTable, notify, SearchInput, Spinner, Tag,
} from '@/components/kit';

import { useIsMounted } from '../../hooks';
import { IdText } from './IdText';
import {
  AdminApi, Group, memberName, PAGE_SIZE, Rule, RuleMember,
} from './api';
import { NameDialog, NameValues } from './NameDialog';
import { organizationOptions, RemoteSelect, userOptions } from './RemoteSelect';
import { useSearch } from './useSearch';

type Props = {
  api: AdminApi;
};

/** "1 member", "3 rules". */
export function countLabel(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** The rules of `organization` a group can join, as select options. */
export const ruleOptions = (api: AdminApi, organization: string) => async (q: string) => (
  (await api.searchRules({ q, organizationId: organization, page: 1 })).items.map((r) => ({
    value: r.id,
    label: `${r.workspaceName} / ${r.description}`,
  }))
);

// Groups of users of an organization. A group placed in a rule makes its
// members members of the rule, for as long as they stay in the group.
export function GroupsTab({ api }: Props) {
  const [organization, setOrganization] = useState<string | null>(null);
  const [organizationLabel, setOrganizationLabel] = useState<string | undefined>();
  const [selected, setSelected] = useState<Group | null>(null);
  const [dialog, setDialog] = useState<'new' | 'rename' | null>(null);
  const {
    q, setQ, page, setPage, result, loading, error, reload,
  } = useSearch(
    (term, p) => api.searchGroups({ q: term, organizationId: organization, page: p }),
    [api, organization]
  );

  // The listing's copy, so a rename or a new count shows.
  const group = result?.items.find((g) => g.id === selected?.id) || selected;

  async function save({ name, organization: org }: NameValues) {
    if (dialog === 'new') {
      setSelected(await api.createGroup(org!, name));
    } else {
      await api.renameGroup(group!.id, name);
    }
    notify.success(`${name} saved`);
    reload();
  }

  const columns: Column<Group>[] = [
    {
      key: 'select',
      title: '',
      width: 32,
      render: (_, g) => (
        <input
          type="radio"
          name="group"
          aria-label={`Select ${g.name}`}
          className="tw:cursor-pointer tw:accent-primary"
          checked={selected?.id === g.id}
          onChange={() => setSelected(g)}
          onClick={(e) => e.stopPropagation()}
        />
      ),
    },
    { key: 'name', title: 'Group', dataIndex: 'name' },
    { key: 'organization', title: 'Organization', dataIndex: 'organizationName' },
    { key: 'id', title: 'Id', dataIndex: 'id', render: (v) => <IdText id={v} /> },
    {
      key: 'members', title: 'Members', dataIndex: 'members', render: (v) => countLabel(v, 'member', 'members'),
    },
    {
      key: 'rules', title: 'Rules', dataIndex: 'rules', render: (v) => countLabel(v, 'rule', 'rules'),
    },
  ];

  return (
    <>
      <div className="tw:mb-4 tw:flex tw:items-center tw:gap-2">
        <RemoteSelect
          allowClear
          placeholder="All organizations"
          style={{ width: 240 }}
          value={organization ?? undefined}
          valueLabel={organizationLabel}
          onChange={(v, label) => {
            setOrganization(v || null);
            setOrganizationLabel(label);
            setSelected(null);
          }}
          fetchOptions={organizationOptions(api)}
        />
        <Button onClick={() => setDialog('new')}>New group</Button>
      </div>

      <SearchInput
        className="tw:mb-4"
        placeholder="Search groups, or their organization"
        value={q}
        onChange={setQ}
      />
      {error ? (
        <Alert variant="destructive" className="tw:mb-4">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      <DataTable<Group>
        rowKey="id"
        loading={loading}
        rows={result?.items || []}
        emptyText="No groups yet"
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

      {group ? (
        <GroupCard
          key={group.id}
          api={api}
          group={group}
          onRename={() => setDialog('rename')}
          onChanged={reload}
          onDeleted={() => {
            setSelected(null);
            reload();
          }}
        />
      ) : null}

      <NameDialog
        title={dialog && (dialog === 'new' ? 'New group' : 'Rename group')}
        initialName={dialog === 'rename' ? group?.name : ''}
        choice={dialog === 'new' ? {
          field: 'organization',
          label: 'Organization',
          message: 'Pick an organization',
          fetchOptions: organizationOptions(api),
          initial: organization ?? undefined,
          initialLabel: organizationLabel,
        } : undefined}
        onSubmit={save}
        onClose={() => setDialog(null)}
      />
    </>
  );
}

type CardProps = {
  api: AdminApi;
  group: Group;
  onRename: () => void;
  // Members or rules changed: the counts in the list.
  onChanged: () => void;
  onDeleted: () => void;
};

function GroupCard({
  api, group, onRename, onChanged, onDeleted,
}: CardProps) {
  const isMounted = useIsMounted();
  const [members, setMembers] = useState<RuleMember[] | null>(null);
  const [rules, setRules] = useState<Rule[] | null>(null);
  const [user, setUser] = useState<string | undefined>();
  const [userName, setUserName] = useState<string | undefined>();
  const [rule, setRule] = useState<string | undefined>();
  const [ruleName, setRuleName] = useState<string | undefined>();
  // Remounts the pickers, so a done choice clears them.
  const [picks, setPicks] = useState(0);

  async function load() {
    try {
      const [m, r] = await Promise.all([api.groupMembers(group.id), api.groupRules(group.id)]);
      if (isMounted()) {
        setMembers(m);
        setRules(r);
      }
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  useEffect(() => {
    load();
  }, [group.id]);

  async function act(action: () => Promise<unknown>, done: string) {
    try {
      await action();
      notify.success(done);
      await load();
      onChanged();
      return true;
    } catch (e: any) {
      notify.error(e.message);
      return false;
    }
  }

  const inRules = (rules?.length ?? group.rules) > 0;

  return (
    <Card className="tw:mt-4 tw:gap-4 tw:py-4">
      <CardHeader className="tw:px-4">
        <CardTitle>{group.name}</CardTitle>
        <CardDescription>{group.organizationName}</CardDescription>
        <CardAction className="tw:flex tw:items-center tw:gap-2">
          <Button size="sm" variant="outline" onClick={onRename}>Rename</Button>
          <Confirm
            title={`Delete ${group.name}? Its members leave it.`}
            disabled={inRules}
            onConfirm={() => act(() => api.deleteGroup(group.id), `${group.name} deleted`).then((ok) => ok && onDeleted())}
          >
            <Button
              size="sm"
              variant="outline"
              className="tw:text-destructive"
              disabled={inRules}
              title={inRules ? 'Take the group out of its rules first' : undefined}
            >
              Delete
            </Button>
          </Confirm>
        </CardAction>
      </CardHeader>
      <CardContent className="tw:grid tw:grid-cols-1 tw:gap-6 tw:px-4 tw:text-sm tw:lg:grid-cols-2">
        <section>
          <strong className="tw:font-semibold">Members</strong>
          <div className="tw:text-muted-foreground">
            They hold what the group&apos;s rules grant, while they are in the group.
          </div>
          <List
            items={members}
            empty="No members yet."
            render={(m) => (
              <Row key={m.user}>
                <span>{memberName(m)}</span>
                <Confirm
                  title={`Remove ${memberName(m)} from ${group.name}?`}
                  onConfirm={() => act(
                    () => api.removeGroupMember(group.id, m.user),
                    `${memberName(m)} left ${group.name}`
                  )}
                >
                  <Button size="icon-sm" variant="ghost" className="tw:text-destructive" aria-label={`Remove ${memberName(m)}`}>
                    <X />
                  </Button>
                </Confirm>
              </Row>
            )}
          />
          <div className="tw:mt-2 tw:flex tw:items-center tw:gap-2">
            <RemoteSelect
              key={`user-${picks}`}
              size="small"
              placeholder="User"
              style={{ width: 220 }}
              value={user}
              onChange={(v, label) => {
                setUser(v);
                setUserName(label);
              }}
              fetchOptions={userOptions(api)}
            />
            <Button
              size="sm"
              variant="outline"
              disabled={!user}
              onClick={async () => {
                if (await act(() => api.addGroupMember(group.id, user!), `${userName ?? user} joined ${group.name}`)) {
                  setUser(undefined);
                  setPicks((n) => n + 1);
                }
              }}
            >
              Add member
            </Button>
          </div>
        </section>

        <section>
          <strong className="tw:font-semibold">Rules</strong>
          <div className="tw:text-muted-foreground">
            {`Rules of ${group.organizationName} the group's members are in.`}
          </div>
          <List
            items={rules}
            empty="In no rule yet."
            render={(r) => (
              <Row key={r.id}>
                <span className="tw:flex tw:min-w-0 tw:items-center tw:gap-2">
                  <span className="tw:truncate">{`${r.workspaceName} / ${r.description}`}</span>
                  <Tag>{r.roleName}</Tag>
                </span>
                <Confirm
                  title={`Take ${group.name} out of ${r.description}? Its members lose what the rule gives them, unless they are in it themselves.`}
                  onConfirm={() => act(
                    () => api.removeGroupRule(group.id, r.id),
                    `${group.name} is out of ${r.description}`
                  )}
                >
                  <Button size="icon-sm" variant="ghost" className="tw:text-destructive" aria-label={`Take out of ${r.description}`}>
                    <X />
                  </Button>
                </Confirm>
              </Row>
            )}
          />
          <div className="tw:mt-2 tw:flex tw:items-center tw:gap-2">
            <RemoteSelect
              key={`rule-${picks}`}
              size="small"
              placeholder="Rule"
              style={{ width: 260 }}
              value={rule}
              onChange={(v, label) => {
                setRule(v);
                setRuleName(label);
              }}
              fetchOptions={ruleOptions(api, group.organizationId)}
            />
            <Button
              size="sm"
              variant="outline"
              disabled={!rule}
              onClick={async () => {
                if (await act(() => api.addGroupRule(group.id, rule!), `${group.name} is in ${ruleName ?? rule}`)) {
                  setRule(undefined);
                  setPicks((n) => n + 1);
                }
              }}
            >
              Add to rule
            </Button>
          </div>
        </section>
      </CardContent>
    </Card>
  );
}

function List<T>({ items, empty, render }: {
  items: T[] | null;
  empty: string;
  render: (item: T) => ReactNode;
}) {
  if (items == null) {
    return <div className="tw:py-2"><Spinner /></div>;
  }
  if (!items.length) {
    return <div className="tw:py-2 tw:text-muted-foreground">{empty}</div>;
  }
  return <ul className="tw:m-0 tw:mt-2 tw:list-none tw:divide-y tw:divide-border tw:rounded-md tw:border tw:p-0">{items.map(render)}</ul>;
}

function Row({ children }: { children: ReactNode }) {
  return <li className="tw:flex tw:items-center tw:justify-between tw:gap-2 tw:px-3 tw:py-1.5">{children}</li>;
}

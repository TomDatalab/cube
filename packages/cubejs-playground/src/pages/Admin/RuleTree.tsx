import { DragEvent, useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronRight, CircleAlert, X } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import {
  Confirm, EmptyState, notify, SearchInput, Spinner, Tag,
} from '@/components/kit';
import { cn } from '@/lib/utils';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, memberName, PAGE_SIZE, Rule, RuleMember, WorkspaceListing,
} from './api';
import { USER_DRAG_NAME_TYPE, USER_DRAG_TYPE } from './MembershipTree';
import { groupByOrganization } from './membershipGroups';
import { useSearch } from './useSearch';

// The drag payload of a rule row: `{id, workspaceId, description}` as JSON.
// A custom type, so text dragged in from elsewhere is not taken for a rule.
export const RULE_DRAG_TYPE = 'application/x-cube-rule';

export type RuleDrag = Pick<Rule, 'id' | 'workspaceId' | 'description'>;

export function ruleDragData(rule: Rule): string {
  const payload: RuleDrag = { id: rule.id, workspaceId: rule.workspaceId, description: rule.description };
  return JSON.stringify(payload);
}

export function parseRuleDrag(data: string): RuleDrag | null {
  try {
    const parsed = JSON.parse(data);
    return typeof parsed?.id === 'string' && typeof parsed?.workspaceId === 'string' ? parsed : null;
  } catch {
    return null;
  }
}

type Props = {
  api: AdminApi;
  // Bumped by the page when rules changed elsewhere (created, edited,
  // deleted), so open workspaces load theirs again.
  version?: number;
  // Called after a drop here changed a rule (moved it).
  onChanged?: () => void;
};

const dropClass = (highlighted: boolean) => cn(
  'tw:rounded tw:border tw:border-dashed',
  highlighted ? 'tw:border-primary tw:bg-primary/10' : 'tw:border-transparent'
);

const toggleClass = 'tw:flex tw:cursor-pointer tw:items-center tw:gap-1 tw:select-none';

function count(n: number, one: string, many: string) {
  return n === 1 ? `(1 ${one})` : `(${n} ${many})`;
}

// Organization → workspace → rule → members, one searchable page of
// workspaces at a time. A rule dropped on a workspace moves there with its
// members; a user dropped on a rule joins it. Members leave from here.
export function RuleTree({ api, version = 0, onChanged }: Props) {
  const isMounted = useIsMounted();
  const {
    q, setQ, page, setPage, result, loading, error,
  } = useSearch((term, p) => api.searchWorkspaces({ q: term, page: p }), []);
  const [rules, setRules] = useState<Record<string, Rule[]>>({});
  const [members, setMembers] = useState<Record<string, RuleMember[]>>({});
  const [openWorkspaces, setOpenWorkspaces] = useState<Set<string>>(new Set());
  const [openRules, setOpenRules] = useState<Set<string>>(new Set());
  // `ws:<id>` or `rule:<id>`: what a drag hovers.
  const [target, setTarget] = useState<string | null>(null);
  const openRef = useRef({ workspaces: openWorkspaces, rules: openRules });
  openRef.current = { workspaces: openWorkspaces, rules: openRules };

  async function loadRules(ws: string): Promise<Rule[] | null> {
    try {
      const list = await api.rulesOf(ws);
      if (isMounted()) {
        setRules((r) => ({ ...r, [ws]: list }));
      }
      return list;
    } catch (e: any) {
      notify.error(e.message);
      return null;
    }
  }

  async function loadMembers(rule: string): Promise<RuleMember[] | null> {
    try {
      const list = await api.ruleMembers(rule);
      if (isMounted()) {
        setMembers((m) => ({ ...m, [rule]: list }));
      }
      return list;
    } catch (e: any) {
      notify.error(e.message);
      return null;
    }
  }

  // Rules and members are kept until the next search or outside change;
  // what stays open loads again.
  useEffect(() => {
    setRules({});
    setMembers({});
    const { workspaces, rules: open } = openRef.current;
    (result?.items || []).forEach((ws) => {
      if (workspaces.has(ws.id)) {
        loadRules(ws.id);
      }
    });
    open.forEach((rule) => loadMembers(rule));
  }, [result, version]);

  function toggleWorkspace(ws: string) {
    const next = new Set(openWorkspaces);
    if (next.has(ws)) {
      next.delete(ws);
    } else {
      next.add(ws);
      if (!rules[ws]) {
        loadRules(ws);
      }
    }
    setOpenWorkspaces(next);
  }

  function toggleRule(rule: string) {
    const next = new Set(openRules);
    if (next.has(rule)) {
      next.delete(rule);
    } else {
      next.add(rule);
      if (!members[rule]) {
        loadMembers(rule);
      }
    }
    setOpenRules(next);
  }

  function accept(e: DragEvent, type: string, key: string) {
    if (e.dataTransfer.types.includes(type)) {
      e.preventDefault();
      e.stopPropagation();
      e.dataTransfer.dropEffect = type === RULE_DRAG_TYPE ? 'move' : 'copy';
      setTarget(key);
    }
  }

  function leave(key: string) {
    setTarget((t) => (t === key ? null : t));
  }

  async function dropRule(e: DragEvent, ws: WorkspaceListing) {
    e.preventDefault();
    setTarget(null);
    const rule = parseRuleDrag(e.dataTransfer.getData(RULE_DRAG_TYPE));
    if (!rule) {
      return;
    }
    setOpenWorkspaces(new Set(openWorkspaces).add(ws.id));
    if (rule.workspaceId === ws.id) {
      notify.info(`${rule.description} is already in ${ws.name}`);
      if (!rules[ws.id]) {
        loadRules(ws.id);
      }
      return;
    }
    try {
      await api.updateRule(rule.id, { workspaceId: ws.id });
      notify.success(`${rule.description} moved to ${ws.name}, with its members`);
      onChanged?.();
    } catch (err: any) {
      notify.error(err.message);
    }
    await Promise.all([loadRules(ws.id), rules[rule.workspaceId] ? loadRules(rule.workspaceId) : null]);
  }

  async function dropUser(e: DragEvent, rule: Rule) {
    e.preventDefault();
    e.stopPropagation();
    setTarget(null);
    const user = e.dataTransfer.getData(USER_DRAG_TYPE);
    if (!user) {
      return;
    }
    const username = e.dataTransfer.getData(USER_DRAG_NAME_TYPE) || user;
    setOpenRules(new Set(openRules).add(rule.id));
    const current = members[rule.id] || await loadMembers(rule.id);
    if (current?.some((m) => m.user === user)) {
      notify.info(`${username} is already in ${rule.description}`);
      return;
    }
    if (!current) {
      return;
    }
    try {
      await api.addRuleMember(rule.id, user);
      notify.success(`${username} added to ${rule.description} (${rule.roleName} in ${rule.workspaceName})`);
    } catch (err: any) {
      notify.error(err.message);
    }
    await loadMembers(rule.id);
  }

  async function removeMember(rule: Rule, member: RuleMember) {
    const { user } = member;
    try {
      await api.removeRuleMember(rule.id, user);
      notify.success(`${memberName(member)} removed from ${rule.description}`);
    } catch (err: any) {
      notify.error(err.message);
    }
    await loadMembers(rule.id);
  }

  function renderRule(rule: Rule) {
    const key = `rule:${rule.id}`;
    const list = members[rule.id];
    const expanded = openRules.has(rule.id);
    return (
      <div
        key={rule.id}
        data-rule={rule.id}
        onDragOver={(e) => accept(e, USER_DRAG_TYPE, key)}
        onDragLeave={() => leave(key)}
        onDrop={(e) => dropUser(e, rule)}
        className={cn(dropClass(target === key), 'tw:my-0.5 tw:px-1.5 tw:py-0.5')}
      >
        <div
          role="button"
          tabIndex={0}
          onClick={() => toggleRule(rule.id)}
          onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && toggleRule(rule.id)}
          className={cn(toggleClass, 'tw:flex-wrap')}
        >
          {expanded ? <ChevronDown className="tw:size-4" /> : <ChevronRight className="tw:size-4" />}
          <Tag className="tw:mr-1" title={rule.permissions.join(', ')}>{rule.roleName}</Tag>
          <span>{rule.description}</span>
          <span className="tw:text-muted-foreground">{`· ${rule.type}`}</span>
          {list ? <span className="tw:text-muted-foreground">{count(list.length, 'member', 'members')}</span> : null}
        </div>
        {expanded ? (
          <div className="tw:pl-5">
            {list?.map((member) => (
              <div key={member.user} className="tw:mt-1 tw:flex tw:items-center tw:gap-2">
                <span className="tw:min-w-[120px]">{memberName(member)}</span>
                <Confirm
                  title={`Remove ${memberName(member)} from ${rule.description}?`}
                  onConfirm={() => removeMember(rule, member)}
                >
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    className="tw:text-destructive"
                    aria-label={`Remove ${memberName(member)} from ${rule.description}`}
                  >
                    <X />
                  </Button>
                </Confirm>
              </div>
            ))}
            {list && !list.length ? <span className="tw:text-muted-foreground">Drag users here</span> : null}
            {list ? null : <Spinner />}
          </div>
        ) : null}
      </div>
    );
  }

  const groups = groupByOrganization(result?.items || []);
  const pages = Math.max(1, Math.ceil((result?.total || 0) / PAGE_SIZE));

  return (
    <div>
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
      {result == null && !error ? <Spinner className="tw:size-5" /> : null}
      {result && !groups.length && !loading ? (
        <EmptyState>{q ? 'No workspaces match' : 'No workspaces yet: create them in the Workspaces tab'}</EmptyState>
      ) : null}
      {groups.map((group) => (
        <div key={group.organizationId} className="tw:mb-4 tw:text-sm">
          <span className="tw:font-semibold">{group.organizationName}</span>
          {group.workspaces.map((ws) => {
            const key = `ws:${ws.id}`;
            const list = rules[ws.id];
            const expanded = openWorkspaces.has(ws.id);
            return (
              <div
                key={ws.id}
                data-workspace={ws.id}
                onDragOver={(e) => accept(e, RULE_DRAG_TYPE, key)}
                onDragLeave={() => leave(key)}
                onDrop={(e) => dropRule(e, ws)}
                className={cn(dropClass(target === key), 'tw:my-1 tw:ml-4 tw:px-2 tw:py-1')}
              >
                <div
                  role="button"
                  tabIndex={0}
                  onClick={() => toggleWorkspace(ws.id)}
                  onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && toggleWorkspace(ws.id)}
                  className={toggleClass}
                >
                  {expanded ? <ChevronDown className="tw:size-4" /> : <ChevronRight className="tw:size-4" />}
                  <span>{ws.name}</span>
                  {list ? <span className="tw:text-muted-foreground">{count(list.length, 'rule', 'rules')}</span> : null}
                </div>
                {expanded ? (
                  <div className="tw:pl-4">
                    {list?.map(renderRule)}
                    {list && !list.length ? (
                      <span className="tw:text-muted-foreground">No rules: drag one here</span>
                    ) : null}
                    {list ? null : <Spinner />}
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      ))}
      {result && result.total > PAGE_SIZE ? (
        <div className="tw:flex tw:items-center tw:gap-2">
          <Button size="sm" variant="outline" disabled={page <= 1 || loading} onClick={() => setPage(page - 1)}>
            Previous
          </Button>
          <span className="tw:text-sm tw:text-muted-foreground">{`page ${page} of ${pages}`}</span>
          <Button size="sm" variant="outline" disabled={page >= pages || loading} onClick={() => setPage(page + 1)}>
            Next
          </Button>
        </div>
      ) : null}
    </div>
  );
}

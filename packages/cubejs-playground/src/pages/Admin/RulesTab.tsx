import {
  FormEvent, ReactNode, useMemo, useRef, useState,
} from 'react';
import {
  CircleAlert, GripVertical, Pencil, Trash2,
} from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@/components/ui/select';
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@/components/ui/table';
import { Textarea } from '@/components/ui/textarea';
import {
  Tooltip, TooltipContent, TooltipProvider, TooltipTrigger,
} from '@/components/ui/tooltip';
import {
  Confirm, EmptyState, Field, FormDialog, notify, Pager, SearchInput, Spinner, Tag,
} from '@/components/kit';

import { useIsMounted } from '../../hooks';
import { IdText } from './IdText';
import {
  AdminApi, OrgRole, PAGE_SIZE, Rule,
} from './api';
import { organizationOptions, RemoteSelect, workspaceOptions } from './RemoteSelect';
import { RULE_DRAG_TYPE, ruleDragData, RuleTree } from './RuleTree';
import { useSearch } from './useSearch';

type Props = {
  api: AdminApi;
};

// `MAX_DESCRIPTION` and `MAX_TYPE` in `cubeserver/src/rbac/handlers/rules.rs`.
const MAX_DESCRIPTION = 500;
const MAX_TYPE = 100;

// The form's role: the id of one of the organization's roles.
type FormValues = { description: string; type: string; role?: string; workspaceId?: string };

type Missing = Partial<Record<keyof FormValues, string>>;

const EMPTY_FORM: FormValues = { description: '', type: '' };

function Hint({ children }: { children: ReactNode }) {
  return <p className="tw:text-sm tw:text-muted-foreground">{children}</p>;
}

// Every rule (a named role in one workspace) of every organization. Rows
// drag onto a workspace of the tree to move there; users dragged from the
// Users section onto a rule join it.
export function RulesTab({ api }: Props) {
  const [organization, setOrganization] = useState<string | null>(null);
  const {
    q, setQ, page, setPage, result, loading, error: searchError, reload,
  } = useSearch(
    (term, p) => api.searchRules({ q: term, organizationId: organization, page: p }),
    [api, organization]
  );
  // `true`: a new rule; a rule: editing it.
  const [editing, setEditing] = useState<Rule | true | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Bumped when the table changed rules, so the tree loads them again.
  const [treeVersion, setTreeVersion] = useState(0);
  const [values, setValues] = useState<FormValues>(EMPTY_FORM);
  // The role as it is now, for a roles load that answers later.
  const roleRef = useRef<string | undefined>();
  roleRef.current = values.role;
  const [missing, setMissing] = useState<Missing>({});
  const isMounted = useIsMounted();
  // The roles of the organization of the workspace picked in the form.
  const [orgRoles, setOrgRoles] = useState<OrgRole[]>([]);
  const [rolesLoaded, setRolesLoaded] = useState(false);
  const rolesRequest = useRef(0);

  function set(patch: Partial<FormValues>) {
    if ('role' in patch) {
      roleRef.current = patch.role;
    }
    setValues((v) => ({ ...v, ...patch }));
    setMissing((m) => {
      const next = { ...m };
      Object.keys(patch).forEach((k) => {
        delete next[k as keyof FormValues];
      });
      return next;
    });
  }

  async function loadRoles(org: string | null, isNew = false) {
    const request = ++rolesRequest.current;
    setOrgRoles([]);
    setRolesLoaded(false);
    if (!org) {
      return;
    }
    try {
      const loaded = await api.roles(org);
      if (isMounted() && request === rolesRequest.current) {
        setOrgRoles(loaded);
        // A role of another organization does not carry over.
        const role = roleRef.current;
        if (!role || !loaded.some((r) => r.id === role)) {
          // A new rule starts as the organization's default viewer, if it has
          // one. A role the organization lacks is cleared, to be picked again.
          const next = isNew ? loaded.find((r) => r.builtin === 'viewer')?.id : undefined;
          roleRef.current = next;
          setValues((v) => ({ ...v, role: next }));
        }
      }
    } catch (e: any) {
      notify.error(e.message);
    } finally {
      // Also after a failure, so the Select stops spinning on an empty list.
      if (isMounted() && request === rolesRequest.current) {
        setRolesLoaded(true);
      }
    }
  }

  async function workspaceChanged(ws: string | undefined, isNew: boolean) {
    if (!ws) {
      loadRoles(null);
      return;
    }
    try {
      loadRoles((await api.workspace(ws)).organizationId, isNew);
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  // Until the roles load, an edited rule shows its own role by name, not id.
  const roleOptions = !rolesLoaded && editing && editing !== true && editing.roleId
    ? [{ value: editing.roleId, label: editing.roleName }]
    : orgRoles.map((r) => ({ value: r.id, label: `${r.name} — ${r.permissions.join(', ')}` }));
  // The types already in use on this page, offered while typing a new one.
  const typeOptions = useMemo(
    () => [...new Set((result?.items || []).map((r) => r.type))].sort(),
    [result]
  );

  function changed() {
    reload();
    setTreeVersion((v) => v + 1);
  }

  function open(rule: Rule | true) {
    const next = rule === true
      ? EMPTY_FORM
      : {
        description: rule.description,
        type: rule.type,
        role: rule.roleId ?? undefined,
        workspaceId: rule.workspaceId,
      };
    roleRef.current = next.role;
    setValues(next);
    setMissing({});
    loadRoles(rule === true ? null : rule.organizationId);
    setError(null);
    setEditing(rule);
  }

  async function save(event?: FormEvent) {
    event?.preventDefault();
    const lacking: Missing = {};
    if (!values.description.trim()) {
      lacking.description = 'Describe the rule';
    } else if (values.description.length > MAX_DESCRIPTION) {
      lacking.description = `At most ${MAX_DESCRIPTION} characters`;
    }
    if (!values.type.trim()) {
      lacking.type = 'Type a type';
    } else if (values.type.length > MAX_TYPE) {
      lacking.type = `At most ${MAX_TYPE} characters`;
    }
    if (!values.workspaceId) {
      lacking.workspaceId = 'Pick a workspace';
    }
    if (!values.role) {
      lacking.role = 'Pick a role';
    }
    setMissing(lacking);
    if (Object.keys(lacking).length) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const description = values.description.trim();
      const type = values.type.trim();
      if (editing === true) {
        await api.createRule(values.workspaceId!, { description, type, roleId: values.role! });
      } else if (editing) {
        await api.updateRule(editing.id, {
          description,
          type,
          roleId: values.role,
          workspaceId: values.workspaceId,
        });
      }
      notify.success(`${description} saved`);
      setEditing(null);
      changed();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  async function remove(rule: Rule) {
    try {
      await api.deleteRule(rule.id);
      notify.success(`${rule.description} deleted`);
      changed();
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  const rows = result?.items || [];
  const rolesLoading = !rolesLoaded && editing !== true;

  return (
    <TooltipProvider delayDuration={300}>
      <div className="tw:grid tw:grid-cols-1 tw:gap-6 tw:xl:grid-cols-[minmax(0,1fr)_320px]">
        <div className="tw:min-w-0">
          <div className="tw:mb-4 tw:flex tw:items-center tw:gap-2">
            <RemoteSelect
              allowClear
              placeholder="All organizations"
              style={{ width: 240 }}
              value={organization ?? undefined}
              onChange={(v) => setOrganization(v || null)}
              fetchOptions={organizationOptions(api)}
            />
            <Button onClick={() => open(true)}>
              New rule
            </Button>
          </div>

          <SearchInput
            className="tw:mb-4"
            placeholder="Search rules by description, type, role, workspace or organization"
            value={q}
            onChange={setQ}
          />
          {searchError ? (
            <Alert variant="destructive" className="tw:mb-4">
              <CircleAlert />
              <AlertDescription>{searchError}</AlertDescription>
            </Alert>
          ) : null}

          <div className="tw:relative">
            <div className="tw:rounded-md tw:border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead style={{ width: 32 }} />
                    <TableHead>Description</TableHead>
                    <TableHead>Id</TableHead>
                    <TableHead>Type</TableHead>
                    <TableHead>Role</TableHead>
                    <TableHead>Workspace</TableHead>
                    <TableHead>Organization</TableHead>
                    <TableHead className="tw:sticky tw:right-0 tw:bg-background" />
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={8}>
                        <EmptyState className="tw:py-6">{loading ? <Spinner className="tw:mx-auto" /> : 'No data'}</EmptyState>
                      </TableCell>
                    </TableRow>
                  ) : rows.map((rule) => (
                    <TableRow
                      key={rule.id}
                      // A rule drags onto a workspace of the tree to move there.
                      draggable
                      onDragStart={(e) => {
                        e.dataTransfer.setData(RULE_DRAG_TYPE, ruleDragData(rule));
                        e.dataTransfer.effectAllowed = 'move';
                      }}
                      className="tw:cursor-grab"
                    >
                      <TableCell>
                        <span title="Drag onto a workspace" className="tw:inline-flex tw:text-muted-foreground">
                          <GripVertical className="tw:size-4" />
                        </span>
                      </TableCell>
                      <TableCell>
                        <span title={rule.description} className="tw:block tw:max-w-[240px] tw:truncate">
                          {rule.description}
                        </span>
                      </TableCell>
                      <TableCell><IdText id={rule.id} /></TableCell>
                      <TableCell>{rule.type}</TableCell>
                      <TableCell>
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <span className="tw:inline-flex"><Tag>{rule.roleName}</Tag></span>
                          </TooltipTrigger>
                          <TooltipContent>{rule.permissions.join(', ')}</TooltipContent>
                        </Tooltip>
                      </TableCell>
                      <TableCell>{rule.workspaceName}</TableCell>
                      <TableCell>{rule.organizationName}</TableCell>
                      <TableCell className="tw:sticky tw:right-0 tw:bg-background tw:text-right">
                        <span className="tw:inline-flex tw:items-center tw:gap-1">
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button
                                size="icon-sm"
                                variant="outline"
                                aria-label={`Edit ${rule.description}`}
                                onClick={() => open(rule)}
                              >
                                <Pencil />
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent>Edit</TooltipContent>
                          </Tooltip>
                          <Tooltip>
                            <Confirm
                              title={`Delete ${rule.description}? Its members lose the ${rule.roleName} role it gives.`}
                              onConfirm={() => remove(rule)}
                            >
                              <TooltipTrigger asChild>
                                <Button
                                  size="icon-sm"
                                  variant="outline"
                                  className="tw:text-destructive"
                                  aria-label={`Delete ${rule.description}`}
                                >
                                  <Trash2 />
                                </Button>
                              </TooltipTrigger>
                            </Confirm>
                            <TooltipContent>Delete</TooltipContent>
                          </Tooltip>
                        </span>
                      </TableCell>
                    </TableRow>
                  ))}
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

          <FormDialog
            open={editing != null}
            title={editing === true ? 'New rule' : 'Edit rule'}
            onClose={() => setEditing(null)}
            onOk={() => save()}
            okText="Save"
            saving={saving}
          >
            <form className="tw:flex tw:flex-col tw:gap-4" onSubmit={save} noValidate>
              <Field label="Description" htmlFor="rule-description" error={missing.description}>
                <Textarea
                  id="rule-description"
                  className="tw:max-h-40"
                  rows={2}
                  maxLength={MAX_DESCRIPTION}
                  value={values.description}
                  aria-invalid={Boolean(missing.description)}
                  onChange={(e) => set({ description: e.target.value })}
                />
                <span className="tw:self-end tw:text-xs tw:text-muted-foreground">
                  {`${values.description.length} / ${MAX_DESCRIPTION}`}
                </span>
              </Field>
              <Field label="Type" htmlFor="rule-type" error={missing.type}>
                <Input
                  id="rule-type"
                  list="rule-type-options"
                  maxLength={MAX_TYPE}
                  autoComplete="off"
                  value={values.type}
                  aria-invalid={Boolean(missing.type)}
                  onChange={(e) => set({ type: e.target.value })}
                />
                <datalist id="rule-type-options">
                  {typeOptions.map((t) => <option key={t} value={t}>{t}</option>)}
                </datalist>
                <Hint>Any label, e.g. finance or audit.</Hint>
              </Field>
              <Field label="Workspace" htmlFor="rule-workspace" error={missing.workspaceId}>
                <RemoteSelect
                  id="rule-workspace"
                  placeholder="Organization / workspace"
                  fetchOptions={workspaceOptions(api)}
                  value={values.workspaceId}
                  invalid={Boolean(missing.workspaceId)}
                  onChange={(v) => {
                    set({ workspaceId: v });
                    workspaceChanged(v, editing === true);
                  }}
                  valueLabel={editing && editing !== true
                    ? `${editing.organizationName} / ${editing.workspaceName}`
                    : undefined}
                />
                <Hint>The organization is the workspace&apos;s.</Hint>
              </Field>
              <Field label="Role" htmlFor="rule-role" error={missing.role}>
                <Select value={values.role ?? ''} onValueChange={(v) => set({ role: v })}>
                  <SelectTrigger id="rule-role" className="tw:w-full" aria-invalid={Boolean(missing.role)}>
                    <SelectValue />
                    {rolesLoading ? <Spinner className="tw:ml-auto" /> : null}
                  </SelectTrigger>
                  <SelectContent>
                    {roleOptions.map((o) => (
                      <SelectItem key={o.value} value={o.value}>{o.label}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Hint>
                  What the rule&apos;s members can do in the workspace: one of the organization&apos;s roles
                  (Admin → Roles).
                </Hint>
              </Field>
              {error ? (
                <Alert variant="destructive">
                  <CircleAlert />
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              ) : null}
              {/* Enter in the type submits. */}
              <button type="submit" hidden aria-hidden tabIndex={-1} />
            </form>
          </FormDialog>
        </div>
        <div className="tw:min-w-0">
          <h5 className="tw:mb-2 tw:text-sm tw:font-semibold">Workspaces</h5>
          <p className="tw:mb-4 tw:text-sm tw:text-muted-foreground">
            Drag a rule onto a workspace to move it there, with its members. Users join a rule by
            being dragged onto it in Users.
          </p>
          <RuleTree api={api} version={treeVersion} onChanged={reload} />
        </div>
      </div>
    </TooltipProvider>
  );
}

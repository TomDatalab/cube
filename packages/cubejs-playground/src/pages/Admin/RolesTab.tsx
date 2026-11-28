import { FormEvent, ReactElement, useEffect, useState } from 'react';
import {
  CircleAlert, Info, Pencil, Trash2, TriangleAlert,
} from 'lucide-react';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import {
  Tooltip, TooltipContent, TooltipProvider, TooltipTrigger,
} from '@/components/ui/tooltip';
import {
  Column, Confirm, DataTable, EmptyState, Field, FormDialog, notify, Tag,
} from '@/components/kit';

import { useIsMounted } from '../../hooks';
import { IdText } from './IdText';
import {
  AdminApi, ModelCube, OrgRole, Permission, PERMISSION_LABELS, PERMISSIONS, restrictionSummary, withImplied,
} from './api';
import { organizationOptions, RemoteSelect } from './RemoteSelect';
import {
  FilterErrors, hasRestrictionErrors, restrictionErrors, RestrictionsFields, RestrictionsFormValues,
  toFormRestrictions, toRestrictions,
} from './RestrictionsFields';
import { useLatestCall } from './useLatestCall';

type Props = {
  api: AdminApi;
};

// `MAX_NAME` in `cubeserver/src/rbac/handlers/roles.rs`.
const MAX_NAME = 100;

type FormValues = { name: string; permissions: Permission[]; restrictions: RestrictionsFormValues };

const EMPTY_FORM: FormValues = { name: '', permissions: [], restrictions: { rowFilters: [], blockedMembers: [] } };

function PermissionTags({ permissions }: { permissions: readonly string[] }) {
  return (
    <span className="tw:flex tw:flex-wrap tw:gap-1">
      {permissions.map((p) => <Tag key={p}>{p}</Tag>)}
    </span>
  );
}

// A button with a tooltip. A disabled one goes in a span, which still gets
// the pointer events.
function WithTooltip({ title, children }: { title: string; children: ReactElement }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent>{title}</TooltipContent>
    </Tooltip>
  );
}

// The roles of one organization: a name and the workspace permissions its
// rules give their members. Editing a role changes the access of every rule
// that uses it.
export function RolesTab({ api }: Props) {
  const isMounted = useIsMounted();
  const [organization, setOrganization] = useState<string | null>(null);
  const [roles, setRoles] = useState<OrgRole[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  // `true`: a new role; a role: editing it.
  const [editing, setEditing] = useState<OrgRole | true | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cubes, setCubes] = useState<ModelCube[]>([]);
  const [cubesError, setCubesError] = useState<string | null>(null);
  const [values, setValues] = useState<FormValues>(EMPTY_FORM);
  // Shown after a submit, cleared as their field changes.
  const [missing, setMissing] = useState<{ name?: string; permissions?: string }>({});
  const [filterErrors, setFilterErrors] = useState<FilterErrors[]>([]);
  const beginMembers = useLatestCall();

  async function load(org: string) {
    setLoading(true);
    setLoadError(null);
    try {
      const loaded = await api.roles(org);
      if (isMounted()) {
        setRoles(loaded);
      }
    } catch (e: any) {
      if (isMounted()) {
        setLoadError(e.message);
      }
    } finally {
      if (isMounted()) {
        setLoading(false);
      }
    }
  }

  useEffect(() => {
    setRoles(null);
    // The members of the organization left behind are not this one's.
    beginMembers();
    setCubes([]);
    if (organization) {
      load(organization);
    }
  }, [api, organization]);

  // The members the restrictions can name; the form stays usable without them.
  // Only the latest load counts: one for another organization may answer
  // after it.
  async function loadMembers(org: string) {
    const current = beginMembers();
    setCubes([]);
    setCubesError(null);
    try {
      const loaded = await api.modelMembers(org);
      if (isMounted() && current()) {
        setCubes(loaded);
      }
    } catch (e: any) {
      if (isMounted() && current()) {
        setCubesError(e.message);
      }
    }
  }

  function open(role: OrgRole | true) {
    setValues(role === true
      ? { ...EMPTY_FORM, permissions: ['view'] }
      : {
        name: role.name,
        permissions: role.permissions,
        restrictions: toFormRestrictions(role.restrictions),
      });
    setMissing({});
    setFilterErrors([]);
    setError(null);
    setEditing(role);
    loadMembers(organization!);
  }

  async function save(event?: FormEvent) {
    event?.preventDefault();
    let nameError: string | undefined;
    if (!values.name.trim()) {
      nameError = 'Name the role';
    } else if (values.name.length > MAX_NAME) {
      nameError = `At most ${MAX_NAME} characters`;
    }
    const lacking = {
      name: nameError,
      permissions: values.permissions.length ? undefined : 'Pick at least one permission',
    };
    const filters = restrictionErrors(values.restrictions);
    setMissing(lacking);
    setFilterErrors(filters);
    if (lacking.name || lacking.permissions || hasRestrictionErrors(filters)) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const role = {
        name: values.name.trim(),
        permissions: withImplied(values.permissions),
        restrictions: toRestrictions(values.restrictions),
      };
      if (editing === true) {
        await api.createRole(organization!, role);
      } else if (editing) {
        await api.updateRole(editing.id, role);
      }
      notify.success(`${role.name} saved`);
      setEditing(null);
      load(organization!);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  async function remove(role: OrgRole) {
    try {
      await api.deleteRole(role.id);
      notify.success(`${role.name} deleted`);
      load(organization!);
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  // Checking a permission checks what it implies.
  function togglePermission(p: Permission, checked: boolean) {
    const chosen = checked ? [...values.permissions, p] : values.permissions.filter((x) => x !== p);
    setValues((v) => ({ ...v, permissions: withImplied(chosen) }));
    setMissing((m) => ({ ...m, permissions: undefined }));
  }

  const editingRules = editing && editing !== true ? editing.rules : 0;

  const columns: Column<OrgRole>[] = [
    {
      key: 'name',
      title: 'Role',
      dataIndex: 'name',
      render: (v: string, role) => (
        <span className="tw:inline-flex tw:items-center tw:gap-1">
          {v}
          {role.builtin ? <Tag color="blue">default</Tag> : null}
        </span>
      ),
    },
    { key: 'id', title: 'Id', dataIndex: 'id', render: (v) => <IdText id={v} /> },
    {
      key: 'permissions',
      title: 'Permissions',
      dataIndex: 'permissions',
      render: (permissions: Permission[]) => <PermissionTags permissions={permissions} />,
    },
    {
      key: 'restrictions',
      title: 'Restrictions',
      dataIndex: 'restrictions',
      render: (r: OrgRole['restrictions']) => restrictionSummary(r) || <span className="tw:text-muted-foreground">None</span>,
    },
    {
      key: 'rules',
      title: 'Rules',
      dataIndex: 'rules',
      render: (n: number) => (n === 1 ? '1 rule' : `${n} rules`),
    },
    {
      key: 'actions',
      title: '',
      align: 'right',
      render: (_, role) => (
        <span className="tw:inline-flex tw:items-center tw:gap-1">
          <WithTooltip title="Edit">
            <Button
              size="icon-sm"
              variant="outline"
              aria-label={`Edit ${role.name}`}
              onClick={() => open(role)}
            >
              <Pencil />
            </Button>
          </WithTooltip>
          {role.rules ? (
            <WithTooltip title="In use: give its rules another role first">
              <span className="tw:inline-flex">
                <Button
                  size="icon-sm"
                  variant="outline"
                  className="tw:text-destructive"
                  disabled
                  aria-label={`Delete ${role.name}`}
                >
                  <Trash2 />
                </Button>
              </span>
            </WithTooltip>
          ) : (
            <Tooltip>
              <Confirm title={`Delete ${role.name}?`} onConfirm={() => remove(role)}>
                <TooltipTrigger asChild>
                  <Button
                    size="icon-sm"
                    variant="outline"
                    className="tw:text-destructive"
                    aria-label={`Delete ${role.name}`}
                  >
                    <Trash2 />
                  </Button>
                </TooltipTrigger>
              </Confirm>
              <TooltipContent>Delete</TooltipContent>
            </Tooltip>
          )}
        </span>
      ),
    },
  ];

  return (
    <TooltipProvider delayDuration={300}>
      <div className="tw:mb-4 tw:flex tw:items-center tw:gap-2">
        <RemoteSelect
          placeholder="Pick an organization"
          style={{ width: 240 }}
          value={organization ?? undefined}
          onChange={(v) => setOrganization(v || null)}
          fetchOptions={organizationOptions(api)}
        />
        <Button disabled={!organization} onClick={() => open(true)}>
          New role
        </Button>
      </div>

      {loadError ? (
        <Alert variant="destructive" className="tw:mb-4">
          <CircleAlert />
          <AlertDescription>{loadError}</AlertDescription>
        </Alert>
      ) : null}

      {organization ? (
        <DataTable<OrgRole>
          rowKey="id"
          loading={loading}
          rows={roles || []}
          emptyText="No roles"
          columns={columns}
        />
      ) : (
        <EmptyState>Pick an organization to see its roles</EmptyState>
      )}

      <FormDialog
        open={editing != null}
        title={editing === true ? 'New role' : 'Edit role'}
        onClose={() => setEditing(null)}
        onOk={() => save()}
        okText="Save"
        saving={saving}
        className="tw:sm:max-w-[640px]"
      >
        <form className="tw:flex tw:flex-col tw:gap-4" onSubmit={save} noValidate>
          <Field label="Name" htmlFor="role-name" error={missing.name}>
            <Input
              id="role-name"
              maxLength={MAX_NAME}
              autoComplete="off"
              value={values.name}
              aria-invalid={Boolean(missing.name)}
              onChange={(e) => {
                const name = e.target.value;
                setValues((v) => ({ ...v, name }));
                setMissing((m) => ({ ...m, name: undefined }));
              }}
            />
          </Field>
          <div className="tw:flex tw:flex-col tw:gap-2">
            <Label>Permissions in the rule&apos;s workspace</Label>
            <div className="tw:flex tw:flex-col tw:gap-1.5">
              {PERMISSIONS.map((p) => (
                <div key={p} className="tw:flex tw:items-center tw:gap-2">
                  <Checkbox
                    id={`role-permission-${p}`}
                    checked={values.permissions.includes(p)}
                    aria-invalid={Boolean(missing.permissions)}
                    onCheckedChange={(checked) => togglePermission(p, checked === true)}
                  />
                  <Label htmlFor={`role-permission-${p}`} className="tw:font-normal">
                    {PERMISSION_LABELS[p]}
                    <span className="tw:text-muted-foreground">{`(${p})`}</span>
                  </Label>
                </div>
              ))}
            </div>
            <p className="tw:text-sm tw:text-muted-foreground">
              Manage implies every permission; writing objects implies reading them; any implies viewing.
            </p>
            {missing.permissions ? <p className="tw:text-sm tw:text-destructive">{missing.permissions}</p> : null}
          </div>
          {cubesError ? (
            <Alert>
              <TriangleAlert />
              <AlertTitle className="tw:line-clamp-none">Could not load the model members, so the pickers below are empty</AlertTitle>
              <AlertDescription>{cubesError}</AlertDescription>
            </Alert>
          ) : null}
          <div>
            <RestrictionsFields
              cubes={cubes}
              value={values.restrictions}
              onChange={(restrictions) => setValues((v) => ({ ...v, restrictions }))}
              errors={filterErrors}
              onErrorsChange={setFilterErrors}
            />
          </div>
          {editingRules ? (
            <Alert>
              <Info />
              <AlertDescription>
                {`${editingRules === 1 ? '1 rule uses' : `${editingRules} rules use`} this role: their members get the new permissions at once.`}
              </AlertDescription>
            </Alert>
          ) : null}
          {error ? (
            <Alert variant="destructive">
              <CircleAlert />
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          {/* Enter in the name submits. */}
          <button type="submit" hidden aria-hidden tabIndex={-1} />
        </form>
      </FormDialog>
    </TooltipProvider>
  );
}

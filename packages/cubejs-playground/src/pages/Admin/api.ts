import { AccessGraph } from './access/types';
import { authHeaders, onUnauthorized } from '../../shared/auth';

// The RBAC management API (`/v1/users`, `/v1/organizations`, ...). These
// routes are not scoped to a workspace, so no `X-Cube-Workspace` goes out.

export type User = {
  id: string;
  name: string | null;
  email: string | null;
  disabled: boolean;
  createdAt: string;
};

export type Organization = {
  id: string;
  name: string;
  createdAt: string;
  createdBy: string | null;
};

export type Workspace = {
  id: string;
  organizationId: string;
  name: string;
  createdAt: string;
  createdBy: string | null;
};

export const PAGE_SIZE = 50;

// One page of a search, with the total of matches across all pages.
export type Page<T> = { items: T[]; total: number };

export type WorkspaceListing = Workspace & { organizationName: string };

// A connection and the organization that holds it, as the platform admin sees
// every one of them.
export type TrackedConnection = { name: string; organizationId: string; origin: 'created' | 'assigned' };

export const ROLES = ['owner', 'editor', 'viewer'] as const;
export type Role = typeof ROLES[number];

export type Member = { user: string; role: Role };

// The workspace permissions an organization's role can grant, in order.
export const PERMISSIONS = [
  'view', 'query', 'read_objects', 'write_objects', 'execute', 'manage',
] as const;
export type Permission = typeof PERMISSIONS[number];

export const PERMISSION_LABELS: Record<Permission, string> = {
  view: 'View the workspace',
  query: 'Query data',
  read_objects: 'Read objects',
  write_objects: 'Write objects',
  execute: 'Execute',
  manage: 'Manage (members, rules, the workspace)',
};

// What a permission implies, as the server adds it: `manage` implies all,
// `write_objects` implies `read_objects`, any implies `view`.
export function withImplied(chosen: readonly string[]): Permission[] {
  const has = (p: string) => chosen.includes(p);
  const manage = has('manage');
  return PERMISSIONS.filter((p) => {
    if (p === 'view') return chosen.length > 0;
    if (p === 'read_objects') return manage || has('read_objects') || has('write_objects');
    return manage || has(p);
  });
}

// A role an organization defines, with how many rules use it.
export type OrgRole = {
  id: string;
  organizationId: string;
  name: string;
  permissions: Permission[];
  rules: number;
  createdAt: string;
  createdBy: string | null;
};

// A named role in one workspace: its members hold a built-in `role`, or the
// organization's role `roleId`, there. `type` is any label (e.g. "finance").
export type Rule = {
  id: string;
  description: string;
  type: string;
  role: Role | null;
  roleId: string | null;
  roleName: string;
  permissions: Permission[];
  workspaceId: string;
  workspaceName: string;
  organizationId: string;
  organizationName: string;
  createdAt: string;
  createdBy: string | null;
};

// What a rule grants: a built-in role or an organization role's id.
export type RuleGrant = { role: Role } | { roleId: string };

export type RulePatch = {
  description?: string;
  type?: string;
  workspaceId?: string;
} & Partial<{ role: Role; roleId: string }>;

// A connection of an organization, with the workspaces it is released to.
// `created` by the organization (its owners edit and delete it) or
// `assigned` to it by a platform admin (only platform admins edit, delete or
// unassign it).
export type Connection = {
  name: string;
  type: string | null;
  origin: 'created' | 'assigned';
  workspaces: string[];
};

export type DataSource = { name: string; type: string | null };

// The data sources a platform admin can assign to an organization: never
// `default`, and none that already belongs to an organization.
export function assignableDataSources(sources: DataSource[], owned: Iterable<string>): DataSource[] {
  const taken = new Set(owned);
  return sources.filter((ds) => ds.name !== 'default' && !taken.has(ds.name));
}

export function adminApi(basePath: string) {
  const base = basePath.replace(/\/$/, '');
  const id = encodeURIComponent;

  async function call(method: string, path: string, body?: unknown): Promise<any> {
    const response = await fetch(`${base}${path}`, {
      method,
      headers: {
        ...authHeaders(),
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    onUnauthorized(response);
    let json: any = null;
    try {
      json = await response.json();
    } catch {
      // An empty or non-JSON body; the status says enough.
    }
    if (!response.ok) {
      throw new Error(json?.error || `${response.status} ${response.statusText}`);
    }
    return json;
  }

  // `/v1/<resource>?q&...&limit&offset`; an empty `q` is not sent. `page`
  // starts at 1.
  async function search(resource: string, key: string, params: {
    q: string;
    page: number;
    organizationId?: string | null;
    workspaceId?: string | null;
  }): Promise<Page<any>> {
    const query = new URLSearchParams();
    if (params.q) {
      query.set('q', params.q);
    }
    query.set('limit', String(PAGE_SIZE));
    query.set('offset', String((params.page - 1) * PAGE_SIZE));
    if (params.organizationId) {
      query.set('organizationId', params.organizationId);
    }
    if (params.workspaceId) {
      query.set('workspaceId', params.workspaceId);
    }
    const json = await call('GET', `/v1/${resource}?${query}`);
    return { items: json[key], total: json.total };
  }

  const api = {
    searchUsers: (q: string, page: number): Promise<Page<User>> => search('users', 'users', { q, page }),
    searchOrganizations: (q: string, page: number): Promise<Page<Organization>> => search(
      'organizations', 'organizations', { q, page }
    ),
    searchWorkspaces: (opts: {
      q: string;
      organizationId?: string | null;
      page: number;
    }): Promise<Page<WorkspaceListing>> => search('workspaces', 'workspaces', opts),

    searchRules: (opts: {
      q: string;
      organizationId?: string | null;
      workspaceId?: string | null;
      page: number;
    }): Promise<Page<Rule>> => search('rules', 'rules', opts),

    users: async (): Promise<User[]> => (await call('GET', '/v1/users')).users,
    createUser: (user: { id: string; name?: string; email?: string; password: string }) => call(
      'POST', '/v1/users', user
    ),
    updateUser: (user: string, patch: { name?: string; email?: string; disabled?: boolean }) => call(
      'PATCH', `/v1/users/${id(user)}`, patch
    ),
    access: (user: string): Promise<AccessGraph> => call('GET', `/v1/users/${id(user)}/access`),
    setPassword: (user: string, password: string) => call(
      'PUT', `/v1/users/${id(user)}/password`, { password }
    ),

    organizations: async (): Promise<Organization[]> => (
      await call('GET', '/v1/organizations')
    ).organizations,
    createOrganization: (name: string, owner: string) => call(
      'POST', '/v1/organizations', { name, owner }
    ),
    renameOrganization: (org: string, name: string) => call(
      'PATCH', `/v1/organizations/${id(org)}`, { name }
    ),
    deleteOrganization: (org: string) => call('DELETE', `/v1/organizations/${id(org)}`),
    addOwner: (org: string, user: string) => call(
      'PUT', `/v1/organizations/${id(org)}/owners/${id(user)}`
    ),
    removeOwner: (org: string, user: string) => call(
      'DELETE', `/v1/organizations/${id(org)}/owners/${id(user)}`
    ),

    workspaces: async (org: string): Promise<Workspace[]> => (
      await call('GET', `/v1/organizations/${id(org)}/workspaces`)
    ).workspaces,
    createWorkspace: (org: string, name: string) => call(
      'POST', `/v1/organizations/${id(org)}/workspaces`, { name }
    ),
    renameWorkspace: (ws: string, name: string) => call(
      'PATCH', `/v1/workspaces/${id(ws)}`, { name }
    ),
    deleteWorkspace: (ws: string) => call('DELETE', `/v1/workspaces/${id(ws)}`),
    members: async (ws: string): Promise<Member[]> => (
      await call('GET', `/v1/workspaces/${id(ws)}/members`)
    ).members,
    setMember: (ws: string, user: string, role: Role) => call(
      'PUT', `/v1/workspaces/${id(ws)}/members/${id(user)}`, { role }
    ),
    removeMember: (ws: string, user: string) => call(
      'DELETE', `/v1/workspaces/${id(ws)}/members/${id(user)}`
    ),

    rulesOf: async (ws: string): Promise<Rule[]> => (
      await call('GET', `/v1/workspaces/${id(ws)}/rules`)
    ).rules,
    createRule: (ws: string, rule: { description: string; type: string } & RuleGrant): Promise<Rule> => call(
      'POST', `/v1/workspaces/${id(ws)}/rules`, rule
    ),
    // `workspaceId` moves the rule, with its members, to that workspace.
    updateRule: (rule: string, patch: RulePatch): Promise<Rule> => call(
      'PATCH', `/v1/rules/${id(rule)}`, patch
    ),
    deleteRule: (rule: string) => call('DELETE', `/v1/rules/${id(rule)}`),
    ruleMembers: async (rule: string): Promise<string[]> => (
      await call('GET', `/v1/rules/${id(rule)}/members`)
    ).members.map((m: { user: string }) => m.user),
    addRuleMember: (rule: string, user: string) => call(
      'PUT', `/v1/rules/${id(rule)}/members/${id(user)}`
    ),
    removeRuleMember: (rule: string, user: string) => call(
      'DELETE', `/v1/rules/${id(rule)}/members/${id(user)}`
    ),

    roles: async (org: string): Promise<OrgRole[]> => (
      await call('GET', `/v1/organizations/${id(org)}/roles`)
    ).roles,
    createRole: (org: string, role: { name: string; permissions: Permission[] }): Promise<OrgRole> => call(
      'POST', `/v1/organizations/${id(org)}/roles`, role
    ),
    // New permissions reach every rule with the role at once.
    updateRole: (role: string, patch: { name?: string; permissions?: Permission[] }): Promise<OrgRole> => call(
      'PATCH', `/v1/roles/${id(role)}`, patch
    ),
    // 409 while a rule uses the role.
    deleteRole: (role: string) => call('DELETE', `/v1/roles/${id(role)}`),
    workspace: (ws: string): Promise<Workspace> => call('GET', `/v1/workspaces/${id(ws)}`),

    connections: async (org: string): Promise<Connection[]> => (
      await call('GET', `/v1/organizations/${id(org)}/connections`)
    ).connections,
    dataSources: async (): Promise<DataSource[]> => (
      (await call('GET', '/v1/data-sources')).dataSources || []
    ).map((ds) => ({ name: ds.name, type: ds.type ?? null })),
    // Every connection that belongs to an organization.
    trackedConnections: async (): Promise<TrackedConnection[]> => (
      await call('GET', '/v1/connections')
    ).connections,
    // `dataSources()` without `default` and those an organization holds.
    assignableDataSources: async (): Promise<DataSource[]> => {
      const [sources, tracked] = await Promise.all([api.dataSources(), api.trackedConnections()]);
      return assignableDataSources(sources, tracked.map((c) => c.name));
    },
    assignConnection: (name: string, org: string) => call(
      'PUT', `/v1/connections/${id(name)}/organization`, { organizationId: org }
    ),
    // Takes the connection away from its organization, with every release
    // to its workspaces; the data source itself stays.
    unassignConnection: (name: string) => call(
      'DELETE', `/v1/connections/${id(name)}/organization`
    ),
    release: (ws: string, name: string) => call(
      'PUT', `/v1/workspaces/${id(ws)}/connections/${id(name)}`
    ),
    withdraw: (ws: string, name: string) => call(
      'DELETE', `/v1/workspaces/${id(ws)}/connections/${id(name)}`
    ),
  };
  return api;
}

export type AdminApi = ReturnType<typeof adminApi>;

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

export const ROLES = ['owner', 'editor', 'viewer'] as const;
export type Role = typeof ROLES[number];

export type Member = { user: string; role: Role };

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

  const api = {
    users: async (): Promise<User[]> => (await call('GET', '/v1/users')).users,
    createUser: (user: { id: string; name?: string; email?: string; password: string }) => call(
      'POST', '/v1/users', user
    ),
    updateUser: (user: string, patch: { name?: string; email?: string; disabled?: boolean }) => call(
      'PATCH', `/v1/users/${id(user)}`, patch
    ),
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

    connections: async (org: string): Promise<Connection[]> => (
      await call('GET', `/v1/organizations/${id(org)}/connections`)
    ).connections,
    dataSources: async (): Promise<DataSource[]> => (
      (await call('GET', '/v1/data-sources')).dataSources || []
    ).map((ds) => ({ name: ds.name, type: ds.type ?? null })),
    // `dataSources()` without `default` and those any of `orgs` holds.
    assignableDataSources: async (orgs: string[]): Promise<DataSource[]> => {
      const [sources, ...held] = await Promise.all([
        api.dataSources(),
        ...orgs.map((org) => api.connections(org)),
      ]);
      return assignableDataSources(sources as DataSource[], (held as Connection[][]).flat().map((c) => c.name));
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

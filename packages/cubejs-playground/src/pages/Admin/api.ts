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

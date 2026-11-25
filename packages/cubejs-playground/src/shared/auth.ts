// With RBAC on, `playground/context` hands out no token (`authRequired`):
// a user logs in, and the token goes on every request after. Platform admins
// see the whole Playground; anyone else gets the query builder and the
// Frontend Integrations page, scoped to a workspace they pick.
// Both live in sessionStorage, so they end with the tab.

const STORAGE_KEY = 'cubePlaygroundSession';
const WORKSPACE_KEY = 'cubePlaygroundWorkspace';
const ORGANIZATION_KEY = 'cubePlaygroundOrganization';

export const WORKSPACE_HEADER = 'X-Cube-Workspace';
export const ORGANIZATION_HEADER = 'X-Cube-Organization';
// The scope of an admin without an organization: what sending no header
// gives too.
export const PLATFORM_SCOPE = '_platform';

export type Session = {
  token: string;
  expiresAt: string;
  user: {
    id: string;
    // What people see; `id` is the UUID the server's calls name the user by.
    username: string;
    name: string | null;
    platformAdmin: boolean;
    // The organizations the user owns: they open the Data Model page.
    ownedOrganizations: { id: string; slug: string; name: string }[];
  };
};

function storage(): Storage | null {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

function read(key: string): string | null {
  try {
    return storage()?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function write(key: string, value: string | null) {
  try {
    if (value == null) {
      storage()?.removeItem(key);
    } else {
      storage()?.setItem(key, value);
    }
  } catch {
    // Storage full or blocked: the value lasts until the next reload.
  }
}

// Whether this page has seen a session: a 401 after it expired still has
// to go back to the login.
let hadSession = false;
// The last session token this page saw, to tell it from one typed in.
let lastToken: string | null = null;
// The workspace when sessionStorage cannot keep it (blocked storage): it
// lasts as long as the page.
let memoryWorkspace: string | null = null;
// The same for the organization scope.
let memoryOrganization: string | null = null;
// One reload is enough, however many requests answer 401 at once.
let reloading = false;

export function getSession(): Session | null {
  const raw = read(STORAGE_KEY);
  if (!raw) {
    return null;
  }
  try {
    const session: Session = JSON.parse(raw);
    if (session.token && Date.parse(session.expiresAt) > Date.now()) {
      hadSession = true;
      lastToken = session.token;
      // Stored by an older page, before users had a username.
      if (session.user && session.user.username == null) {
        session.user.username = session.user.id;
      }
      return session;
    }
  } catch {
    // A value this page did not write; drop it.
  }
  logout();
  return null;
}

export function logout() {
  write(STORAGE_KEY, null);
  write(WORKSPACE_KEY, null);
  write(ORGANIZATION_KEY, null);
  memoryWorkspace = null;
  memoryOrganization = null;
}

// Whether a 401 for this token means the session is gone; a token typed into
// the API console failing says nothing about the session.
export function isSessionToken(token: string | null | undefined): boolean {
  getSession();
  return Boolean(token) && token === lastToken;
}

export function authHeaders(): Record<string, string> {
  const session = getSession();
  return session ? { Authorization: session.token } : {};
}

// The workspace data requests are scoped to; `null` sends no header, which
// only platform admins may do (the Platform scope).
export function getWorkspace(): string | null {
  if (storage() == null) {
    return memoryWorkspace;
  }
  try {
    return storage()?.getItem(WORKSPACE_KEY) || null;
  } catch {
    return memoryWorkspace;
  }
}

const workspaceListeners = new Set<() => void>();

export function onWorkspaceChange(listener: () => void): () => void {
  workspaceListeners.add(listener);
  return () => {
    workspaceListeners.delete(listener);
  };
}

// The organization a platform admin without a workspace reads the model of;
// `null` is the Platform scope.
export function getOrganizationScope(): string | null {
  if (storage() == null) {
    return memoryOrganization;
  }
  try {
    return storage()?.getItem(ORGANIZATION_KEY) || null;
  } catch {
    return memoryOrganization;
  }
}

// Whether `key` holds `value` (so a reload keeps it).
function kept(key: string, value: string | null): boolean {
  try {
    return storage() != null && (storage()?.getItem(key) ?? null) === value;
  } catch {
    return false;
  }
}

// Workspace and organization scope exclude each other. Returns whether the
// choice outlives a reload (sessionStorage took it).
function setScope(workspace: string | null, organization: string | null): boolean {
  memoryWorkspace = workspace || null;
  memoryOrganization = organization || null;
  write(WORKSPACE_KEY, memoryWorkspace);
  write(ORGANIZATION_KEY, memoryOrganization);
  workspaceListeners.forEach((listener) => listener());
  return kept(WORKSPACE_KEY, memoryWorkspace) && kept(ORGANIZATION_KEY, memoryOrganization);
}

// Returns whether the choice outlives a reload (sessionStorage took it).
// A workspace replaces the organization scope.
export function setWorkspace(id: string | null): boolean {
  return setScope(id, null);
}

// Switch workspace. With the choice in sessionStorage the page reloads, so
// everything starts over in the new workspace; without storage a reload
// would forget it (and pick again, forever), so the page applies it live:
// the Cube clients follow `onWorkspaceChange`, and other requests read the
// workspace when they are sent.
export function chooseWorkspace(id: string | null, reload = () => window.location.reload()) {
  if (setWorkspace(id)) {
    reload();
  }
}

// Switch to `scope` (an organization of `null`/`_platform`: the Platform),
// then run `then` (a navigation), then reload like `chooseWorkspace`, so the
// page `then` opened starts over in the new scope.
export function switchScope(
  scope: { workspace: string | null; organization: string | null },
  then: () => void,
  reload = () => window.location.reload()
) {
  const organization = scope.organization === PLATFORM_SCOPE ? null : scope.organization;
  const survives = setScope(scope.workspace, scope.workspace ? null : organization);
  then();
  if (survives) {
    reload();
  }
}

// Switch to an organization's model (`null` or `_platform`: the Platform),
// clearing the workspace; reloads like `chooseWorkspace`.
export function chooseOrganizationScope(
  slug: string | null,
  reload = () => window.location.reload()
) {
  if (setScope(null, slug === PLATFORM_SCOPE ? null : slug)) {
    reload();
  }
}

export function workspaceHeaders(): Record<string, string> {
  const workspace = getWorkspace();
  if (workspace) {
    return { [WORKSPACE_HEADER]: workspace };
  }
  const organization = getOrganizationScope();
  return organization ? { [ORGANIZATION_HEADER]: organization } : {};
}

// A request answered 401 while logged in: the token expired or was revoked.
// A 403 is a refusal for this user, not a lost session: callers show it.
export function onUnauthorized(response: { status: number }) {
  if (response.status !== 401 || reloading) {
    return;
  }
  if (getSession() || hadSession) {
    reloading = true;
    logout();
    window.location.reload();
  }
}

export async function login(
  basePath: string,
  username: string,
  password: string
): Promise<Session> {
  const response = await fetch(`${basePath}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
  });
  let body: any = null;
  try {
    body = await response.json();
  } catch {
    // An empty or non-JSON body; the status says enough.
  }
  if (!response.ok) {
    throw new Error(body?.error || `${response.status} ${response.statusText}`);
  }

  const session: Session = {
    token: body.token,
    expiresAt: body.expiresAt,
    user: {
      ...body.user,
      username: body.user?.username ?? body.user?.id,
      platformAdmin: Boolean(body.user?.platformAdmin),
      // Older servers do not send the list.
      ownedOrganizations: body.user?.ownedOrganizations ?? [],
    },
  };
  write(STORAGE_KEY, JSON.stringify(session));
  lastToken = session.token;
  // A workspace picked by whoever used this tab before.
  write(WORKSPACE_KEY, null);
  write(ORGANIZATION_KEY, null);
  memoryWorkspace = null;
  memoryOrganization = null;
  return session;
}

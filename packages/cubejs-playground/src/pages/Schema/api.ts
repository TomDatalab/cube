import type { Rbac } from '../../hooks/rbac';
import { authHeaders, onUnauthorized } from '../../shared/auth';
import { playgroundFetch } from '../../shared/helpers';
import type { OrgDetail, OrgSummary } from './tree';

export const PAGE_SIZE = 50;

export const PLATFORM = '_platform';

export type FileContent = {
  id?: string;
  path?: string;
  content: string;
  version?: number;
  editable?: boolean;
  managedBy?: string | null;
  updatedAt?: string;
  updatedBy?: string | null;
  // Only without RBAC, where the file is on disk.
  absPath?: string;
};

export type OrgNode = Pick<OrgSummary, 'id' | 'slug' | 'name'>;

export class ApiError extends Error {
  status: number;

  // The 400 body's `problems`: `{file, cube, message}`.
  problems?: { file: string; cube: string; message: string }[];

  constructor(message: string, status: number, problems?: ApiError['problems']) {
    super(message);
    this.status = status;
    this.problems = problems;
  }
}

// A node the `/cube/v1` model routes serve: an organization with an id, or
// Platform. Without RBAC the only node has none, and the playground tree
// routes serve it.
const routed = (org: OrgNode) => org.slug === PLATFORM || Boolean(org.id);

async function answer(response: Response): Promise<any> {
  let body: any = null;
  let text = '';
  try {
    text = await response.text();
    body = text ? JSON.parse(text) : null;
  } catch {
    // Not JSON; the text or the status says enough.
  }
  if (!response.ok) {
    throw new ApiError(body?.error || text || `${response.status} ${response.statusText}`, response.status, body?.problems);
  }
  return body;
}

export type CubeModelView = {
  id: string;
  organizationId: string;
  name: string;
  definition: unknown;
  cubeName: string | null;
  version: number;
  updatedAt: string;
  updatedBy: string | null;
};

export function modelApi(rbac: Rbac, basePath = '/cube') {
  const base = basePath.replace(/\/$/, '');
  const enc = encodeURIComponent;
  const owned = rbac.session?.user.ownedOrganizations ?? [];

  // The `/v1` routes: the login token and no workspace.
  async function call(method: string, path: string, body?: unknown): Promise<any> {
    const response = await fetch(`${base}/v1${path}`, {
      method,
      headers: {
        ...authHeaders(),
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    onUnauthorized(response);
    return answer(response);
  }

  const legacy = async (path: string) => answer(await playgroundFetch(`playground/tree${path}`));

  const model = (org: OrgNode) => (
    org.slug === PLATFORM ? '/platform/model' : `/organizations/${enc(org.id!)}/model`
  );

  return {
    async list(q: string, offset: number): Promise<{ organizations: OrgSummary[]; total: number }> {
      if (!rbac.isAdmin) {
        const needle = q.trim().toLowerCase();
        const matching = owned.filter((o) => (
          !needle || o.name.toLowerCase().includes(needle) || o.slug.toLowerCase().includes(needle)
        ));
        return { organizations: matching.slice(offset, offset + PAGE_SIZE), total: matching.length };
      }
      const params = new URLSearchParams({ limit: String(PAGE_SIZE), offset: String(offset) });
      if (q) {
        params.set('q', q);
      }
      const result = await legacy(`?${params}`);
      return { organizations: result.organizations || [], total: result.total || 0 };
    },

    detail(org: OrgNode): Promise<OrgDetail> {
      return routed(org) ? call('GET', model(org)) : legacy(`/${enc(org.slug)}`);
    },

    // `id` is the file's id; without RBAC, its path.
    file(org: OrgNode, id: string): Promise<FileContent> {
      if (!routed(org)) {
        return legacy(`/${enc(org.slug)}/files/${id.split('/').map(enc).join('/')}`);
      }
      return call('GET', `${model(org)}/files/${enc(id)}`);
    },

    save(org: OrgNode, id: string, content: string, version: number): Promise<OrgDetail> {
      return call('PUT', `${model(org)}/files/${enc(id)}`, { content, version });
    },

    async remove(org: OrgNode, id: string, version: number): Promise<void> {
      await call('DELETE', `${model(org)}/files/${enc(id)}?version=${version}`);
    },

    // `tables`: `[<dataSource>.<schema>, table]` pairs.
    // Without RBAC the files go to disk through `playground/generate-schema`,
    // which also wants the format and the tables' columns.
    async generate(
      org: OrgNode,
      tables: [string, string][],
      legacyOptions?: { format: string; tablesSchema: Record<string, unknown> }
    ): Promise<void> {
      if (routed(org)) {
        await call('POST', `${model(org)}/generate`, { tables });
        return;
      }
      await answer(await playgroundFetch('playground/generate-schema', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ...legacyOptions, tables }),
      }));
    },

    // Platform admins only; `null` moves the file to Platform.
    async move(id: string, organizationId: string | null): Promise<void> {
      await call('PUT', `/model/files/${enc(id)}/organization`, { organizationId });
    },

    // Models (RBAC only): a canvas of joined tables built into one cube.
    async models(org: OrgNode): Promise<CubeModelView[]> {
      return (await call('GET', `/organizations/${enc(org.id!)}/cube-models`)).models || [];
    },

    model(id: string): Promise<CubeModelView> {
      return call('GET', `/cube-models/${enc(id)}`);
    },

    createModel(org: OrgNode, name: string, definition: unknown): Promise<CubeModelView> {
      return call('POST', `/organizations/${enc(org.id!)}/cube-models`, { name, definition });
    },

    saveModel(
      id: string,
      change: { name?: string; definition?: unknown; version: number }
    ): Promise<CubeModelView> {
      return call('PUT', `/cube-models/${enc(id)}`, change);
    },

    async deleteModel(id: string, version: number): Promise<void> {
      await call('DELETE', `/cube-models/${enc(id)}?version=${version}`);
    },

    buildModel(
      id: string,
      force?: boolean
    ): Promise<{ fileName: string; content: string; cubeName: string; detail: OrgDetail }> {
      return call('POST', `/cube-models/${enc(id)}/build`, { force });
    },

    async workspaces(org: OrgNode): Promise<{ id: string; name: string }[]> {
      return (await call('GET', `/organizations/${enc(org.id!)}/workspaces`)).workspaces || [];
    },

    // The names of the connections released to a workspace.
    async released(workspaceId: string): Promise<string[]> {
      const { connections = [] } = await call('GET', `/workspaces/${enc(workspaceId)}/connections`);
      return connections.map((c: { name: string }) => c.name);
    },
  };
}

export type OpenBlock = { disabled: boolean; hint?: string; warning?: string };

// Whether "Open in Playground" works for a cube, and what to tell the user.
// `released` is null until the connections of the chosen workspace are known.
export function openBlock(
  cube: { dataSource: string; measures?: unknown[]; dimensions?: unknown[] },
  ctx: {
    owner: boolean;
    workspaces: { id: string; name: string }[];
    chosen: string | null;
    released: string[] | null;
  }
): OpenBlock {
  if (!(cube.measures?.length || cube.dimensions?.length)) {
    return { disabled: true, hint: 'The cube has no members' };
  }
  if (ctx.owner && !ctx.workspaces.length) {
    return { disabled: true, hint: 'Create a workspace in this organization to query it' };
  }
  const workspace = ctx.workspaces.find((w) => w.id === ctx.chosen);
  if (workspace && ctx.released && !ctx.released.includes(cube.dataSource)) {
    return {
      disabled: false,
      warning: `Release the connection ${cube.dataSource} to the workspace ${workspace.name}`,
    };
  }
  return { disabled: false };
}

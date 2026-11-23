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

  constructor(message: string, status: number) {
    super(message);
    this.status = status;
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
    throw new ApiError(body?.error || text || `${response.status} ${response.statusText}`, response.status);
  }
  return body;
}

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

// Whether "Abrir no Playground" works for a cube, and what to tell the user.
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
    return { disabled: true, hint: 'O cubo não tem membros' };
  }
  if (ctx.owner && !ctx.workspaces.length) {
    return { disabled: true, hint: 'Crie um workspace nesta organização para consultar' };
  }
  const workspace = ctx.workspaces.find((w) => w.id === ctx.chosen);
  if (workspace && ctx.released && !ctx.released.includes(cube.dataSource)) {
    return {
      disabled: false,
      warning: `libere a conexão ${cube.dataSource} para o workspace ${workspace.name}`,
    };
  }
  return { disabled: false };
}

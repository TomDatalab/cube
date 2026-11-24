// The data preview of the cube panel: the first raw rows of the cube (every
// dimension, ungrouped) and its measures over all the data, queried through
// the Cube API like any other query, with the caller's access.

import type { TreeMember } from './tree';

export type PreviewQuery = Record<string, unknown>;
export type Row = Record<string, string | number | boolean | null>;

export const PREVIEW_LIMITS = [10, 50, 100] as const;

type PreviewCube = { name: string; measures?: TreeMember[]; dimensions?: TreeMember[] };

/**
 * The two queries of a preview. `hidden`: whether members query users do not
 * see are included (platform admins see every member).
 */
export function previewQueries(
  cube: PreviewCube,
  { limit, hidden }: { limit: number; hidden: boolean }
): { rows: PreviewQuery | null; totals: PreviewQuery | null } {
  const names = (list: TreeMember[] = []) => list.filter((m) => hidden || m.public !== false).map((m) => m.name);
  const dimensions = names(cube.dimensions);
  const measures = names(cube.measures);
  return {
    rows: dimensions.length ? { dimensions, ungrouped: true, limit } : null,
    totals: measures.length ? { measures } : null,
  };
}

/**
 * Where the preview queries: a platform admin in the model of the cube's
 * organization; anyone else in the workspace chosen in the header, or nowhere
 * (null) while none is.
 */
export function previewHeaders({ isAdmin, slug, workspace }: {
  isAdmin: boolean;
  slug: string;
  workspace: string | null;
}): Record<string, string> | null {
  if (isAdmin) {
    return { 'X-Cube-Organization': slug };
  }
  return workspace ? { 'X-Cube-Workspace': workspace } : null;
}

const WAIT_TRIES = 30;

/**
 * Runs `query` on `/load`, asking again while the server answers "Continue
 * wait" (a long query); the rows, or the server's error as an Error.
 */
export async function loadRows(
  url: string,
  headers: Record<string, string>,
  query: PreviewQuery,
  doFetch: (url: string, init: RequestInit) => Promise<Response> = fetch,
  waitMs = 1000,
  triesLeft = WAIT_TRIES
): Promise<Row[]> {
  const response = await doFetch(url, {
    method: 'POST',
    headers: { ...headers, 'Content-Type': 'application/json' },
    body: JSON.stringify({ query }),
  });
  let body: any = null;
  try {
    body = await response.json();
  } catch {
    // Not JSON: the status says enough.
  }
  if (body?.error === 'Continue wait') {
    if (triesLeft <= 1) {
      throw new Error('The query is still running: try again later');
    }
    await new Promise((resolve) => setTimeout(resolve, waitMs));
    return loadRows(url, headers, query, doFetch, waitMs, triesLeft - 1);
  }
  if (!response.ok || body?.error) {
    throw new Error(body?.error || `${response.status} ${response.statusText}`);
  }
  return body?.data ?? [];
}

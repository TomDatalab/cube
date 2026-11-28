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

const PLATFORM = '_platform';

/**
 * The workspace an owner queries a cube of an organization in: the one chosen
 * in the header when it is of that organization, else its first one; null
 * when it has none. While its workspaces load (null), the chosen one.
 */
export function orgWorkspace(
  chosen: string | null,
  workspaces: { id: string }[] | null
): string | null {
  if (workspaces === null) {
    return chosen;
  }
  if (chosen && workspaces.some((w) => w.id === chosen)) {
    return chosen;
  }
  return workspaces[0]?.id ?? null;
}

/**
 * The header scope "Open in Playground" moves to, so the query reaches the
 * model of the cube's organization (`slug`), or null when the current one
 * already does: a platform admin in a workspace of it or in its model, an
 * owner in one of its workspaces. An owner of an organization without
 * workspaces stays (the button is disabled then).
 */
export function openScope({ isAdmin, slug, workspace, organization, workspaces }: {
  isAdmin: boolean;
  slug: string;
  workspace: string | null;
  organization: string | null;
  workspaces: { id: string }[];
}): { workspace: string | null; organization: string | null } | null {
  const inOrg = Boolean(workspace && workspaces.some((w) => w.id === workspace));
  if (isAdmin) {
    if (inOrg || (!workspace && (organization ?? PLATFORM) === slug)) {
      return null;
    }
    return { workspace: null, organization: slug === PLATFORM ? null : slug };
  }
  if (inOrg || !workspaces.length) {
    return null;
  }
  return { workspace: workspaces[0].id, organization: null };
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

const NUMERIC = new Set([
  'number', 'count', 'count_distinct', 'countDistinct', 'count_distinct_approx', 'countDistinctApprox',
  'sum', 'avg', 'min', 'max', 'running_total', 'runningTotal',
]);

/** Whether a member of this type holds numbers (right-aligned, grouped). */
export const isNumeric = (type: string | undefined) => Boolean(type && NUMERIC.has(type));

const TIME = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})(:\d{2})?(\.\d+)?Z?$/;

/**
 * A value as the preview shows it, or null for a missing one: a time as its
 * date (and its hour when it has one), a number with grouped digits (the API
 * sends numbers as text), anything else as text.
 */
export function formatValue(value: unknown, type: string | undefined): string | null {
  if (value === null || value === undefined) {
    return null;
  }
  const text = String(value);
  if (type === 'time') {
    const m = TIME.exec(text);
    if (!m) {
      return text;
    }
    const [, date, hm, seconds = ':00'] = m;
    if (hm === '00:00' && seconds === ':00') {
      return date;
    }
    return seconds === ':00' ? `${date} ${hm}` : `${date} ${hm}${seconds}`;
  }
  if (isNumeric(type) && /^-?\d+(\.\d+)?$/.test(text)) {
    const [whole, decimals] = text.split('.');
    const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return decimals ? `${grouped}.${decimals}` : grouped;
  }
  return text;
}

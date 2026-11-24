import { useEffect, useState } from 'react';

import { useWorkspace } from '../../hooks/workspace';
import { authHeaders, ORGANIZATION_HEADER, onUnauthorized } from '../../shared/auth';
import { apiFetch, isSameOrigin } from '../../shared/helpers';
import { fetchMetaGroups, MetaGroup, mergeMetaGroups } from '../../shared/meta-groups';

// The groups of the cubes `/meta` shows (`<apiUrl>/meta/groups`), fetched
// again whenever the meta or the workspace changes. `null` while loading and
// when the API has no such route: the side panel then lists without groups.
export function useMetaGroups(
  apiUrl: string | null | undefined,
  apiToken: string | null | undefined,
  meta: unknown
): MetaGroup[] | null {
  const workspace = useWorkspace();
  const [groups, setGroups] = useState<MetaGroup[] | null>(null);

  useEffect(() => {
    if (!apiUrl || !apiToken || !meta) {
      setGroups(null);
      return undefined;
    }
    let live = true;
    fetchMetaGroups(
      `${apiUrl.replace(/\/$/, '')}/meta/groups`,
      { headers: { Authorization: apiToken } },
      (url, init) => apiFetch(url, init)
    ).then((result) => {
      if (live) {
        setGroups(result);
      }
    });
    return () => {
      live = false;
    };
  }, [apiUrl, apiToken, meta, workspace]);

  return groups;
}

// Every organization's groups, for a platform admin: the Platform's model
// (no header), then each organization's (`X-Cube-Organization`), merged.
// `null` until known, when `enabled` is off, or when the organizations
// cannot be listed: the side panel then shows the current model only.
export function useAllScopeGroups(
  apiUrl: string | null | undefined,
  enabled: boolean,
  meta: unknown
): MetaGroup[] | null {
  const [groups, setGroups] = useState<MetaGroup[] | null>(null);

  useEffect(() => {
    if (!apiUrl || !enabled || !meta || !isSameOrigin(apiUrl)) {
      setGroups(null);
      return undefined;
    }
    let live = true;
    const base = apiUrl.replace(/\/$/, '');
    // Plain fetch: `apiFetch` would put the current scope's header on it.
    const doFetch = (url: string, init: RequestInit) => fetch(url, init).then((r) => {
      onUnauthorized(r);
      return r;
    });
    (async () => {
      const response = await doFetch(`${base}/organizations`, { headers: authHeaders() });
      if (!response.ok) {
        return null;
      }
      const organizations: { slug: string }[] = (await response.json())?.organizations ?? [];
      const perScope = await Promise.all([
        fetchMetaGroups(`${base}/meta/groups`, { headers: authHeaders() }, doFetch),
        ...organizations.map((o) => fetchMetaGroups(
          `${base}/meta/groups`,
          { headers: { ...authHeaders(), [ORGANIZATION_HEADER]: o.slug } },
          doFetch
        )),
      ]);
      return mergeMetaGroups(perScope.map((g) => g || []));
    })()
      .catch(() => null)
      .then((result) => {
        if (live) {
          setGroups(result);
        }
      });
    return () => {
      live = false;
    };
  }, [apiUrl, enabled, meta]);

  return groups;
}

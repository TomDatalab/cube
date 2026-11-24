import { useEffect, useState } from 'react';

import { useWorkspace } from '../../hooks/workspace';
import { apiFetch } from '../../shared/helpers';
import { fetchMetaGroups, MetaGroup } from '../../shared/meta-groups';

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

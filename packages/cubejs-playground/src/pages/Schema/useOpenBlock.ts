import { useEffect, useState } from 'react';

import { useWorkspace } from '../../hooks';
import { openBlock, OpenBlock, OrgNode } from './api';
import type { modelApi } from './api';
import type { TreeCube } from './tree';

// Whether "Abrir no Playground" works for a cube of `org`: the workspaces of
// the organization and the connections released to the chosen one.
export function useOpenBlock(
  api: ReturnType<typeof modelApi>,
  org: OrgNode | undefined,
  owner: boolean,
  cube: TreeCube
): OpenBlock {
  const chosen = useWorkspace();
  const [workspaces, setWorkspaces] = useState<{ id: string; name: string }[] | null>(null);
  const [released, setReleased] = useState<string[] | null>(null);

  useEffect(() => {
    setWorkspaces(null);
    if (!org?.id) {
      return undefined;
    }
    let cancelled = false;
    api.workspaces(org).then(
      (list) => !cancelled && setWorkspaces(list),
      () => !cancelled && setWorkspaces(null)
    );
    return () => {
      cancelled = true;
    };
  }, [api, org?.id]);

  useEffect(() => {
    setReleased(null);
    if (!chosen || !workspaces?.some((w) => w.id === chosen)) {
      return undefined;
    }
    let cancelled = false;
    api.released(chosen).then(
      (list) => !cancelled && setReleased(list),
      () => !cancelled && setReleased(null)
    );
    return () => {
      cancelled = true;
    };
  }, [api, chosen, workspaces]);

  // Unknown workspaces (still loading, or failed) block nothing.
  return openBlock(cube, { owner: owner && workspaces !== null, workspaces: workspaces ?? [], chosen, released });
}

import { getSession, Session } from '../shared/auth';
import { usePlaygroundContext } from './app-context';

export type Rbac = {
  // RBAC is on: the Playground needs a login.
  enabled: boolean;
  session: Session | null;
  // May use the Data Model and Data Source pages: everyone without RBAC.
  isAdmin: boolean;
  // May use the Data Model page: platform admins and organization owners.
  canModel: boolean;
};

export function buildRbac(
  context: { authRequired?: boolean; rbac?: boolean } | null | undefined,
  current: Session | null
): Rbac {
  const enabled = Boolean(context?.authRequired || context?.rbac);
  const session = enabled ? current : null;
  const isAdmin = !enabled || Boolean(session?.user.platformAdmin);

  return {
    enabled,
    session,
    isAdmin,
    canModel: isAdmin || (session?.user.ownedOrganizations?.length ?? 0) > 0,
  };
}

export function useRbac(): Rbac {
  const context = usePlaygroundContext();
  return buildRbac(context, getSession());
}

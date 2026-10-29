import { getSession, Session } from '../shared/auth';
import { usePlaygroundContext } from './app-context';

export type Rbac = {
  // RBAC is on: the Playground needs a login.
  enabled: boolean;
  session: Session | null;
  // May use the Data Model and Data Source pages: everyone without RBAC.
  isAdmin: boolean;
};

export function useRbac(): Rbac {
  const context = usePlaygroundContext();
  const enabled = Boolean(context?.authRequired || context?.rbac);
  const session = enabled ? getSession() : null;

  return {
    enabled,
    session,
    isAdmin: !enabled || Boolean(session?.user.platformAdmin),
  };
}

import { ReactNode } from 'react';
import { Redirect } from 'react-router-dom';

import { useRbac } from '../../hooks';

// With RBAC on, the pages that read the model and the database are for
// platform admins; anyone else goes to the query builder.
export function AdminOnly({ children }: { children: ReactNode }) {
  const { isAdmin } = useRbac();

  return isAdmin ? <>{children}</> : <Redirect to="/build" />;
}

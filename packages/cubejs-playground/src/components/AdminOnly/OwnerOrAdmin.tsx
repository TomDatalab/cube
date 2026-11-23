import { ReactNode } from 'react';
import { Redirect } from 'react-router-dom';

import { useRbac } from '../../hooks';

// The Data Model page is for platform admins and organization owners;
// anyone else goes to the query builder.
export function OwnerOrAdmin({ children }: { children: ReactNode }) {
  const { canModel } = useRbac();

  return canModel ? <>{children}</> : <Redirect to="/build" />;
}

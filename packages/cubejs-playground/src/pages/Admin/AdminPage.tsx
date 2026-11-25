import { useMemo, useState } from 'react';
import { Info } from 'lucide-react';
import { Redirect, useHistory, useLocation } from 'react-router-dom';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';

import { usePlaygroundContext, useRbac } from '../../hooks';
import { Content, Header } from '../components/Ui';
import { adminApi } from './api';
import { OrganizationsTab } from './OrganizationsTab';
import { RolesTab } from './RolesTab';
import { RulesTab } from './RulesTab';
import { ADMIN_SECTIONS, adminPath, adminSectionOf } from './sections';
import { UsersTab } from './UsersTab';
import { WorkspacesTab } from './WorkspacesTab';

// With RBAC on: organizations, workspaces (with their members and connection
// releases), roles (what an organization's rules may grant), rules (named
// roles in a workspace) and users, each at its own /admin/<section> path, listed under
// Admin in the sidebar, through the management API. A section loads when
// shown, so it sees what the others changed. Platform admins only
// (`AdminOnly`).
export function AdminPage() {
  const { basePath = '' } = usePlaygroundContext() || {};
  const { enabled, session } = useRbac();
  const api = useMemo(() => adminApi(basePath), [basePath]);
  const { pathname } = useLocation();
  const history = useHistory();
  const section = adminSectionOf(pathname);
  // The organization the Workspaces section shows, `null` for all; this page
  // stays mounted across sections, so it survives going there.
  const [workspacesOf, setWorkspacesOf] = useState<string | null>(null);

  if (pathname !== adminPath(section)) {
    return <Redirect to={adminPath(section)} />;
  }

  const title = ADMIN_SECTIONS.find((s) => s.key === section)!.label;

  return (
    <div className="tw:text-foreground">
      <Header>
        <h1 className="tw:text-3xl tw:font-semibold">{title}</h1>
      </Header>
      <Content className="tw:pb-8">
        {enabled ? (
          <>
            {section === 'organizations' ? (
              <OrganizationsTab
                api={api}
                onShowWorkspaces={(org) => {
                  setWorkspacesOf(org);
                  history.push(adminPath('workspaces'));
                }}
              />
            ) : null}
            {section === 'workspaces' ? (
              <WorkspacesTab
                api={api}
                organization={workspacesOf}
                onOrganizationChange={setWorkspacesOf}
              />
            ) : null}
            {section === 'roles' ? <RolesTab api={api} /> : null}
            {section === 'rules' ? <RulesTab api={api} /> : null}
            {section === 'users' ? (
              <UsersTab
                api={api}
                currentUser={session ? { id: session.user.id, username: session.user.username } : null}
              />
            ) : null}
          </>
        ) : (
          <Alert>
            <Info />
            <AlertTitle>RBAC is off</AlertTitle>
            <AlertDescription>
              Set CUBE_RBAC=spicedb to manage users, organizations, workspaces and rules here.
            </AlertDescription>
          </Alert>
        )}
      </Content>
    </div>
  );
}

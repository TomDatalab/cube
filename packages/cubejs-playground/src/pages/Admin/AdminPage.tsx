import { Alert, Layout, Typography } from 'antd';
import { useMemo, useState } from 'react';
import { Redirect, useHistory, useLocation } from 'react-router-dom';

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
    <Layout>
      <Header>
        <Typography.Title>{title}</Typography.Title>
      </Header>
      <Content>
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
                currentUser={session?.user.id ?? null}
              />
            ) : null}
          </>
        ) : (
          <Alert
            type="info"
            showIcon
            message="RBAC is off"
            description="Set CUBE_RBAC=spicedb to manage users, organizations, workspaces and rules here."
          />
        )}
      </Content>
    </Layout>
  );
}

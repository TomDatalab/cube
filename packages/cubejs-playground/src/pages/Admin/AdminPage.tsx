import {
  Alert, Layout, Tabs, Typography,
} from 'antd';
import { useMemo, useState } from 'react';

import { usePlaygroundContext, useRbac } from '../../hooks';
import { Content, Header } from '../components/Ui';
import { adminApi } from './api';
import { OrganizationsTab } from './OrganizationsTab';
import { UsersTab } from './UsersTab';
import { WorkspacesTab } from './WorkspacesTab';

// With RBAC on: organizations, workspaces (with their members and connection
// releases) and users, each in a tab, through the management API. A tab loads
// when shown, so it sees what the others changed. Platform admins only
// (`AdminOnly`).
export function AdminPage() {
  const { basePath = '' } = usePlaygroundContext() || {};
  const { enabled, session } = useRbac();
  const api = useMemo(() => adminApi(basePath), [basePath]);
  const [tab, setTab] = useState('organizations');
  // The organization the Workspaces tab shows, `null` for all.
  const [workspacesOf, setWorkspacesOf] = useState<string | null>(null);

  return (
    <Layout>
      <Header>
        <Typography.Title>Admin</Typography.Title>
      </Header>
      <Content>
        {enabled ? (
          <Tabs activeKey={tab} onChange={setTab} destroyInactiveTabPane>
            <Tabs.TabPane tab="Organizations" key="organizations">
              <OrganizationsTab
                api={api}
                onShowWorkspaces={(org) => {
                  setWorkspacesOf(org);
                  setTab('workspaces');
                }}
              />
            </Tabs.TabPane>
            <Tabs.TabPane tab="Workspaces" key="workspaces">
              <WorkspacesTab
                api={api}
                organization={workspacesOf}
                onOrganizationChange={setWorkspacesOf}
              />
            </Tabs.TabPane>
            <Tabs.TabPane tab="Users" key="users">
              <UsersTab
                api={api}
                currentUser={session?.user.id ?? null}
              />
            </Tabs.TabPane>
          </Tabs>
        ) : (
          <Alert
            type="info"
            showIcon
            message="RBAC is off"
            description="Set CUBE_RBAC=spicedb to manage users, organizations and workspaces here."
          />
        )}
      </Content>
    </Layout>
  );
}

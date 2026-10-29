import {
  Alert, Layout, message, Tabs, Typography,
} from 'antd';
import { useEffect, useMemo, useState } from 'react';

import { useIsMounted, usePlaygroundContext, useRbac } from '../../hooks';
import { Content, Header } from '../components/Ui';
import { adminApi, User } from './api';
import { OrganizationsTab } from './OrganizationsTab';
import { UsersTab } from './UsersTab';
import { WorkspacesTab } from './WorkspacesTab';

// With RBAC on: organizations, workspaces (with their members and connection
// releases) and users, each in a tab, through the management API. A tab loads
// when shown, so it sees what the others changed. Platform admins only
// (`AdminOnly`).
export function AdminPage() {
  const isMounted = useIsMounted();
  const { basePath = '' } = usePlaygroundContext() || {};
  const { enabled, session } = useRbac();
  const api = useMemo(() => adminApi(basePath), [basePath]);
  const [users, setUsers] = useState<User[] | null>(null);
  const [tab, setTab] = useState('organizations');
  // The organization the Workspaces tab shows, `null` for all.
  const [workspacesOf, setWorkspacesOf] = useState<string | null>(null);

  async function loadUsers() {
    try {
      const loaded = await api.users();
      if (isMounted()) {
        setUsers(loaded);
      }
    } catch (e: any) {
      message.error(e.message);
    }
  }

  useEffect(() => {
    if (enabled) {
      loadUsers();
    }
  }, [enabled, api]);

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
                users={users || []}
                onShowWorkspaces={(org) => {
                  setWorkspacesOf(org);
                  setTab('workspaces');
                }}
              />
            </Tabs.TabPane>
            <Tabs.TabPane tab="Workspaces" key="workspaces">
              <WorkspacesTab
                api={api}
                users={users || []}
                organization={workspacesOf}
                onOrganizationChange={setWorkspacesOf}
              />
            </Tabs.TabPane>
            <Tabs.TabPane tab="Users" key="users">
              <UsersTab
                api={api}
                users={users}
                currentUser={session?.user.id ?? null}
                reload={loadUsers}
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

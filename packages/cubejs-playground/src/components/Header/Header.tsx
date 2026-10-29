import { LogoutOutlined, MenuOutlined } from '@ant-design/icons';
import { Dropdown, Layout, Menu } from 'antd';
import { useMediaQuery } from 'react-responsive';
import { Link } from 'react-router-dom';
import styled from 'styled-components';

import { usePlaygroundContext, useRbac } from '../../hooks';
import { logout } from '../../shared/auth';
import { StyledMenu, StyledMenuButton, StyledMenuItem } from './Menu';
import { RunOnCubeCloud } from './RunOnCubeCloud';
import { ThemeSwitcher } from '../ThemeSwitcher';
import { WorkspaceSelector } from './WorkspaceSelector';

const StyledHeader = styled(Layout.Header)`
  && {
    background-color: var(--pg-bar);
    color: white;
    padding: 0 16px;
    line-height: 44px;
    height: 48px;
  }
`;

type Props = {
  selectedKeys: string[];
};

export default function Header({ selectedKeys }: Props) {
  const isDesktopOrLaptop = useMediaQuery({
    query: '(min-width: 992px)',
  });

  const isMobileOrTable = useMediaQuery({
    query: '(max-width: 991px)',
  });

  // With RBAC on, who logged in; Data Model and Data Source are for
  // platform admins only.
  const { session, isAdmin } = useRbac();
  const { basePath = '' } = usePlaygroundContext();

  const workspaceSelector = session ? (
    <div style={{ float: 'right', marginLeft: 8 }}>
      <WorkspaceSelector basePath={basePath} isAdmin={isAdmin} />
    </div>
  ) : null;

  // floats right, so it comes first in the markup to sit left of the selector
  const themeSwitcher = (
    <div style={{ float: 'right', marginLeft: 8 }}>
      <ThemeSwitcher />
    </div>
  );

  function logOut() {
    logout();
    window.location.reload();
  }

  return (
    <StyledHeader>
      <div style={{ float: 'left' }}>
        <img
          src="./cube-core-logo-adapted_for_dark_bg.svg"
          style={{ height: 28, marginRight: 28 }}
          alt=""
        />
      </div>

      {workspaceSelector}
      {themeSwitcher}

      {isDesktopOrLaptop && (
        <StyledMenu theme="light" mode="horizontal" selectedKeys={selectedKeys}>
          <StyledMenuItem key="/build">
            <Link to="/build">Playground</Link>
          </StyledMenuItem>

          {isAdmin ? (
            <StyledMenuItem key="/schema">
              <Link to="/schema">Data Model</Link>
            </StyledMenuItem>
          ) : null}

          {isAdmin ? (
            <StyledMenuItem key="/data-sources">
              <Link to="/data-sources">Data Source</Link>
            </StyledMenuItem>
          ) : null}

          <StyledMenuItem key="/frontend-integrations">
            <Link to="/frontend-integrations">Frontend Integrations</Link>
          </StyledMenuItem>

          {session && isAdmin ? (
            <StyledMenuItem key="/admin">
              <Link to="/admin">Admin</Link>
            </StyledMenuItem>
          ) : null}

          <RunOnCubeCloud />

          {session ? (
            <StyledMenuButton onClick={logOut}>
              <LogoutOutlined />
              Log out {session.user.name || session.user.id}
            </StyledMenuButton>
          ) : null}
        </StyledMenu>
      )}

      {isMobileOrTable && (
        <div style={{ float: 'right' }}>
          <Dropdown
            overlay={(
              <Menu>
                <Menu.Item key="/build">
                  <Link to="/build">Playground</Link>
                </Menu.Item>

                {isAdmin ? (
                  <Menu.Item key="/schema">
                    <Link to="/schema">Data Model</Link>
                  </Menu.Item>
                ) : null}

                {isAdmin ? (
                  <Menu.Item key="/data-sources">
                    <Link to="/data-sources">Data Source</Link>
                  </Menu.Item>
                ) : null}

                <Menu.Item key="/frontend-integrations">
                  <Link to="/frontend-integrations">Frontend Integrations</Link>
                </Menu.Item>

                {session && isAdmin ? (
                  <Menu.Item key="/admin">
                    <Link to="/admin">Admin</Link>
                  </Menu.Item>
                ) : null}

                {session ? (
                  <Menu.Item key="logout" onClick={logOut}>
                    Log out
                  </Menu.Item>
                ) : null}
              </Menu>
            )}
          >
            <MenuOutlined />
          </Dropdown>
        </div>
      )}
    </StyledHeader>
  );
}

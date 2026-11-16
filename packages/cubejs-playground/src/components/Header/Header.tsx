import { LogoutOutlined, RightOutlined } from '@ant-design/icons';
import styled from 'styled-components';

import { usePlaygroundContext, useRbac } from '../../hooks';
import { logout } from '../../shared/auth';
import { pageTrail } from '../Sidebar/Sidebar';
import { ThemeSwitcher } from '../ThemeSwitcher';
import { WorkspaceSelector } from './WorkspaceSelector';

// The bossa-arq-dashboard top bar: the page on the left, the actions on the
// right. The pages themselves are in the sidebar.
const Bar = styled.header`
  position: sticky;
  top: 0;
  z-index: 9;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  height: 44px;
  padding: 0 24px;
  background: var(--pg-surface);
  border-bottom: 1px solid var(--pg-line);
  color: var(--pg-text);
`;

const Crumb = styled.span`
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-size: 13px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
`;

const Actions = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;
  flex: none;
`;

const User = styled.span`
  font-size: 13px;
  color: var(--pg-muted);
  white-space: nowrap;

  @media (max-width: 767px) {
    display: none;
  }
`;

const IconButton = styled.button`
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  background: none;
  color: var(--pg-muted);
  cursor: pointer;

  &:hover {
    color: var(--pg-text);
  }

  &:focus-visible {
    outline: 2px solid var(--pg-accent);
    outline-offset: 2px;
  }
`;

type Props = {
  pathname: string;
};

export default function Header({ pathname }: Props) {
  // With RBAC on, who logged in and which workspace the data comes from.
  const { session, isAdmin } = useRbac();
  const { basePath = '' } = usePlaygroundContext();
  // Admin > Users for a page inside a group
  const trail = pageTrail(pathname);

  function logOut() {
    logout();
    window.location.reload();
  }

  return (
    <Bar>
      <Crumb data-testid="page-title">
        {trail[0]?.icon}
        {trail.map((page, i) => (
          <span key={page.path}>
            {i > 0 ? <RightOutlined style={{ fontSize: 9, margin: '0 6px' }} /> : null}
            {page.label}
          </span>
        ))}
      </Crumb>

      <Actions>
        {session ? <WorkspaceSelector basePath={basePath} isAdmin={isAdmin} /> : null}
        <ThemeSwitcher />
        {session ? (
          <>
            <User>{session.user.name || session.user.id}</User>
            <IconButton type="button" aria-label="Sair" title="Sair" onClick={logOut}>
              <LogoutOutlined />
            </IconButton>
          </>
        ) : null}
      </Actions>
    </Bar>
  );
}

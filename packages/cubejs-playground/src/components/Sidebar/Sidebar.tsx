import {
  ApartmentOutlined,
  ApiOutlined,
  AppstoreOutlined,
  CaretRightOutlined,
  CodeOutlined,
  DatabaseOutlined,
  DownOutlined,
  IdcardOutlined,
  LeftOutlined,
  NodeIndexOutlined,
  RightOutlined,
  SafetyOutlined,
  TeamOutlined,
  UsergroupAddOutlined,
} from '@ant-design/icons';
import { ReactNode, useEffect, useState } from 'react';
import { useMediaQuery } from 'react-responsive';
import { Link } from 'react-router-dom';
import styled, { css } from 'styled-components';

import { useRbac } from '../../hooks';
import { ADMIN_SECTIONS, AdminSection, adminPath } from '../../pages/Admin/sections';

// The bossa-arq-dashboard sidebar: 196px open, 64px collapsed.
const OPEN = 196;
const COLLAPSED = 64;
const STORAGE_KEY = 'playground:sidebar-collapsed';

export type NavPage = {
  path: string;
  label: string;
  icon: ReactNode;
  // Platform admins only (with RBAC on).
  admin?: boolean;
  // Platform admins and organization owners (with RBAC on).
  model?: boolean;
  // Shown under the "Administração" divider.
  section?: 'admin';
  // A group: the row opens and closes them instead of being a page itself.
  children?: NavPage[];
};

// Typed by section, so a new Admin page cannot go without an icon.
const ADMIN_ICONS: Record<AdminSection, ReactNode> = {
  organizations: <ApartmentOutlined />,
  workspaces: <AppstoreOutlined />,
  roles: <IdcardOutlined />,
  rules: <NodeIndexOutlined />,
  groups: <UsergroupAddOutlined />,
  users: <TeamOutlined />,
};

export const NAV_PAGES: NavPage[] = [
  { path: '/build', label: 'Playground', icon: <CaretRightOutlined /> },
  { path: '/schema', label: 'Data Model', icon: <ApiOutlined />, model: true },
  { path: '/data-sources', label: 'Data Source', icon: <DatabaseOutlined />, admin: true },
  { path: '/api', label: 'API', icon: <CodeOutlined /> },
  {
    path: '/admin',
    label: 'Admin',
    icon: <SafetyOutlined />,
    admin: true,
    section: 'admin',
    children: ADMIN_SECTIONS.map((s) => ({
      path: adminPath(s.key),
      label: s.label,
      icon: ADMIN_ICONS[s.key],
    })),
  },
];

const within = (pathname: string, path: string) => pathname === path || pathname.startsWith(`${path}/`);

// The pages a path is in, outermost first: /admin/users gives Admin, Users;
// /build?query=... and /schema/x count as /build and /schema.
export function pageTrail(pathname: string): NavPage[] {
  const page = NAV_PAGES.find((p) => within(pathname, p.path));
  if (!page) {
    return [];
  }
  const child = page.children?.find((c) => within(pathname, c.path));
  return child ? [page, child] : [page];
}

// The innermost page of a path.
export function pageFor(pathname: string): NavPage | undefined {
  const trail = pageTrail(pathname);
  return trail[trail.length - 1];
}

function readCollapsed(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === 'true';
  } catch {
    return false;
  }
}

function saveCollapsed(value: boolean) {
  try {
    window.localStorage.setItem(STORAGE_KEY, String(value));
  } catch {
    // Storage unavailable: the choice lasts until the page reloads.
  }
}

const Aside = styled.aside<{ $collapsed: boolean }>`
  position: sticky;
  top: 0;
  height: 100vh;
  width: ${(props) => (props.$collapsed ? COLLAPSED : OPEN)}px;
  flex: none;
  overflow-x: hidden;
  overflow-y: auto;
  padding: 0 12px;
  background: var(--pg-nav);
  color: var(--pg-nav-text);
  z-index: 10;
`;

const Brand = styled.div<{ $collapsed: boolean }>`
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 44px;
  margin: 0 2px 2px;

  ${(props) => props.$collapsed && `
    flex-direction: column;
    justify-content: center;
    height: 72px;
    gap: 4px;
  `}
`;

// Collapsed, only the mark at the logo's left edge shows.
const Logo = styled.span<{ $collapsed: boolean }>`
  display: block;
  overflow: hidden;
  width: ${(props) => (props.$collapsed ? '24px' : 'auto')};
  padding-left: ${(props) => (props.$collapsed ? '0' : '4px')};

  img {
    display: block;
    height: 22px;
    width: auto;
    max-width: none;
  }
`;

const CollapseButton = styled.button`
  display: grid;
  place-items: center;
  flex: none;
  width: 24px;
  height: 24px;
  padding: 0;
  border: 1.5px solid var(--pg-nav-text);
  border-radius: 50%;
  background: none;
  color: var(--pg-nav-text);
  font-size: 10px;
  cursor: pointer;

  &:focus-visible {
    outline: 2px solid var(--pg-nav-accent);
    outline-offset: 2px;
  }
`;

type RowProps = {
  // The current page: deep navy with teal text.
  $active: boolean;
  // A group holding the current page: teal text only.
  $current?: boolean;
  $collapsed: boolean;
  // A page inside a group, indented under it.
  $nested?: boolean;
};

const row = css<RowProps>`
  &&& {
    display: flex;
    align-items: center;
    justify-content: ${(props) => (props.$collapsed ? 'center' : 'flex-start')};
    gap: 10px;
    width: 100%;
    height: 30px;
    margin: 3.6px 0;
    padding: ${(props) => (props.$nested && !props.$collapsed ? '0 5px 0 23px' : '0 5px')};
    border: 0;
    border-radius: 8px;
    white-space: nowrap;
    font: inherit;
    font-size: ${(props) => (props.$nested ? '13px' : '14px')};
    text-align: left;
    cursor: pointer;
    color: ${(props) => (props.$active || props.$current ? 'var(--pg-nav-accent)' : 'var(--pg-nav-text)')};
    background: ${(props) => (props.$active ? 'var(--pg-nav-deep)' : 'transparent')};
  }

  &&&:hover {
    color: ${(props) => (props.$active || props.$current ? 'var(--pg-nav-accent)' : 'var(--pg-nav-text)')};
    background: ${(props) => (props.$active ? 'var(--pg-nav-deep)' : 'var(--pg-nav-hover)')};
  }

  &&&:focus-visible {
    outline: 2px solid var(--pg-nav-accent);
    outline-offset: -2px;
  }

  .anticon {
    flex: none;
    font-size: ${(props) => (props.$nested ? '15px' : '17px')};
  }
`;

const Item = styled(Link)<RowProps>`
  ${row}
`;

const GroupButton = styled.button<RowProps>`
  ${row}
`;

// The group's chevron, turned up when the group is open.
const Chevron = styled(DownOutlined)<{ $open: boolean }>`
  &&& {
    margin-left: auto;
    margin-right: 5px;
    font-size: 11px;
    transform: rotate(${(props) => (props.$open ? '180deg' : '0deg')});
  }
`;

const Divider = styled.div<{ $collapsed: boolean }>`
  display: flex;
  align-items: center;
  justify-content: ${(props) => (props.$collapsed ? 'center' : 'flex-start')};
  gap: 8px;
  margin: 7px -12px;
  padding: ${(props) => (props.$collapsed ? '7px 0' : '7px 21px')};
  border-top: 1px solid var(--pg-nav-line);
  border-bottom: 1px solid var(--pg-nav-line);
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.3px;
  white-space: nowrap;

  .anticon {
    font-size: 11px;
  }
`;

type Props = {
  pathname: string;
};

export function Sidebar({ pathname }: Props) {
  const { session, isAdmin, canModel } = useRbac();
  // Below a laptop the sidebar stays collapsed, so the page keeps its width.
  const narrow = useMediaQuery({ query: '(max-width: 991px)' });
  const [chosen, setChosen] = useState(readCollapsed);
  const collapsed = narrow || chosen;
  // The groups shown open, by path.
  const [open, setOpen] = useState<Set<string>>(() => new Set());

  useEffect(() => saveCollapsed(chosen), [chosen]);

  const trail = pageTrail(pathname);
  const active = trail[trail.length - 1];

  // Going to a page inside a group opens the group.
  useEffect(() => {
    if (trail.length > 1 && !open.has(trail[0].path)) {
      setOpen(new Set(open).add(trail[0].path));
    }
  }, [pathname]);

  const visible = NAV_PAGES.filter((page) => {
    if (page.section === 'admin') {
      // The Admin pages manage logins: there are none without RBAC.
      return session != null && isAdmin;
    }
    if (page.model) {
      return canModel;
    }
    return !page.admin || isAdmin;
  });
  const main = visible.filter((page) => page.section == null);
  const admin = visible.filter((page) => page.section === 'admin');

  function toggle(path: string) {
    const next = new Set(open);
    if (!next.delete(path)) {
      next.add(path);
    }
    setOpen(next);
  }

  const link = (page: NavPage, nested = false) => (
    <Item
      key={page.path}
      to={page.path}
      title={page.label}
      aria-current={page === active ? 'page' : undefined}
      $active={page === active}
      $collapsed={collapsed}
      $nested={nested}
    >
      {page.icon}
      {collapsed ? null : <span className="nav-label">{page.label}</span>}
    </Item>
  );

  const item = (page: NavPage) => {
    if (!page.children) {
      return link(page);
    }
    const isOpen = open.has(page.path);
    const listId = `nav-group${page.path.replace(/\//g, '-')}`;
    return (
      <div key={page.path}>
        <GroupButton
          type="button"
          title={page.label}
          aria-expanded={isOpen}
          aria-controls={listId}
          data-testid={`nav-group${page.path.replace(/\//g, '-')}`}
          onClick={() => toggle(page.path)}
          $active={false}
          $current={trail[0] === page}
          $collapsed={collapsed}
        >
          {page.icon}
          {collapsed ? null : <span className="nav-label">{page.label}</span>}
          {collapsed ? null : <Chevron $open={isOpen} />}
        </GroupButton>
        {isOpen ? (
          <div id={listId} role="group" aria-label={page.label}>
            {page.children.map((child) => link(child, true))}
          </div>
        ) : null}
      </div>
    );
  };

  return (
    <Aside $collapsed={collapsed} data-testid="sidebar">
      <Brand $collapsed={collapsed}>
        <Logo $collapsed={collapsed}>
          <img src="./cube-core-logo-adapted_for_dark_bg.svg" alt="Cube" />
        </Logo>
        {narrow ? null : (
          <CollapseButton
            type="button"
            data-testid="sidebar-toggle"
            aria-label={collapsed ? 'Expandir barra lateral' : 'Recolher barra lateral'}
            aria-expanded={!collapsed}
            onClick={() => setChosen(!chosen)}
          >
            {collapsed ? <RightOutlined /> : <LeftOutlined />}
          </CollapseButton>
        )}
      </Brand>

      <nav aria-label="Páginas">
        {main.map(item)}

        {admin.length > 0 ? (
          <>
            <Divider $collapsed={collapsed}>
              <DownOutlined />
              {collapsed ? null : <span>Administração</span>}
            </Divider>
            {admin.map(item)}
          </>
        ) : null}
      </nav>
    </Aside>
  );
}

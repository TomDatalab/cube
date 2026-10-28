import { MenuOutlined } from '@ant-design/icons';
import { Dropdown, Layout, Menu } from 'antd';
import { useMediaQuery } from 'react-responsive';
import { Link } from 'react-router-dom';
import styled from 'styled-components';

import { StyledMenu, StyledMenuItem } from './Menu';
import { RunOnCubeCloud } from './RunOnCubeCloud';

const StyledHeader = styled(Layout.Header)`
  && {
    background-color: var(--dark-02-color);
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

  return (
    <StyledHeader>
      <div style={{ float: 'left' }}>
        <img
          src="./cube-core-logo-adapted_for_dark_bg.svg"
          style={{ height: 28, marginRight: 28 }}
          alt=""
        />
      </div>

      {isDesktopOrLaptop && (
        <StyledMenu theme="light" mode="horizontal" selectedKeys={selectedKeys}>
          <StyledMenuItem key="/build">
            <Link to="/build">Playground</Link>
          </StyledMenuItem>

          <StyledMenuItem key="/schema">
            <Link to="/schema">Data Model</Link>
          </StyledMenuItem>

          <StyledMenuItem key="/data-sources">
            <Link to="/data-sources">Data Source</Link>
          </StyledMenuItem>

          <StyledMenuItem key="/frontend-integrations">
            <Link to="/frontend-integrations">Frontend Integrations</Link>
          </StyledMenuItem>

          <RunOnCubeCloud />
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

                <Menu.Item key="/schema">
                  <Link to="/schema">Data Model</Link>
                </Menu.Item>

                <Menu.Item key="/data-sources">
                  <Link to="/data-sources">Data Source</Link>
                </Menu.Item>
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

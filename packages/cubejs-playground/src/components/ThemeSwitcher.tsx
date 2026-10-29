import { BulbFilled, BulbOutlined, CheckOutlined, DesktopOutlined } from '@ant-design/icons';
import { Dropdown, Menu } from 'antd';
import styled from 'styled-components';

import { useTheme } from '../shared/useTheme';
import type { ThemePreference } from '../shared/theme';

const Trigger = styled.button`
  && {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 32px;
    width: 32px;
    padding: 0;
    background: transparent;
    border: 1px solid currentColor;
    border-radius: 4px;
    color: inherit;
    opacity: 0.85;
    cursor: pointer;
    transition: opacity 0.25s ease;
  }

  &&:hover,
  &&:focus-visible {
    opacity: 1;
  }
`;

const OPTIONS: { value: ThemePreference; label: string; icon: JSX.Element }[] = [
  { value: 'system', label: 'System', icon: <DesktopOutlined /> },
  { value: 'light', label: 'Light', icon: <BulbOutlined /> },
  { value: 'dark', label: 'Dark', icon: <BulbFilled /> },
];

// Independent of the app context, so it also works on the login page. The
// trigger takes the color of its container.
export function ThemeSwitcher() {
  const { preference, setPreference } = useTheme();
  const current = OPTIONS.find((option) => option.value === preference) ?? OPTIONS[0];

  return (
    <Dropdown
      trigger={['click']}
      overlay={(
        <Menu
          selectedKeys={[preference]}
          onClick={({ key }) => setPreference(key as ThemePreference)}
        >
          {OPTIONS.map((option) => (
            <Menu.Item key={option.value} icon={option.icon}>
              {option.label}
              {option.value === preference ? (
                <CheckOutlined style={{ marginLeft: 12 }} />
              ) : null}
            </Menu.Item>
          ))}
        </Menu>
      )}
    >
      <Trigger type="button" data-testid="theme-switcher" aria-label="Theme">
        {current.icon}
      </Trigger>
    </Dropdown>
  );
}

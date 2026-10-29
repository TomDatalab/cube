import { Alert, Button, Card, Form, Input } from 'antd';
import { useState } from 'react';
import styled from 'styled-components';

import { login, Session } from '../../shared/auth';

const Wrapper = styled.div`
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  background-color: var(--pg-bar, #43436b);
`;

const StyledCard = styled(Card)`
  width: 100%;
  max-width: 360px;
`;

type Props = {
  basePath: string;
  onLogin: (session: Session) => void;
};

// With RBAC on, every user logs in with the username and password RBAC knows
// them by; what they then see depends on their role.
export function LoginPage({ basePath, onLogin }: Props) {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(values: { username: string; password: string }) {
    setLoading(true);
    setError(null);
    try {
      onLogin(await login(basePath, values.username, values.password));
    } catch (e: any) {
      setError(e.message);
      setLoading(false);
    }
  }

  return (
    <Wrapper>
      <StyledCard
        title={(
          <img src="./cube-logo.svg" style={{ height: 28 }} alt="Cube" />
        )}
      >
        <Form layout="vertical" onFinish={submit} requiredMark={false}>
          <Form.Item
            label="Username"
            name="username"
            rules={[{ required: true, message: 'Enter your username' }]}
          >
            <Input autoComplete="username" autoFocus />
          </Form.Item>

          <Form.Item
            label="Password"
            name="password"
            rules={[{ required: true, message: 'Enter your password' }]}
          >
            <Input.Password autoComplete="current-password" />
          </Form.Item>

          {error ? (
            <Form.Item>
              <Alert type="error" message={error} showIcon />
            </Form.Item>
          ) : null}

          <Button type="primary" htmlType="submit" loading={loading} block>
            Log in
          </Button>
        </Form>
      </StyledCard>
    </Wrapper>
  );
}

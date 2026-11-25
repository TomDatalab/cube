import { FormEvent, useState } from 'react';
import { CircleAlert, Eye, EyeOff, Loader2 } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { ThemeSwitcher } from '../../components/ThemeSwitcher';
import { login, Session } from '../../shared/auth';

type Props = {
  basePath: string;
  onLogin: (session: Session) => void;
};

type Missing = { username?: string; password?: string };

/** What the form lacks before it can be sent: each empty field's message. */
export function missingFields(username: string, password: string): Missing {
  const missing: Missing = {};
  if (!username.trim()) {
    missing.username = 'Enter your username';
  }
  if (!password) {
    missing.password = 'Enter your password';
  }
  return missing;
}

// With RBAC on, every user logs in with the username and password RBAC knows
// them by; what they then see depends on their role.
export function LoginPage({ basePath, onLogin }: Props) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [missing, setMissing] = useState<Missing>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const lacking = missingFields(username, password);
    setMissing(lacking);
    if (lacking.username || lacking.password) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      onLogin(await login(basePath, username, password));
    } catch (e: any) {
      setError(e.message);
      setLoading(false);
    }
  }

  return (
    <div
      className="tw:relative tw:flex tw:min-h-screen tw:items-center tw:justify-center tw:p-4"
      style={{ backgroundColor: 'var(--pg-bar, #21263f)', color: 'var(--pg-bar-text, #fff)' }}
    >
      <div className="tw:absolute tw:top-4 tw:right-4">
        <ThemeSwitcher />
      </div>

      <Card className="tw:w-full tw:max-w-[360px] tw:gap-4 tw:py-5">
        <CardHeader className="tw:border-b tw:px-6 tw:pb-4">
          <img src="./cube-logo-bossa.svg" className="tw:h-7 tw:w-auto tw:self-start" alt="Cube" />
        </CardHeader>
        <CardContent className="tw:px-6">
          <form className="tw:flex tw:flex-col tw:gap-4" onSubmit={submit} noValidate>
            <div className="tw:flex tw:flex-col tw:gap-2">
              <Label htmlFor="login-username">Username</Label>
              <Input
                id="login-username"
                autoComplete="username"
                autoFocus
                value={username}
                aria-invalid={Boolean(missing.username)}
                onChange={(e) => {
                  setUsername(e.target.value);
                  setMissing((m) => ({ ...m, username: undefined }));
                }}
              />
              {missing.username && <p className="tw:text-destructive tw:text-sm">{missing.username}</p>}
            </div>

            <div className="tw:flex tw:flex-col tw:gap-2">
              <Label htmlFor="login-password">Password</Label>
              <div className="tw:relative">
                <Input
                  id="login-password"
                  type={showPassword ? 'text' : 'password'}
                  autoComplete="current-password"
                  className="tw:pr-9"
                  value={password}
                  aria-invalid={Boolean(missing.password)}
                  onChange={(e) => {
                    setPassword(e.target.value);
                    setMissing((m) => ({ ...m, password: undefined }));
                  }}
                />
                <button
                  type="button"
                  data-slot="password-toggle"
                  className="tw:text-muted-foreground tw:hover:text-foreground tw:absolute tw:inset-y-0 tw:right-0 tw:flex tw:w-9 tw:cursor-pointer tw:items-center tw:justify-center"
                  aria-label={showPassword ? 'Hide password' : 'Show password'}
                  onClick={() => setShowPassword((shown) => !shown)}
                >
                  {showPassword ? <EyeOff className="tw:size-4" /> : <Eye className="tw:size-4" />}
                </button>
              </div>
              {missing.password && <p className="tw:text-destructive tw:text-sm">{missing.password}</p>}
            </div>

            {error ? (
              <Alert variant="destructive">
                <CircleAlert />
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            ) : null}

            <Button type="submit" className="tw:w-full" disabled={loading}>
              {loading && <Loader2 className="tw:animate-spin" />}
              Log in
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}

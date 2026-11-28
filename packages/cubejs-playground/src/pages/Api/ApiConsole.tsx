import { useEffect, useState } from 'react';
import { CircleAlert, Eye, EyeOff, Info, Send } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Field, SimpleSelect, Spinner } from '@/components/kit';
import { cn } from '@/lib/utils';

import { usePlaygroundContext, useToken } from '../../hooks';
import { authHeaders, getOrganizationScope, getWorkspace, ORGANIZATION_HEADER, WORKSPACE_HEADER } from '../../shared/auth';
import { apiFetch } from '../../shared/helpers';
import {
  Endpoint, fillPath, Method, pathParams,
} from './catalog';
import { CodeBlock, MethodBadge } from './parts';

const METHODS: Method[] = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'];

type Result = { status: number; statusText: string; ms: number; body: string };

/** The base the console sends to: the API prefix, or the server root for absolute paths. */
export function requestUrl(origin: string, basePath: string, path: string, absolute: boolean) {
  const clean = path.startsWith('/') ? path : `/${path}`;
  return `${origin}${absolute ? '' : basePath}${clean}`;
}

/** The curl that does what the console would send; the token itself is left out. */
export function curlFor(opts: {
  method: Method;
  url: string;
  hasToken: boolean;
  scopeHeader: [string, string] | null;
  body: string;
}) {
  const { method, url, hasToken, scopeHeader, body } = opts;
  const lines = [`curl -X ${method}`];
  if (hasToken) {
    lines.push('  -H "Authorization: <token>"');
  }
  if (scopeHeader) {
    lines.push(`  -H "${scopeHeader[0]}: ${scopeHeader[1]}"`);
  }
  if (method !== 'GET' && body.trim()) {
    lines.push('  -H "Content-Type: application/json"');
    lines.push(`  -d '${body.replace(/\s*\n\s*/g, ' ').replace(/'/g, "'\\''")}'`);
  }
  lines.push(`  '${url}'`);
  return lines.join(' \\\n');
}

const statusClass = (status: number) => {
  if (status < 300) return 'tw:bg-emerald-500/15 tw:text-emerald-700 tw:dark:text-emerald-300';
  if (status < 500) return 'tw:bg-amber-500/15 tw:text-amber-700 tw:dark:text-amber-300';
  return 'tw:bg-destructive/15 tw:text-destructive';
};

// The request on the right of the API page: what the chosen endpoint sends,
// with its path parameters, the token and a JSON body; the answer below.
export function ApiConsole({ endpoint }: { endpoint: Endpoint | null }) {
  const context = usePlaygroundContext();
  const basePath = (context?.basePath || '').replace(/\/$/, '');
  const { origin } = window.location;
  // The login's token with RBAC on; else the one set on the Playground page
  // or minted for it.
  const playgroundToken = useToken();
  const appToken = authHeaders().Authorization || playgroundToken || context?.cubejsToken || '';

  const [method, setMethod] = useState<Method>('GET');
  const [template, setTemplate] = useState('/v1/meta');
  const [absolute, setAbsolute] = useState(false);
  const [params, setParams] = useState<Record<string, string>>({});
  const [query, setQuery] = useState('');
  const [body, setBody] = useState('');
  const [token, setToken] = useState('');
  const [showToken, setShowToken] = useState(false);
  const [sending, setSending] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (appToken && !token) {
      setToken(appToken);
    }
  }, [appToken]);

  useEffect(() => {
    if (!endpoint) {
      return;
    }
    setMethod(endpoint.method);
    setTemplate(endpoint.path);
    setAbsolute(Boolean(endpoint.absolute));
    setParams({});
    setQuery(endpoint.query ?? '');
    setBody(endpoint.body === undefined ? '' : JSON.stringify(endpoint.body, null, 2));
    setResult(null);
    setError(null);
  }, [endpoint]);

  const names = pathParams(template);
  const path = `${fillPath(template, params)}${query}`;
  const missing = names.filter((n) => !params[n]);
  const url = requestUrl(origin, basePath, path, absolute);
  const workspace = getWorkspace();
  const organization = getOrganizationScope();
  let scopeHeader: [string, string] | null = null;
  if (workspace) {
    scopeHeader = [WORKSPACE_HEADER, workspace];
  } else if (organization) {
    scopeHeader = [ORGANIZATION_HEADER, organization];
  }
  const isSocket = !absolute && template === '/ws';

  async function send() {
    setError(null);
    let payload: string | undefined;
    if (method !== 'GET' && body.trim()) {
      try {
        payload = JSON.stringify(JSON.parse(body));
      } catch (e: any) {
        setError(`The body is not valid JSON: ${e.message}`);
        return;
      }
    }
    setSending(true);
    const started = performance.now();
    try {
      const response = await apiFetch(url, {
        method,
        headers: {
          ...(token ? { Authorization: token } : {}),
          ...(payload ? { 'Content-Type': 'application/json' } : {}),
        },
        body: payload,
      });
      const text = await response.text();
      let pretty = text;
      try {
        pretty = JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        // Not JSON: shown as it came.
      }
      setResult({
        status: response.status,
        statusText: response.statusText,
        ms: Math.round(performance.now() - started),
        body: pretty,
      });
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="tw:flex tw:flex-col tw:gap-3 tw:rounded-lg tw:border tw:bg-card tw:p-4 tw:text-sm" data-testid="api-console">
      <div className="tw:flex tw:items-center tw:justify-between">
        <h3 className="tw:text-base tw:font-semibold">Console</h3>
        {endpoint ? <MethodBadge method={method} /> : null}
      </div>

      <div className="tw:flex tw:gap-2">
        <SimpleSelect
          className="tw:w-28"
          value={method}
          onChange={(v) => v && setMethod(v as Method)}
          options={METHODS.map((m) => ({ value: m, label: m }))}
        />
        <div className="tw:flex tw:min-w-0 tw:flex-1 tw:items-center tw:rounded-md tw:border tw:border-input">
          {!absolute && basePath ? (
            <span className="tw:shrink-0 tw:border-r tw:bg-muted tw:px-2 tw:py-2 tw:font-mono tw:text-xs tw:text-muted-foreground">{basePath}</span>
          ) : null}
          <input
            data-slot="console-path"
            aria-label="Path"
            className="tw:min-w-0 tw:flex-1 tw:bg-transparent tw:px-2 tw:py-1.5 tw:font-mono tw:text-xs tw:outline-none"
            value={template + query}
            onChange={(e) => {
              const [p, ...rest] = e.target.value.split('?');
              setTemplate(p);
              setQuery(rest.length ? `?${rest.join('?')}` : '');
            }}
            onKeyDown={(e) => e.key === 'Enter' && !missing.length && send()}
          />
        </div>
      </div>

      {names.length ? (
        <div className="tw:grid tw:grid-cols-2 tw:gap-2">
          {names.map((name) => (
            <Field key={name} label={`:${name}`} htmlFor={`param-${name}`}>
              <Input
                id={`param-${name}`}
                className="tw:h-8 tw:font-mono tw:text-xs"
                placeholder={name === 'user' ? 'UUID or username' : name}
                value={params[name] ?? ''}
                onChange={(e) => setParams({ ...params, [name]: e.target.value })}
              />
            </Field>
          ))}
        </div>
      ) : null}

      <Field label="Authorization" htmlFor="console-token">
        <div className="tw:relative">
          <Input
            id="console-token"
            type={showToken ? 'text' : 'password'}
            className="tw:pr-9 tw:font-mono tw:text-xs"
            placeholder="A token: from login, or a JWT signed with the API secret"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
          <button
            type="button"
            data-slot="token-toggle"
            aria-label={showToken ? 'Hide token' : 'Show token'}
            className="tw:absolute tw:inset-y-0 tw:right-0 tw:flex tw:w-9 tw:cursor-pointer tw:items-center tw:justify-center tw:text-muted-foreground tw:hover:text-foreground"
            onClick={() => setShowToken((s) => !s)}
          >
            {showToken ? <EyeOff className="tw:size-4" /> : <Eye className="tw:size-4" />}
          </button>
        </div>
      </Field>
      <p className="tw:m-0 tw:text-xs tw:text-muted-foreground">
        {scopeHeader
          ? `Sent with ${scopeHeader[0]}: ${scopeHeader[1]}, the scope chosen in the header.`
          : 'No workspace header: the scope chosen in the header is the Platform.'}
      </p>

      {method !== 'GET' ? (
        <Field label="Body (JSON)" htmlFor="console-body">
          <Textarea
            id="console-body"
            className="tw:min-h-28 tw:font-mono tw:text-xs"
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
        </Field>
      ) : null}

      {isSocket ? (
        <Alert>
          <Info />
          <AlertDescription>
            {`A WebSocket: connect with a WebSocket client to ${url.replace(/^http/, 'ws')} and send {"authorization": "<token>"} first.`}
          </AlertDescription>
        </Alert>
      ) : null}

      <Button onClick={send} disabled={sending || missing.length > 0 || isSocket}>
        {sending ? <Spinner className="tw:text-current" /> : <Send />}
        {missing.length ? `Fill in ${missing.map((n) => `:${n}`).join(', ')}` : 'Send'}
      </Button>

      <div>
        <div className="tw:mb-1 tw:text-xs tw:text-muted-foreground">Equivalent command</div>
        <CodeBlock code={curlFor({
          method, url, hasToken: Boolean(token), scopeHeader, body,
        })}
        />
      </div>

      {error ? (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      {result ? (
        <div data-testid="api-console-result">
          <div className="tw:mb-1 tw:flex tw:items-center tw:gap-2">
            <span className={cn('tw:rounded tw:px-1.5 tw:py-0.5 tw:font-mono tw:text-xs', statusClass(result.status))} data-testid="api-console-status">
              {`${result.status} ${result.statusText}`}
            </span>
            <span className="tw:text-xs tw:text-muted-foreground">{`${result.ms} ms`}</span>
          </div>
          <CodeBlock code={result.body || '(empty body)'} maxHeight={420} />
        </div>
      ) : null}
    </div>
  );
}

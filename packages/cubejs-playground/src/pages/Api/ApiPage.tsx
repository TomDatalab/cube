import { useMemo, useState } from 'react';

import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { EmptyState, SearchInput } from '@/components/kit';
import { cn } from '@/lib/utils';

import { usePlaygroundContext, useRbac } from '../../hooks';
import { Content, Header } from '../components/Ui';
import { ApiConsole } from './ApiConsole';
import {
  Endpoint, endpointKey, filterSections, SECTIONS,
} from './catalog';
import { CodeBlock, MethodBadge } from './parts';

const OVERVIEW = 'overview';
const SDKS = 'sdks';

function scrollTo(key: string) {
  document.getElementById(`api-${key}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
}

/** The client libraries' setup, with this server's address. */
export function sdkSnippets(apiUrl: string) {
  const query = '{ measures: [\'Orders.count\'] }';
  const init = `import cube from '@cubejs-client/core';

const cubeApi = cube('<token>', { apiUrl: '${apiUrl}/v1' });`;
  return [
    {
      key: 'terminal',
      label: 'Terminal',
      blocks: [
        ['REST', `curl -H "Authorization: <token>" \\
  -G --data-urlencode 'query={"measures":["Orders.count"]}' \\
  ${apiUrl}/v1/load`],
        ['GraphQL', `curl -H "Authorization: <token>" -H "Content-Type: application/json" \\
  -d '{"query":"{ cube { orders { count } } }"}' \\
  ${apiUrl}/graphql`],
      ],
    },
    {
      key: 'js',
      label: 'JavaScript',
      blocks: [
        ['Set up the client', init],
        ['Run a query', `const resultSet = await cubeApi.load(${query});
console.log(resultSet.tablePivot());`],
      ],
    },
    {
      key: 'react',
      label: 'React',
      blocks: [
        ['Set up the client', init],
        ['Provide it', `import { CubeProvider } from '@cubejs-client/react';

<CubeProvider cubeApi={cubeApi}>{/* your app */}</CubeProvider>`],
        ['Query in a component', `import { useCubeQuery } from '@cubejs-client/react';

const { resultSet, isLoading, error } = useCubeQuery(${query});`],
      ],
    },
    {
      key: 'vue',
      label: 'Vue',
      blocks: [
        ['Set up the client', init],
        ['Query', `import { QueryBuilder } from '@cubejs-client/vue3';
// <query-builder :cubeApi="cubeApi" :query="${query}" />`],
      ],
    },
    {
      key: 'angular',
      label: 'Angular',
      blocks: [
        ['Module options', `CubeClientModule.forRoot({
  token: '<token>',
  options: { apiUrl: '${apiUrl}/v1' },
})`],
      ],
    },
  ];
}

// Every endpoint of the server by section, a console to call them, and the
// client libraries' setup.
export function ApiPage() {
  const context = usePlaygroundContext();
  const { enabled } = useRbac();
  const basePath = (context?.basePath || '').replace(/\/$/, '');
  const { origin } = window.location;
  const apiUrl = `${origin}${basePath}`;
  const wsUrl = `${apiUrl.replace(/^http/, 'ws')}/ws`;

  const [q, setQ] = useState('');
  const [selected, setSelected] = useState<Endpoint | null>(null);
  const sections = useMemo(() => filterSections(SECTIONS, q), [q]);
  const total = SECTIONS.reduce((n, s) => n + s.endpoints.length, 0);

  const nav = [
    { key: OVERVIEW, title: 'Overview' },
    ...sections.map((s) => ({ key: s.key, title: s.title, count: s.endpoints.length })),
    { key: SDKS, title: 'Client libraries' },
  ];

  return (
    <div className="tw:text-foreground">
      <Header>
        <h1 className="tw:text-3xl tw:font-semibold">API</h1>
      </Header>
      <Content className="tw:pb-12">
        <div className="tw:grid tw:grid-cols-1 tw:gap-6 tw:xl:grid-cols-[180px_minmax(0,1fr)_minmax(360px,440px)]">
          <nav aria-label="API sections" className="tw:hidden tw:xl:block">
            <ul className="tw:sticky tw:top-16 tw:m-0 tw:flex tw:list-none tw:flex-col tw:gap-0.5 tw:p-0 tw:text-sm">
              {nav.map((item) => (
                <li key={item.key}>
                  <button
                    type="button"
                    data-slot="api-nav"
                    className="tw:flex tw:w-full tw:cursor-pointer tw:items-center tw:justify-between tw:rounded-md tw:px-2 tw:py-1 tw:text-left tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground"
                    onClick={() => scrollTo(item.key)}
                  >
                    <span>{item.title}</span>
                    {'count' in item ? <span className="tw:text-xs">{item.count}</span> : null}
                  </button>
                </li>
              ))}
            </ul>
          </nav>

          <main className="tw:flex tw:min-w-0 tw:flex-col tw:gap-8">
            <section id={`api-${OVERVIEW}`} className="tw:flex tw:flex-col tw:gap-3 tw:text-sm">
              <p className="tw:m-0">
                {`${total} endpoints of this server, by what they do. Pick one to load it into the console, fill in its parameters and send it.`}
              </p>
              <dl className="tw:m-0 tw:grid tw:grid-cols-[max-content_minmax(0,1fr)] tw:items-center tw:gap-x-4 tw:gap-y-2">
                <dt className="tw:text-muted-foreground">REST</dt>
                <dd className="tw:m-0"><CodeBlock code={`${apiUrl}/v1`} /></dd>
                <dt className="tw:text-muted-foreground">GraphQL</dt>
                <dd className="tw:m-0"><CodeBlock code={`${apiUrl}/graphql`} /></dd>
                <dt className="tw:text-muted-foreground">WebSocket</dt>
                <dd className="tw:m-0"><CodeBlock code={wsUrl} /></dd>
              </dl>
              <div className="tw:rounded-md tw:border tw:bg-muted/40 tw:p-3">
                <strong className="tw:font-semibold">Authentication</strong>
                {enabled ? (
                  <ul className="tw:m-0 tw:mt-1 tw:list-disc tw:pl-5">
                    <li>
                      Log in with POST /v1/auth/login and send the token as Authorization: Bearer &lt;token&gt;;
                      programs use a service token (POST /v1/users/:id/tokens).
                    </li>
                    <li>
                      Data requests of anyone but a platform admin name their workspace in X-Cube-Workspace;
                      a platform admin may name an organization&apos;s model in X-Cube-Organization.
                    </li>
                    <li>What a user may do follows their roles: the &quot;Who&quot; of each endpoint.</li>
                  </ul>
                ) : (
                  <ul className="tw:m-0 tw:mt-1 tw:list-disc tw:pl-5">
                    <li>Send a JWT signed with the API secret in Authorization.</li>
                    <li>
                      Its scopes (meta, data, graphql, jobs...) decide what it reaches; data_sources and
                      execute come only from CUBEJS_DEFAULT_API_SCOPES.
                    </li>
                  </ul>
                )}
              </div>
              <SearchInput
                placeholder="Search endpoints: path, method, what they do"
                value={q}
                onChange={setQ}
              />
            </section>

            {sections.length ? sections.map((section) => (
              <section key={section.key} id={`api-${section.key}`} className="tw:scroll-mt-16">
                <h2 className="tw:m-0 tw:text-xl tw:font-semibold">{section.title}</h2>
                <p className="tw:mt-1 tw:mb-3 tw:text-sm tw:text-muted-foreground">{section.description}</p>
                <ul className="tw:m-0 tw:list-none tw:divide-y tw:divide-border tw:rounded-md tw:border tw:p-0">
                  {section.endpoints.map((e) => {
                    const active = selected != null && endpointKey(selected) === endpointKey(e);
                    return (
                      <li key={endpointKey(e)}>
                        <button
                          type="button"
                          data-slot="api-endpoint"
                          aria-pressed={active}
                          className={cn(
                            'tw:grid tw:w-full tw:cursor-pointer tw:grid-cols-[auto_minmax(0,1fr)] tw:items-start tw:gap-x-3 tw:gap-y-0.5 tw:px-3 tw:py-2 tw:text-left tw:text-sm tw:hover:bg-accent',
                            active && 'tw:bg-accent'
                          )}
                          onClick={() => setSelected(e)}
                        >
                          <MethodBadge method={e.method} className="tw:mt-0.5" />
                          <span className="tw:min-w-0">
                            <code className="tw:font-mono tw:text-xs tw:break-all">{e.path}</code>
                            <span className="tw:block tw:text-muted-foreground">{e.summary}</span>
                          </span>
                          <span />
                          <span className="tw:text-xs tw:text-muted-foreground">{`Who: ${e.who}`}</span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </section>
            )) : <EmptyState>{`No endpoint matches "${q}".`}</EmptyState>}

            <section id={`api-${SDKS}`} className="tw:scroll-mt-16">
              <h2 className="tw:m-0 tw:text-xl tw:font-semibold">Client libraries</h2>
              <p className="tw:mt-1 tw:mb-3 tw:text-sm tw:text-muted-foreground">
                Cube&apos;s JavaScript client and its framework bindings, pointed at this server.
              </p>
              <Tabs defaultValue="terminal">
                <TabsList>
                  {sdkSnippets(apiUrl).map((t) => <TabsTrigger key={t.key} value={t.key}>{t.label}</TabsTrigger>)}
                </TabsList>
                {sdkSnippets(apiUrl).map((t) => (
                  <TabsContent key={t.key} value={t.key} className="tw:flex tw:flex-col tw:gap-3">
                    {t.blocks.map(([title, code]) => (
                      <div key={title}>
                        <div className="tw:mb-1 tw:text-sm tw:font-medium">{title}</div>
                        <CodeBlock code={code} />
                      </div>
                    ))}
                  </TabsContent>
                ))}
              </Tabs>
            </section>
          </main>

          <aside className="tw:min-w-0">
            <div className="tw:sticky tw:top-16">
              <ApiConsole endpoint={selected} />
            </div>
          </aside>
        </div>
      </Content>
    </div>
  );
}

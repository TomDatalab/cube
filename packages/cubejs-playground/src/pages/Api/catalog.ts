// Every endpoint of this server's API, by section, for the API page.
// `api-catalog.test.ts` checks it against the server's routes, so a new
// route without an entry here fails the tests.

export type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

export type Endpoint = {
  method: Method;
  // Relative to the API prefix (the base path, e.g. `/cube`), unless
  // `absolute`. `:name` segments are filled in on the console.
  path: string;
  absolute?: boolean;
  summary: string;
  // Who may call it: a token scope, a role, or "anyone".
  who: string;
  body?: unknown;
  // Query string to start from, e.g. `?q=&limit=50`.
  query?: string;
};

export type Section = {
  key: string;
  title: string;
  description: string;
  endpoints: Endpoint[];
};

const ORDERS_QUERY = { query: { measures: ['Orders.count'], dimensions: ['Orders.status'], limit: 10 } };

export const SECTIONS: Section[] = [
  {
    key: 'data',
    title: 'Data queries',
    description:
      'Read the data model and run queries in Cube\'s JSON format. With RBAC on, every call of a '
      + 'non-admin needs X-Cube-Workspace and sees only the cubes released to that workspace.',
    endpoints: [
      { method: 'GET', path: '/v1/meta', summary: 'The data model: cubes, views, their members', who: 'meta scope' },
      { method: 'GET', path: '/v1/meta/groups', summary: 'The model\'s folders and groups of members', who: 'meta scope' },
      { method: 'POST', path: '/v1/load', summary: 'Run a query and get its rows', who: 'data scope', body: ORDERS_QUERY },
      { method: 'GET', path: '/v1/load', summary: 'Run a query given in the query string', who: 'data scope', query: '?query={"measures":["Orders.count"]}' },
      { method: 'POST', path: '/v1/subscribe', summary: 'The same as load, the path the JS client polls', who: 'data scope', body: ORDERS_QUERY },
      { method: 'GET', path: '/v1/subscribe', summary: 'Subscribe with the query in the query string', who: 'data scope', query: '?query={"measures":["Orders.count"]}' },
      { method: 'POST', path: '/v1/sql', summary: 'The SQL a query would run, without running it', who: 'data scope', body: ORDERS_QUERY },
      { method: 'GET', path: '/v1/sql', summary: 'The SQL of a query given in the query string', who: 'data scope', query: '?query={"measures":["Orders.count"]}' },
      { method: 'POST', path: '/v1/dry-run', summary: 'Check and normalize a query', who: 'data scope', body: ORDERS_QUERY },
      { method: 'GET', path: '/v1/dry-run', summary: 'Dry run of a query in the query string', who: 'data scope', query: '?query={"measures":["Orders.count"]}' },
      { method: 'DELETE', path: '/v1/running-query/:requestId', summary: 'Cancel a running query by its request id', who: 'data scope' },
    ],
  },
  {
    key: 'graphql',
    title: 'GraphQL and real time',
    description: 'The same data through GraphQL, and pushed over a WebSocket.',
    endpoints: [
      {
        method: 'POST', path: '/graphql', summary: 'A GraphQL query', who: 'graphql scope', body: { query: '{ cube { orders { count status } } }' },
      },
      { method: 'GET', path: '/graphql', summary: 'GraphQL with the query in the query string (and the GraphiQL page)', who: 'graphql scope', query: '?query={ __schema { queryType { name } } }' },
      {
        method: 'POST', path: '/v1/graphql-to-json', summary: 'The JSON query a GraphQL query becomes', who: 'graphql scope', body: { query: '{ cube { orders { count } } }' },
      },
      { method: 'GET', path: '/ws', summary: 'WebSocket: subscriptions and real-time results (open it with a WebSocket client)', who: 'data scope' },
    ],
  },
  {
    key: 'sql',
    title: 'SQL',
    description: 'Query the model in SQL, and turn SQL into Cube queries.',
    endpoints: [
      {
        method: 'POST', path: '/v1/cubesql', summary: 'Run SQL against the model (rows back)', who: 'platform admin', body: { query: 'SELECT status, MEASURE(count) FROM orders GROUP BY 1' },
      },
      {
        method: 'POST', path: '/v1/convert-query', summary: 'Convert SQL to a Cube query', who: 'platform admin', body: { input: 'sql', query: 'SELECT status FROM orders' },
      },
    ],
  },
  {
    key: 'model',
    title: 'Data model files',
    description:
      'The YAML/Jinja files of the model, per organization and for the Platform, kept in the database. '
      + 'A write reloads the model at once.',
    endpoints: [
      { method: 'GET', path: '/v1/organizations/:id/model', summary: 'An organization\'s files, cubes, tables and problems', who: 'org owner' },
      { method: 'GET', path: '/v1/organizations/:id/model/files/:file', summary: 'One file of an organization', who: 'org owner' },
      {
        method: 'PUT', path: '/v1/organizations/:id/model/files/:file', summary: 'Write a file (with the version read, to catch a concurrent change)', who: 'org owner', body: { content: 'cubes: []\n', version: 1 },
      },
      { method: 'DELETE', path: '/v1/organizations/:id/model/files/:file', summary: 'Delete a file', who: 'org owner' },
      {
        method: 'POST', path: '/v1/organizations/:id/model/generate', summary: 'Generate cubes from tables of the organization\'s connections', who: 'org owner', body: { tables: [['public', 'orders']] },
      },
      { method: 'GET', path: '/v1/platform/model', summary: 'The Platform\'s files, cubes and problems', who: 'platform admin' },
      { method: 'GET', path: '/v1/platform/model/files/:file', summary: 'One Platform file', who: 'platform admin' },
      {
        method: 'PUT', path: '/v1/platform/model/files/:file', summary: 'Write a Platform file', who: 'platform admin', body: { content: 'cubes: []\n', version: 1 },
      },
      { method: 'DELETE', path: '/v1/platform/model/files/:file', summary: 'Delete a Platform file', who: 'platform admin' },
      {
        method: 'POST', path: '/v1/platform/model/generate', summary: 'Generate Platform cubes from tables', who: 'platform admin', body: { tables: [['public', 'orders']] },
      },
      {
        method: 'PUT', path: '/v1/model/files/:file/organization', summary: 'Move a file to another organization (or the Platform)', who: 'platform admin', body: { organizationId: null },
      },
      { method: 'GET', path: '/v1/model/export', summary: 'Every file of the model, to back it up', who: 'platform admin' },
      { method: 'GET', path: '/v1/model/sync/:connection/context', summary: 'What a model sync of a connection starts from', who: 'sync user' },
      {
        method: 'PUT', path: '/v1/model/sync/:connection', summary: 'Replace the files a connection\'s sync manages', who: 'sync user', body: { files: [{ path: 'orders.yml', content: 'cubes: []\n' }], context: { taken: [], covered: {} } },
      },
    ],
  },
  {
    key: 'data-sources',
    title: 'Data sources and execution',
    description:
      'The database connections the server reads, their settings (secrets included) and running '
      + 'statements on them. These scopes are granted only through CUBEJS_DEFAULT_API_SCOPES.',
    endpoints: [
      { method: 'GET', path: '/v1/data-sources', summary: 'Every data source, with its owner', who: 'data_sources scope, platform admin' },
      {
        method: 'POST', path: '/v1/data-sources', summary: 'Add a data source (with RBAC on, for an organization)', who: 'data_sources scope, platform admin', body: { name: 'example', organizationId: '<organization id>', variables: { CUBEJS_DB_TYPE: 'sqlite', CUBEJS_DB_NAME: '/tmp/cube-example.db' } },
      },
      { method: 'POST', path: '/v1/data-sources/test', summary: 'Test a connection', who: 'data_sources scope, platform admin', body: { dataSource: 'default' } },
      { method: 'GET', path: '/v1/data-sources/:name', summary: 'One data source\'s settings', who: 'data_sources scope, platform admin' },
      {
        method: 'PUT', path: '/v1/data-sources/:name', summary: 'Edit a data source', who: 'data_sources scope, platform admin', body: { variables: { CUBEJS_DB_NAME: '/tmp/cube-example-2.db' } },
      },
      { method: 'DELETE', path: '/v1/data-sources/:name', summary: 'Delete a data source', who: 'data_sources scope, platform admin' },
      { method: 'GET', path: '/v1/connectors', summary: 'The database types this server can connect to', who: 'meta scope' },
      {
        method: 'POST', path: '/v1/execute', summary: 'Run a statement on a connection (DDL, tables, views...)', who: 'execute scope, workspace owner', body: { connection: 'default', query: 'SELECT 1' },
      },
    ],
  },
  {
    key: 'pre-aggregations',
    title: 'Pre-aggregations',
    description: 'Rollups: whether a query can use one, and building them.',
    endpoints: [
      { method: 'POST', path: '/v1/pre-aggregations/can-use', summary: 'Whether a query can be served by a pre-aggregation', who: 'platform admin', body: ORDERS_QUERY },
      { method: 'POST', path: '/v1/pre-aggregations/jobs', summary: 'Start building pre-aggregations, or read the jobs', who: 'jobs scope, platform admin', body: { action: 'post', selector: { contexts: [{ securityContext: {} }], timezones: ['UTC'] } } },
      { method: 'GET', path: '/cube-system/v1/pre-aggregations', absolute: true, summary: 'Every pre-aggregation of the model', who: 'platform admin, Playground token' },
      { method: 'GET', path: '/cube-system/v1/pre-aggregations/security-contexts', absolute: true, summary: 'The security contexts pre-aggregations are built for', who: 'platform admin, Playground token' },
      { method: 'GET', path: '/cube-system/v1/pre-aggregations/timezones', absolute: true, summary: 'The time zones they are built for', who: 'platform admin, Playground token' },
      { method: 'POST', path: '/cube-system/v1/pre-aggregations/partitions', absolute: true, summary: 'The partitions of pre-aggregations', who: 'platform admin, Playground token', body: { query: { preAggregations: [] } } },
      { method: 'POST', path: '/cube-system/v1/pre-aggregations/preview', absolute: true, summary: 'Preview a pre-aggregation\'s rows', who: 'platform admin, Playground token', body: { query: {} } },
      { method: 'POST', path: '/cube-system/v1/pre-aggregations/build', absolute: true, summary: 'Build pre-aggregations now', who: 'platform admin, Playground token', body: { query: {} } },
      { method: 'POST', path: '/cube-system/v1/pre-aggregations/queue', absolute: true, summary: 'The build queue', who: 'platform admin, Playground token', body: { query: {} } },
      { method: 'POST', path: '/cube-system/v1/pre-aggregations/cancel', absolute: true, summary: 'Cancel queued builds', who: 'platform admin, Playground token', body: { query: {} } },
    ],
  },
  {
    key: 'auth',
    title: 'Authentication',
    description:
      'With RBAC on, a user logs in with username and password and sends the token as '
      + '"Authorization: Bearer <token>". Service tokens are for programs.',
    endpoints: [
      {
        method: 'POST', path: '/v1/auth/login', summary: 'Log in: a token, its expiry and the user', who: 'anyone', body: { username: 'ana', password: 'her-password' },
      },
      {
        method: 'PUT', path: '/v1/auth/password', summary: 'Change one\'s own password', who: 'the user', body: { currentPassword: 'old-password', newPassword: 'a-new-long-password' },
      },
      {
        method: 'POST', path: '/v1/users/:id/tokens', summary: 'A service token for a user, valid as long as a login', who: 'platform admin',
      },
    ],
  },
  {
    key: 'users',
    title: 'Users',
    description: 'Accounts. There is no delete: disable a user instead.',
    endpoints: [
      { method: 'GET', path: '/v1/users', summary: 'Search users', who: 'platform admin', query: '?q=&limit=50&offset=0' },
      {
        method: 'POST', path: '/v1/users', summary: 'Create a user', who: 'platform admin', body: { username: 'ana', name: 'Ana', email: 'ana@example.com', password: 'a-long-password' },
      },
      {
        method: 'PATCH', path: '/v1/users/:id', summary: 'Edit or disable a user', who: 'platform admin', body: { name: 'Ana Maria', disabled: false },
      },
      {
        method: 'PUT', path: '/v1/users/:id/password', summary: 'Set a user\'s password', who: 'platform admin', body: { password: 'a-new-long-password' },
      },
      { method: 'GET', path: '/v1/users/:id/attributes', summary: 'A user\'s attributes (read by row filters)', who: 'platform admin' },
      {
        method: 'PUT', path: '/v1/users/:id/attributes', summary: 'Replace a user\'s attributes', who: 'platform admin', body: { attributes: { region: ['south'] } },
      },
      { method: 'GET', path: '/v1/users/:id/access', summary: 'What the user can reach, and why (the access graph)', who: 'platform admin' },
    ],
  },
  {
    key: 'organizations',
    title: 'Organizations and connections',
    description: 'Organizations own workspaces, connections, roles, groups and their part of the model.',
    endpoints: [
      { method: 'GET', path: '/v1/organizations', summary: 'Search the organizations the caller sees', who: 'any member', query: '?q=&limit=50&offset=0' },
      {
        method: 'POST', path: '/v1/organizations', summary: 'Create an organization with its first owner', who: 'platform admin', body: { name: 'Acme', owner: 'ana' },
      },
      { method: 'GET', path: '/v1/organizations/:id', summary: 'One organization', who: 'any member' },
      { method: 'PATCH', path: '/v1/organizations/:id', summary: 'Rename an organization', who: 'org owner', body: { name: 'Acme Inc.' } },
      { method: 'DELETE', path: '/v1/organizations/:id', summary: 'Delete an empty organization', who: 'org owner' },
      { method: 'PUT', path: '/v1/organizations/:id/owners/:user', summary: 'Make a user an owner', who: 'org owner' },
      { method: 'DELETE', path: '/v1/organizations/:id/owners/:user', summary: 'Remove an owner', who: 'org owner' },
      { method: 'GET', path: '/v1/organizations/:id/connections', summary: 'The organization\'s connections', who: 'org owner' },
      {
        method: 'POST', path: '/v1/organizations/:id/connections', summary: 'Add a connection for the organization', who: 'org owner', body: { name: 'acme_pg', variables: { CUBEJS_DB_TYPE: 'postgres' } },
      },
      { method: 'GET', path: '/v1/connections', summary: 'Every connection and its organization', who: 'platform admin' },
      {
        method: 'PUT', path: '/v1/connections/:name', summary: 'Edit a connection', who: 'org owner (created), platform admin (assigned)', body: { variables: {} },
      },
      { method: 'DELETE', path: '/v1/connections/:name', summary: 'Delete a connection', who: 'org owner (created), platform admin (assigned)' },
      {
        method: 'PUT', path: '/v1/connections/:name/organization', summary: 'Assign a data source to an organization, or move it', who: 'platform admin', body: { organizationId: '<organization id>' },
      },
      { method: 'DELETE', path: '/v1/connections/:name/organization', summary: 'Give a connection back to the Platform', who: 'platform admin' },
      { method: 'GET', path: '/v1/organizations/:id/model-members', summary: 'The members a role can filter or block', who: 'org owner' },
    ],
  },
  {
    key: 'workspaces',
    title: 'Workspaces',
    description: 'Where people query: members with a role, and the connections released to them.',
    endpoints: [
      { method: 'GET', path: '/v1/workspaces', summary: 'Search workspaces', who: 'any member', query: '?q=&organizationId=&limit=50&offset=0' },
      { method: 'GET', path: '/v1/organizations/:id/workspaces', summary: 'An organization\'s workspaces', who: 'any member' },
      { method: 'POST', path: '/v1/organizations/:id/workspaces', summary: 'Create a workspace', who: 'org owner', body: { name: 'Sales' } },
      { method: 'GET', path: '/v1/workspaces/:id', summary: 'One workspace', who: 'workspace member' },
      { method: 'PATCH', path: '/v1/workspaces/:id', summary: 'Rename a workspace', who: 'workspace owner', body: { name: 'Sales EU' } },
      { method: 'DELETE', path: '/v1/workspaces/:id', summary: 'Delete a workspace', who: 'workspace owner' },
      { method: 'GET', path: '/v1/workspaces/:id/members', summary: 'Members and their roles', who: 'workspace member' },
      { method: 'PUT', path: '/v1/workspaces/:id/members/:user', summary: 'Give a user a role', who: 'workspace owner', body: { role: 'viewer' } },
      { method: 'DELETE', path: '/v1/workspaces/:id/members/:user', summary: 'Remove a member', who: 'workspace owner' },
      { method: 'GET', path: '/v1/workspaces/:id/connections', summary: 'Connections released to the workspace', who: 'workspace member' },
      { method: 'PUT', path: '/v1/workspaces/:id/connections/:name', summary: 'Release a connection to the workspace', who: 'org owner' },
      { method: 'DELETE', path: '/v1/workspaces/:id/connections/:name', summary: 'Withdraw a connection', who: 'org owner' },
    ],
  },
  {
    key: 'roles',
    title: 'Roles',
    description: 'What an organization\'s rules can grant: permissions and data restrictions.',
    endpoints: [
      { method: 'GET', path: '/v1/organizations/:id/roles', summary: 'An organization\'s roles', who: 'any member' },
      {
        method: 'POST', path: '/v1/organizations/:id/roles', summary: 'Create a role', who: 'org owner', body: { name: 'finance', permissions: ['view', 'query'], restrictions: { rowFilters: [], blockedMembers: [] } },
      },
      { method: 'GET', path: '/v1/roles/:id', summary: 'One role', who: 'any member' },
      { method: 'PATCH', path: '/v1/roles/:id', summary: 'Edit a role (reaches every rule with it)', who: 'org owner', body: { permissions: ['view', 'query', 'read_objects'] } },
      { method: 'DELETE', path: '/v1/roles/:id', summary: 'Delete a role no rule uses', who: 'org owner' },
    ],
  },
  {
    key: 'rules',
    title: 'Rules',
    description: 'A named role in a workspace, and its members.',
    endpoints: [
      { method: 'GET', path: '/v1/rules', summary: 'Search rules', who: 'any member', query: '?q=&organizationId=&workspaceId=&limit=50&offset=0' },
      { method: 'GET', path: '/v1/workspaces/:id/rules', summary: 'A workspace\'s rules', who: 'workspace member' },
      {
        method: 'POST', path: '/v1/workspaces/:id/rules', summary: 'Create a rule', who: 'workspace owner', body: { description: 'Analysts', type: 'analytics', role: 'viewer' },
      },
      { method: 'GET', path: '/v1/rules/:id', summary: 'One rule', who: 'workspace member' },
      {
        method: 'PATCH', path: '/v1/rules/:id', summary: 'Edit a rule, or move it to another workspace', who: 'workspace owner', body: { description: 'Analysts (EU)' },
      },
      { method: 'DELETE', path: '/v1/rules/:id', summary: 'Delete a rule', who: 'workspace owner' },
      { method: 'GET', path: '/v1/rules/:id/members', summary: 'A rule\'s members', who: 'workspace member' },
      { method: 'PUT', path: '/v1/rules/:id/members/:user', summary: 'Add a user to a rule', who: 'workspace owner' },
      { method: 'DELETE', path: '/v1/rules/:id/members/:user', summary: 'Remove a user from a rule', who: 'workspace owner' },
    ],
  },
  {
    key: 'groups',
    title: 'Groups',
    description: 'Users of an organization placed in its rules together.',
    endpoints: [
      { method: 'GET', path: '/v1/groups', summary: 'Search groups', who: 'any member', query: '?q=&organizationId=&limit=50&offset=0' },
      { method: 'GET', path: '/v1/organizations/:id/groups', summary: 'An organization\'s groups', who: 'any member' },
      { method: 'POST', path: '/v1/organizations/:id/groups', summary: 'Create a group', who: 'org owner', body: { name: 'Finance team' } },
      { method: 'GET', path: '/v1/groups/:id', summary: 'One group, with its counts', who: 'any member' },
      { method: 'PATCH', path: '/v1/groups/:id', summary: 'Rename a group', who: 'org owner', body: { name: 'Finance' } },
      { method: 'DELETE', path: '/v1/groups/:id', summary: 'Delete a group that is in no rule', who: 'org owner' },
      { method: 'GET', path: '/v1/groups/:id/members', summary: 'A group\'s members', who: 'any member' },
      { method: 'PUT', path: '/v1/groups/:id/members/:user', summary: 'Add a user to a group', who: 'org owner' },
      { method: 'DELETE', path: '/v1/groups/:id/members/:user', summary: 'Remove a user from a group', who: 'org owner' },
      { method: 'GET', path: '/v1/groups/:id/rules', summary: 'The rules a group is in', who: 'any member' },
      { method: 'PUT', path: '/v1/groups/:id/rules/:rule', summary: 'Put a group in a rule', who: 'org owner' },
      { method: 'DELETE', path: '/v1/groups/:id/rules/:rule', summary: 'Take a group out of a rule', who: 'org owner' },
    ],
  },
  {
    key: 'objects',
    title: 'Workspace objects',
    description: 'Documents a workspace keeps (dashboards, saved queries...), with versions.',
    endpoints: [
      { method: 'GET', path: '/v1/workspaces/:id/objects', summary: 'A workspace\'s objects', who: 'read_objects' },
      {
        method: 'POST', path: '/v1/workspaces/:id/objects', summary: 'Create an object', who: 'write_objects', body: { type: 'dashboard', name: 'Sales', contentType: 'application/json', content: '{}' },
      },
      { method: 'GET', path: '/v1/objects/:id', summary: 'One object', who: 'read_objects' },
      {
        method: 'PUT', path: '/v1/objects/:id', summary: 'Replace an object (with the version read)', who: 'write_objects', body: { content: '{}', version: 1 },
      },
      { method: 'DELETE', path: '/v1/objects/:id', summary: 'Delete an object', who: 'write_objects' },
    ],
  },
  {
    key: 'system',
    title: 'System',
    description: 'Health checks and the request context.',
    endpoints: [
      { method: 'GET', path: '/readyz', absolute: true, summary: 'Ready to answer (model loaded, databases reachable)', who: 'anyone' },
      { method: 'GET', path: '/livez', absolute: true, summary: 'The process is alive', who: 'anyone' },
      { method: 'GET', path: '/cube-system/v1/context', absolute: true, summary: 'The security context of the token', who: 'platform admin, Playground token' },
    ],
  },
];

/** `:name` segments of a path, in order. */
export function pathParams(path: string): string[] {
  return [...path.matchAll(/:([A-Za-z]+)/g)].map((m) => m[1]);
}

/** The path with each `:name` replaced by its (encoded) value; empty ones stay `:name`. */
export function fillPath(path: string, values: Record<string, string>): string {
  return path.replace(/:([A-Za-z]+)/g, (whole, name) => (
    values[name] ? encodeURIComponent(values[name]) : whole
  ));
}

/** The sections whose endpoints match `q` (method, path, summary or who), with only those. */
export function filterSections(sections: Section[], q: string): Section[] {
  const term = q.trim().toLowerCase();
  if (!term) {
    return sections;
  }
  return sections
    .map((s) => ({
      ...s,
      endpoints: s.endpoints.filter((e) => [e.method, e.path, e.summary, e.who, s.title]
        .some((text) => text.toLowerCase().includes(term))),
    }))
    .filter((s) => s.endpoints.length);
}

export const endpointKey = (e: Endpoint) => `${e.method} ${e.absolute ? '' : '~'}${e.path}`;

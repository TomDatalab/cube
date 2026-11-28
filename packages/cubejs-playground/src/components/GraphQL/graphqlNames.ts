import { Query } from '@cubejs-client/core';

const GRAPHQL_NAME = /^[_A-Za-z][_0-9A-Za-z]*$/;

const lowerFirst = (s: string) => s.charAt(0).toLowerCase() + s.slice(1);
const upperFirst = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

// The GraphQL field a member is queried by, as the server names it
// (cubegraphql::naming::graphql_field): its name when that is already a
// GraphQL name, else its words in lower camel case, accents stripped —
// `Quantidade Total` → `quantidadeTotal`.
export function graphqlField(member: string): string {
  if (GRAPHQL_NAME.test(member)) {
    return member;
  }
  const words = member
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .split(/[^A-Za-z0-9_]+/)
    .filter(Boolean);
  const out = words.map((w, i) => (i === 0 ? lowerFirst(w) : upperFirst(w))).join('');
  if (out === '') {
    return '_';
  }
  return /^[0-9]/.test(out) ? `_${out}` : out;
}

// `cube.member[.granularity]` with the member as its GraphQL field.
export function graphqlPath(path: string): string {
  const dot = path.indexOf('.');
  if (dot < 0) {
    return path;
  }
  const [member, ...rest] = path.slice(dot + 1).split('.');
  return [path.slice(0, dot), graphqlField(member), ...rest].join('.');
}

type AnyFilter = { member?: string; dimension?: string; and?: AnyFilter[]; or?: AnyFilter[] };

function filters(list: AnyFilter[] | undefined): AnyFilter[] | undefined {
  return list?.map((f) => ({
    ...f,
    ...(f.member ? { member: graphqlPath(f.member) } : {}),
    ...(f.dimension ? { dimension: graphqlPath(f.dimension) } : {}),
    ...(f.and ? { and: filters(f.and) } : {}),
    ...(f.or ? { or: filters(f.or) } : {}),
  }));
}

// The query and member types with every member named by its GraphQL field,
// for the converters that write a GraphQL document from a REST query.
export function withGraphQLNames<T extends Record<string, string>>(
  query: Query,
  types: T
): { query: Query; types: T } {
  const q: any = { ...query };
  for (const key of ['measures', 'dimensions', 'segments'] as const) {
    if (Array.isArray(q[key])) {
      q[key] = q[key].map(graphqlPath);
    }
  }
  if (Array.isArray(q.timeDimensions)) {
    q.timeDimensions = q.timeDimensions.map((td: any) => ({ ...td, dimension: graphqlPath(td.dimension) }));
  }
  if (q.filters) {
    q.filters = filters(q.filters);
  }
  if (Array.isArray(q.order)) {
    q.order = q.order.map(([member, dir]: [string, string]) => [graphqlPath(member), dir]);
  } else if (q.order && typeof q.order === 'object') {
    q.order = Object.fromEntries(Object.entries(q.order).map(([member, dir]) => [graphqlPath(member), dir]));
  }
  const renamed = Object.fromEntries(Object.entries(types).map(([member, type]) => [graphqlPath(member), type]));
  return { query: q, types: renamed as T };
}

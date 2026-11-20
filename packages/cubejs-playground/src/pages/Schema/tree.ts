import type { DataNode } from 'antd/lib/tree';

export type TreeCube = {
  name: string;
  title?: string;
  type: 'cube' | 'view';
  dataSource: string;
  file: string;
  members: { measures: number; dimensions: number };
  joins?: { name: string; relationship?: string | null }[];
  // A view's source cubes.
  includes?: string[];
};

export type OrgSummary = {
  id?: string;
  slug: string;
  name: string;
  counts: { cubes: number; files: number; connections: number };
  errors: number;
};

export type OrgDetail = {
  cubes: TreeCube[];
  tables: Record<string, Record<string, Record<string, { name: string; type: string }[]>>>;
  files: { fileName: string }[];
  errors: { file: string; cube: string; message: string }[];
};

export type NodeKey = string;

export type ParsedKey = {
  kind: 'org' | 'cubes' | 'cube' | 'tables' | 'table' | 'files' | 'file' | 'datasource' | 'schema';
  slug: string;
  rest?: string;
};

export function orgNodes(orgs: OrgSummary[]): DataNode[] {
  return orgs.map((org) => ({
    key: `org:${org.slug}`,
    title: org.name,
    isLeaf: false,
    checkable: false,
  }));
}

export function detailNodes(slug: string, d: OrgDetail): DataNode[] {
  const tableNodes: DataNode[] = [];
  let tableCount = 0;

  Object.entries(d.tables || {}).forEach(([ds, schemas]) => {
    const schemaNodes: DataNode[] = Object.entries(schemas).map(([schema, tables]) => ({
      key: `schema:${slug}:${ds}.${schema}`,
      title: schema,
      checkable: false,
      children: Object.keys(tables).map((table) => {
        tableCount += 1;
        return {
          key: `table:${slug}:${ds}.${schema}.${table}`,
          title: table,
          isLeaf: true,
          checkable: true,
        };
      }),
    }));
    tableNodes.push({
      key: `datasource:${slug}:${ds}`,
      title: ds,
      checkable: false,
      children: schemaNodes,
    });
  });

  return [
    {
      key: `org:${slug}:cubes`,
      title: `Cubes (${d.cubes.length})`,
      checkable: false,
      children: d.cubes.map((cube) => ({
        key: `cube:${slug}:${cube.name}`,
        title: cube.title || cube.name,
        isLeaf: true,
        checkable: false,
      })),
    },
    {
      key: `tables:${slug}`,
      title: `Tables (${tableCount})`,
      checkable: false,
      children: tableNodes,
    },
    {
      key: `files:${slug}`,
      title: `Files (${d.files.length})`,
      checkable: false,
      children: d.files.map((file) => ({
        key: `file:${slug}:${file.fileName}`,
        title: file.fileName,
        isLeaf: true,
        checkable: false,
      })),
    },
  ];
}

export function filterTree(nodes: DataNode[], term: string): DataNode[] {
  const needle = term.trim().toLowerCase();
  if (!needle) {
    return nodes;
  }

  const matches = (node: DataNode) => (
    (typeof node.title === 'string' && node.title.toLowerCase().includes(needle))
    || String(node.key).toLowerCase().includes(needle)
  );

  const visit = (list: DataNode[]): DataNode[] => list.flatMap((node) => {
    const children = node.children ? visit(node.children) : undefined;
    if (children && children.length) {
      return [{ ...node, children }];
    }
    return matches(node) ? [node] : [];
  });

  return visit(nodes);
}

const KINDS: ParsedKey['kind'][] = [
  'org', 'cubes', 'cube', 'tables', 'table', 'files', 'file', 'datasource', 'schema',
];

export function parseKey(key: NodeKey): ParsedKey {
  const first = key.indexOf(':');
  const second = first === -1 ? -1 : key.indexOf(':', first + 1);
  const prefix = first === -1 ? key : key.slice(0, first);
  const kind = KINDS.find((k) => k === prefix);
  if (!kind) {
    throw new Error(`Unknown tree node key: ${key}`);
  }
  const slug = first === -1 ? '' : key.slice(first + 1, second === -1 ? undefined : second);
  const rest = second === -1 ? undefined : key.slice(second + 1);

  if (kind === 'org' && rest === 'cubes') {
    return { kind: 'cubes', slug, rest: undefined };
  }
  return { kind, slug, rest };
}

export function buildQueryUrl(cube: {
  name: string;
  measures: { name: string }[];
  dimensions: { name: string; primaryKey?: boolean }[];
}): string {
  let query: Record<string, string[]> = {};
  if (cube.measures.length) {
    query = { measures: cube.measures.map((m) => m.name) };
  } else if (cube.dimensions.length) {
    const dim = cube.dimensions.find((d) => !d.primaryKey) || cube.dimensions[0];
    query = { dimensions: [dim.name] };
  }
  return `/build?query=${encodeURIComponent(JSON.stringify(query))}`;
}

export function tablesFromSameOrg(checked: NodeKey[]): string | null {
  const slugs = new Set(checked.map((key) => parseKey(key).slug));
  return slugs.size === 1 ? [...slugs][0] : null;
}

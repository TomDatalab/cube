import type { DataNode } from 'antd/lib/tree';

export type TreeMember = {
  name: string;
  type?: string;
  title?: string;
  primaryKey?: boolean;
  // false: query users do not see the member.
  public?: boolean;
};

export type TreeCube = {
  name: string;
  title?: string;
  type: 'cube' | 'view';
  dataSource: string;
  file: string;
  members: { measures: number; dimensions: number };
  measures?: TreeMember[];
  dimensions?: TreeMember[];
  segments?: TreeMember[];
  joins?: { name: string; relationship?: string | null }[];
  // A view's source cubes.
  includes?: string[];
};

export type OrgSummary = {
  id?: string;
  slug: string;
  name: string;
  // Only the admin's list has them; an owner's comes from the session.
  counts?: { cubes: number; files: number; connections: number };
  errors?: number;
};

export type OrgDetail = {
  cubes: TreeCube[];
  tables: Record<string, Record<string, Record<string, { name: string; type: string }[]>>>;
  files: { id?: string; fileName: string; path?: string; version?: number; editable?: boolean }[];
  errors: { file: string; cube: string; message: string }[];
  models?: { id: string; name: string; cubeNames?: string[] }[];
};

export type NodeKey = string;

// A table's column in the tree: its database type shows after the name.
export type ColumnNode = DataNode & { columnType: string };

export type ParsedKey = {
  kind: 'org' | 'cubes' | 'views' | 'models' | 'model' | 'cube' | 'tables' | 'table' | 'column' | 'files' | 'file' | 'datasource' | 'schema';
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
      children: Object.entries(tables).map(([table, columns]) => {
        tableCount += 1;
        const key = `table:${slug}:${ds}.${schema}.${table}`;
        const columnNodes: ColumnNode[] = (columns || []).map((column) => ({
          key: `column:${slug}:${ds}.${schema}.${table}.${column.name}`,
          title: column.name,
          columnType: column.type,
          isLeaf: true,
          checkable: false,
          selectable: false,
        }));
        return {
          key,
          title: table,
          isLeaf: columnNodes.length === 0,
          checkable: true,
          children: columnNodes.length ? columnNodes : undefined,
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

  // A view opens the same panel as a cube (`cube:` key), under its own group.
  const group = (kind: 'cubes' | 'views', label: string, type: TreeCube['type']): DataNode => {
    const members = d.cubes.filter((cube) => cube.type === type);
    return {
      key: `org:${slug}:${kind}`,
      title: `${label} (${members.length})`,
      checkable: false,
      children: members.map((cube) => ({
        key: `cube:${slug}:${cube.name}`,
        title: cube.title || cube.name,
        isLeaf: true,
        checkable: false,
      })),
    };
  };

  // Only organization scopes carry models (Platform and the legacy tree do not).
  const modelsGroup: DataNode[] = d.models === undefined ? [] : [{
    key: `org:${slug}:models`,
    title: `Models (${d.models.length})`,
    checkable: false,
    children: d.models.map((model) => ({
      key: `model:${slug}:${model.id}`,
      title: model.name,
      isLeaf: true,
      checkable: false,
    })),
  }];

  return [
    group('cubes', 'Cubes', 'cube'),
    ...modelsGroup,
    group('views', 'Views', 'view'),
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
        // Every file of the node starts with the organization's folder; the
        // full path stays in the key, the tooltip and the file panel.
        title: file.fileName.startsWith(`${slug}/`)
          ? file.fileName.slice(slug.length + 1)
          : file.fileName,
        isLeaf: true,
        checkable: false,
      })),
    },
  ];
}

/**
 * The keys to give the Tree as `loadedKeys`. Only organizations load on
 * demand (`loadData`); every other node arrives with its children. A node the
 * Tree is not told is loaded, and that has no children (an empty Cubes or
 * Files group), would be loaded again on every render once expanded, freezing
 * the page. So: every node that is not an organization, plus the
 * organizations whose detail is loaded.
 */
export function loadedTreeKeys(nodes: DataNode[], loadedSlugs: string[]): string[] {
  const loaded = new Set(loadedSlugs.map((slug) => `org:${slug}`));
  const keys: string[] = [];
  const walk = (list: DataNode[]) => {
    list.forEach((node) => {
      const key = String(node.key);
      const isOrg = key.startsWith('org:') && key.indexOf(':', 4) === -1;
      if (!isOrg || loaded.has(key)) {
        keys.push(key);
      }
      if (node.children) {
        walk(node.children);
      }
    });
  };
  walk(nodes);
  return keys;
}

export function filterTree(nodes: DataNode[], term: string): DataNode[] {
  const needle = term.trim().toLowerCase();
  if (!needle) {
    return nodes;
  }

  // The title, and the part of the key after kind and slug (a cube name, a
  // file path, `ds.schema.table`): never the kind or the slug, so `cube` or
  // an organization's slug does not match every node of a kind.
  const rest = (node: DataNode) => {
    try {
      return parseKey(String(node.key)).rest || '';
    } catch {
      return '';
    }
  };
  const matches = (node: DataNode) => (
    (typeof node.title === 'string' && node.title.toLowerCase().includes(needle))
    || rest(node).toLowerCase().includes(needle)
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
  'org', 'cubes', 'views', 'models', 'model', 'cube', 'tables', 'table', 'column', 'files', 'file', 'datasource', 'schema',
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

  if (kind === 'org' && (rest === 'cubes' || rest === 'views' || rest === 'models')) {
    return { kind: rest, slug, rest: undefined };
  }
  return { kind, slug, rest };
}

export function buildQueryUrl(cube: {
  name: string;
  measures?: TreeMember[];
  dimensions?: TreeMember[];
}): string {
  // A hidden member is not in the query users' meta.
  const shown = (list: TreeMember[] = []) => list.filter((m) => m.public !== false);
  const measures = shown(cube.measures);
  const dimensions = shown(cube.dimensions);
  let query: Record<string, string[]> = {};
  if (measures.length) {
    query = { measures: measures.map((m) => m.name) };
  } else if (dimensions.length) {
    const dim = dimensions.find((d) => !d.primaryKey) || dimensions[0];
    query = { dimensions: [dim.name] };
  }
  return `/build?query=${encodeURIComponent(JSON.stringify(query))}`;
}

export function tablesFromSameOrg(checked: NodeKey[]): string | null {
  const slugs = new Set(checked.map((key) => parseKey(key).slug));
  return slugs.size === 1 ? [...slugs][0] : null;
}

export type TreeIconKind = 'cube' | 'table' | 'file' | 'model';

/**
 * Which icon a node shows: the three groups and their leaves get one each;
 * organizations, connections and schemas stay bare so the tree reads as
 * folders and things.
 */
export function iconKind(key: NodeKey): TreeIconKind | null {
  let kind: ParsedKey['kind'];
  try {
    ({ kind } = parseKey(key));
  } catch {
    return null;
  }
  switch (kind) {
    case 'cubes':
    case 'views':
    case 'cube':
      return 'cube';
    case 'models':
    case 'model':
      return 'model';
    case 'tables':
    case 'table':
      return 'table';
    case 'files':
    case 'file':
      return 'file';
    default:
      return null;
  }
}

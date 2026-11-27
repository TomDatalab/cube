import {
  CubeDimension, CubeJoin, CubeMeasure, CubeNode, CubeSegment, CubeSource, DimensionType, IDENTIFIER, ModelDefinition,
  Relationship,
} from './modelGraph';
import { freeId, freeName, merged } from './objects';

// The cubes drawn on the canvas, and the references between them. The
// server builds one YAML per cube; its auto mode (a cube with no dimensions
// and no measures) is mirrored here, so the editor shows the same names.

export type ColumnsOf = (schema: string, table: string) => { name: string; type: string }[] | undefined;
export type MemberGroup = 'dimensions' | 'measures' | 'segments';

const MAX_IDENTIFIER = 63;
const round = (n: number) => Math.round(n);

const cubesOf = (def: ModelDefinition) => def.cubes ?? [];
const joinsOf = (def: ModelDefinition) => def.cubeJoins ?? [];

function mapCube(def: ModelDefinition, id: string, f: (c: CubeNode) => CubeNode): ModelDefinition {
  return { ...def, cubes: cubesOf(def).map((c) => (c.id === id ? f(c) : c)) };
}

export function addCube(def: ModelDefinition, x: number, y: number): { def: ModelDefinition; id: string } {
  const cubes = cubesOf(def);
  const id = freeId('c', cubes.map((c) => c.id));
  const name = freeName('cube_', (n) => cubes.some((c) => c.name === n));
  const table = def.tables[0]?.alias;
  const cube: CubeNode = {
    id,
    name,
    color: 'violet',
    source: table ? { kind: 'table', table } : { kind: 'table' },
    dimensions: [],
    measures: [],
    segments: [],
    x: round(x),
    y: round(y),
  };
  return { id, def: { ...def, cubes: [...cubes, cube] } };
}

// A patch value of `undefined` removes its key.
export function setCube(def: ModelDefinition, id: string, patch: Partial<Omit<CubeNode, 'id'>>): ModelDefinition {
  return mapCube(def, id, (c) => merged(c, patch));
}

export function renameCube(def: ModelDefinition, id: string, next: string): { def: ModelDefinition; error?: string } {
  const cubes = cubesOf(def);
  if (cubes.find((c) => c.id === id)?.name === next) {
    return { def };
  }
  if (!IDENTIFIER.test(next) || next.length > MAX_IDENTIFIER) {
    return { def, error: `Invalid name: ${next}` };
  }
  if (cubes.some((c) => c.id !== id && c.name === next)) {
    return { def, error: `${next} is already in this model` };
  }
  return { def: setCube(def, id, { name: next }) };
}

// Members whose column no longer resolves keep their names: the client
// validation flags them.
export function setCubeSource(def: ModelDefinition, id: string, source: CubeSource): ModelDefinition {
  return mapCube(def, id, (c) => ({ ...c, source: merged({ kind: source.kind } as CubeSource, source) }));
}

const MEMBER_BASE: Record<MemberGroup, string> = {
  dimensions: 'dimension_',
  measures: 'measure_',
  segments: 'segment_',
};

function memberNames(c: CubeNode): string[] {
  return [...c.dimensions, ...c.measures, ...c.segments].map((m) => m.name);
}

export function addMember(
  def: ModelDefinition,
  id: string,
  group: MemberGroup
): { def: ModelDefinition; name: string } {
  const cube = cubesOf(def).find((c) => c.id === id);
  if (!cube) {
    return { def, name: '' };
  }
  const taken = new Set(memberNames(cube));
  const name = freeName(MEMBER_BASE[group], (n) => taken.has(n));
  let next: CubeNode;
  if (group === 'dimensions') {
    next = { ...cube, dimensions: [...cube.dimensions, { name, type: 'string', sql: '' }] };
  } else if (group === 'measures') {
    next = { ...cube, measures: [...cube.measures, { name, type: 'count' }] };
  } else {
    next = { ...cube, segments: [...cube.segments, { name, sql: '1 = 1' }] };
  }
  return { name, def: mapCube(def, id, () => next) };
}

export function setMember(
  def: ModelDefinition,
  id: string,
  group: MemberGroup,
  index: number,
  patch: Partial<CubeDimension & CubeMeasure & CubeSegment>
): ModelDefinition {
  return mapCube(def, id, (c) => ({
    ...c,
    [group]: (c[group] as object[]).map((m, i) => (i === index ? merged(m, patch) : m)),
  }));
}

export function removeMember(def: ModelDefinition, id: string, group: MemberGroup, index: number): ModelDefinition {
  return mapCube(def, id, (c) => ({ ...c, [group]: (c[group] as object[]).filter((_, i) => i !== index) }));
}

const FLIPPED: Record<Relationship, Relationship> = {
  one_to_many: 'many_to_one', many_to_one: 'one_to_many', one_to_one: 'one_to_one',
};

// The tables `def.joins` reach from `primary`, breadth-first, the way the
// server walks them; `multiplies` when a step repeats the primary's rows.
function walk(def: ModelDefinition, primary: string): { order: string[]; multiplies: boolean } {
  const order = [primary];
  const visited = new Set([primary]);
  const queue = [primary];
  let multiplies = false;
  while (queue.length) {
    const alias = queue.shift() as string;
    for (const j of def.joins) {
      let step: { to: string; relationship: Relationship } | null = null;
      if (j.from.alias === alias && !visited.has(j.to.alias)) {
        step = { to: j.to.alias, relationship: j.relationship };
      } else if (j.to.alias === alias && !visited.has(j.from.alias)) {
        step = { to: j.from.alias, relationship: FLIPPED[j.relationship] };
      }
      if (step) {
        visited.add(step.to);
        queue.push(step.to);
        order.push(step.to);
        multiplies = multiplies || step.relationship === 'one_to_many';
      }
    }
  }
  return { order, multiplies };
}

// The aliases a cube reads: the primary first for `joined`.
export function cubeReads(def: ModelDefinition, cube: CubeNode): string[] {
  const { kind, table, tables } = cube.source;
  if (kind === 'sql') {
    return tables ?? [];
  }
  if (!table) {
    return [];
  }
  return kind === 'table' ? [table] : walk(def, table).order;
}

// `fiberCableId` → `fiber_cable_id`: scaffold::snake on the server.
export function snake(name: string): string {
  let out = '';
  let prev: string | undefined;
  for (const c of name) {
    const upper = c >= 'A' && c <= 'Z';
    if (upper && prev !== undefined && ((prev >= 'a' && prev <= 'z') || (prev >= '0' && prev <= '9'))) {
      out += '_';
    }
    if (/^[A-Za-z0-9]$/.test(c)) {
      out += c.toLowerCase();
    } else if (!out.endsWith('_')) {
      out += '_';
    }
    prev = c;
  }
  out = out.replace(/^_+|_+$/g, '');
  if (out === '') {
    return 'col';
  }
  return out[0] >= '0' && out[0] <= '9' ? `c_${out}` : out;
}

const NUMERIC = [
  'int', 'uint', 'integer', 'smallint', 'bigint', 'tinyint', 'mediumint', 'float', 'double', 'decimal', 'numeric',
  'real', 'number', 'money', 'serial', 'bigserial',
];

// A database type as a Cube dimension type, none for what Cube cannot
// hold: scaffold::cube_type on the server.
export function cubeType(rawType: string): DimensionType | undefined {
  let t = rawType.trim().toLowerCase();
  // ClickHouse wrappers that do not change what the value is.
  for (const wrapper of ['nullable(', 'lowcardinality(']) {
    while (t.startsWith(wrapper)) {
      const inner = t.slice(wrapper.length);
      t = inner.endsWith(')') ? inner.slice(0, -1) : inner;
    }
  }
  const word = t.split(/[(\s]/)[0] ?? '';
  if (['array', 'map', 'tuple', 'json'].some((p) => word.startsWith(p)) || word.endsWith('[]')) {
    return undefined;
  }
  if (word.startsWith('bool')) {
    return 'boolean';
  }
  if (word.startsWith('date') || word.startsWith('time')) {
    return 'time';
  }
  const numeric = NUMERIC.some((n) => word === n || (word.startsWith(n) && /^[0-9]*$/.test(word.slice(n.length))));
  return numeric ? 'number' : 'string';
}

function columnsOfAlias(def: ModelDefinition, alias: string, columnsOf: ColumnsOf): { name: string; type: string }[] {
  const t = def.tables.find((x) => x.alias === alias);
  if (!t) {
    return [];
  }
  return t.draft ? t.draft.columns.map((c) => ({ name: c.name, type: c.type })) : columnsOf(t.schema, t.table) ?? [];
}

// `base`, or `base_2`, `base_3`… when taken; the result is taken.
function unique(taken: Set<string>, base: string): string {
  let member = base;
  for (let n = 2; taken.has(member); n++) {
    member = `${base}_${n}`;
  }
  taken.add(member);
  return member;
}

// The members the server's auto mode writes, with the same names. `sql`
// cubes have none.
export function autoMembers(
  def: ModelDefinition,
  cube: CubeNode,
  columnsOf: ColumnsOf
): { dimensions: CubeDimension[]; measures: CubeMeasure[] } {
  const { kind, table: primary, primaryKey } = cube.source;
  if (kind === 'sql' || !primary) {
    return { dimensions: [], measures: [] };
  }
  const isJoined = kind === 'joined';
  const { order, multiplies } = isJoined ? walk(def, primary) : { order: [primary], multiplies: false };
  const taken = new Set(['count']);
  const dimensions: CubeDimension[] = [];
  for (const alias of order) {
    for (const column of columnsOfAlias(def, alias, columnsOf)) {
      const type = cubeType(column.type);
      // A column Cube cannot hold is left out.
      if (type) {
        const name = unique(taken, isJoined ? `${snake(alias)}_${snake(column.name)}` : snake(column.name));
        const isKey = alias === primary && primaryKey === column.name;
        dimensions.push({
          name,
          type,
          column: isJoined ? `${alias}.${column.name}` : column.name,
          ...(isKey ? { primaryKey: true } : {}),
        });
      }
    }
  }
  // A one_to_many repeats the primary's rows: count its key, once.
  const measures: CubeMeasure[] = multiplies && primaryKey
    ? [{ name: 'count', type: 'count_distinct', column: `${primary}.${primaryKey}` }]
    : [{ name: 'count', type: 'count' }];
  return { dimensions, measures };
}

const isAuto = (c: CubeNode) => c.dimensions.length === 0 && c.measures.length === 0;

// "Generate members": the auto members, written out.
export function generateMembers(def: ModelDefinition, id: string, columnsOf: ColumnsOf): ModelDefinition {
  return mapCube(def, id, (c) => ({ ...c, ...autoMembers(def, c, columnsOf) }));
}

// What a cube join may pair on this cube.
export function dimensionNames(def: ModelDefinition, cube: CubeNode, columnsOf: ColumnsOf): string[] {
  const dimensions = isAuto(cube) ? autoMembers(def, cube, columnsOf).dimensions : cube.dimensions;
  return dimensions.map((d) => d.name);
}

export function addCubeJoin(
  def: ModelDefinition,
  from: string,
  to: string,
  relationship: Relationship,
  on: { from: string; to: string }[]
): { def: ModelDefinition; id: string } {
  const joins = joinsOf(def);
  const id = freeId('cj', joins.map((j) => j.id));
  return {
    id,
    def: {
      ...def,
      cubeJoins: [...joins, {
        id, from, to, relationship, on, points: [],
      }],
    },
  };
}

export function setCubeJoin(def: ModelDefinition, id: string, patch: Partial<Omit<CubeJoin, 'id'>>): ModelDefinition {
  return { ...def, cubeJoins: joinsOf(def).map((j) => (j.id === id ? merged(j, patch) : j)) };
}

export function removeCubeJoin(def: ModelDefinition, id: string): ModelDefinition {
  return { ...def, cubeJoins: joinsOf(def).filter((j) => j.id !== id) };
}

// The primary key a `table` or `joined` cube gives the table `alias`.
export function cubeKeyOf(def: ModelDefinition, alias: string): string | undefined {
  return cubesOf(def).find((c) => c.source.kind !== 'sql' && c.source.table === alias && c.source.primaryKey)
    ?.source.primaryKey;
}

export type Relationship = 'one_to_one' | 'one_to_many' | 'many_to_one';
export type JoinKind = 'left' | 'inner';

export type ModelTable = {
  alias: string;
  schema: string;
  table: string;
  x: number;
  y: number;
};

export type End = { alias: string; column: string };

export type ModelJoin = {
  from: End;
  to: End;
  relationship: Relationship;
  join: JoinKind;
};

export type ModelDefinition = {
  dataSource: string;
  tables: ModelTable[];
  primary?: string | null;
  primaryKey?: string | null;
  joins: ModelJoin[];
};

export function emptyDefinition(): ModelDefinition {
  return { dataSource: '', tables: [], joins: [] };
}

export function addTable(
  def: ModelDefinition,
  ds: string,
  schema: string,
  table: string,
  x: number,
  y: number
): { def: ModelDefinition; error?: string } {
  if (def.tables.length > 0 && def.dataSource !== ds) {
    return {
      def,
      error: `Tables of one connection only: this model reads ${def.dataSource}`,
    };
  }
  const taken = new Set(def.tables.map((t) => t.alias));
  let alias = table;
  for (let n = 2; taken.has(alias); n++) {
    alias = `${table}_${n}`;
  }
  const first = def.tables.length === 0;
  return {
    def: {
      ...def,
      dataSource: ds,
      primary: first ? alias : def.primary,
      tables: [...def.tables, { alias, schema, table, x, y }],
    },
  };
}

export function removeTable(def: ModelDefinition, alias: string): ModelDefinition {
  const tables = def.tables.filter((t) => t.alias !== alias);
  const joins = def.joins.filter((j) => j.from.alias !== alias && j.to.alias !== alias);
  if (def.primary !== alias) {
    return { ...def, tables, joins };
  }
  return {
    ...def,
    dataSource: tables.length === 0 ? '' : def.dataSource,
    tables,
    joins,
    primary: tables[0]?.alias ?? null,
    primaryKey: null,
  };
}

export function setPrimary(def: ModelDefinition, alias: string): ModelDefinition {
  if (def.primary === alias || !def.tables.some((t) => t.alias === alias)) {
    return def;
  }
  return { ...def, primary: alias, primaryKey: null };
}

export function canConnect(def: ModelDefinition, from: End, to: End): string | null {
  for (const end of [from, to]) {
    if (!def.tables.some((t) => t.alias === end.alias)) {
      return `Unknown table: ${end.alias}`;
    }
  }
  if (from.alias === to.alias) {
    return 'A table cannot link to itself';
  }
  const samePair = (j: ModelJoin) => {
    const forward = j.from.alias === from.alias && j.to.alias === to.alias;
    const backward = j.from.alias === to.alias && j.to.alias === from.alias;

    return forward || backward;
  };
  if (def.joins.some(samePair)) {
    return 'These tables are already linked';
  }
  const parent = new Map<string, string>();
  const find = (a: string): string => {
    let r = a;
    while (parent.has(r) && parent.get(r) !== r) {
      r = parent.get(r) as string;
    }
    parent.set(a, r);
    return r;
  };
  for (const j of def.joins) {
    parent.set(find(j.from.alias), find(j.to.alias));
  }
  if (find(from.alias) === find(to.alias)) {
    return 'This link would create a cycle';
  }
  return null;
}

export function connect(
  def: ModelDefinition,
  from: End,
  to: End,
  relationship: Relationship,
  join: JoinKind
): ModelDefinition {
  return {
    ...def,
    joins: [...def.joins, { from: { ...from }, to: { ...to }, relationship, join }],
  };
}

export function updateJoin(
  def: ModelDefinition,
  index: number,
  patch: Partial<Pick<ModelJoin, 'relationship' | 'join'>>
): ModelDefinition {
  return {
    ...def,
    joins: def.joins.map((j, i) => {
      if (i !== index) {
        return j;
      }
      return {
        ...j,
        relationship: patch.relationship ?? j.relationship,
        join: patch.join ?? j.join,
      };
    }),
  };
}

export function removeJoin(def: ModelDefinition, index: number): ModelDefinition {
  return { ...def, joins: def.joins.filter((_, i) => i !== index) };
}

const REL_LABEL: Record<Relationship, string> = {
  one_to_one: '1:1',
  one_to_many: '1:N',
  many_to_one: 'N:1',
};

export function joinLabel(j: ModelJoin): string {
  return `${REL_LABEL[j.relationship]} · ${j.join.toUpperCase()}`;
}

export function unlinked(def: ModelDefinition): string[] {
  if (def.tables.length === 0) {
    return [];
  }
  const start = def.primary ?? def.tables[0].alias;
  const seen = new Set([start]);
  const queue = [start];
  while (queue.length) {
    const cur = queue.shift() as string;
    for (const j of def.joins) {
      let other: string | null = null;
      if (j.from.alias === cur) {
        other = j.to.alias;
      } else if (j.to.alias === cur) {
        other = j.from.alias;
      }
      if (other && !seen.has(other)) {
        seen.add(other);
        queue.push(other);
      }
    }
  }
  return def.tables.map((t) => t.alias).filter((a) => !seen.has(a));
}

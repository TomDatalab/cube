import { Dialect, validType } from './dialect';
import { IDENTIFIER, ModelDefinition, ModelJoin, ModelTable } from './modelGraph';
import { referenceEnds } from './physical';

// What a problem is about: a table (its columns, keys, indexes and checks
// included), a reference or a view (its columns included).
export type IssueTarget = { kind: 'table'; alias: string } | { kind: 'reference'; id: string } | { kind: 'view'; id: string };
export type Issue = { rule: string; severity: 'error' | 'warning'; target: IssueTarget; message: string };

type Group = 'Table' | 'Table column' | 'Table alternate key' | 'Table check' | 'Table index' | 'Reference' | 'View' | 'View column';
export type Rule = { id: string; group: Group; label: string; severity: 'error' | 'warning'; dialect?: true };

// Redgate's model validation rules for the objects this model has, in its
// order and with its labels.
export const RULES: Rule[] = [
  {
    id: 'table.emptyName', group: 'Table', label: 'Empty name', severity: 'error',
  },
  {
    id: 'table.uniqueName', group: 'Table', label: 'Unique Name', severity: 'error',
  },
  {
    id: 'table.noColumns', group: 'Table', label: 'No columns', severity: 'error',
  },
  {
    id: 'table.generatedName', group: 'Table', label: 'Generated name', severity: 'warning',
  },
  {
    id: 'table.invalidName', group: 'Table', label: 'Invalid name', severity: 'warning',
  },
  {
    id: 'table.nameLength', group: 'Table', label: 'Name length', severity: 'warning',
  },
  {
    id: 'table.hasPk', group: 'Table', label: 'If has PK column(s)', severity: 'warning',
  },
  {
    id: 'column.emptyName', group: 'Table column', label: "Name can't be empty", severity: 'error',
  },
  {
    id: 'column.emptyType', group: 'Table column', label: 'Empty type', severity: 'error',
  },
  {
    id: 'column.uniqueName', group: 'Table column', label: 'Unique name', severity: 'error',
  },
  {
    id: 'column.pkNotNull', group: 'Table column', label: 'PK column must be not null', severity: 'error',
  },
  {
    id: 'column.percent', group: 'Table column', label: 'Percent placeholder in type', severity: 'error',
  },
  {
    id: 'column.generatedName', group: 'Table column', label: 'Generated column name', severity: 'warning',
  },
  {
    id: 'column.nameLength', group: 'Table column', label: 'Name length', severity: 'warning',
  },
  {
    id: 'column.invalidName', group: 'Table column', label: 'Invalid name', severity: 'warning',
  },
  {
    id: 'column.supportedType', group: 'Table column', label: 'Supported database type', severity: 'warning', dialect: true,
  },
  {
    id: 'ak.emptyName', group: 'Table alternate key', label: "Name can't be empty", severity: 'error',
  },
  {
    id: 'ak.uniqueName', group: 'Table alternate key', label: 'Unique name', severity: 'error',
  },
  {
    id: 'ak.columns', group: 'Table alternate key', label: 'Check if has at least one column', severity: 'error',
  },
  {
    id: 'ak.invalidName', group: 'Table alternate key', label: 'Invalid name', severity: 'warning',
  },
  {
    id: 'ak.nameLength', group: 'Table alternate key', label: 'Name length', severity: 'warning',
  },
  {
    id: 'check.emptyExpression', group: 'Table check', label: "Check expression can't be empty", severity: 'error',
  },
  {
    id: 'check.invalidName', group: 'Table check', label: 'Invalid name', severity: 'warning',
  },
  {
    id: 'index.emptyName', group: 'Table index', label: "Name can't be empty", severity: 'error',
  },
  {
    id: 'index.uniqueName', group: 'Table index', label: 'Unique name', severity: 'error',
  },
  {
    id: 'index.columns', group: 'Table index', label: 'At least one column', severity: 'error',
  },
  {
    id: 'index.invalidName', group: 'Table index', label: 'Invalid name', severity: 'warning',
  },
  {
    id: 'index.nameLength', group: 'Table index', label: 'Name length', severity: 'warning',
  },
  {
    id: 'reference.emptyName', group: 'Reference', label: 'Name cannot be empty', severity: 'error',
  },
  {
    id: 'reference.uniqueName', group: 'Reference', label: 'Unique name', severity: 'error',
  },
  {
    id: 'reference.keys', group: 'Reference', label: 'Check PK/FK keys', severity: 'error',
  },
  {
    id: 'reference.invalidName', group: 'Reference', label: 'Invalid name', severity: 'warning',
  },
  {
    id: 'reference.nameLength', group: 'Reference', label: 'Name length', severity: 'warning',
  },
  {
    id: 'reference.selfMandatory', group: 'Reference', label: 'Check if self reference is not mandatory', severity: 'warning',
  },
  {
    id: 'view.emptyName', group: 'View', label: "Name can't be empty", severity: 'error',
  },
  {
    id: 'view.uniqueName', group: 'View', label: 'Unique name', severity: 'error',
  },
  {
    id: 'view.emptyQuery', group: 'View', label: "Query can't be empty", severity: 'error',
  },
  {
    id: 'view.invalidName', group: 'View', label: 'Invalid name', severity: 'warning',
  },
  {
    id: 'view.nameLength', group: 'View', label: 'Name length', severity: 'warning',
  },
  {
    id: 'viewColumn.emptyName', group: 'View column', label: "Name can't be empty", severity: 'error',
  },
  {
    id: 'viewColumn.uniqueName', group: 'View column', label: 'Unique name', severity: 'error',
  },
  {
    id: 'viewColumn.emptyType', group: 'View column', label: 'Empty type', severity: 'error',
  },
  {
    id: 'viewColumn.supportedType', group: 'View column', label: 'Supported database type', severity: 'warning', dialect: true,
  },
];

const MAX_NAME = 63;
const GENERATED = /_\d+$/;
const SEVERITY = new Map(RULES.map((r) => [r.id, r.severity]));

type Columns = { name: string; type: string }[];

// The columns a table's key is made of, or null when the model cannot know
// them (a table of the database that is not the principal).
function keysOf(def: ModelDefinition, t: ModelTable): string[][] | null {
  if (t.draft) {
    const pk = t.draft.columns.filter((c) => c.primaryKey).map((c) => c.name);
    return [...(pk.length ? [pk] : []), ...(t.draft.uniqueKeys ?? []).map((k) => k.columns)];
  }
  if (def.primary === t.alias && def.primaryKey) {
    return [[def.primaryKey]];
  }
  return null;
}

// The child and parent columns of a reference, pair by pair.
function pairsOf(j: ModelJoin): { child: string; parent: string }[] {
  const { parent } = referenceEnds(j);
  const parentIsFrom = parent === j.from;
  const all = [{ from: j.from.column, to: j.to.column }, ...(j.pairs ?? [])];
  return all.map((p) => (parentIsFrom ? { child: p.to, parent: p.from } : { child: p.from, parent: p.to }));
}

export function issues(
  def: ModelDefinition,
  columnsOf: (t: ModelTable) => Columns,
  dialect: Dialect
): Issue[] {
  const off = def.settings?.validation ?? {};
  const out: Issue[] = [];
  const add = (rule: string, target: IssueTarget, message: string) => {
    const meta = RULES.find((r) => r.id === rule);
    if (off[rule] === false || (meta?.dialect && !dialect)) {
      return;
    }
    out.push({
      rule, severity: SEVERITY.get(rule) ?? 'warning', target, message,
    });
  };
  const nameRules = (
    prefix: 'table' | 'column' | 'ak' | 'index' | 'reference' | 'view',
    target: IssueTarget,
    what: string,
    name: string
  ) => {
    if (!IDENTIFIER.test(name)) {
      add(`${prefix}.invalidName`, target, `${what} has an invalid name`);
    }
    if (name.length > MAX_NAME) {
      add(`${prefix}.nameLength`, target, `${what}'s name is longer than ${MAX_NAME} characters`);
    }
  };

  const designed = new Map<string, string>();
  for (const t of def.tables.filter((x) => x.draft)) {
    const target: IssueTarget = { kind: 'table', alias: t.alias };
    const draft = t.draft!;
    if (!t.table.trim()) {
      add('table.emptyName', target, `Table ${t.alias} has an empty name`);
    } else {
      nameRules('table', target, `Table ${t.alias}`, t.table);
    }
    const full = `${t.schema}.${t.table}`;
    if (designed.has(full)) {
      add('table.uniqueName', target, `Table ${t.alias}: ${full} is used by another table`);
    }
    designed.set(full, t.alias);
    if (GENERATED.test(t.alias)) {
      add('table.generatedName', target, `Table ${t.alias} has a generated name`);
    }
    if (draft.columns.length === 0) {
      add('table.noColumns', target, `Table ${t.alias} has no columns`);
    }
    if (!draft.columns.some((c) => c.primaryKey)) {
      add('table.hasPk', target, `Table ${t.alias} has no primary key`);
    }

    const seen = new Set<string>();
    for (const c of draft.columns) {
      const what = `Column ${t.alias}.${c.name}`;
      if (!c.name.trim()) {
        add('column.emptyName', target, `A column of ${t.alias} has an empty name`);
      } else {
        nameRules('column', target, what, c.name);
      }
      if (seen.has(c.name.toLowerCase())) {
        add('column.uniqueName', target, `${what} is repeated`);
      }
      seen.add(c.name.toLowerCase());
      if (!c.type.trim()) {
        add('column.emptyType', target, `${what} has no type`);
      } else {
        if (c.type.includes('%')) {
          add('column.percent', target, `${what} has a % placeholder in its type`);
        }
        if (!validType(dialect, c.type)) {
          add('column.supportedType', target, `${what}: ${c.type} is not a ${dialect} type`);
        }
      }
      if (c.primaryKey && c.nullable) {
        add('column.pkNotNull', target, `${what} is in the primary key and allows nulls`);
      }
      if (/^column_\d+$/.test(c.name)) {
        add('column.generatedName', target, `${what} has a generated name`);
      }
    }

    const keyNames = new Set<string>();
    for (const k of draft.uniqueKeys ?? []) {
      const what = `Alternate key ${k.name} of ${t.alias}`;
      if (!k.name.trim()) {
        add('ak.emptyName', target, `An alternate key of ${t.alias} has an empty name`);
      } else {
        nameRules('ak', target, what, k.name);
      }
      if (keyNames.has(k.name)) {
        add('ak.uniqueName', target, `${what} is repeated`);
      }
      keyNames.add(k.name);
      if (k.columns.length === 0) {
        add('ak.columns', target, `${what} has no columns`);
      }
    }
    for (const check of draft.checks ?? []) {
      const what = `Check ${check.name} of ${t.alias}`;
      if (!check.expression.trim()) {
        add('check.emptyExpression', target, `${what} has an empty expression`);
      }
      if (!IDENTIFIER.test(check.name)) {
        add('check.invalidName', target, `${what} has an invalid name`);
      }
    }
    const indexNames = new Set<string>();
    for (const ix of draft.indexes ?? []) {
      const what = `Index ${ix.name} of ${t.alias}`;
      if (!ix.name.trim()) {
        add('index.emptyName', target, `An index of ${t.alias} has an empty name`);
      } else {
        nameRules('index', target, what, ix.name);
      }
      if (indexNames.has(ix.name)) {
        add('index.uniqueName', target, `${what} is repeated`);
      }
      indexNames.add(ix.name);
      if (ix.columns.length === 0) {
        add('index.columns', target, `${what} has no columns`);
      }
    }
  }

  const referenceNames = new Map<string, string>();
  for (const j of def.joins) {
    const id = j.id ?? '';
    const target: IssueTarget = { kind: 'reference', id };
    if (j.name !== undefined) {
      if (!j.name.trim()) {
        add('reference.emptyName', target, `Reference ${id} has an empty name`);
      } else {
        nameRules('reference', target, `Reference ${id}`, j.name);
        if (referenceNames.has(j.name)) {
          add('reference.uniqueName', target, `Reference ${id}: the name ${j.name} is repeated`);
        }
        referenceNames.set(j.name, id);
      }
    }
    const { parent, child } = referenceEnds(j);
    const parentTable = def.tables.find((t) => t.alias === parent.alias);
    const childTable = def.tables.find((t) => t.alias === child.alias);
    if (parentTable && childTable) {
      const pairs = pairsOf(j);
      const keys = keysOf(def, parentTable);
      const parentCols = columnsOf(parentTable);
      const childCols = columnsOf(childTable);
      const isKey = keys === null || keys.some((k) => k.length === pairs.length
        && k.every((c) => pairs.some((p) => p.parent === c)));
      const typesMatch = pairs.every((p) => {
        const a = childCols.find((c) => c.name === p.child)?.type;
        const b = parentCols.find((c) => c.name === p.parent)?.type;
        return !a || !b || a.toLowerCase() === b.toLowerCase();
      });
      if (!isKey || !typesMatch) {
        add(
          'reference.keys',
          target,
          `Reference ${id}: ${child.alias}.${child.column} does not match a key of ${parent.alias}`
        );
      }
    }
    if (j.from.alias === j.to.alias && (j.mandatory ?? j.join === 'inner')) {
      add('reference.selfMandatory', target, `Reference ${id} links ${j.from.alias} to itself and is mandatory`);
    }
  }

  const viewNames = new Set<string>();
  for (const v of def.views ?? []) {
    const target: IssueTarget = { kind: 'view', id: v.id };
    const what = `View ${v.name || v.id}`;
    if (!v.name.trim()) {
      add('view.emptyName', target, `View ${v.id} has an empty name`);
    } else {
      nameRules('view', target, what, v.name);
      if (viewNames.has(v.name)) {
        add('view.uniqueName', target, `${what} is used by another view`);
      }
      viewNames.add(v.name);
    }
    if (!v.sql.trim()) {
      add('view.emptyQuery', target, `${what} has an empty query`);
    }
    const seen = new Set<string>();
    for (const c of v.columns) {
      const col = `Column ${v.name || v.id}.${c.name}`;
      if (!c.name.trim()) {
        add('viewColumn.emptyName', target, `A column of ${what} has an empty name`);
      } else if (seen.has(c.name.toLowerCase())) {
        add('viewColumn.uniqueName', target, `${col} is repeated`);
      }
      seen.add(c.name.toLowerCase());
      if (!c.type.trim()) {
        add('viewColumn.emptyType', target, `${col} has no type`);
      } else if (!validType(dialect, c.type)) {
        add('viewColumn.supportedType', target, `${col}: ${c.type} is not a ${dialect} type`);
      }
    }
  }
  return out;
}

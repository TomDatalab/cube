// Edits of a cube's members in its YAML file, for the cube panel of the Data
// Model page: a measure's type, a new measure, a dimension's name and type.
// The file keeps its comments, quoting and layout; the server compiles the
// result when it is saved and refuses what breaks the model.

import { Document, isMap, isSeq, parseDocument, YAMLMap, YAMLSeq } from 'yaml';

export const MEASURE_TYPES = [
  'count',
  'count_distinct',
  'count_distinct_approx',
  'sum',
  'avg',
  'min',
  'max',
  'number',
  'string',
  'time',
  'boolean',
] as const;
export type MeasureType = (typeof MEASURE_TYPES)[number];

// `geo` is left out: it takes latitude and longitude, not a type change.
export const DIMENSION_TYPES = ['string', 'number', 'time', 'boolean'] as const;
export type DimensionType = (typeof DIMENSION_TYPES)[number];

/** An edit the file cannot take; its message is for the user. */
export class CubeEditError extends Error {}

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Every measure type but `count` reads a column or an expression. */
export const measureNeedsSql = (type: MeasureType) => type !== 'count';

function load(text: string): Document {
  const doc = parseDocument(text);
  if (doc.errors.length) {
    throw new CubeEditError(`The file is not valid YAML: ${doc.errors[0].message}`);
  }
  return doc;
}

// The file's own layout: a list under a key either indented or not.
function save(doc: Document, text: string): string {
  const indentSeq = !/^[^\s#][^\n]*:[ \t]*\n- /m.test(text);
  return doc.toString({ indentSeq, singleQuote: true, lineWidth: 0 });
}

function cubeOf(doc: Document, cube: string): YAMLMap {
  const cubes = doc.get('cubes');
  const found = isSeq(cubes)
    ? (cubes as YAMLSeq).items.find((c) => isMap(c) && c.get('name') === cube)
    : undefined;
  if (!found) {
    throw new CubeEditError(`Cube ${cube} is not in this file`);
  }
  return found as YAMLMap;
}

function list(cube: YAMLMap, key: 'measures' | 'dimensions' | 'segments'): YAMLMap[] {
  const items = cube.get(key);
  return isSeq(items) ? (items as YAMLSeq).items.filter(isMap) as YAMLMap[] : [];
}

function member(cube: YAMLMap, key: 'measures' | 'dimensions', name: string): YAMLMap {
  const found = list(cube, key).find((m) => m.get('name') === name);
  if (!found) {
    const kind = key === 'measures' ? 'Measure' : 'Dimension';
    throw new CubeEditError(`${kind} ${name} does not exist in cube ${String(cube.get('name'))}`);
  }
  return found;
}

// A new member name: valid, and not taken by any member of the cube.
function checkNewName(cube: YAMLMap, name: string) {
  if (!NAME.test(name)) {
    throw new CubeEditError(`The name ${name} is invalid: use letters, digits and _, not starting with a digit`);
  }
  const taken = (['measures', 'dimensions', 'segments'] as const)
    .some((key) => list(cube, key).some((m) => m.get('name') === name));
  if (taken) {
    throw new CubeEditError(`The name ${name} is already used by another member of the cube`);
  }
}

function checkSql(type: MeasureType, sql: unknown) {
  if (measureNeedsSql(type) && !(typeof sql === 'string' && sql.trim())) {
    throw new CubeEditError(`A ${type} measure needs SQL (the column or an expression)`);
  }
}

export function setMeasureType(text: string, cube: string, measure: string, type: MeasureType, sql?: string): string {
  if (!MEASURE_TYPES.includes(type)) {
    throw new CubeEditError(`${type} is not a measure type`);
  }
  const doc = load(text);
  const m = member(cubeOf(doc, cube), 'measures', measure);
  const current = m.get('sql');
  checkSql(type, sql?.trim() || current);
  m.set('type', type);
  if (sql?.trim() && !current) {
    m.set('sql', sql.trim());
  }
  return save(doc, text);
}

export type NewMeasure = { name: string; type: MeasureType; sql?: string; title?: string };

export function addMeasure(text: string, cube: string, measure: NewMeasure): string {
  if (!MEASURE_TYPES.includes(measure.type)) {
    throw new CubeEditError(`${measure.type} is not a measure type`);
  }
  const doc = load(text);
  const c = cubeOf(doc, cube);
  checkNewName(c, measure.name);
  checkSql(measure.type, measure.sql);
  const item: Record<string, string> = { name: measure.name, type: measure.type };
  if (measure.sql?.trim()) {
    item.sql = measure.sql.trim();
  }
  if (measure.title?.trim()) {
    item.title = measure.title.trim();
  }
  const measures = c.get('measures');
  if (isSeq(measures)) {
    (measures as YAMLSeq).add(doc.createNode(item));
  } else {
    c.set('measures', doc.createNode([item]));
  }
  return save(doc, text);
}

export function setDimensionType(text: string, cube: string, dimension: string, type: DimensionType): string {
  if (!DIMENSION_TYPES.includes(type)) {
    throw new CubeEditError(`${type} cannot be chosen here (geo takes latitude and longitude)`);
  }
  const doc = load(text);
  member(cubeOf(doc, cube), 'dimensions', dimension).set('type', type);
  return save(doc, text);
}

// The references to a member inside its own cube's SQL: `{name}`,
// `{CUBE.name}` and `{<cube>.name}`. Not `{CUBE}.name`: that is the table's
// column, which keeps its name.
function renameReferences(sql: string, cube: string, from: string, to: string): string {
  const owner = `(?:CUBE|${cube.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`;
  return sql.replace(new RegExp(`\\{(${owner}\\.)?${from}\\}`, 'g'), (_, prefix = '') => `{${prefix}${to}}`);
}

/**
 * Renames a dimension and the references to it in its own cube (other
 * members' SQL, segments, the primary key stays a flag). References from
 * other cubes, views and outside the model are not followed: the server
 * refuses a model that no longer compiles.
 */
export function renameDimension(text: string, cube: string, from: string, to: string): string {
  const doc = load(text);
  const c = cubeOf(doc, cube);
  const dim = member(c, 'dimensions', from);
  checkNewName(c, to);
  dim.set('name', to);
  (['measures', 'dimensions', 'segments'] as const).forEach((key) => list(c, key).forEach((m) => {
    if (m === dim) {
      return;
    }
    const node = m.get('sql', true) as { value?: unknown } | undefined;
    if (node && typeof node.value === 'string') {
      node.value = renameReferences(node.value, cube, from, to);
    }
  }));
  return save(doc, text);
}

// Whether `sql` refers to the member `name` of `cube`: `{name}`,
// `{CUBE.name}` or `{<cube>.name}`.
function refersTo(sql: string, cube: string, name: string): boolean {
  const owner = `(?:CUBE|${cube.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`;
  return new RegExp(`\\{(${owner}\\.)?${name}\\}`).test(sql);
}

/**
 * Removes a measure. Refused while another member of the cube (its SQL) or
 * one of the cube's pre-aggregations uses it; uses from other cubes, views
 * and outside the model are not seen here.
 */
export function removeMeasure(text: string, cube: string, measure: string): string {
  const doc = load(text);
  const c = cubeOf(doc, cube);
  const target = member(c, 'measures', measure);
  const users: string[] = [];
  (['measures', 'dimensions', 'segments'] as const).forEach((key) => list(c, key).forEach((m) => {
    const sql = m.get('sql');
    if (m !== target && typeof sql === 'string' && refersTo(sql, cube, measure)) {
      users.push(String(m.get('name')));
    }
  }));
  const rollups = c.get('pre_aggregations');
  if (isSeq(rollups)) {
    (rollups as YAMLSeq).items.filter(isMap).forEach((p) => {
      const measures = (p as YAMLMap).get('measures');
      const names = isSeq(measures)
        ? (measures as YAMLSeq).items.map((i) => String((i as { value?: unknown }).value ?? i))
        : [];
      if (names.some((n) => n === measure || n.endsWith(`.${measure}`))) {
        users.push(`pre-aggregation ${String((p as YAMLMap).get('name'))}`);
      }
    });
  }
  if (users.length) {
    throw new CubeEditError(`Measure ${measure} is used by ${users.join(', ')}: change those first`);
  }
  const measures = c.get('measures') as YAMLSeq;
  measures.items = measures.items.filter((m) => m !== target);
  if (!measures.items.length) {
    c.delete('measures');
  }
  return save(doc, text);
}

/**
 * The file without `cube`, or null when the cube is all the file defines (no
 * other cube, no view): the file is deleted then.
 */
export function removeCube(text: string, cube: string): string | null {
  const doc = load(text);
  const found = cubeOf(doc, cube);
  const cubes = doc.get('cubes') as YAMLSeq;
  cubes.items = cubes.items.filter((c) => c !== found);
  if (!cubes.items.length) {
    const views = doc.get('views');
    if (!(isSeq(views) && (views as YAMLSeq).items.length)) {
      return null;
    }
    doc.delete('cubes');
  }
  return save(doc, text);
}

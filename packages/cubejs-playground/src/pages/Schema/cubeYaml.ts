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
    throw new CubeEditError(`O arquivo não é um YAML válido: ${doc.errors[0].message}`);
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
    throw new CubeEditError(`O cubo ${cube} não está neste arquivo`);
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
    const kind = key === 'measures' ? 'measure' : 'dimension';
    throw new CubeEditError(`A ${kind} ${name} não existe no cubo ${String(cube.get('name'))}`);
  }
  return found;
}

// A new member name: valid, and not taken by any member of the cube.
function checkNewName(cube: YAMLMap, name: string) {
  if (!NAME.test(name)) {
    throw new CubeEditError(`O nome ${name} é inválido: use letras, números e _, sem começar por número`);
  }
  const taken = (['measures', 'dimensions', 'segments'] as const)
    .some((key) => list(cube, key).some((m) => m.get('name') === name));
  if (taken) {
    throw new CubeEditError(`O nome ${name} já é usado por outro membro do cubo`);
  }
}

function checkSql(type: MeasureType, sql: unknown) {
  if (measureNeedsSql(type) && !(typeof sql === 'string' && sql.trim())) {
    throw new CubeEditError(`Uma measure do tipo ${type} precisa de SQL (a coluna ou a expressão)`);
  }
}

export function setMeasureType(text: string, cube: string, measure: string, type: MeasureType, sql?: string): string {
  if (!MEASURE_TYPES.includes(type)) {
    throw new CubeEditError(`O tipo ${type} não é um tipo de measure`);
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
    throw new CubeEditError(`O tipo ${measure.type} não é um tipo de measure`);
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
    throw new CubeEditError(`O tipo ${type} não pode ser escolhido aqui (geo pede latitude e longitude)`);
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

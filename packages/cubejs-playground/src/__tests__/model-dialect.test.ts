import { describe, expect, it } from 'vitest';

import {
  convertDrafts, portableAs, registerDialects, toDialect, typesOf, validInAnyDialect, validType,
} from '../pages/Schema/models/dialect';
import type { ModelDefinition } from '../pages/Schema/models/modelGraph';

const CH_FEATURES = Object.fromEntries(
  ['primaryKey', 'check', 'index', 'comments', 'notNull', 'default', 'views'].map((k) => [k, true])
) as any;

describe('dialect', () => {
  it('toDialect maps portable types and leaves others', () => {
    expect(toDialect('postgres', 'numeric')).toBe('numeric(18,4)');
    expect(toDialect('postgres', 'integer')).toBe('integer');
    expect(toDialect('mysql', 'integer')).toBe('int');
    expect(toDialect('mysql', 'numeric')).toBe('decimal(18,4)');
    expect(toDialect('mysql', 'timestamp')).toBe('datetime');
    expect(toDialect('mysql', 'varchar(10)')).toBe('varchar(10)');
    expect(toDialect(null, 'numeric')).toBe('numeric');
  });

  it('validType mirrors the server cases', () => {
    ['varchar(64)', 'numeric(18,4)', 'numeric', 'double precision', 'timestamptz', 'integer']
      .forEach((t) => expect(validType('postgres', t), t).toBe(true));
    ['varchar(x)', 'int', 'varchar(0)', 'numeric(1,2,3)', 'jsonb(2)']
      .forEach((t) => expect(validType('postgres', t), t).toBe(false));
    ['int', 'varchar(255)', 'decimal(10, 2)', 'datetime', 'integer', 'numeric']
      .forEach((t) => expect(validType('mysql', t), t).toBe(true));
    expect(validType('mysql', 'varchar')).toBe(false);
    expect(validType('mysql', 'jsonb')).toBe(false);
    expect(validType(null, 'integer')).toBe(true);
    expect(validType(null, 'jsonb')).toBe(false);
    expect(typesOf('postgres')).toContain('double precision');
    expect(typesOf('mysql')).toContain('varbinary');
  });

  it('convertDrafts changes only portable draft types', () => {
    const def: ModelDefinition = {
      dataSource: 'ds',
      tables: [
        {
          alias: 'items',
          schema: 's',
          table: 'items',
          x: 0,
          y: 0,
          draft: {
            columns: [
              { name: 'id', type: 'integer', nullable: false, primaryKey: true },
              { name: 'price', type: 'numeric', nullable: true, primaryKey: false },
              { name: 'sku', type: 'varchar(9)' as any, nullable: true, primaryKey: false },
            ],
          },
        },
        {
          alias: 'orders', schema: 's', table: 'orders', x: 0, y: 0,
        },
      ],
      joins: [],
    };
    const next = convertDrafts(def, 'mysql');
    expect(next.tables[0].draft!.columns.map((c) => c.type)).toEqual(['int', 'decimal(18,4)', 'varchar(9)']);
    expect(next.tables[1]).toBe(def.tables[1]);
    expect(convertDrafts(def, null)).toBe(def);
    expect(convertDrafts(next, 'mysql')).toBe(next);
  });

  it('takes the dialects the server sends', () => {
    expect(validType('clickhouse', 'String')).toBe(false);
    registerDialects({
      clickhouse: {
        id: 'clickhouse',
        label: 'ClickHouse',
        types: [['String', 'none'], ['Int64', 'none'], ['Nullable', 'type'], ['DateTime64', 'free'], ['Decimal', 'precision']],
        portable: portableAs('String', 'Int32', 'Int64', 'Decimal(18,4)', 'Bool', 'Date', 'DateTime64(3)'),
        text: 'String',
        canCreate: true,
        canSql: true,
        features: CH_FEATURES,
      },
    });
    expect(validType('clickhouse', 'String')).toBe(true);
    expect(validType('clickhouse', 'string')).toBe(true);
    expect(validType('clickhouse', 'Nullable(Int64)')).toBe(true);
    expect(validType('clickhouse', 'Nullable(Nope)')).toBe(false);
    expect(validType('clickhouse', "DateTime64(3, 'UTC')")).toBe(true);
    expect(validType('clickhouse', 'Decimal(18,4)')).toBe(true);
    expect(validType('clickhouse', 'text')).toBe(true);
    expect(validInAnyDialect('Int64')).toBe(true);
    expect(toDialect('clickhouse', 'timestamp')).toBe('DateTime64(3)');
    expect(typesOf('clickhouse')).toContain('Nullable');
  });
});

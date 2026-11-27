import { describe, expect, it } from 'vitest';

import { CubeSQLConverter, sqlPath } from '../QueryBuilderV2/utils/cube-sql-converter';

describe('SQL of friendly-named members', () => {
  it('quotes a part that is not an identifier', () => {
    expect(sqlPath('orders.count')).toBe('orders.count');
    expect(sqlPath('bossa.Quantidade Total')).toBe('bossa."Quantidade Total"');
  });

  it('writes a query the SQL API reads', () => {
    const meta: any = {
      cubes: [{
        name: 'bossa',
        measures: [{ name: 'bossa.Quantidade Total', type: 'number' }],
        dimensions: [
          { name: 'bossa.Fabricante do Cabo', type: 'string' },
          { name: 'bossa.Data de Fabricação', type: 'time' },
        ],
        segments: [{ name: 'bossa.Só Corning' }],
      }],
    };
    const sql = new CubeSQLConverter({
      measures: ['bossa.Quantidade Total'],
      dimensions: ['bossa.Fabricante do Cabo'],
      timeDimensions: [{ dimension: 'bossa.Data de Fabricação', granularity: 'month' }],
      filters: [{ member: 'bossa.Fabricante do Cabo', operator: 'equals', values: ['AFL'] }],
      segments: ['bossa.Só Corning'],
      order: [{ id: 'bossa.Quantidade Total', desc: true }],
    } as any, meta).buildQuery();
    expect(sql).toContain('MEASURE(bossa."Quantidade Total")');
    expect(sql).toContain('bossa."Fabricante do Cabo" = \'AFL\'');
    expect(sql).toContain('DATE_TRUNC(\'month\', bossa."Data de Fabricação")');
    expect(sql).toContain('bossa."Só Corning"');
    expect(sql).not.toMatch(/bossa\.Quantidade Total/);
  });
});

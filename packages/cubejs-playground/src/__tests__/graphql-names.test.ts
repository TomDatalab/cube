import { describe, expect, it } from 'vitest';
import { parse } from 'graphql/language';

import { CubeGraphQLConverter } from '../components/GraphQL/CubeGraphQLConverter';
import { graphqlField, graphqlPath, withGraphQLNames } from '../components/GraphQL/graphqlNames';
import { convertJsonQueryToGraphQL } from '../QueryBuilderV2/utils/graphql-converters';

describe('GraphQL names of friendly-named members', () => {
  it('match the server', () => {
    expect(graphqlField('created_at')).toBe('created_at');
    expect(graphqlField('Quantidade Total')).toBe('quantidadeTotal');
    expect(graphqlField('Data de Fabricação')).toBe('dataDeFabricacao');
    expect(graphqlField('1ª conexão')).toBe('_1Conexao');
    expect(graphqlPath('bossa.Data de Fabricação.month')).toBe('bossa.dataDeFabricacao.month');
  });

  it('write a valid document from a query by friendly names', () => {
    const types = {
      'bossa.Quantidade Total': 'number',
      'bossa.Fabricante do Cabo': 'string',
      'bossa.Data de Fabricação': 'time',
    } as const;
    const query = {
      measures: ['bossa.Quantidade Total'],
      dimensions: ['bossa.Fabricante do Cabo'],
      timeDimensions: [{ dimension: 'bossa.Data de Fabricação', granularity: 'month' as const }],
      filters: [{ or: [{ member: 'bossa.Fabricante do Cabo', operator: 'equals' as const, values: ['AFL'] }] }],
      order: [['bossa.Quantidade Total', 'desc']] as [string, 'desc'][],
    };
    const named = withGraphQLNames(query, types);
    const doc = new CubeGraphQLConverter(named.query, named.types).convert();
    expect(() => parse(doc)).not.toThrow();
    expect(doc).toContain('quantidadeTotal');
    expect(doc).toContain('fabricanteDoCabo');
    expect(doc).toContain('dataDeFabricacao');
    expect(doc).not.toContain('Quantidade Total');

    const meta: any = {
      cubesMap: {
        bossa: {
          measures: { 'bossa.Quantidade Total': { name: 'bossa.Quantidade Total', type: 'number' } },
          dimensions: { 'bossa.Fabricante do Cabo': { name: 'bossa.Fabricante do Cabo', type: 'string' } },
        },
      },
    };
    const v2 = convertJsonQueryToGraphQL({ meta, query: { measures: ['bossa.Quantidade Total'], dimensions: ['bossa.Fabricante do Cabo'] } });
    expect(() => parse(v2)).not.toThrow();
    expect(v2).toContain('quantidadeTotal');
  });
});

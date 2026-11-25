import { describe, expect, it } from 'vitest';

import { missingCubesError } from '../QueryBuilderV2/utils/missing-cubes';

describe('missingCubesError', () => {
  it('lets a query whose cubes are all in the model run', () => {
    expect(missingCubesError([])).toBeNull();
  });

  it('names the cubes the scope does not have', () => {
    expect(missingCubesError(['dfos_fibercable'])).toBe(
      'Cube dfos_fibercable is not in the data model of the scope chosen in the header. '
      + 'Choose the organization that has it, or remove it from the query.'
    );
    expect(missingCubesError(['a', 'b'])).toMatch(/^Cubes a, b are not in the data model/);
  });
});

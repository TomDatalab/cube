import { describe, expect, it } from 'vitest';

import { dropAction, sourcesByOrganization } from '../pages/DataSources/organizationDrop';

const on = { id: 'o1', name: 'ON' };
const digital = { id: 'o2', name: 'Digital' };

describe('dropAction', () => {
  it('assigns a Platform data source', () => {
    expect(dropAction({ name: 'sofiadb', organization: null }, 'o1')).toBe('assign');
    expect(dropAction({ name: 'sofiadb' }, 'o1')).toBe('assign');
  });

  it('moves a data source of another organization', () => {
    expect(dropAction({ name: 'sales', organization: digital }, 'o1')).toBe('move');
  });

  it('does nothing on the organization it already belongs to', () => {
    expect(dropAction({ name: 'sales', organization: on }, 'o1')).toBe('none');
  });

  it('never assigns default', () => {
    expect(dropAction({ name: 'default', organization: null }, 'o1')).toBe('forbidden');
  });
});

describe('sourcesByOrganization', () => {
  it('groups by organization, sorted by name, leaving Platform out', () => {
    const groups = sourcesByOrganization([
      { name: 'zeta', organization: on },
      { name: 'alpha', organization: on },
      { name: 'beta', organization: digital },
      { name: 'sofiadb', organization: null },
    ]);
    expect(Object.keys(groups).sort()).toEqual(['o1', 'o2']);
    expect(groups.o1.map((ds) => ds.name)).toEqual(['alpha', 'zeta']);
    expect(groups.o2.map((ds) => ds.name)).toEqual(['beta']);
  });
});

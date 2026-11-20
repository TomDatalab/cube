import { describe, expect, it } from 'vitest';

import { connectionLabel, dropAction, sourcesByOrganization } from '../pages/DataSources/organizationDrop';

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

describe('dropAction onto Platform', () => {
  it('unassigns a data source of an organization', () => {
    expect(dropAction({ name: 'sales', organization: on }, null)).toBe('unassign');
  });

  it('does nothing for one already Platform', () => {
    expect(dropAction({ name: 'sofiadb', organization: null }, null)).toBe('none');
  });

  it('never touches default', () => {
    expect(dropAction({ name: 'default', organization: null }, null)).toBe('forbidden');
  });
});

describe('connectionLabel', () => {
  it('joins user, host, port and database', () => {
    expect(connectionLabel({
      user: 'default', host: '127.0.0.1', port: 8201, database: 'cluster_test',
    })).toBe('default@127.0.0.1:8201/cluster_test');
  });

  it('leaves out what is missing', () => {
    expect(connectionLabel({ host: '127.0.0.1', port: 8201, database: 'cluster_test' }))
      .toBe('127.0.0.1:8201/cluster_test');
    expect(connectionLabel({ database: '/tmp/cube-example.db' })).toBe('/tmp/cube-example.db');
    expect(connectionLabel({ host: 'h' })).toBe('h');
  });

  it('is a dash when there is nothing', () => {
    expect(connectionLabel({})).toBe('—');
    expect(connectionLabel({ host: null, database: null })).toBe('—');
  });
});

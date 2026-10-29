import { describe, expect, it } from 'vitest';

import { WorkspaceListing } from '../pages/Admin/api';
import { groupByOrganization } from '../pages/Admin/membershipGroups';

function listing(id: string, name: string, organizationId: string, organizationName: string): WorkspaceListing {
  return {
    id, name, organizationId, organizationName, createdAt: '', createdBy: null,
  };
}

describe('groupByOrganization', () => {
  it('groups consecutive workspaces by organization in server order', () => {
    const groups = groupByOrganization([
      listing('w1', 'Ops', 'o1', 'ON'),
      listing('w2', 'Vendas', 'o1', 'ON'),
      listing('w3', 'Fin', 'o2', 'Digital'),
    ]);
    expect(groups.map((g) => g.organizationName)).toEqual(['ON', 'Digital']);
    expect(groups[0].workspaces.map((w) => w.name)).toEqual(['Ops', 'Vendas']);
    expect(groups[1].workspaces.map((w) => w.name)).toEqual(['Fin']);
  });
});

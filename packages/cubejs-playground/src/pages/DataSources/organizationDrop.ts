// The drag payload of a data source row: its name. A custom type, so text
// dragged in from elsewhere is not taken for a data source.
export const DATA_SOURCE_DRAG_TYPE = 'application/x-cube-data-source';

type Owned = { name: string; organization?: { id: string; name: string } | null };

// What dropping a data source on an organization does:
// - `assign`: it has no organization (Platform) and now gets this one;
// - `move`: it belongs to another organization, whose workspace releases of
//   it are withdrawn;
// - `none`: it already belongs to this organization;
// - `forbidden`: `default`, which stays Platform for good.
export type DropAction = 'assign' | 'move' | 'none' | 'forbidden';

export function dropAction(ds: Owned, organizationId: string): DropAction {
  if (ds.name === 'default') {
    return 'forbidden';
  }
  if (!ds.organization) {
    return 'assign';
  }
  return ds.organization.id === organizationId ? 'none' : 'move';
}

// The data sources of each organization, by name, for the drop panel.
export function sourcesByOrganization<T extends Owned>(sources: T[]): Record<string, T[]> {
  const out: Record<string, T[]> = {};
  sources.forEach((ds) => {
    if (ds.organization) {
      (out[ds.organization.id] ||= []).push(ds);
    }
  });
  Object.values(out).forEach((list) => list.sort((a, b) => a.name.localeCompare(b.name)));
  return out;
}

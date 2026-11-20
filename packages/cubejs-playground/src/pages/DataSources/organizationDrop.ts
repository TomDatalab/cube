// The drag payload of a data source row: its name. A custom type, so text
// dragged in from elsewhere is not taken for a data source.
export const DATA_SOURCE_DRAG_TYPE = 'application/x-cube-data-source';

type Owned = { name: string; organization?: { id: string; name: string } | null };

// What dropping a data source on an organization (`null`: on Platform)
// does:
// - `assign`: it has no organization (Platform) and now gets this one;
// - `move`: it belongs to another organization, whose workspace releases of
//   it are withdrawn;
// - `unassign`: dropped on Platform, it leaves its organization (and the
//   workspaces it was released to);
// - `none`: it already is where it was dropped;
// - `forbidden`: `default`, which stays Platform for good.
export type DropAction = 'assign' | 'move' | 'unassign' | 'none' | 'forbidden';

export function dropAction(ds: Owned, organizationId: string | null): DropAction {
  if (ds.name === 'default') {
    return 'forbidden';
  }
  if (organizationId === null) {
    return ds.organization ? 'unassign' : 'none';
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

type Located = {
  user?: string | null;
  host?: string | null;
  port?: number | null;
  database?: string | null;
};

// `user@host:port/database`, leaving out what a data source does not have
// (a SQLite file is only its database); `—` for none of them.
export function connectionLabel(ds: Located): string {
  const address = [ds.host, ds.port].filter((v) => v != null && v !== '').join(':');
  const user = ds.user ? `${ds.user}@` : '';
  const located = address ? `${user}${address}` : '';
  const database = ds.database || '';
  if (located && database) {
    return `${located}/${database}`;
  }
  return located || database || '—';
}

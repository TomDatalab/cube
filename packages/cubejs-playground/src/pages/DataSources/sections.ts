import type { MetaGroup } from '../../shared/meta-groups';
import { PLATFORM_GROUP } from '../../shared/meta-groups';

type Owned = { name: string; organization?: { id: string; name: string } | null };

export type DataSourceSection<T extends Owned> = {
  // `platform`, or the organization's id.
  key: string;
  name: string;
  dataSources: T[];
  // Cubes and views of the section's model that `/meta` shows.
  cubes: number;
  // The model's problems in the section (`null`: not known).
  errors: number | null;
};

export const PLATFORM_SECTION = 'platform';

// The data sources by owner, as the Data Model page splits the model:
// Platform first, then every organization by name, those without data
// sources too. A data source whose organization is not in `organizations`
// (the list is still loading) gets a section of its own all the same.
export function dataSourceSections<T extends Owned>(
  dataSources: T[],
  organizations: { id: string; name: string }[],
  groups: MetaGroup[] | null
): DataSourceSection<T>[] {
  const byGroup = new Map<string, MetaGroup>();
  (groups || []).forEach((g) => {
    byGroup.set(g.slug === PLATFORM_GROUP ? PLATFORM_SECTION : g.id || g.slug, g);
  });
  const cubesOf = (key: string) => byGroup.get(key)?.dataSources
    .reduce((n, ds) => n + ds.cubes.length, 0) ?? 0;
  const errorsOf = (key: string) => (groups ? byGroup.get(key)?.errors ?? 0 : null);

  const orgs = new Map<string, string>();
  organizations.forEach((o) => orgs.set(o.id, o.name));
  dataSources.forEach((ds) => {
    if (ds.organization && !orgs.has(ds.organization.id)) {
      orgs.set(ds.organization.id, ds.organization.name);
    }
  });

  const section = (key: string, name: string): DataSourceSection<T> => ({
    key,
    name,
    dataSources: dataSources.filter((ds) => (
      key === PLATFORM_SECTION ? !ds.organization : ds.organization?.id === key
    )),
    cubes: cubesOf(key),
    errors: errorsOf(key),
  });

  return [
    section(PLATFORM_SECTION, 'Platform'),
    ...[...orgs.entries()]
      .sort((a, b) => a[1].localeCompare(b[1]))
      .map(([id, name]) => section(id, name)),
  ];
}

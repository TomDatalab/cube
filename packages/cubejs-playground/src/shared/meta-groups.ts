// `GET <api>/meta/groups`: the cubes and views `/meta` shows, by the
// organization whose model folder holds them and, inside it, by data source.
// The query builder's side panel and the Data Source page group with it, as
// the Data Model page does.

export const PLATFORM_GROUP = '_platform';
// A cube the server could not place (its index lags a model reload).
export const OTHER_GROUP = '_other';

export type MetaGroup = {
  slug: string;
  id: string | null;
  name: string;
  // Rejected cubes and model errors of the group: platform admins only
  // (everyone without RBAC); `null` otherwise.
  errors: number | null;
  dataSources: { name: string; cubes: string[] }[];
  // Which of the names in `dataSources` are views (older servers: absent).
  views?: string[];
};

export type CubeSection<T> = {
  // Unique among the sections: the group's slug.
  key: string;
  slug: string;
  name: string;
  errors: number | null;
  // Every item of the section, in the order they came in.
  items: T[];
  // The same items by data source, in the server's order of data sources.
  dataSources: { name: string; items: T[] }[];
};

// The groups, or `null` when the server has no such route (an older server,
// or another Cube API) or cannot answer: the caller then lists without
// groups.
export async function fetchMetaGroups(
  url: string,
  init: RequestInit,
  doFetch: (url: string, init: RequestInit) => Promise<Response> = fetch
): Promise<MetaGroup[] | null> {
  try {
    const response = await doFetch(url, init);
    if (!response.ok) {
      return null;
    }
    const body = await response.json();
    return Array.isArray(body?.groups) ? body.groups : null;
  } catch {
    return null;
  }
}

// `items` split by `groups`, keeping the groups' order (Platform first) and
// each item's order in `items`. An item no group names goes to "Other", last.
// Groups with none of the items are left out unless `keepEmpty`.
export function sectionsOf<T extends { name: string }>(
  items: T[],
  groups: MetaGroup[],
  { keepEmpty = false }: { keepEmpty?: boolean } = {}
): CubeSection<T>[] {
  const placeOf = new Map<string, { group: number; dataSource: number }>();
  groups.forEach((group, g) => {
    group.dataSources.forEach((ds, d) => {
      ds.cubes.forEach((cube) => placeOf.set(cube, { group: g, dataSource: d }));
    });
  });

  const sections: CubeSection<T>[] = groups.map((group) => ({
    key: group.slug,
    slug: group.slug,
    name: group.name,
    errors: group.errors,
    items: [],
    dataSources: group.dataSources.map((ds) => ({ name: ds.name, items: [] })),
  }));
  const other: T[] = [];

  items.forEach((item) => {
    const place = placeOf.get(item.name);
    if (!place) {
      other.push(item);
      return;
    }
    const section = sections[place.group];
    section.items.push(item);
    section.dataSources[place.dataSource].items.push(item);
  });

  const result = sections
    .map((section) => ({
      ...section,
      dataSources: section.dataSources.filter((ds) => ds.items.length),
    }))
    .filter((section) => keepEmpty || section.items.length);

  if (other.length) {
    const existing = result.find((s) => s.slug === OTHER_GROUP);
    if (existing) {
      existing.items.push(...other);
      existing.dataSources.push({ name: '', items: other });
    } else {
      result.push({
        key: OTHER_GROUP,
        slug: OTHER_GROUP,
        name: 'Other',
        errors: null,
        items: other,
        dataSources: [{ name: '', items: other }],
      });
    }
  }
  return result;
}

// Whether grouping says anything: more than one organization, or one with
// more than one data source.
export function worthGrouping<T>(sections: CubeSection<T>[]): boolean {
  return sections.length > 1 || (sections[0]?.dataSources.length ?? 0) > 1;
}

// How many cubes and views read each data source, over every group.
export function cubeCountByDataSource(groups: MetaGroup[]): Record<string, number> {
  const counts: Record<string, number> = {};
  groups.forEach((group) => group.dataSources.forEach((ds) => {
    counts[ds.name] = (counts[ds.name] || 0) + ds.cubes.length;
  }));
  return counts;
}

// The organizations a filter offers, in the groups' order.
export function filterOptions(groups: MetaGroup[]): { value: string; label: string }[] {
  return groups.map((g) => ({ value: g.slug, label: g.name }));
}

// The answers of several scopes as one list: Platform first, then the
// organizations. Every organization's answer repeats the Platform group
// (`_platform/**` is in its scope), so a slug seen before is skipped.
export function mergeMetaGroups(groupsPerScope: MetaGroup[][]): MetaGroup[] {
  const seen = new Set<string>();
  const merged: MetaGroup[] = [];
  groupsPerScope.flat().forEach((group) => {
    if (!seen.has(group.slug)) {
      seen.add(group.slug);
      merged.push(group);
    }
  });
  return merged;
}

// An organization whose model the query builder is not on: its cubes (or
// views) by name only. Choosing one switches to that organization's model.
export type ScopeSection = {
  slug: string;
  name: string;
  errors: number | null;
  count: number;
  dataSources: { name: string; names: string[] }[];
};

// The groups of `all` (every scope's, merged) that the current model does not
// serve: neither the Platform (in every model) nor a group of `current`.
// Their names of `type`, matching `filter` (lowercase) when given. A group
// with none is left out, unless it has errors and nothing is filtered.
export function otherScopeSections(
  all: MetaGroup[],
  current: MetaGroup[],
  type: 'cubes' | 'views',
  filter = ''
): ScopeSection[] {
  const served = new Set(current.map((g) => g.slug));
  return all
    .filter((group) => group.slug !== PLATFORM_GROUP && !served.has(group.slug))
    .map((group) => {
      const views = new Set(group.views ?? []);
      const dataSources = group.dataSources
        .map((ds) => ({
          name: ds.name,
          names: ds.cubes.filter((name) => (
            (type === 'views') === views.has(name)
            && (!filter || name.toLowerCase().includes(filter))
          )),
        }))
        .filter((ds) => ds.names.length);
      return {
        slug: group.slug,
        name: group.name,
        errors: group.errors,
        count: dataSources.reduce((n, ds) => n + ds.names.length, 0),
        dataSources,
      };
    })
    .filter((section) => section.count || (!filter && (section.errors ?? 0) > 0));
}

// The cube to open once the page is back on another organization's model:
// kept in sessionStorage across the reload that switches models.
const CUBE_TO_OPEN_KEY = 'cube.playground.cubeToOpen';

export function rememberCubeToOpen(name: string) {
  try {
    window.sessionStorage.setItem(CUBE_TO_OPEN_KEY, name);
  } catch {
    // Without storage the cube is just not opened.
  }
}

export function takeCubeToOpen(): string | null {
  try {
    const name = window.sessionStorage.getItem(CUBE_TO_OPEN_KEY);
    window.sessionStorage.removeItem(CUBE_TO_OPEN_KEY);
    return name;
  } catch {
    return null;
  }
}

export type FilterSection = { slug: string; name: string; count: number };

// The organizations the filter offers, shared by the Cubes and Views tabs:
// every organization with something on either tab (from the current model,
// `own`, or another one, `other`), counting only the `shown` tab's items,
// in `order` (slugs; one it does not name goes last).
export function filterSectionsOf(
  byType: Record<'cubes' | 'views', {
    own: { slug: string; name: string; items: unknown[] }[];
    other: ScopeSection[];
  }>,
  shown: 'cubes' | 'views',
  order: string[]
): FilterSection[] {
  const result: FilterSection[] = [];
  (['cubes', 'views'] as const).forEach((type) => {
    const { own, other } = byType[type];
    [
      ...own.map((section) => ({ slug: section.slug, name: section.name, count: section.items.length })),
      ...other,
    ].forEach(({ slug, name, count }) => {
      const counted = type === shown ? count : 0;
      const existing = result.find((section) => section.slug === slug);
      if (existing) {
        existing.count += counted;
      } else {
        result.push({ slug, name, count: counted });
      }
    });
  });
  const rank = (slug: string) => {
    const at = order.indexOf(slug);
    return at === -1 ? order.length : at;
  };
  return result.sort((a, b) => rank(a.slug) - rank(b.slug));
}

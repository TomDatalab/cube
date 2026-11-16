// The Admin pages, each at /admin/<key>; the sidebar lists them under Admin.
export const ADMIN_SECTIONS = [
  { key: 'organizations', label: 'Organizations' },
  { key: 'workspaces', label: 'Workspaces' },
  { key: 'users', label: 'Users' },
] as const;

export type AdminSection = typeof ADMIN_SECTIONS[number]['key'];

export const adminPath = (section: AdminSection) => `/admin/${section}`;

// The section a path shows; /admin alone and unknown sections show the first.
export function adminSectionOf(pathname: string): AdminSection {
  const key = pathname.split('/')[2];
  return ADMIN_SECTIONS.find((section) => section.key === key)?.key ?? ADMIN_SECTIONS[0].key;
}

// The admin's scopes besides workspaces: Platform, then each organization.
export function adminOptions(
  organizations: { name: string; slug: string }[]
): { value: string; label: string }[] {
  return [
    { value: '_platform', label: 'Platform (admin)' },
    ...organizations.map((o) => ({ value: o.slug, label: `${o.name} (admin)` })),
  ];
}

type NextChoiceOptions = {
  current: string | null;
  // Whether the server still lets this user see `current`.
  currentVisible: boolean;
  firstPage: { value: string }[];
  isAdmin: boolean;
};

// What to do once the selector has loaded: `{ choose }` switches workspace
// (null is "Platform (admin)"), `null` keeps the current one.
export function nextChoice({
  current, currentVisible, firstPage, isAdmin,
}: NextChoiceOptions): { choose: string | null } | null {
  if (current && currentVisible) {
    return null;
  }
  // Nothing chosen yet, or a workspace this user no longer sees.
  if (!isAdmin && firstPage.length) {
    return { choose: firstPage[0].value };
  }
  return current ? { choose: null } : null;
}

type NextChoiceOptions = {
  current: string | null;
  // Whether the server still lets this user see `current`.
  currentVisible: boolean;
  firstPage: { value: string }[];
  isAdmin: boolean;
};

// What to do once the selector has loaded: `{ choose }` switches workspace
// (null is "Todos (admin)"), `null` keeps the current one.
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

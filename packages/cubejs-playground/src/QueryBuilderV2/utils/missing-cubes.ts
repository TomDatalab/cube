/**
 * Why a query cannot run in the current scope, or null when it can: the cubes
 * it uses that the scope's data model does not have. The Playground keeps its
 * query tabs across scopes, so a query of another organization's cube comes
 * back after the header moves; sent, the server would only say
 * "Cannot resolve: <cube>".
 */
export function missingCubesError(missingCubes: string[]): string | null {
  if (!missingCubes.length) {
    return null;
  }
  const names = missingCubes.join(', ');
  const them = missingCubes.length === 1 ? `Cube ${names} is` : `Cubes ${names} are`;
  return `${them} not in the data model of the scope chosen in the header. `
    + 'Choose the organization that has it, or remove it from the query.';
}

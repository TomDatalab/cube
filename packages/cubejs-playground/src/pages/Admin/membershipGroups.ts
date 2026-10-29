import type { WorkspaceListing } from './api';

export type OrganizationGroup = {
  organizationId: string;
  organizationName: string;
  workspaces: WorkspaceListing[];
};

// Groups a page of workspaces under their organization. The server orders by
// organization, so the workspaces of one organization are consecutive.
export function groupByOrganization(items: WorkspaceListing[]): OrganizationGroup[] {
  const groups: OrganizationGroup[] = [];
  items.forEach((ws) => {
    const last = groups[groups.length - 1];
    if (last && last.organizationId === ws.organizationId) {
      last.workspaces.push(ws);
    } else {
      groups.push({
        organizationId: ws.organizationId,
        organizationName: ws.organizationName,
        workspaces: [ws],
      });
    }
  });
  return groups;
}

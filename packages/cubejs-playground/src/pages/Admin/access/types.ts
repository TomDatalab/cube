export type AccessNodeKind = 'user' | 'organization' | 'workspace' | 'connection';

export type AccessNode = {
  id: string;
  kind: AccessNodeKind;
  name: string;
  organizationId: string | null;
  notes: string[];
};

export type AccessEdge = {
  from: string;
  to: string;
  label: string;
  kind: 'direct' | 'derived';
};

export type Reason = {
  text: string;
  /** Node ids from the user to the resource. */
  path: string[];
};

export type Grant = {
  /** Node id. */
  resource: string;
  permission: string;
  reasons: Reason[];
};

export type AccessGraph = {
  user: { id: string; name: string | null; disabled: boolean };
  platformAdmin: boolean;
  nodes: AccessNode[];
  edges: AccessEdge[];
  grants: Grant[];
};

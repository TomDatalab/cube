CREATE TABLE IF NOT EXISTS users (
    id text PRIMARY KEY,
    name text,
    email text,
    password_hash text NOT NULL,
    disabled boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    password_changed_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS organizations (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL
);

CREATE TABLE IF NOT EXISTS workspaces (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id),
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL,
    UNIQUE (organization_id, name)
);

CREATE TABLE IF NOT EXISTS connections (
    name text PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id)
);

-- How the connection came to its organization: `created` by the
-- organization (its owners may edit and delete it) or `assigned` by a
-- platform admin (only platform admins may edit and delete it). Rows from
-- before the column existed were all assigned or are treated as such.
ALTER TABLE connections ADD COLUMN IF NOT EXISTS origin text NOT NULL DEFAULT 'assigned';

CREATE TABLE IF NOT EXISTS objects (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    type text NOT NULL,
    name text NOT NULL,
    content_type text NOT NULL,
    content bytea NOT NULL,
    version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by text NOT NULL
);

CREATE INDEX IF NOT EXISTS objects_workspace_idx ON objects (workspace_id);

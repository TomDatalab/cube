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

-- A rule: a named role in one workspace. `type` is a free label (e.g.
-- `finance`); `role` (`owner`, `editor` or `viewer`) is what its members
-- hold in the workspace through SpiceDB (`workspace#<role>_rule@rule`,
-- `rule#member@user`); the organization is the workspace's.
CREATE TABLE IF NOT EXISTS rules (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    description text NOT NULL,
    type text NOT NULL,
    role text NOT NULL CHECK (role IN ('owner', 'editor', 'viewer')),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL
);

CREATE INDEX IF NOT EXISTS rules_workspace_idx ON rules (workspace_id);

-- Rules from before `role` existed had the role in `type`, restricted to the
-- three roles: it becomes their role, and `type` any label.
ALTER TABLE rules ADD COLUMN IF NOT EXISTS role text;
UPDATE rules SET role = type WHERE role IS NULL;
ALTER TABLE rules ALTER COLUMN role SET NOT NULL;
ALTER TABLE rules DROP CONSTRAINT IF EXISTS rules_type_check;
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'rules_role_check') THEN
        ALTER TABLE rules ADD CONSTRAINT rules_role_check
            CHECK (role IN ('owner', 'editor', 'viewer'));
    END IF;
END $$;

-- A role an organization defines: a name and the workspace permissions it
-- grants (`view`, `query`, `read_objects`, `write_objects`, `execute`,
-- `manage`). In SpiceDB each permission is `role#can_<permission>@user:*`;
-- a rule with the role gives its members those permissions in its
-- workspace.
CREATE TABLE IF NOT EXISTS roles (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name text NOT NULL,
    permissions text[] NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL,
    UNIQUE (organization_id, name)
);

-- A rule holds a built-in role (`role`) or one of its organization's
-- (`role_id`), never both. A role in use cannot be deleted.
ALTER TABLE rules ADD COLUMN IF NOT EXISTS role_id uuid REFERENCES roles (id);
ALTER TABLE rules ALTER COLUMN role DROP NOT NULL;
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'rules_one_role') THEN
        ALTER TABLE rules ADD CONSTRAINT rules_one_role
            CHECK ((role IS NULL) <> (role_id IS NULL));
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS rules_role_idx ON rules (role_id);

-- Data restrictions of an organization role (blocked members, row filters)
-- and the attributes of a user those filters read.
ALTER TABLE roles ADD COLUMN IF NOT EXISTS restrictions jsonb NOT NULL DEFAULT '{}';

CREATE TABLE IF NOT EXISTS user_attributes (
    user_id text NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name text NOT NULL,
    values text[] NOT NULL,
    PRIMARY KEY (user_id, name)
);

-- Every data source's settings, with or without an organization (with
-- RBAC on they are kept here, not in cube.yml/.env). `settings` holds the
-- plain `CUBEJS_DB_*` variables; `secrets` the secret ones as a JSON
-- object sealed with AES-256-GCM under CUBE_RBAC_SECRET_KEY.
CREATE TABLE IF NOT EXISTS data_sources (
    id uuid UNIQUE NOT NULL DEFAULT gen_random_uuid(),
    name text PRIMARY KEY,
    db_type text NOT NULL,
    settings jsonb NOT NULL DEFAULT '{}',
    secrets bytea,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

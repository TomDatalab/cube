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

ALTER TABLE organizations ADD COLUMN IF NOT EXISTS slug text;
CREATE UNIQUE INDEX IF NOT EXISTS organizations_slug ON organizations (slug);

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
-- Only before `role_id` exists: once it does, a rule with an organization's
-- role has no `role`, and filling it would break `rules_one_role` (and with
-- it every boot).
ALTER TABLE rules ADD COLUMN IF NOT EXISTS role text;
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = current_schema() AND table_name = 'rules' AND column_name = 'role_id'
    ) THEN
        UPDATE rules SET role = type WHERE role IS NULL;
        ALTER TABLE rules ALTER COLUMN role SET NOT NULL;
    END IF;
END $$;
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

-- The Cube data model (YAML/Jinja files), one row per file. The owner is
-- `organization_id` (NULL: Platform); `path` is relative to the organization.
CREATE TABLE IF NOT EXISTS model_files (
    id uuid PRIMARY KEY,
    organization_id uuid REFERENCES organizations (id),
    path text NOT NULL,
    content text NOT NULL,
    version integer NOT NULL DEFAULT 1,
    managed_by text,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by text NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS model_files_path_idx
    ON model_files (coalesce(organization_id, '00000000-0000-0000-0000-000000000000'), path);

-- One row; `version` goes up in the same transaction as every model write.
CREATE TABLE IF NOT EXISTS model_state (
    id boolean PRIMARY KEY DEFAULT true CHECK (id),
    version bigint NOT NULL DEFAULT 0
);
INSERT INTO model_state (id, version) VALUES (true, 0) ON CONFLICT DO NOTHING;

-- One-off data migrations: a row per step once it has completed.
CREATE TABLE IF NOT EXISTS rbac_migrations (
    name text PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);

-- `users_uuid`: a user's id was their username. It becomes a UUID (as
-- lower-case hyphenated text, so `user_attributes.user_id` keeps its type)
-- and the username moves to its own column, NOT NULL UNIQUE. The FK from
-- `user_attributes` cascades the new id. One DO block: all or nothing.
DO $$
DECLARE
    fk text;
BEGIN
    IF EXISTS (SELECT 1 FROM rbac_migrations WHERE name = 'users_uuid') THEN
        RETURN;
    END IF;

    ALTER TABLE users ADD COLUMN IF NOT EXISTS username text;
    UPDATE users SET username = id WHERE username IS NULL;
    ALTER TABLE users ALTER COLUMN username SET NOT NULL;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint
                   WHERE conname = 'users_username_key' AND conrelid = 'users'::regclass) THEN
        ALTER TABLE users ADD CONSTRAINT users_username_key UNIQUE (username);
    END IF;

    FOR fk IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'user_attributes'::regclass AND contype = 'f'
    LOOP
        EXECUTE format('ALTER TABLE user_attributes DROP CONSTRAINT %I', fk);
    END LOOP;
    ALTER TABLE user_attributes ADD CONSTRAINT user_attributes_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES users (id)
        ON DELETE CASCADE ON UPDATE CASCADE;

    -- The same test as `store::is_uuid`: canonical form only.
    UPDATE users SET id = gen_random_uuid()::text
    WHERE id !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$';

    INSERT INTO rbac_migrations (name) VALUES ('users_uuid');
END $$;

-- When the username last changed (NULL: never). A token naming the user by
-- username (a legacy `sub`) issued before it no longer authenticates, so a
-- name given up by a rename cannot be picked up by someone else's old token.
ALTER TABLE users ADD COLUMN IF NOT EXISTS username_changed_at timestamptz;

-- The key of a default role of an organization (`owner`, `editor`,
-- `viewer`); one per key per organization. Null for any other role.
ALTER TABLE roles ADD COLUMN IF NOT EXISTS builtin text;
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'roles_builtin_check') THEN
        ALTER TABLE roles ADD CONSTRAINT roles_builtin_check
            CHECK (builtin IN ('owner', 'editor', 'viewer'));
    END IF;
END $$;
CREATE UNIQUE INDEX IF NOT EXISTS roles_builtin_idx
    ON roles (organization_id, builtin) WHERE builtin IS NOT NULL;

-- A group of users of an organization. Its members are in SpiceDB
-- (`group#member@user`), as are the rules it is placed in
-- (`rule#group@group`): there its members are members of the rule.
CREATE TABLE IF NOT EXISTS groups (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL,
    UNIQUE (organization_id, name)
);

-- A model: a canvas of joined tables (`definition`) built into one Cube YAML
-- cube. `cube_*` record the last build; they do not bump `version`.
CREATE TABLE IF NOT EXISTS cube_models (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name text NOT NULL,
    definition jsonb NOT NULL,
    cube_name text,
    cube_file_id uuid,
    cube_built_hash text,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by text NOT NULL,
    UNIQUE (organization_id, name)
);

-- What the last build of a model wrote, per cube: its file and the hash of
-- what was written. Moved here from cube_models.cube_file_id/cube_built_hash
-- (cube_models.cube_name stays: it names the converted cube).
CREATE TABLE IF NOT EXISTS cube_model_cubes (
    model_id uuid NOT NULL REFERENCES cube_models (id) ON DELETE CASCADE,
    cube_id text NOT NULL,
    cube_name text NOT NULL,
    file_id uuid NOT NULL,
    built_hash text NOT NULL,
    PRIMARY KEY (model_id, cube_id)
);
INSERT INTO cube_model_cubes (model_id, cube_id, cube_name, file_id, built_hash)
    SELECT id, 'c1', cube_name, cube_file_id, cube_built_hash FROM cube_models
    WHERE cube_name IS NOT NULL AND cube_file_id IS NOT NULL AND cube_built_hash IS NOT NULL
    ON CONFLICT DO NOTHING;
UPDATE cube_models SET cube_file_id = NULL, cube_built_hash = NULL WHERE cube_file_id IS NOT NULL;

-- Who is editing a model: one live lock per model (see handlers::cube_models).
CREATE TABLE IF NOT EXISTS cube_model_locks (
    model_id uuid PRIMARY KEY REFERENCES cube_models (id) ON DELETE CASCADE,
    user_name text NOT NULL,
    token text NOT NULL,
    heartbeat_at timestamptz NOT NULL DEFAULT now()
);

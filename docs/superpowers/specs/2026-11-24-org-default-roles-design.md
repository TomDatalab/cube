# Each organization owns its owner, editor and viewer roles — design

Approved by the user on 2026-11-24 (choice "Uma cópia por organização").

## Intent

There are two kinds of rule roles today:
- **Built-in roles** (`owner`, `editor`, `viewer`). They are names hard-wired in code and in the SpiceDB schema:
  - a rule with one is `workspace#owner_rule|editor_rule|viewer_rule@rule:<id>`;
  - their permissions are fixed by the schema.
- **Organization roles.** These are rows in `roles`, with a UUID and editable permissions and restrictions. A rule with one is `workspace#rule@rule` plus `rule#role@role`.

The user wants every role to have a UUID id and to be editable. Each organization gets its own owner, editor and viewer roles.

## Goals

- Every organization has three **default roles**. Each is a real `roles` row with a UUID, carries today's permissions and has no restrictions:
  - `owner`: view, query, read_objects, write_objects, execute, manage;
  - `editor`: view, query, read_objects, write_objects;
  - `viewer`: view, query, read_objects.
- Where they are created:
  - Existing organizations get them from a boot migration.
  - A new organization gets them when it is created, in the same transaction.
- **They are ordinary roles.** Name, permissions and restrictions are editable in the Roles tab. They can be deleted when no rule uses them.
- **Rules move to them.** Every rule that names a built-in role is moved to its organization's default role with the same key.
- **The API stays compatible.**
  - Creating or updating a rule with `"role": "owner|editor|viewer"` still works. The server resolves it to the organization's default role by its fixed key (`builtin`), not by its name, so it survives a rename. If that default role was deleted, the answer is 400.
  - A rule's answer keeps its fields:
    - `role` is the default role's key, or null for any other role;
    - `roleId` is always the role's UUID;
    - the role name and permissions are the role's own.
- **Playground.**
  - The Roles tab lists the default roles with the organization's roles, tagged "default". They have the same edit and delete actions as any other role.
  - The separate "Built-in roles" table is gone.
  - The Rules tab picks among the organization's roles only.

## Non-goals and anti-goals

- **Nobody gains or loses access.** A migrated rule grants exactly what its built-in role granted. The owner role's `manage` gives what `owner_rule` gave.
- **Workspace membership roles are untouched.** A user who is directly owner, editor or viewer of a workspace, with no rule, keeps that fixed role. `ROLES` stays for that use.
- **The SpiceDB schema does not change.** The `*_rule` relations stay in it, unused.

## Decisions

- **Database.**
  - `roles.builtin` is a nullable text column restricted to `owner|editor|viewer`. It is unique per organization when not null.
  - Seeding is idempotent: an organization that already has a role with that `builtin` is skipped.
  - When the organization already has a role named `owner`, `editor` or `viewer` (a custom one), the default role is named `<key> (default)` instead, so the existing role keeps its name and meaning.
- **Migration.**
  - Rules move from `role` to `role_id`.
  - In SpiceDB, the new relationships (`workspace#rule`, `rule#role`) are written before the old one (`workspace#<key>_rule`) is deleted.
  - It is idempotent, records a marker in `rbac_migrations` when complete, and logs every relationship it touches. It runs at boot, after the store migration.
- **Rollback.** Back up `cube_app` and the SpiceDB relationships first, as for the user UUIDs.

## Left to the builder

- Whether the database half (seeding, `role_id`) and the SpiceDB half are one boot step or two. Each must be idempotent and safe to rerun after a crash between them.
- Whether the `rules.role` column is kept (null after the migration) or dropped.

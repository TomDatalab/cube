# Users keyed by a UUID — design

Approved by the user on 2026-11-24 (choice "UUID como chave").

## Intent

Roles and Rules already have a real id, a UUID primary key. A user's id is their username, and that username is the key everywhere:
- the `users` table and `user_attributes`;
- SpiceDB `user:<username>`;
- the token `sub`;
- the API paths;
- `CUBE_RBAC_ADMINS`.

The user wants a real id for users: a UUID that is the key everywhere. The username becomes a renamable attribute.

## Goals

- A UUID `id` for every user, generated at creation and given to existing users by a migration.
- The UUID is the key in:
  - the database (`users.id`, and `user_attributes.user_id`, which references it);
  - SpiceDB (`user:<uuid>` in every relationship);
  - new tokens (`sub`);
  - the API's `id` field.
- `username` is a unique column, used to log in and shown everywhere in the UI.
- Usernames can be renamed (Users → edit).
- The Users tab shows the UUID in its Id column.

## Non-goals and anti-goals

- **Never break existing access.** After the migration every user keeps exactly the relationships they had.
- **Never strand existing tokens.** A token whose `sub` is a username stays valid until it expires. This includes the `sofiadb-sync` integration token.
- **Never break existing scripts.** API paths and bodies that name a user by username keep working.
- **No change to `role`'s `user:*` wildcard.**

## Decisions

- **API.**
  - Every user object has `id` (the UUID) and `username`.
  - Every path segment naming a user (`/users/:id/...`, `.../owners/:user`, `.../members/:user`, `/rules/:id/members/:user`) accepts the UUID or the username.
  - Creating a user takes `username`; the old body field `id` is accepted as the username.
  - Member and owner lists name each user by `id` and `username`.
- **Tokens.**
  - New tokens carry `sub` = UUID.
  - When resolving `sub`, the server looks it up as a UUID first, then as a username (legacy tokens).
  - `iat` is still checked against `password_changed_at`.
- **SpiceDB migration.** It runs once, at boot, after the database migration:
  - For every relation with a user subject (`platform#admin`, `organization#owner|member`, `workspace#owner|editor|viewer`, `rule#member`), write `user:<uuid>`, then delete `user:<username>`.
  - It is idempotent: a subject that is already a UUID is skipped.
  - When it finishes, a marker is recorded in the database.
  - Every relationship it touches is logged before it is changed.
- **Unchanged.**
  - `CUBE_RBAC_ADMINS` still lists usernames; the server resolves them to UUIDs.
  - Audit columns (`created_by` and `updated_by`) keep storing the username at the time, as text. They are a historical label, not a key.

## Left to the builder

- How the database migration is guarded and ordered. It must be idempotent: migrations run on every start.
- How the request context carries the username, which the audit columns need.

# Per-Organization Default Roles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The built-in rule roles (`owner`, `editor`, `viewer`) become real, editable roles with a UUID, one set per organization, and every rule that used a built-in role moves to its organization's copy without any change in access.

**Architecture:**
- Database: `roles.builtin` marks an organization's default roles.
- One idempotent boot migration does three things:
  - seeds the default roles of every organization (rows plus their `role#can_*@user:*` grants);
  - moves built-in rules to them, in the database and in SpiceDB;
  - records a marker when done.
- Creating an organization seeds its default roles.
- The rules API keeps accepting `"role": "<key>"` and resolves it to the default role.
- The Playground shows default roles in the organization's list.

**Tech Stack:** Rust (axum, tokio-postgres, SpiceDB HTTP), React/TS Playground (antd 4, vitest).

**Spec:** `docs/superpowers/specs/2026-11-24-org-default-roles-design.md`

## Global Constraints

- **Default roles and their permissions** (copied from today's `builtin_permissions`), with no restrictions:
  - `owner`: view, query, read_objects, write_objects, execute, manage;
  - `editor`: view, query, read_objects, write_objects;
  - `viewer`: view, query, read_objects.
- **The `builtin` column:**
  - `roles.builtin` is `text NULL CHECK (builtin IN ('owner','editor','viewer'))`, unique per `(organization_id, builtin)` when not null.
  - Its value never changes after seeding; renames change `name` only.
- **Name conflicts.** When an organization already has a role named `<key>`, the default role is named `<key> (default)`.
- **Nobody gains or loses access.**
  - A migrated rule's members hold exactly the permissions they had.
  - In SpiceDB, write `workspace#rule@rule:<id>` and `rule#role@role:<default>` first, then delete `workspace#<key>_rule@rule:<id>`.
- **Unchanged:**
  - the SpiceDB schema, including the unused `*_rule` relations;
  - workspace membership roles (`ROLES` for `workspace#owner|editor|viewer@user`).
- **Migration:** idempotent, safe to rerun after a crash at any point; marker `default_roles` in `rbac_migrations`. It logs every relationship it writes or deletes (`tracing::info!`).
- **Rule API compatibility:**
  - `role: "<key>"` in create/update resolves to the organization's default role with that `builtin`. When none exists (it was deleted): 400 "This organization has no <key> role".
  - Answers keep their shape:
    - `role` is the role's `builtin` key, or null;
    - `roleId` is always the UUID;
    - name and permissions come from the role row.
- **Running the tests:**
  - Integration tests: `set -a; . /root/cube-rust/.env; set +a; URL="${CUBE_APP_DB_URL%/cube_app*}/postgres"; env -i PATH="$PATH" HOME="$HOME" CUBE_TEST_PG_URL="$URL" cargo test -p cubeserver --test <suite>`.
  - Never print secrets.
  - No workspace-wide `cargo fmt`.

## Review Focus

- An organization created before the migration: its built-in rules now point at its default roles, and each member's `can` checks answer exactly as before (owner can manage the workspace, viewer cannot write).
- The migration runs twice, or after a crash between the SpiceDB write and the database update: no duplicate roles, no lost relationship, no error.
- An organization that already had a custom role named `viewer`: the custom role keeps its name and rules; the default is `viewer (default)`.
- Renaming the default `viewer` role, then creating a rule with `"role": "viewer"`, still gives that role. Deleting it while rules use it is refused; once unused it can be deleted, and then `"role": "viewer"` gives 400.
- A new organization immediately has the three roles, with their SpiceDB grants.

---

### Task 1: Store: default roles and moving rules to them

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/migrations.sql`, `rust/cube/cubeserver/src/rbac/store.rs`
- Test: `rust/cube/cubeserver/tests/rbac_store.rs`

**Interfaces (produces):**
- `Role` gains `pub builtin: Option<String>`. `ROLE_COLS` reads it.
- `pub const DEFAULT_ROLES: [(&str, &[&str]); 3]`: the key and permissions of each default role, from the Global Constraints. It lives in `store.rs`, and `handlers::roles::builtin_permissions` reads from it.
- Seeding the default roles of an organization: `seed_default_roles(organization_id: Uuid, created_by: &str) -> Result<Vec<Role>, StoreError>`.
  - It exists on both the transaction and `Store`, like `create_role`.
  - It inserts the missing ones (with the `<key> (default)` name rule) and returns all three default roles, the new and the existing ones.
- `Store::default_role(organization_id: Uuid, key: &str) -> Result<Option<Role>, StoreError>`.
- `Store::organization_ids() -> Result<Vec<Uuid>, StoreError>` (if no equivalent exists).
- `Store::rules_with_builtin_role() -> Result<Vec<(Rule, Uuid /*organization*/)>, StoreError>`.
- `Store::set_rule_role_id(rule: Uuid, role_id: Uuid)`: sets `role_id`, clears `role`.

**Steps:**
- [ ] **Step 1: Failing tests:**
  - `seeding_creates_the_three_default_roles_once`: seed twice, then check:
    - three rows;
    - same ids on the second call;
    - permissions as in the Global Constraints.
  - `seeding_beside_a_custom_role_named_viewer_names_the_default_viewer_default`.
  - `default_role_is_found_by_key_after_a_rename`.
  - `a_rule_moves_from_its_builtin_role_to_a_role_id`.
- [ ] **Step 2:** Run them and confirm they fail.
- [ ] **Step 3: Implement.** Migration SQL:
  - `ALTER TABLE roles ADD COLUMN IF NOT EXISTS builtin text`;
  - a guarded CHECK;
  - `CREATE UNIQUE INDEX IF NOT EXISTS roles_builtin_idx ON roles (organization_id, builtin) WHERE builtin IS NOT NULL`.
- [ ] **Step 4:** Run `--test rbac_store` and `--lib`; both must pass.
- [ ] **Step 5: Commit** `feat(rbac): organizations can hold default owner, editor and viewer roles`.

### Task 2: Boot migration and seeding new organizations

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/default_roles.rs`
- Modify:
  - `rust/cube/cubeserver/src/rbac/mod.rs`: call the migration after `user_ids::migrate_user_subjects`;
  - `rust/cube/cubeserver/src/rbac/handlers/organizations.rs`: seed in the creation transaction and `touch` the grants with the other relationships;
  - `rust/cube/cubeserver/src/rbac/handlers/roles.rs`: make `wildcard_grants` `pub(crate)` for reuse.
- Test:
  - unit tests in `default_roles.rs` against `rbac::fake` authz (follow `user_ids.rs`);
  - `tests/rbac_api.rs` for organization creation.

**Interfaces (produces):** `pub async fn migrate_default_roles(store: &Store, authz: &dyn Authz) -> Result<usize, String>`. It returns how many rules it moved.

**Algorithm:**
1. Return 0 if the marker `default_roles` is present.
2. For every organization:
   - call `seed_default_roles`;
   - `touch` the `wildcard_grants` of each returned role (idempotent).
3. For every rule from `rules_with_builtin_role`:
   - `touch` `workspace#rule@rule` and `rule#role@role:<default>`;
   - `delete` `workspace#<key>_rule@rule`;
   - then `set_rule_role_id`.
4. Record the marker.

**Steps:**
- [ ] **Step 1: Failing tests:**
  - `a_builtin_rule_moves_to_the_default_role_and_keeps_its_permissions`: one case per key. Check workspace `can` before and after with the fake authz: owner can manage; editor can write but not manage; viewer can query but not write.
  - `a_second_run_changes_nothing`.
  - `a_rerun_after_spicedb_moved_but_the_row_did_not_finishes_the_move`.
  - `a_new_organization_has_its_default_roles_and_grants` (API test).
- [ ] **Step 2:** Run them and confirm they fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** Run `--lib`, `--test rbac_api` and `--test org_tree`; all must pass.
- [ ] **Step 5: Commit** `feat(rbac): built-in rules move to their organization's default roles`.

### Task 3: Rules and roles API

**Files:**
- Modify:
  - `rust/cube/cubeserver/src/rbac/handlers/rules.rs`
  - `rust/cube/cubeserver/src/rbac/handlers/roles.rs`
  - `rust/cube/cubeserver/src/rbac/access.rs`
  - `rust/cube/cubeserver/src/rbac/store.rs` (`RuleRole`, `Rule::role_ref`)
- Test: `tests/rbac_api.rs`, `tests/org_tree.rs`. Update assertions that expect `*_rule` relationships or a built-in-only `role`.

**Interfaces (produces):**
- `requested_role` resolves `role: "<key>"` to `RuleRole::Custom(default.id)`, with 400 per the Global Constraints. New rules are always placed as custom.
- Roles answers carry `builtin` (the key or null).
- Rule answers carry `role` = the role's `builtin`, `roleId`, and the role's name and permissions.
- `RuleRole::Builtin` stays only for reading rows not yet migrated, if needed; otherwise remove it, along with `builtin_relation`.

**Steps:**
- [ ] **Step 1: Failing tests:**
  - `a_rule_created_with_a_builtin_key_uses_the_default_role`;
  - `the_key_still_resolves_after_the_default_role_is_renamed`;
  - `a_default_role_in_use_cannot_be_deleted_and_once_deleted_its_key_is_400`;
  - `editing_the_default_viewer_changes_what_its_rules_grant`;
  - `roles_list_marks_default_roles`.
- [ ] **Step 2:** Run them and confirm they fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** Run the suites `rbac_api`, `org_tree`, `api`, `playground`, `model_api` and `rbac_store`, plus `--lib`; all must pass.
- [ ] **Step 5: Commit** `feat(rbac): rules name default roles; roles show which are default`.

### Task 4: Playground

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Admin/{api.ts,RolesTab.tsx,RulesTab.tsx}`, plus any place that reads `ROLE_HELP` or the built-in options for rules
- Test: `src/__tests__/` (existing admin tests updated, plus new ones)

**Interfaces:** `OrgRole` gains `builtin: Role | null`.

**Steps:**
- [ ] **Step 1: Failing tests:**
  - the Roles tab tags default roles "default" and shows their UUID;
  - there is no "Built-in roles" table;
  - the Rules tab's role choices are the organization's roles only, sent as `roleId`.
- [ ] **Step 2:** Run them and confirm they fail.
- [ ] **Step 3: Implement.** Default roles get the same edit and delete actions; delete stays disabled while in use.
- [ ] **Step 4:** Run vitest (all), `node_modules/.bin/tsc --noEmit -p .` and `../../node_modules/.bin/oxlint` on the changed files; all must be clean.
- [ ] **Step 5: Commit** `feat(playground): default roles are listed, edited and picked like any role`.

### Task 5: Deploy and verify (controller)

- [ ] Back up `cube_app` (`pg_dump`) and the SpiceDB `workspace#*_rule`, `workspace#rule` and `rule#role` relationships, in `/root/cube-rust/backup-20261124-defaultroles/`. Also back up the binary, the Playground and the README.
- [ ] Before deploying, record each user's workspace permissions: the access graph of antonio and of the e2e users.
- [ ] Build, install and restart.
- [ ] Check the log count. Then compare the same permissions after the deploy.
- [ ] Check the Roles tab: the default roles show UUIDs and can be edited.
- [ ] Update `/root/cube-rust/README.md` (roles section, rollback).

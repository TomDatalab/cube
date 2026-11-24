# Complete Access Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Admin → Users → Access shows everything a user's access involves: every connection of their organizations (with or without access), the Platform, their rules and roles as nodes, and the cubes and views behind each connection.

**Architecture:**
- `rbac::access::build` stays pure and gains inputs: the Platform's data sources, the model's cubes and views, and organization slugs.
- `users::access` loads those inputs from the model index and the data sources.
- The Playground's access layout gets three new columns: rule, role and cube. A connection with many cubes collapses as an organization with many workspaces already does.

**Tech Stack:** Rust (axum, cubeserver), React/TS Playground (vitest).

**Spec:** the user's request (2026-11-24): "em users access a árvore não está completa com todos os itens que o usuário possui acesso". They chose all four additions: organization connections, cubes and views, Platform, rules and roles.

## Global Constraints

- The graph tells the truth about access as enforced today:
  - A non-admin queries only connections released to a workspace where they hold `query`.
  - A view needs every data source it spans (`enforce::check_queries`).
  - The Platform's data sources (configured, but in no organization) are queried by platform admins only.
- Anything shown without access carries a note saying why. The UI draws it dimmed.
- A platform admin's graph stays a single node.
- Node ids:
  - `rule:<uuid>`
  - `role:<uuid>` for an organization role
  - `role:builtin:<owner|editor|viewer>` for a built-in role
  - `cube:<org-slug or _platform>:<name>` (names are unique per organization only)
  - `organization:_platform` for the Platform
- Columns, left to right: user, organization, rule, role, workspace, connection, cube.
- Notes, in English like the rest of the graph:
  - `no access: not released to a workspace of the user`
  - `platform admins only`
  - `view`
  - `no access: spans a data source the user cannot query`
  - Existing notes stay as they are.

## Review Focus

- Two organizations each have a cube with the same name: two cube nodes, never one.
- A view in an organization folder whose included cubes span two connections, one released and one not: the view is shown with no `query` grant and the "spans" note.
- A rule of a custom role used in two workspaces: one role node, two rule nodes, and no edge implying the role reaches the other rule's workspace.
- A user with no organizations: only the user node, plus the Platform node.
- The Data Source with 24 cubes: its cubes collapse behind a toggle (COLLAPSE_AT), and a highlighted reason path through a hidden cube expands it.

---

### Task 1: Pure graph: connections, Platform, rules, roles, cubes

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/access.rs`

**Interfaces:**
- Produces, as new fields of `AccessInput`:
  - `platform_sources: Vec<String>`: configured data sources that are in no organization, sorted.
  - `cubes: Vec<AccessCube>`.
  - `organization_slugs: HashMap<Uuid, String>`.
- `AccessCube` is defined as:
  ```rust
  pub struct AccessCube {
      pub name: String,
      pub view: bool,
      /// `_platform` or an organization slug; unowned files are left out by the caller.
      pub folder: String,
      /// Every data source the cube reads: its own; for a view, those of the cubes it includes (in its folder's scope).
      pub data_sources: BTreeSet<String>,
  }
  ```
- Node kinds added: `"rule"`, `"role"`, `"cube"`. `Node` keeps its fields.

**Steps:**

- [ ] **Step 1: Write the failing unit tests in `access.rs` `mod tests`.** Use the existing helpers.

  `every_connection_of_a_users_organization_is_shown`:
  - Setup: member of O (no workspace release); connection `c` of O is Assigned and configured.
  - Expect node `connection:c` with note `no access: not released to a workspace of the user`, no grant on it, and an edge `organization:<O> → connection:c` labelled `of` with kind `derived`.

  `the_platform_is_shown_for_admins_only`:
  - Setup: `platform_sources = ["default"]`, plus a Platform cube `u` on `default`.
  - Expect node `organization:_platform` named `Platform`, an edge `user → organization:_platform` labelled `platform`, node `connection:default` with note `platform admins only`, and node `cube:_platform:u`. None of them has a grant.

  `a_rule_and_its_role_are_nodes`:
  - Setup: rule R (custom role X with a blocked member) in workspace W.
  - Expect nodes `rule:<R>` and `role:<X>`, and these edges:
    - `user → rule:<R>`: `member`, direct.
    - `rule:<R> → role:<X>`: `role`, derived.
    - `rule:<R> → workspace:<W>`: the role's name, derived.
  - The role node carries the restriction note. The workspace grants are as before, with the reason path running through `rule:<R>`.

  `a_builtin_rule_role_is_one_shared_node`:
  - Setup: two rules with role viewer in two workspaces.
  - Expect one `role:builtin:viewer` node and two rule nodes.

  `cubes_hang_off_their_connections`:
  - Setup: connection `c` released to W where the user is viewer; cubes `a` (O, on `c`) and `b` (other org P, on `c2`).
  - Expect `cube:<slug>:a` with a `query` grant whose path is `user → workspace → connection:c → cube`. Expect no node for `b`, because the user is not in P.

  `same_name_in_two_organizations_is_two_cubes`:
  - Setup: user in O and P, each with a cube `x`.
  - Expect nodes `cube:o:x` and `cube:p:x`.

  `a_view_spanning_an_unreleased_connection_has_no_query`:
  - Setup: view `v` in O over `c` (released) and `d` (not released).
  - Expect the node to have notes `view` and `no access: spans a data source the user cannot query`, and no grant. There is an edge from each connection.

  Update `a_platform_admin_is_a_single_node` so it still expects one node.

- [ ] **Step 2: Run them.** Run `cargo test -p cubeserver --lib rbac::access`. Expected: the new tests fail to compile or fail.

- [ ] **Step 3: Implement in `build`.**
  - Rule nodes: one per rule in `input.rules` whose workspace is known.
    - The edge `user → rule` replaces today's `user → workspace` "via rule" edge.
    - Keep `ws_sources` reasons, but with path `[user, rule, workspace]`.
  - Role nodes: one per distinct role. Notes: restriction note (custom) or none. Perm lines come from grants. Add no grants for roles; the UI shows the role's permissions from a new optional `Node.details: Vec<String>` (serialised as `details`, empty by default) filled with `role_perms`.
  - Organization connections: every row of `input.connections` whose organization is in the graph's organizations. Without a `query` or `manage` source, add the no-access note and the `of` edge from its organization.
  - Platform node: added whenever `platform_sources` or Platform cubes exist. It is placed first in the organization column.
  - Cubes: for each cube whose `folder` is `_platform` or the slug of an organization in the graph:
    - Add one edge from each of its data sources' connection nodes, labelled `serves` with kind `derived`.
    - Add the `query` grant only when every data source has a `query` source on its connection. The reasons are that connection's query reasons, extended with the cube node.
    - Cubes of a data source with no connection node in the graph are left out.
  - Nodes are emitted in column order. Cubes are sorted by (folder order, name), with Platform first.

- [ ] **Step 4: Run tests.** Run `cargo test -p cubeserver --lib rbac::access`. Expected: all pass.

- [ ] **Step 5: Commit.** Message: `feat(rbac): the access graph shows connections, Platform, rules, roles and cubes`.

### Task 2: Load the new inputs in `users::access`

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/handlers/users.rs`
- Test: `rust/cube/cubeserver/tests/rbac_api.rs` (the "access graph" section)

**Interfaces:**
- Consumes `AccessCube`, `AccessInput` fields (Task 1), `crate::handlers::tree::index(&state)` (the model index), `data_source_types(&state)` (already used), `rbac.store.all_connections()`.
- `platform_sources` is `configured` minus every connection row's name.
- Build `cubes` from the index: skip `Owner::Unowned`.
  - A view's `data_sources` are the data sources of the cubes in `includes`, looked up among cubes in the same folder or `_platform`.
  - A view whose includes cannot all be found gets the view's own `data_source`.

**Steps:**

- [ ] **Step 1: Write the failing integration test** `access_graph_lists_connections_cubes_and_rules`, reusing `scenario`.
  - Setup: a Digital connection not released, a Platform cube, and a rule with a custom role.
  - Assert the new node ids exist:
    - `connection:<name>` with the no-access note.
    - `organization:_platform`.
    - `rule:<id>`.
    - `role:<id>`.
    - `cube:digital:<name>`.
- [ ] **Step 2: Run it.** Run `cargo test -p cubeserver --test rbac_api access_graph`. Expected: it fails.
- [ ] **Step 3: Implement the loading.** Also load connections for every organization in the graph, which the handler already does.
- [ ] **Step 4: Run tests.** Run `cargo test -p cubeserver --test rbac_api access_graph` and `--lib rbac::access`. Expected: pass.
- [ ] **Step 5: Commit.** Message: `feat(rbac): the access route loads cubes, Platform sources and roles`.

### Task 3: Playground: draw the new kinds

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Admin/access/types.ts`, `layout.ts`, `AccessModal.tsx`
- Test: `packages/cubejs-playground/src/__tests__/access-layout.test.ts`

**Interfaces:**
- `AccessNodeKind` adds `'rule' | 'role' | 'cube'`.
- `AccessNode` adds `details?: string[]`.
- `COLUMNS = ['user', 'organization', 'rule', 'role', 'workspace', 'connection', 'cube']`.

**Steps:**

- [ ] **Step 1: Write the failing tests.**
  - `columns follow user → organization → rule → role → workspace → connection → cube`: the x of each kind increases in that order, and an empty kind takes no column.
  - `a connection with more than COLLAPSE_AT cubes collapses them`:
    - Its node is `foldable` with `hiddenCount`.
    - Edges into hidden cubes are dropped.
    - `expandFor` with a path through a hidden cube returns the connection's id.
  - `a role node lists its details as permission lines`.
  - `a node without access is marked dim`: a laid-out node has `dim: true` when it has no grant and is not the user, the organization or the Platform.
- [ ] **Step 2: Run them.** Run `node_modules/.bin/vitest run src/__tests__/access-layout.test.ts` in `packages/cubejs-playground`. Expected: fail.
- [ ] **Step 3: Implement.**
  - Generalise collapse to a parent of a kind and its children:
    - organization → workspace, keyed by `organizationId`.
    - connection → cube, keyed by edges `connection → cube`.
  - In `AccessModal.tsx`:
    - Add `KIND_COLOR` entries for rule, role and cube.
    - Draw a `dim` node at reduced opacity.
    - The kind label is uppercase as today.
    - A toggle pill works for connections the same way.
- [ ] **Step 4: Run, typecheck and lint.** Run vitest (all), `../../node_modules/.bin/tsc --noEmit -p .` and `../../node_modules/.bin/oxlint <changed files>`. Expected: pass.
- [ ] **Step 5: Commit.** Message: `feat(playground): the access graph draws rules, roles, cubes and access-less items`.

### Task 4: Deploy and verify (controller)

- [ ] Build, back up, install and restart as in the deploy memory.
- [ ] Check antonio's graph:
  - The Digital connections `example` and `sofiadb` are dimmed with the no-access note.
  - The Platform with `default` and its 4 cubes and the view `sales` shows "platform admins only".
  - The cubes of Digital hang under `sofiadb` and `example`, collapsed behind a toggle.
- [ ] Update `/root/cube-rust/README.md` (the access graph paragraph; the deploy README is not in the repo).

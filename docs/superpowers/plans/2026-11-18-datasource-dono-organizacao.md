# Datasource sempre com dono — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Com RBAC ligado, todo datasource novo nasce com uma organização dona; os sem dono que já existem aparecem como "Platform"; um datasource com dono nunca volta a ficar sem dono ("Unassign" vira "Move").

**Architecture:**
- "Platform" = datasource sem linha em `connections` (nada muda no schema nem no SpiceDB).
- A criação pela plataforma (`POST /v1/data-sources` e `POST /playground/data-sources`) passa a exigir `organizationId` e reaproveita o fluxo atômico de `connections::create` (linha + `connection#organization` antes, desfeitos se o datasource não puder ser criado), com `origin: assigned`.
- `PUT /v1/connections/{name}/organization` também move entre organizações; `DELETE` da mesma rota deixa de existir.

**Tech Stack:** Rust (axum, tokio-postgres), SpiceDB (sem mudança de schema), React + antd 4.16.13, vitest.

**Spec:** `docs/superpowers/specs/2026-11-18-datasource-dono-organizacao-design.md`

## Global Constraints

- Branch: `feat/datasource-owner-organization` (a partir de `feat/role-data-restrictions`). Commits com `git -c user.name="Tom DataLab" -c user.email="sofiadb@sofiadb.org" commit ...` e a linha `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Com RBAC **desligado** nada muda: `organizationId` é ignorado e não há coluna Owner.
- Criar pela plataforma sem `organizationId` (RBAC ligado): **400** `"Choose the organization that owns the data source"`.
- `organizationId` que não é UUID: 400 (como `resource_id`); organização inexistente: **404** `"organization not found"`.
- Não existe a opção "Platform" na criação nem como destino de "Move".
- `default` nunca é atribuído nem movido (400, mensagem atual).
- Mover para outra organização: retira todas as liberações (`connection#workspace`) e troca `connection#organization` numa única escrita atômica do SpiceDB (`authz.write`), e atualiza a linha (`organization_id`, `origin = 'assigned'`) na mesma transação do store.
- Mover para a própria organização atual: no-op 204 (comportamento atual de reatribuir).
- `DELETE /v1/connections/{name}/organization`: a rota sai; o axum responde **405**.
- `POST /v1/organizations/{id}/connections` com `organizationId` no corpo: **400** `"organizationId comes from the path"`.
- Comandos de teste:
  - Rust: `cd rust/cube && CUBE_TEST_PG_URL=postgres://postgres:test@127.0.0.1:55432/postgres CARGO_TARGET_DIR=/root/cube/rust/cube/target cargo test -p cubeserver --lib --test rbac_api --test rbac_store`;
  - Playground, em `packages/cubejs-playground`: `node_modules/.bin/vitest run` e `node_modules/.bin/tsc --noEmit -p .`.
- Frontend: antd 4.16.13 (`Modal visible`, sem `Form.useWatch`), tokens `--pg-*`.

## Review Focus

1. Falha no meio da criação (SpiceDB fora, teste de conexão falha, `cube.yml` inválido) não deixa nem datasource solto nem linha/relação órfã → coberto em Task 2.
2. Mover com liberações em vários workspaces: nenhuma liberação antiga sobrevive e o owner antigo perde `manage` → Task 3.
3. Falha do SpiceDB ao mover: a linha continua na organização antiga (rollback) → Task 3.
4. Nome já registrado em `connections` (linha antiga) ao criar pela plataforma continua recusado (`refuse_tracked_name`) → Task 2.
5. Datasource "Platform" na lista não pode ser confundido com um de organização apagada: `organization` é `null` só quando não há linha → Task 4.

---

### Task 1: `move_connection` no store

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/store.rs` (query em `q`, método no macro de `Store`/transação, junto de `create_connection`/`delete_connection`)
- Test: `rust/cube/cubeserver/tests/rbac_store.rs`

**Interfaces:**
- Produces: `pub async fn move_connection(&self, name: &str, organization_id: Uuid) -> Result<bool, StoreError>` em `Store` e na transação; `UPDATE connections SET organization_id = $2, origin = 'assigned' WHERE name = $1`; `true` quando uma linha mudou.

- [ ] **Step 1:** teste `db_test!(moving_a_connection_changes_its_organization_and_origin, ...)`: cria orgs A e B, `create_connection("x", A, Created)`, `move_connection("x", B)` → `true`; `connection("x")` tem `organization_id == B` e `origin == Assigned`; `move_connection("nope", B)` → `false`; `delete_organization(A)` agora passa (sem conexões).
- [ ] **Step 2:** rodar; falha por método inexistente.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** rodar `--test rbac_store`; passa.
- [ ] **Step 5:** commit `feat(cubeserver): move a connection to another organization in the RBAC store`.

### Task 2: criar datasource exige a organização

**Files:**
- Modify: `rust/cube/cubeserver/src/handlers/data_sources.rs` (`CreateRequest`, `create_unless_tracked`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/connections.rs` (`create` e helper novo)
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Produces:
  - `CreateRequest` ganha `#[serde(rename = "organizationId", default)] pub(crate) organization_id: Option<String>`.
  - Em `connections.rs`: `pub(crate) async fn create_owned(state: &AppState, rbac: &Rbac, org: Uuid, origin: ConnectionOrigin, request: CreateRequest) -> Result<Response, ApiError>`, extraído do corpo atual de `connections::create` (valida nome, recusa nome de datasource existente, tx + `touch` + `data_sources::create` + commit, com desfazer). `connections::create` passa a chamá-lo com `Created`; antes, recusa `organization_id.is_some()` (400 das Global Constraints).
  - `create_unless_tracked` com RBAC ligado: `refuse_tracked_name`; exige `organization_id` (400); resolve a organização (`resource_id` + `store.organization`, 404); chama `create_owned(..., ConnectionOrigin::Assigned, ...)`. Resposta 201 como `connections::create`. Com RBAC desligado, segue como hoje.

- [ ] **Step 1:** testes em `rbac_api.rs` (usar `connections_scenario`, SQLite em `dir` como `add_connection`):
  - `platform_create_needs_an_organization`: `POST /cube/v1/data-sources` como admin sem `organizationId` → 400 e `cube.yml` não ganha o nome; com UUID desconhecido → 404; com `"x"` → 400.
  - `platform_create_assigns_the_organization`: com `organizationId = org_a` → 201; `store.connection(name)` é `org_a` e `Assigned`; `GET /cube/v1/organizations/{org_a}/connections` como ana lista o nome com `"origin": "assigned"`; `PUT /cube/v1/connections/{name}` como ana → 403.
  - `playground_create_assigns_the_organization`: o mesmo por `POST /cube/playground/data-sources` (header de chave como em `playground_delete_drops_the_connections_grants`).
  - `a_failed_platform_create_leaves_no_row`: banco SQLite em caminho impossível (teste de conexão falha) → erro, sem linha em `connections` e sem relação `connection#organization`.
  - `org_create_refuses_an_organization_in_the_body`: `add_connection` com `organizationId` no corpo → 400.
  - Atualizar testes existentes que criam pela plataforma sem organização para enviar `organizationId`.
- [ ] **Step 2:** rodar; os novos falham.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** rodar a suíte Rust inteira; passa.
- [ ] **Step 5:** commit `feat(cubeserver): a data source created by the platform needs its organization`.

### Task 3: "Move" no lugar de "Unassign"

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/handlers/connections.rs` (`assign_organization`; remover `unassign_organization`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/mod.rs:103-104` (só `put`)
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Consumes: `move_connection` (Task 1).
- Produces: `PUT /v1/connections/{name}/organization {organizationId}` — linha de outra organização deixa de ser 409 e passa a mover: tx `move_connection`; `authz.write(updates, &[])` com `Update::Delete` de cada liberação lida por `releases_of`, `Update::Delete` da `connection#organization` antiga e `Update::Touch` da nova; commit; se o commit falhar, escrever o inverso no SpiceDB e logar erro (mesmo molde do desfazer atual). Doc-comment atualizado.

- [ ] **Step 1:** testes:
  - Reescrever `assigning_an_existing_data_source_to_an_org`: o `assign` para B, que esperava 409 seguido de `unassign`, agora espera 204 direto; liberação em W1 sumiu; ana sem `manage`, bia com; linha em B e `Assigned`.
  - `moving_a_created_connection_makes_it_assigned`: `add_connection` em A (origin `created`), liberar em dois workspaces, mover para B → 204; `released` vazio nos dois; origin `assigned`; owner de B recebe 403 ao editar (`PUT /v1/connections/{name}`).
  - `a_failed_spicedb_write_keeps_the_connection_where_it_was`: no molde de `a_failed_spicedb_write_adds_no_connection` → erro 503 e linha ainda em A.
  - Reescrever `unassigning_frees_the_connection_and_the_org` como `unassign_route_is_gone`: `DELETE .../organization` → 405 e a linha continua; apagar a organização ainda é recusado enquanto tiver conexões.
  - Mover `default` → 400.
- [ ] **Step 2:** rodar; falham.
- [ ] **Step 3:** implementar e remover `unassign_organization` e o helper de teste `unassign`.
- [ ] **Step 4:** suíte Rust inteira; passa.
- [ ] **Step 5:** commit `feat(cubeserver): move connections between organizations; no unassign`.

### Task 4: a lista de datasources mostra o dono

**Files:**
- Modify: `rust/cube/cubeserver/src/handlers/data_sources.rs` (`list`)
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Produces: com RBAC ligado, cada item de `GET /v1/data-sources` e `/playground/data-sources` ganha `"organization": {"id": "<uuid>", "name": "<nome>"}` ou `null` (Platform). Lê `store.all_connections()` e as organizações uma vez por chamada; falha do store → 503. Com RBAC desligado, o campo não aparece. [decisão do plano: objeto com nome em vez de só `organizationId`, para a tela não precisar de outra chamada.]

- [ ] **Step 1:** teste `data_sources_list_shows_the_owner`: após atribuir `shared` a A, a lista tem `shared.organization == {id: org_a, name: "A"}` e `default.organization == null`.
- [ ] **Step 2:** rodar; falha.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** suíte Rust inteira; passa.
- [ ] **Step 5:** commit `feat(cubeserver): data source listing names its owner`.

### Task 5: Playground

**Files:**
- Modify: `packages/cubejs-playground/src/pages/DataSources/DataSourcesPage.tsx`
- Modify: `packages/cubejs-playground/src/pages/Admin/OrganizationsTab.tsx`
- Modify: `packages/cubejs-playground/src/pages/Admin/api.ts`
- Test: `packages/cubejs-playground/src/__tests__/admin-api.test.ts` (ou arquivo novo `data-source-owner.test.ts`)

**Interfaces:**
- Consumes: campo `organization` (Task 4), `organizationId` na criação (Task 2), move pelo `PUT` (Task 3).
- Produces em `api.ts`:
  - `export function ownerLabel(ds: { organization?: { id: string; name: string } | null }): string` → nome da organização, ou `'Platform'` quando `null`/ausente.
  - `export function moveTargets(orgs: Organization[], current: string): Organization[]` → todas menos `current`, ordenadas por nome.
  - `moveConnection: (name: string, org: string) => call('PUT', `/v1/connections/${id(name)}/organization`, { organizationId: org })`; remover `unassignConnection`.

- [ ] **Step 1:** testes vitest de `ownerLabel` (objeto → nome; `null` → `Platform`; sem campo → `Platform`) e `moveTargets` (exclui a atual, ordena).
- [ ] **Step 2:** rodar; falham.
- [ ] **Step 3:** implementar as funções e as telas:
  - Data Source, com RBAC ligado (`useRbac().enabled`): carregar organizações com `adminApi(basePath).organizations()` (como `AdminPage.tsx:24`); no formulário de **criação** um `Form.Item name="organizationId" label="Organization"` obrigatório (`rules: [{ required: true, message: 'Choose the organization that owns the data source' }]`), `Select` sem valor inicial; não aparece na edição; enviado no corpo do `POST` e **não** em `variables`. Tabela: coluna **Owner** com `ownerLabel`, e "Platform" como `Tag`; filtro antd (`filters` = organizações + Platform, `onFilter` por `ownerLabel`).
  - Admin → Organizations → Connections: trocar o botão "Unassign" (e seu `Popconfirm`) por "Move to…": `Select` com `moveTargets(orgs, org.id)` + `Popconfirm` `Move ${c.name} to ${target}? Its releases to ${org.name}'s workspaces are withdrawn.`; chama `moveConnection` e recarrega. O "Assign" continua igual.
- [ ] **Step 4:** `vitest run` e `tsc --noEmit -p .`; passam.
- [ ] **Step 5:** commit `feat(playground): data source owner on create, Owner column, move between organizations`.

### Task 6: README e deploy

**Files:**
- Modify: `/root/cube-rust/README.md` (fora do repositório), depois de copiá-lo para `README.md.bak-20261118-owner`: a regra de dono, "Platform", `organizationId` obrigatório na criação, `PUT .../organization` move, `DELETE` removido.

- [ ] **Step 1:** suíte Rust, `vitest` e `tsc`; tudo passa.
- [ ] **Step 2:** build e deploy:
  - backup do binário em `/root/cube-rust/cube-server.bak-20261118-owner`;
  - `cargo build --release -p cubeserver --bin cube-server`;
  - `vite build`;
  - `/root/cube-rust/install-playground.sh <repo>/packages/cubejs-playground/build`;
  - `/root/cube-rust/start-all.sh restart`.
- [ ] **Step 3:** verificar: `readyz` 200; `GET /cube/v1/data-sources` como admin mostra `default`, `sofiadb` e `example` com `organization: null`; `DELETE /cube/v1/connections/sofiadb/organization` → 405.
- [ ] **Step 4:** atualizar o README; commit do que faltar no repositório.

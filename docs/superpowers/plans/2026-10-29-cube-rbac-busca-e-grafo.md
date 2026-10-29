# Busca no Admin e grafo de acessos — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a página Admin e o seletor de workspace passam a buscar e paginar no servidor, para aguentar milhares de usuários, organizações e workspaces. O botão Access abre o grafo de acessos de um usuário no modelo C.

**Architecture:**
- **Servidor** (`cubeserver::rbac`):
  - consultas paginadas no Postgres (`cube_app`) e parâmetros opcionais `q`/`limit`/`offset` nas listagens;
  - duas rotas novas, `GET /v1/workspaces` (os que o usuário vê, de todas as organizações) e `GET /v1/connections` (só platform admins);
  - a rota `GET /v1/users/{id}/access`, cujo grafo é montado por uma função pura (`rbac::access`) que segue as regras do esquema e as restrições do servidor.
- **Cliente** (`cubejs-playground`):
  - cada aba ganha busca, com debounce e paginação no servidor;
  - os seletores passam a buscar enquanto você digita;
  - o grafo é desenhado em SVG à mão, como no modelo aprovado, sem biblioteca nova.

**Tech Stack:** Rust (axum, tokio-postgres, deadpool), SpiceDB pelo trait `Authz` (`FakeAuthz` nos testes), React 18 + antd 4.16.13, vitest.

**Spec:** `docs/superpowers/specs/2026-10-29-cube-rbac-busca-e-grafo-design.md`, apoiado em `docs/superpowers/specs/2026-10-28-cube-rbac-spicedb-design.md`.

## Global Constraints

- **Formato das respostas:** sem `q`, `limit` ou `offset`, toda rota que já existe responde **exatamente** como hoje (mesmas chaves, lista inteira). Com eles, a resposta ganha `total` ao lado da lista de sempre.
- **Página e debounce:** 50 itens por página (`PAGE_SIZE = 50`) e `limit` de no máximo 200. A busca dispara 300 ms depois da última tecla, sem botão.
- **Como a busca casa:** por trecho, sem diferenciar maiúsculas e minúsculas.
  - Usuários: `id` (o username), `name` e `email`.
  - Organizações: `name` e o início do `id` (UUID).
  - Workspaces: `name`, o início do `id` e o nome da organização.
  - `%`, `_` e `\` no termo são literais.
- **Ordem:** usuários por `id`; organizações por `name` e depois `id`; workspaces pelo nome da organização, depois o do workspace, depois o `id`.
- **Permissões:**
  - `GET /v1/users*`, `GET /v1/connections` e `GET /v1/users/{id}/access`: só platform admins;
  - `GET /v1/workspaces` e `GET /v1/organizations`: o que o usuário pode ver (`lookup_resources ... view`). Platform admins veem tudo sem passar por `lookup_resources`.
  - 401 sem token, 403 sem permissão, 404 para usuário desconhecido no `access`.
- **Grafo:** mostra o que a pessoa **de fato** consegue fazer, ou seja, o esquema do SpiceDB **mais** as restrições do servidor:
  - conexão `assigned`: o owner da organização não ganha `manage`;
  - conexão sem organização: não aparece;
  - data source que não existe mais: aparece com a nota, sem `query`.
- **Textos de interface:** em inglês, como o resto do Playground. Os textos fixos aparecem nas tasks.
- **Frontend:** antd 4.16 (`visible`, não `open`, em `Modal`). O drag nativo continua usando `USER_DRAG_TYPE` de `MembershipTree.tsx`. Nenhuma dependência nova.
- **Git:**
  - comandos com `git -c safe.directory='*'`;
  - autor `Tom DataLab <sofiadb@sofiadb.org>`;
  - conventional commits terminando em `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`;
  - branch `feat/rbac-spicedb`.
- **Testes Rust:** sempre com `-j 2`, porque linkar em paralelo esgota a memória da máquina (sem swap). Os testes com Postgres precisam de `CUBE_TEST_PG_URL=postgres://postgres:$CUBEJS_DB_PASS@127.0.0.1:55432/postgres`, com a senha tirada de `/root/cube-rust/.env`.

## Review Focus

1. **Termo com `%`, `_` ou `\`:** casa só o caractere literal, nunca vira curinga. Teste na Task 1.
2. **`offset` além do total:** a lista vem vazia, mas `total` continua certo. Teste na Task 1.
3. **Usuário com dois caminhos para a mesma permissão**, por exemplo editor do workspace **e** owner da organização: o painel mostra os dois motivos, e cada um destaca o seu caminho. Teste na Task 7.
4. **Workspace salvo no seletor da barra que foi excluído** ou deixou de ser visível: o seletor cai no primeiro da lista (quem não é admin) ou em "Todos (admin)" (admin), sem erro. Teste na Task 6.
5. **Organização com muitos workspaces no grafo:** entra recolhida e se expande com um clique. Teste na Task 9.

---

## Parte 1: busca

### Task 1: Consultas paginadas no store

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/store.rs`, com os tipos novos perto de `User` (linha ~60) e os métodos no `impl Store`, depois de `users()` (linha ~456).
- Test: `rust/cube/cubeserver/tests/rbac_store.rs`

**Interfaces:**
- Produces:
  - `pub struct Search { pub q: Option<String>, pub limit: Option<i64>, pub offset: i64 }`: com `limit: None`, sem limite;
  - `pub struct Page<T> { pub items: Vec<T>, pub total: i64 }`;
  - `pub struct WorkspaceListing { pub workspace: Workspace, pub organization_name: String }`;
  - `Store::search_users(&self, s: &Search) -> Result<Page<User>, StoreError>`;
  - `Store::search_organizations(&self, visible: Option<&[Uuid]>, s: &Search) -> Result<Page<Organization>, StoreError>`: `None` traz todas (platform admin);
  - `Store::search_workspaces(&self, visible: Option<&[Uuid]>, organization: Option<Uuid>, s: &Search) -> Result<Page<WorkspaceListing>, StoreError>`;
  - `Store::all_connections(&self) -> Result<Vec<ConnectionRow>, StoreError>`.

- [ ] **Step 1: Write the failing tests** em `tests/rbac_store.rs`, no padrão dos testes que já estão lá (banco descartável por teste, `TestDb`):
  - `search_users_matches_id_name_and_email_case_insensitively`: cria `ana` (name "Ana Souza"), `bia` (email `ANA@x.com`) e `caio`. `q: "ana"` devolve `[ana, bia]`, nessa ordem, com `total == 2`.
  - `search_treats_like_wildcards_literally`: cria `a_b` e `axb`. `q: "a_b"` devolve só `a_b`; `q: "%"` não devolve nada.
  - `search_pages_report_the_full_total`: com 5 usuários, `limit: Some(2), offset: 2` devolve os ids 3 e 4 (por ordem) e `total == 5`; `offset: 10` devolve `items == []` e `total == 5`.
  - `search_organizations_by_name_or_id_prefix`: `q` com os 8 primeiros caracteres do UUID de uma organização devolve só ela; `visible: Some(&[])` devolve `total == 0`.
  - `search_workspaces_matches_the_organization_name`: organizações "ON" (workspaces Operations e Vendas) e "Digital" (Finance). `q: "on"` devolve Operations e Vendas, com `organization_name == "ON"`. O filtro `organization: Some(digital)` devolve só Finance.
  - `all_connections_lists_every_tracked_connection`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd rust/cube && CUBE_TEST_PG_URL=... cargo test -j 2 -p cubeserver --test rbac_store search`
Expected: erro de compilação (`no method named search_users`).

- [ ] **Step 3: Implement os tipos e os métodos**
  - Uma consulta para os itens (`LIMIT`/`OFFSET`) e uma `count(*)` com o mesmo `WHERE`.
  - O termo é escapado com `\` antes de `\`, `%` e `_`, e casado com `ILIKE '%' || $n || '%' ESCAPE '\'`.
  - O prefixo do id é `id::text LIKE lower($n) || '%' ESCAPE '\'`.
  - `visible` vira `AND id = ANY($k)`.
  - Sem índice por enquanto. A spec deixa isso para medir na Task 11.

- [ ] **Step 4: Run the tests to see them pass** (o mesmo comando, Expected: PASS).

- [ ] **Step 5: Commit** `feat(rbac): paged, searchable store queries`

### Task 2: Parâmetros de busca nas rotas e as rotas novas de listagem

**Files:**
- Modify:
  - `rust/cube/cubeserver/src/rbac/handlers/users.rs` (`list`);
  - `rust/cube/cubeserver/src/rbac/handlers/organizations.rs` (`list`);
  - `rust/cube/cubeserver/src/rbac/handlers/workspaces.rs` (nova `list_all`);
  - `rust/cube/cubeserver/src/rbac/handlers/connections.rs` (nova `list_all`);
  - `rust/cube/cubeserver/src/rbac/handlers/mod.rs` (rotas, e `SearchParams` junto dos helpers).
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Consumes: `Search`, `Page`, `WorkspaceListing` e os métodos `search_*` e `all_connections` da Task 1.
- Produces:
  - `pub(crate) struct SearchParams { q: Option<String>, limit: Option<i64>, offset: Option<i64> }` (`Deserialize`, `rename_all = "camelCase"`), com `fn search(&self) -> Result<Search, ApiError>`:
    - `limit` acima de 200, ou `limit`/`offset` negativo: 400;
    - `q` vazio ou só com espaços conta como `None`;
    - `has_paging()` é verdadeiro quando `q`, `limit` ou `offset` vier.
  - `GET /v1/users?q&limit&offset` → `{users: [...], total}`. O `total` só aparece quando `has_paging()`.
  - `GET /v1/organizations?q&limit&offset` → `{organizations: [...], total}`, com a mesma regra.
  - `GET /v1/workspaces?q&organizationId&limit&offset` → `{workspaces: [{id, organizationId, organizationName, name, createdAt, createdBy}], total}`. É uma rota nova e sempre traz `total`. Um `organizationId` que não é UUID dá 400.
  - `GET /v1/connections` (só platform admin) → `{connections: [{name, organizationId, origin}]}`.

- [ ] **Step 1: Write the failing tests** em `tests/rbac_api.rs`, usando `scenario(...)` e os helpers `create_org` e `workspace_id`, dentro de `db_test!`:
  - `listing_users_without_paging_is_unchanged`: `GET /cube/v1/users` não tem a chave `total`.
  - `users_search_and_paging`: `GET /cube/v1/users?q=bi` devolve `["bia"]` com `total == 1`; `?limit=2&offset=0` devolve 2 itens com `total == 4`, contando `admin`, `ana`, `bia` e `caio`.
  - `a_limit_above_200_is_400`.
  - `workspaces_listing_spans_organizations_and_respects_visibility`:
    - o admin cria as organizações A (workspace `ops`) e B (workspace `fin`) e dá a `bia` o papel `viewer` em `fin`;
    - `GET /cube/v1/workspaces` como admin devolve os dois, com `organizationName`;
    - como `bia`, devolve só `fin`;
    - `?organizationId=<A>` como admin devolve só `ops`;
    - `?q=a` como admin devolve `ops`, porque o nome da organização "A" casa.
  - `connections_listing_is_for_platform_admins`: 403 para `ana` e 200 para o admin.

- [ ] **Step 2: Run them to see them fail**

Run: `cd rust/cube && CUBE_TEST_PG_URL=... cargo test -j 2 -p cubeserver --test rbac_api search listing`
Expected: FAIL (as rotas respondem 404/405, ou sem `total`).

- [ ] **Step 3: Implement**
  - Os handlers recebem `Query<SearchParams>`.
  - Para platform admins (`ctx.platform_admin`), `visible = None`. Para os demais, `visible = Some(lookup_resources(...))`.
  - Registre `GET {base}/v1/workspaces` e `GET {base}/v1/connections` em `authenticated_routes`.
  - O formato de cada workspace reaproveita o `view` de `workspaces.rs`, com `organizationName`.

- [ ] **Step 4: Run them to see them pass.** Rode também `--test rbac_api` inteiro, para garantir que nada antigo quebrou. Expected: PASS.

- [ ] **Step 5: Commit** `feat(rbac): search and paging on the management listings`

### Task 3: Cliente da busca no Playground

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Admin/api.ts`
- Create: `packages/cubejs-playground/src/pages/Admin/useSearch.ts`
- Test: `packages/cubejs-playground/src/__tests__/admin-api.test.ts`

**Interfaces:**
- Consumes: as rotas da Task 2.
- Produces, em `api.ts`:
  - `export const PAGE_SIZE = 50`;
  - `export type Page<T> = { items: T[]; total: number }`;
  - `export type WorkspaceListing = Workspace & { organizationName: string }`;
  - `export type TrackedConnection = { name: string; organizationId: string; origin: 'created' | 'assigned' }`;
  - em `adminApi`:
    - `searchUsers(q: string, page: number): Promise<Page<User>>`;
    - `searchOrganizations(q: string, page: number): Promise<Page<Organization>>`;
    - `searchWorkspaces(opts: { q: string; organizationId?: string | null; page: number }): Promise<Page<WorkspaceListing>>`;
    - `trackedConnections(): Promise<TrackedConnection[]>`.
  - `page` começa em 1, e o offset é `(page-1) * PAGE_SIZE`.
  - `assignableDataSources(orgs)` sai. Em seu lugar entra `assignableDataSources(): Promise<DataSource[]>`, que é `dataSources()` menos os nomes de `trackedConnections()`, usando a função pura `assignableDataSources(sources, owned)` que já existe.
- Produces, em `useSearch.ts`: `export function useSearch<T>(fetch: (q: string, page: number) => Promise<Page<T>>, deps: unknown[]): { q, setQ, page, setPage, result: Page<T> | null, loading, reload }`.
  - Debounce de 300 ms em `q`.
  - Trocar `q` volta para a página 1.
  - Uma resposta que chega depois de outra mais nova é descartada.

- [ ] **Step 1: Write the failing tests** em `admin-api.test.ts`, no padrão do arquivo (fetch simulado):
  - `searchUsers('ana', 2)` chama `.../v1/users?q=ana&limit=50&offset=50` e devolve `{items, total}`;
  - `searchWorkspaces({ q: '', organizationId: 'o1', page: 1 })` chama `.../v1/workspaces?limit=50&offset=0&organizationId=o1`, sem `q` vazio;
  - `assignableDataSources()` exclui `default` e os nomes de `trackedConnections()`.

- [ ] **Step 2: Run them to see them fail.** Run: `cd packages/cubejs-playground && node_modules/.bin/vitest run src/__tests__/admin-api.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement** os métodos e o `useSearch`.
- [ ] **Step 4: Run them to see them pass** (o mesmo comando).
- [ ] **Step 5: Commit** `feat(playground): search client for the Admin page`

### Task 4: Busca e paginação nas abas Organizations, Workspaces e Users

**Files:**
- Create: `packages/cubejs-playground/src/pages/Admin/RemoteSelect.tsx`
- Modify:
  - `AdminPage.tsx`: deixa de carregar todos os usuários; não passa mais `users`;
  - `OrganizationsTab.tsx`;
  - `WorkspacesTab.tsx`;
  - `UsersTab.tsx` (só a tabela da esquerda);
  - `NameDialog.tsx`;
  - `WorkspacePanel.tsx`.

  Todos ficam em `packages/cubejs-playground/src/pages/Admin/`.

**Interfaces:**
- Consumes: `useSearch`, `searchUsers`, `searchOrganizations`, `searchWorkspaces`, `assignableDataSources()` e `PAGE_SIZE`, da Task 3.
- Produces:
  - `RemoteSelect` com props `{ value?: string; onChange: (v?: string) => void; fetchOptions: (q: string) => Promise<{ value: string; label: string }[]>; placeholder?: string; allowClear?: boolean; style?: CSSProperties; size?: 'small' | 'middle' }`. É um `Select` do antd com `showSearch`, `filterOption={false}` e debounce de 300 ms, que busca as primeiras 50 opções e mantém o rótulo do valor escolhido.
  - Em `NameDialog`, `Choice` troca `options` por `fetchOptions: (q: string) => Promise<{value: string; label: string}[]>`.
  - Helpers exportados de `RemoteSelect.tsx`:
    - `userOptions(api)`: rótulo `id (name)`, só usuários ativos;
    - `organizationOptions(api)`.

- [ ] **Step 1: Implement**
  - Cada aba ganha, acima da tabela, um `Input.Search` com `allowClear`.
    - Placeholders: "Search organizations by name or id", "Search workspaces, or their organization", "Search users by username, name or email".
  - Cada tabela usa `pagination={{ current: page, pageSize: PAGE_SIZE, total, onChange: setPage, showSizeChanger: false }}`.
  - O filtro de organização da aba Workspaces vira um `RemoteSelect` com `allowClear`. Limpar significa "todas as organizações".
  - Os seletores de owner, de organização e de membro (`WorkspacePanel`) usam `RemoteSelect`.
  - Depois de criar, editar ou excluir, a aba chama `reload()`.
- [ ] **Step 2: Typecheck e lint**
  - `cd packages/cubejs-playground && node_modules/.bin/tsc --noEmit -p . 2>&1 | grep -c "error TS"`: Expected `6`, os mesmos erros de antes, em `App.tsx` e `Header.tsx`, e nenhum com `Admin` no caminho.
  - `cd /home/antonio-rodrigues/Documents/GitHub/cube && node_modules/.bin/oxlint packages/cubejs-playground/src/pages/Admin`: Expected exit 0.
- [ ] **Step 3: Rode todos os testes do Playground:** `node_modules/.bin/vitest run`. Expected: todos passam.
- [ ] **Step 4: Commit** `feat(playground): search and pages in the Admin tabs`

### Task 5: Árvore da aba Users com busca e membros sob demanda

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Admin/MembershipTree.tsx`
- Create: `packages/cubejs-playground/src/pages/Admin/membershipGroups.ts`
- Test: `packages/cubejs-playground/src/__tests__/membership-groups.test.ts`

**Interfaces:**
- Consumes: `useSearch` e `searchWorkspaces` (Task 3).
- Produces: `export function groupByOrganization(items: WorkspaceListing[]): { organizationId: string; organizationName: string; workspaces: WorkspaceListing[] }[]`. Mantém a ordem do servidor e junta workspaces consecutivos da mesma organização.

- [ ] **Step 1: Write the failing test** `groups consecutive workspaces by organization in server order`: com `[ON/Ops, ON/Vendas, Digital/Fin]`, devolve dois grupos, ON com `[Ops, Vendas]` e Digital com `[Fin]`.
- [ ] **Step 2: Run it to see it fail.** `node_modules/.bin/vitest run src/__tests__/membership-groups.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement `groupByOrganization`**, e ajuste a árvore:
  - um `Input.Search` com o placeholder "Search workspaces, or their organization";
  - paginação simples ("Previous" e "Next", mais "page X of Y") abaixo da árvore;
  - os membros (`api.members`) são carregados ao abrir um workspace ou ao soltar um usuário nele, e ficam guardados por workspace até a próxima busca;
  - a contagem de membros aparece só depois do carregamento;
  - o `drop` continua como está: `viewer`, o aviso "already in" e a mensagem "added ... as viewer".
- [ ] **Step 4: Rode o teste e o vitest inteiro.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(playground): searchable membership tree with on-demand members`

### Task 6: Seletor de workspace da barra superior com busca

**Files:**
- Modify: `packages/cubejs-playground/src/components/Header/WorkspaceSelector.tsx`
- Create: `packages/cubejs-playground/src/components/Header/workspaceChoice.ts`
- Test: `packages/cubejs-playground/src/__tests__/workspace-choice.test.ts`

**Interfaces:**
- Consumes: `GET /v1/workspaces?q&limit=50` (Task 2) e `GET /v1/workspaces/{id}`, que já existe e responde 404 quando o workspace é invisível ou não existe.
- Produces: `export function nextChoice(opts: { current: string | null; currentVisible: boolean; firstPage: { value: string }[]; isAdmin: boolean }): { choose: string | null } | null`. `null` significa manter o atual.

- [ ] **Step 1: Write the failing tests** `nextChoice`:
  - um workspace atual e visível: devolve `null`;
  - um workspace atual que deixou de ser visível, e o usuário não é admin, com uma primeira página `[w1]`: devolve `{choose: 'w1'}`;
  - o mesmo caso para um admin: devolve `{choose: null}` ("Todos (admin)");
  - nenhum atual, um não-admin e uma primeira página vazia: devolve `null`.
- [ ] **Step 2: Run them to see them fail.** `node_modules/.bin/vitest run src/__tests__/workspace-choice.test.ts`.
- [ ] **Step 3: Implement**
  - O seletor busca no servidor enquanto você digita, com debounce de 300 ms e 50 opções. O rótulo continua `Org / Workspace`.
  - Para admins, "Todos (admin)" continua sempre no topo.
  - Quando o workspace atual não está na primeira página, o rótulo dele é montado com `GET /v1/workspaces/{id}` e `GET /v1/organizations/{organizationId}`. Um 404 em qualquer das duas conta como `currentVisible: false`.
  - A troca de workspace continua por `chooseWorkspace`.
  - A versão antiga, `loadOptions`, que fazia uma chamada por organização, sai.
- [ ] **Step 4: Rode os testes, o `tsc` (os mesmos 6 erros) e o `oxlint`.** Expected: PASS e exit 0.
- [ ] **Step 5: Commit** `feat(playground): searchable workspace selector`

## Parte 2: grafo de acessos

### Task 7: Montagem do grafo (função pura)

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/access.rs`, registrado como `pub mod access;` em `src/rbac/mod.rs`. Os testes de unidade ficam no próprio arquivo (`#[cfg(test)]`).

**Interfaces:**
- Produces:
  - `AccessInput`, com:
    - `user: User`;
    - `platform_admin: bool`;
    - `direct: Vec<Relationship>`: as relações em que o usuário é o sujeito, ou seja, `organization#owner|member` e `workspace#owner|editor|viewer`;
    - `organizations: Vec<Organization>`;
    - `workspaces: Vec<Workspace>`: os diretos e todos os das organizações em que ele é owner;
    - `connections: Vec<ConnectionRow>`: as das organizações envolvidas;
    - `releases: Vec<(String, Uuid)>`: pares (conexão, workspace);
    - `configured: BTreeSet<String>`: os nomes de data source no `cube.yml`.
  - `pub fn build(input: &AccessInput) -> AccessGraph`.
  - `AccessGraph` (`Serialize`, camelCase):
    - `{ user: {id, name, disabled}, platformAdmin, nodes: Vec<Node>, edges: Vec<Edge>, grants: Vec<Grant> }`;
    - `Node { id, kind, name, organizationId: Option<String>, notes: Vec<String> }`:
      - `id` é `"user:<id>"`, `"organization:<uuid>"`, `"workspace:<uuid>"` ou `"connection:<name>"`;
      - `kind` é `"user"`, `"organization"`, `"workspace"` ou `"connection"`;
    - `Edge { from, to, label, kind }`, com `kind` igual a `"direct"` ou `"derived"`;
    - `Grant { resource, permission, reasons: Vec<Reason> }`;
    - `Reason { text, path: Vec<String> }`: os ids dos nós, do usuário até o recurso.
- **Regras**, que reproduzem o esquema da spec de 2026-10-28 e as restrições do servidor:

| Origem | Concede | `text` | `path` | Aresta |
|---|---|---|---|---|
| `organization#owner` | org: `admin`, `view`, `manage_connections` | `owner` | user→org | direct `owner` |
| `organization#member` | org: `view` | `member` | user→org | direct `member` |
| `workspace#owner` | ws: `manage`, `edit`, `view`, `query`, `execute`, `read objects`, `write objects` | `owner` | user→ws | direct `owner` |
| `workspace#editor` | ws: `edit`, `view`, `query`, `read objects`, `write objects` | `editor` | user→ws | direct `editor` |
| `workspace#viewer` | ws: `view`, `query`, `read objects` | `viewer` | user→ws | direct `viewer` |
| owner da org O, para cada ws W de O | W: as 7 de `workspace#owner` | `owner of <O.name>` | user→O→W | derived `admin ⇒ manage` (O→W) |
| owner da org O, conexão C de O com `origin: created` | C: `manage` | `owner of <O.name>` | user→O→C | derived `manage` (O→C) |
| conexão C liberada para W, e o usuário tem `query` em W | C: `query`, desde que C esteja em `configured` | `released to <W.name>` | o caminho de `query` em W, mais →C | derived `released` (W→C) |

- **Notas:**
  - Conexão `assigned`: `"assigned by a platform admin: only platform admins edit it"`, sem `manage`.
  - Conexão fora de `configured`: `"data source missing"`, sem `query`.
- **Casos especiais:**
  - Conexão sem linha em `connections`, ou seja, sem organização: não entra no grafo.
  - Platform admin: `platformAdmin: true`, e `nodes`, `edges` e `grants` contêm só o nó do usuário.
  - Uma permissão com mais de um caminho ganha um `Reason` por caminho, sem `Reason`s duplicados.
- **Ordem:** os `grants` vêm na ordem dos nós (organizações por nome, depois workspaces, depois conexões), e dentro de cada nó na ordem da tabela.

- [ ] **Step 1: Write the failing tests** em `access.rs`:
  - `a_viewer_can_view_query_and_read_objects`;
  - `an_organization_owner_manages_every_workspace_of_it`: os grants de W têm o `Reason` com `text == "owner of ON"` e `path == ["user:ana", "organization:<O>", "workspace:<W>"]`;
  - `two_paths_give_two_reasons`: ana é `editor` de W e owner de O. O grant `edit` em W tem os motivos `editor` e `owner of ON`;
  - `an_assigned_connection_is_queried_but_not_managed`: tem `query` com `released to Operations`, não tem grant `manage` e tem a nota de assigned;
  - `a_created_connection_is_managed_by_the_owner`;
  - `a_release_to_a_missing_data_source_grants_nothing`: a nota é `data source missing`, e não há grant `query`;
  - `a_connection_without_organization_is_left_out`;
  - `a_platform_admin_is_a_single_node`;
  - `a_disabled_user_keeps_their_grants_flagged`: `user.disabled == true`, e os grants continuam lá.
- [ ] **Step 2: Run them to see them fail.** `cd rust/cube && cargo test -j 2 -p cubeserver --lib rbac::access`. Expected: erro de compilação.
- [ ] **Step 3: Implement `build`.**
- [ ] **Step 4: Run them to see them pass.**
- [ ] **Step 5: Commit** `feat(rbac): build a user's access graph from their grants`

### Task 8: Rota `GET /v1/users/{id}/access`

**Files:**
- Modify:
  - `rust/cube/cubeserver/src/rbac/handlers/users.rs` (novo `access`);
  - `rust/cube/cubeserver/src/rbac/handlers/mod.rs` (rota).
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Consumes:
  - `access::build` e `AccessInput` (Task 7);
  - `Authz::read`, `Rbac::is_platform_admin`;
  - `Store::{user, organizations_by_ids, workspaces_by_ids, workspaces_of_organization, connections_of_organization}`;
  - `handlers::data_sources::data_source_types(&state)`, cujas chaves formam `configured`.
- Produces: `GET {base}/v1/users/:id/access` → `AccessGraph` em JSON. Só platform admin. 404 `User not found`.

- [ ] **Step 1: Write the failing tests**, em `db_test!`:
  - `access_graph_is_for_platform_admins`: 403 para `ana`, 404 para o usuário `nobody`.
  - `access_graph_agrees_with_check`, que é o anti-objetivo da spec:
    - no cenário, `ana` é owner de A, que tem os workspaces `ops` e `fin`; `bia` é `editor` de `ops`; `caio` é `viewer` de `fin`;
    - para cada um dos três usuários, e para cada organização e workspace × permissão do esquema (`admin`, `view` e `manage_connections` numa organização; `manage`, `edit`, `view`, `query` e `execute` num workspace), existir um grant no grafo ⇔ `env.authz.check(...)` dar verdadeiro.
  - `access_graph_hides_manage_on_assigned_connections`:
    - usa o `connections_scenario` e o helper `assign`;
    - atribui a A um data source que já está no `cube.yml` e o libera para `ops`;
    - o grafo de `ana` tem `query` na conexão, não tem `manage` e traz a nota de assigned;
    - `PUT /cube/v1/connections/<name>` como `ana` responde 403, o que confirma que o grafo bate com a API.
- [ ] **Step 2: Run them to see them fail.** `CUBE_TEST_PG_URL=... cargo test -j 2 -p cubeserver --test rbac_api access_graph`.
- [ ] **Step 3: Implement `access`**, montando o `AccessInput`:
  - as relações diretas vêm de `read` com `subject = user:<id>`, uma vez para `organization` e outra para `workspace`;
  - os workspaces das organizações em que ele é owner vêm de `workspaces_of_organization`;
  - as liberações vêm de `read` em `connection#workspace`, para os workspaces em que ele tem `query`.
- [ ] **Step 4: Run them to see them pass.** Depois rode o `cargo clippy -j 2 -p cubeserver --all-targets -- -D warnings`. Expected: PASS, sem avisos.
- [ ] **Step 5: Commit** `feat(rbac): GET /v1/users/{id}/access`

### Task 9: Layout do grafo e caminho destacado (cliente, lógica pura)

**Files:**
- Create: `packages/cubejs-playground/src/pages/Admin/access/types.ts`, que espelha o JSON da Task 8.
- Create: `packages/cubejs-playground/src/pages/Admin/access/layout.ts`
- Test: `packages/cubejs-playground/src/__tests__/access-layout.test.ts`

**Interfaces:**
- Produces:
  - em `types.ts`: `AccessGraph`, `AccessNode`, `AccessEdge`, `Grant` e `Reason`, com os mesmos campos da Task 7, em camelCase;
  - em `layout.ts`:
    - `export const COLLAPSE_AT = 8`: uma organização com mais de 8 workspaces no grafo entra recolhida;
    - `export const NODE_W = 176, NODE_H = 58`;
    - `export function layout(graph: AccessGraph, expanded: Set<string>): { nodes: (AccessNode & { x: number; y: number; perms: string; hiddenCount?: number })[]; edges: AccessEdge[]; width: number; height: number }`:
      - as colunas são usuário, organizações, workspaces e conexões, com `x = 20, 260, 510, 764`;
      - cada coluna é empilhada com 22 px entre os nós;
      - `perms` são as permissões do nó juntas com `" · "`;
      - uma organização recolhida esconde os seus workspaces e informa `hiddenCount`;
      - as arestas que tocam nós escondidos saem;
    - `export function highlight(reason: Reason): { nodes: Set<string>; edges: Set<string> }`, com a chave de aresta `"<from>><to>"`, para cada par consecutivo de `reason.path`.

- [ ] **Step 1: Write the failing tests**:
  - `lays nodes out in four columns`;
  - `collapses an organization with more than COLLAPSE_AT workspaces`: com 9 workspaces na organização O e `expanded` vazio, não sai nenhum nó de workspace de O, e O tem `hiddenCount == 9`. Com `expanded = {O}`, saem os 9;
  - `highlight follows the reason path`: o path `[u, o, w]` dá as arestas `u>o` e `o>w`.
- [ ] **Step 2: Run them to see them fail.** `node_modules/.bin/vitest run src/__tests__/access-layout.test.ts`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them to see them pass.**
- [ ] **Step 5: Commit** `feat(playground): access graph layout`

### Task 10: Janela Access (modelo C) e o botão na aba Users

**Files:**
- Create: `packages/cubejs-playground/src/pages/Admin/access/AccessModal.tsx`
- Modify:
  - `packages/cubejs-playground/src/pages/Admin/api.ts`: `access(user: string): Promise<AccessGraph>`, que chama `GET /v1/users/{id}/access`;
  - `packages/cubejs-playground/src/pages/Admin/UsersTab.tsx`: o botão "Access" logo antes de "Reset password".

**Interfaces:**
- Consumes: `layout`, `highlight`, `COLLAPSE_AT`, `NODE_W` e `NODE_H` (Task 9), e `api.access`.
- Produces: `AccessModal` com props `{ api: AdminApi; user: User | null; onClose: () => void }`. Aberta enquanto `user` não for `null`.

- [ ] **Step 1: Implement**, seguindo a opção C do modelo (https://claude.ai/artifact/Tkka575a24epJAoScwNkk9):
  - **A janela:** um `Modal` com `width="90vw"` e o título `Access of <id>`. A barra do título traz `N resources · M permissions`.
  - **O grafo:** um SVG à esquerda.
    - Nós com uma faixa colorida por tipo e o texto das permissões.
    - Linha cheia nas arestas `direct` e tracejada nas `derived`, cada uma com um rótulo.
    - Uma organização recolhida mostra `+N workspaces`. Clicar nela expande ou recolhe.
  - **O painel** "What \<id\> can do", à direita: cada recurso, cada permissão e cada motivo.
    - Clicar num motivo destaca o caminho dele, com a cor de destaque nas arestas e nos nós. Clicar de novo apaga o destaque.
    - As notas dos nós aparecem em texto secundário.
  - **Abaixo de 820 px de largura**, o painel vai para baixo do grafo.
  - **Platform admin:** um único nó, e o texto "Platform admin: full access to every organization, workspace and connection."
  - **Usuário desativado:** um `Alert` do tipo `warning` com "Disabled: these grants do not apply while the user is disabled."
  - **Carregamento:** um `Spin`. **Erro:** um `Alert` com a mensagem do servidor.
  - **Cores:** as mesmas das linhas do Admin (accent `#7a77ff`). As cores de tipo seguem o modelo.
- [ ] **Step 2: Rode o `tsc` (os mesmos 6 erros), o `oxlint` (exit 0) e o `vitest run` (tudo passa).**
- [ ] **Step 3: Commit** `feat(playground): user access graph in the Admin page`

### Task 11: Implantação e verificação

**Files:** nenhum arquivo do repositório. Os scripts ficam no scratchpad da sessão.

- [ ] **Step 1: Release build**
  - No worktree `/home/antonio-rodrigues/Documents/GitHub/cube-rbac-build`, faça `git checkout --detach <HEAD da branch>`.
  - Rode `CARGO_TARGET_DIR=/root/cube/rust/cube/target cargo build -j 2 --release -p cubeserver`.
  - Depois, `node_modules/.bin/vite build` em `packages/cubejs-playground`.
- [ ] **Step 2: Instalação**
  - `cd /root/cube-rust && VIZARD_SOURCE=/root/cube/packages/cubejs-playground/vizard/build ./install-playground.sh <worktree>/packages/cubejs-playground/build`.
  - Em seguida, `./start-all.sh restart` e `./start-all.sh status`. Expected: tudo ✓ e `/readyz` 200.
- [ ] **Step 3: Volume**
  - Um script cria, pela API, 2000 usuários `load-NNNN`, 50 organizações `Load NN` e 10 workspaces por organização.
  - Meça `GET /v1/users?q=load-19&limit=50`, `GET /v1/workspaces?q=Load%2007&limit=50` e o grafo de um usuário que é owner de uma organização. Expected: menos de 300 ms cada.
  - Se passar de 300 ms, crie o índice que a spec deixou em aberto e meça de novo.
- [ ] **Step 4: No navegador** (`playwright-cli`):
  - a busca e a paginação nas três abas e na árvore;
  - arrastar um usuário encontrado pela busca para um workspace encontrado pela busca;
  - o seletor da barra superior com busca;
  - o grafo de um owner, com o caminho destacado ao clicar num motivo e uma organização recolhida.
- [ ] **Step 5: Limpeza**
  - Apague os dados `load-*` e `Load *`: as organizações e os workspaces pela API, e os usuários com `DELETE FROM users WHERE id LIKE 'load-%'` no banco `cube_app` do container `cube-pg-test`.
  - Não toque nos dados do usuário: a organização "ON", "Digital", o usuário `antonio` e os usuários `e2e_*`.
- [ ] **Step 6: Push** da branch `feat/rbac-spicedb`.

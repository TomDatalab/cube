# RBAC no Cube com SpiceDB — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Organizations, Workspaces, usuários com senha, conexões e objetos no Cube, com as permissões decididas pelo SpiceDB e aplicadas aos dados, aos objetos e ao Playground.

**Architecture:**
- Novo módulo `cubeserver::rbac`, com quatro partes:
  - um cliente HTTP do SpiceDB atrás de uma trait (`Authz`), com uma implementação falsa para os testes;
  - um armazenamento Postgres (`Store`, banco `cube_app`);
  - os handlers `/v1/...`;
  - envoltórios das `QueryService`/`MetaService` existentes.
- `AppState` ganha `rbac: Option<Arc<Rbac>>`. Com `None`, que é o padrão, nada muda.
- O workspace de cada requisição vem do cabeçalho `X-Cube-Workspace` e vai para o `RequestContext`.

**Tech Stack:** Rust (axum 0.7, tokio, reqwest 0.12, tokio-postgres 0.7, deadpool-postgres 0.14, argon2), SpiceDB (Docker, API HTTP), Postgres 16, React 18 + antd 4 (Playground).

**Spec:** `docs/superpowers/specs/2026-10-28-cube-rbac-spicedb-design.md`. Leia-o antes de qualquer tarefa: ele traz o esquema, as tabelas, a tabela de papéis e a lista de endpoints. Este plano não os repete.

## Global Constraints

- **Ativação:** só com `CUBE_RBAC=spicedb`; sem a variável, nenhuma rota nova é montada e todo o comportamento atual se mantém.
- **Variáveis:** `SPICEDB_ENDPOINT` (HTTP, ex. `http://127.0.0.1:18443`), `SPICEDB_PRESHARED_KEY`, `CUBE_APP_DB_URL`, `CUBE_RBAC_ADMINS` (lista separada por vírgulas), `CUBE_RBAC_ADMIN_PASSWORD`, `CUBE_RBAC_TOKEN_TTL` (padrão `12h`).
- **Falhas:**
  - **Na inicialização:** com o RBAC ligado, variável ausente, SpiceDB ou Postgres inacessível, ou um bloco `tenants:` em `cube.yml`, fazem o servidor terminar com erro claro.
  - **Durante a execução:** um erro do SpiceDB resulta em **503** e nega o acesso (fail-closed).
- **Respostas de erro:** 401 sem token, ou token sem `sub`, usuário desativado, ou token emitido antes de `password_changed_at`. 403 sem permissão. 404 quando o recurso não existe **ou** o usuário não tem `view` nele.
- **Senhas:** argon2id, mínimo de 10 caracteres. Uma falha de login espera 1 s e devolve sempre a mesma mensagem: `"Invalid username or password"`.
- **Ids de usuário:** `[a-z0-9._-]{1,64}`. Tipo de objeto: `[a-z0-9_-]{1,64}`.
- **SpiceDB:** checagens com consistência `fully_consistent`. Esquema em `rust/cube/cubeserver/rbac/schema.zed`, aplicado na inicialização quando difere do atual.
- **Portas do SpiceDB (só 127.0.0.1):** gRPC 50051, HTTP 18443, métricas 19090.
- **Platform admins:** dispensados de `X-Cube-Workspace`.
- **Commits:** autor `Tom DataLab <sofiadb@sofiadb.org>`, via `git -c user.name=... -c user.email=...`; mensagens convencionais terminando com `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Implantação:** tudo é feito no repositório. O deploy copia os arquivos alterados para `/root/cube`, compila lá e reinicia `/root/cube-rust/start-all.sh`, como nas tarefas anteriores. O `.env` e o `cube.yml` da instalação ganham backup antes de qualquer mudança.

## Review Focus

1. **Consulta que mistura cubos:** num pedido com vários cubos, um só deles de conexão não liberada deve fazer **toda** a consulta receber 403, inclusive quando o membro proibido aparece só num filtro `or` aninhado ou numa view. Teste na Tarefa 8.
2. **Token antigo depois da troca de senha ou da desativação:** deve receber 401 na hora, sem esperar o token expirar. Teste na Tarefa 4.
3. **Rebaixar um papel:** de editor para viewer, `PUT members` deve substituir o papel, não somar. O ex-editor não pode mais gravar objetos. Teste na Tarefa 6.
4. **Recurso de outra organização acessado por id:** um owner da organização A pedindo workspace, objeto ou conexão da organização B deve receber 404 (não 403), e o mesmo vale para listagens. Teste na Tarefa 6 e na Tarefa 7.
5. **SpiceDB fora do ar durante a execução:** login, consultas e objetos devem responder 503, nunca 200. Teste na Tarefa 8, com o `FakeAuthz` em modo de falha.

---

### Task 1: Infraestrutura — SpiceDB e bancos na instalação

**Files:**
- Create: `rust/cube/cubeserver/rbac/schema.zed` (o esquema da seção "Esquema do SpiceDB" do spec, sem alterações)
- Modify: `/root/cube-rust/start-all.sh` (fora do repositório; faça backup `start-all.sh.bak-<data>`)
- Modify: `/root/cube-rust/.env`, com backup

**Interfaces:**
- Produces:
  - container `cube-spicedb` servindo HTTP em `127.0.0.1:18443`;
  - bancos `cube_app` e `spicedb` no `cube-pg-test`;
  - variáveis do `.env`: `CUBE_RBAC`, `SPICEDB_ENDPOINT`, `SPICEDB_PRESHARED_KEY`, `CUBE_APP_DB_URL`, `CUBE_RBAC_ADMINS=admin`, `CUBE_RBAC_ADMIN_PASSWORD`.
  - Nesta tarefa, `CUBE_RBAC` fica **comentado**. A Tarefa 12 o liga.

- [ ] **Step 1:** No `start-all.sh`, novo módulo `start_spicedb`, chamado depois do Postgres e antes do `sync_model`:
  1. cria os bancos `cube_app` e `spicedb` se não existirem (`docker exec cube-pg-test psql -U postgres -tc "SELECT 1 FROM pg_database WHERE datname='...'"` e `createdb`);
  2. roda `docker run --rm --network host authzed/spicedb:<versão fixada> migrate head --datastore-engine postgres --datastore-conn-uri postgres://postgres:<senha>@127.0.0.1:55432/spicedb?sslmode=disable`;
  3. sobe `cube-spicedb` com `serve --grpc-preshared-key $SPICEDB_PRESHARED_KEY --http-enabled --http-addr 127.0.0.1:18443 --grpc-addr 127.0.0.1:50051 --metrics-addr 127.0.0.1:19090`, o mesmo datastore, `--network host` e `--restart unless-stopped`;
  4. espera `curl -sf http://127.0.0.1:18443/healthz` responder.
  - O `status` mostra o estado do SpiceDB.
  - Na primeira execução, gera `SPICEDB_PRESHARED_KEY` e `CUBE_RBAC_ADMIN_PASSWORD` com `openssl rand -hex 24` e as grava no `.env`, imprimindo a senha do admin uma única vez.
  - O `stop` deixa o SpiceDB rodando, como faz com o Postgres.
- [ ] **Step 2:** Rode `./start-all.sh restart`. Esperado: o módulo do SpiceDB mostra `✓` e o Cube continua `HEALTH` em `/readyz`.
- [ ] **Step 3:** Verifique `curl -s -X POST http://127.0.0.1:18443/v1/schema/read -H "Authorization: Bearer $SPICEDB_PRESHARED_KEY" -d '{}'`. Esperado: resposta JSON (erro "no schema" é aceitável).
- [ ] **Step 4:** Commit do `schema.zed` no repositório: `feat(cubeserver): SpiceDB schema for organizations and workspaces`.

### Task 2: Cliente SpiceDB e implementação falsa

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/mod.rs`, `rust/cube/cubeserver/src/rbac/authz.rs`, `rust/cube/cubeserver/src/rbac/fake.rs` (este último compilado também fora de `#[cfg(test)]`, atrás de `pub mod testing`, para os testes de integração)
- Modify: `rust/cube/cubeserver/src/lib.rs` (`pub mod rbac;`), `rust/cube/cubeserver/Cargo.toml` (adicionar `reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }`)
- Test: testes unitários em `fake.rs`

**Interfaces:**
- Produces:
```rust
pub struct ObjectRef { pub kind: &'static str, pub id: String }      // "workspace", "abc"
pub struct Relationship { pub resource: ObjectRef, pub relation: &'static str, pub subject: ObjectRef }
pub struct RelationshipFilter { pub kind: &'static str, pub id: Option<String>, pub relation: Option<&'static str>, pub subject: Option<ObjectRef> }
#[derive(Debug)] pub enum AuthzError { Unavailable(String), Rejected(String) }  // Unavailable → 503

#[async_trait] pub trait Authz: Send + Sync + std::fmt::Debug {
    async fn check(&self, resource: &ObjectRef, permission: &str, user: &str) -> Result<bool, AuthzError>;
    async fn lookup_resources(&self, kind: &'static str, permission: &str, user: &str) -> Result<Vec<String>, AuthzError>;
    async fn read(&self, filter: &RelationshipFilter) -> Result<Vec<Relationship>, AuthzError>;
    async fn touch(&self, relationships: &[Relationship]) -> Result<(), AuthzError>;   // OPERATION_TOUCH
    async fn delete(&self, relationships: &[Relationship]) -> Result<(), AuthzError>;  // OPERATION_DELETE
    async fn delete_matching(&self, filter: &RelationshipFilter) -> Result<(), AuthzError>;
    async fn ensure_schema(&self, schema: &str) -> Result<(), AuthzError>;
}
pub struct HttpAuthz { /* endpoint, key, reqwest::Client */ }
impl HttpAuthz { pub fn new(endpoint: &str, preshared_key: &str) -> Self }
pub struct FakeAuthz { /* Mutex<Vec<Relationship>>, AtomicBool failing */ }
impl FakeAuthz { pub fn new() -> Self; pub fn set_failing(&self, failing: bool) }
```
- `HttpAuthz` usa a API HTTP do SpiceDB:
  - `POST /v1/permissions/check` com `"consistency":{"fullyConsistent":true}`;
  - `/v1/permissions/resources` (lookup, resposta em stream de objetos JSON por linha);
  - `/v1/relationships/read`, `/v1/relationships/write` e `/v1/relationships/delete`;
  - `/v1/schema/read` e `/v1/schema/write`.
  - Sempre com o cabeçalho `Authorization: Bearer <key>`.
  - Uma falha de rede ou um 5xx vira `AuthzError::Unavailable`.
- O `FakeAuthz` avalia as permissões do esquema escritas em Rust: `platform.manage`, `organization.admin/view/manage_connections`, `workspace.manage/edit/view/query/execute`, `connection.manage/query` e `object.read/write`. Com `set_failing(true)`, toda chamada devolve `Unavailable`.

- [ ] **Step 1:** Testes unitários do fake:
  - `org_owner_can_edit_every_workspace_of_the_org`: owner da organização tem `workspace#edit`, mas não `edit` num workspace de outra organização.
  - `viewer_reads_objects_but_cannot_write`.
  - `connection_query_follows_release`: a conexão liberada para o workspace W dá `connection#query` a quem tem `workspace#query` em W, e mais ninguém além dos admins da organização.
  - `platform_admin_administers_every_org`.
- [ ] **Step 2:** `cargo test -p cubeserver --lib rbac`. Esperado: FAIL, porque os tipos ainda não existem.
- [ ] **Step 3:** Implemente `authz.rs` e `fake.rs`.
- [ ] **Step 4:** `cargo test -p cubeserver --lib rbac`. Esperado: PASS.
- [ ] **Step 5:** Teste manual do `HttpAuthz` contra o SpiceDB real, com um `#[ignore]` (`http_authz_roundtrip`, que lê `SPICEDB_ENDPOINT` e `SPICEDB_PRESHARED_KEY`): `ensure_schema(schema.zed)` → `touch(owner)` → `check` true → `delete` → `check` false. Rode com `cargo test -p cubeserver --lib http_authz_roundtrip -- --ignored`. Esperado: PASS.
- [ ] **Step 6:** Commit: `feat(cubeserver): SpiceDB client and in-memory authz for tests`.

### Task 3: Armazenamento Postgres (`cube_app`)

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/store.rs`, `rust/cube/cubeserver/src/rbac/migrations.sql`
- Modify: `rust/cube/cubeserver/Cargo.toml` (`tokio-postgres = { version = "0.7", features = ["with-uuid-1", "with-chrono-0_4", "with-serde_json-1"] }`, `deadpool-postgres = "0.14"`, `chrono = "0.4"`, e `uuid` com `serde`)
- Test: `rust/cube/cubeserver/tests/rbac_store.rs`

**Interfaces:**
- Produces:
```rust
pub struct Store { /* deadpool_postgres::Pool */ }
impl Store {
    pub async fn connect(url: &str) -> Result<Self, StoreError>;      // aplica migrations.sql (idempotente: CREATE TABLE IF NOT EXISTS)
    // users
    pub async fn create_user(&self, u: &NewUser) -> Result<User, StoreError>;         // StoreError::Conflict se o id existe
    pub async fn user(&self, id: &str) -> Result<Option<User>, StoreError>;
    pub async fn users(&self) -> Result<Vec<User>, StoreError>;
    pub async fn update_user(&self, id: &str, name: Option<String>, email: Option<String>, disabled: Option<bool>) -> Result<Option<User>, StoreError>;
    pub async fn set_password_hash(&self, id: &str, hash: &str) -> Result<(), StoreError>;  // atualiza password_changed_at = now()
    // organizations / workspaces / connections / objects: create_*, *(id), *_by_ids(&[Uuid]), update_*, delete_*
    pub async fn transaction(&self) -> Result<StoreTx<'_>, StoreError>;               // StoreTx expõe os mesmos create_* e commit()/rollback()
}
pub struct User { pub id: String, pub name: Option<String>, pub email: Option<String>, pub password_hash: String, pub disabled: bool, pub created_at: DateTime<Utc>, pub password_changed_at: DateTime<Utc> }
pub struct Organization { pub id: Uuid, pub name: String, pub created_at: DateTime<Utc>, pub created_by: String }
pub struct Workspace { pub id: Uuid, pub organization_id: Uuid, pub name: String, pub created_at: DateTime<Utc>, pub created_by: String }
pub struct ConnectionRow { pub name: String, pub organization_id: Uuid }
pub struct StoredObject { pub id: Uuid, pub workspace_id: Uuid, pub r#type: String, pub name: String, pub content_type: String, pub content: Vec<u8>, pub version: i32, /* created/updated at/by */ }
pub enum StoreError { Conflict(String), NotFound, Db(String) }
```
- As tabelas são exatamente as da seção "Tabelas (`cube_app`)" do spec. Há também `UNIQUE (organization_id, name)` em `workspaces` e `ON DELETE CASCADE` de `objects` para `workspaces`.
- `update_object(id, expected_version, ...)` devolve `StoreError::Conflict` quando a versão não confere, usando `UPDATE ... WHERE id=$1 AND version=$2` e conferindo as linhas afetadas.

- [ ] **Step 1:** Testes em `tests/rbac_store.rs`, que rodam só com `CUBE_TEST_PG_URL` definido (senão, `return` com aviso). Cada teste cria um banco `cube_app_test_<uuid>` e o apaga no fim.
  - `users_roundtrip_and_conflict`.
  - `workspace_names_are_unique_per_organization`.
  - `object_update_with_a_stale_version_conflicts`: cria com `version` 1; atualizar com 1 dá 2; atualizar de novo com 1 dá `Conflict`.
  - `deleting_a_workspace_deletes_its_objects`.
  - `a_rolled_back_transaction_leaves_nothing`.
- [ ] **Step 2:** `CUBE_TEST_PG_URL=postgres://postgres:test@127.0.0.1:55432/postgres cargo test -p cubeserver --test rbac_store`. Esperado: FAIL, porque nada compila ainda.
- [ ] **Step 3:** Implemente `migrations.sql` e `store.rs`.
- [ ] **Step 4:** Rode de novo. Esperado: PASS.
- [ ] **Step 5:** Commit: `feat(cubeserver): Postgres store for users, organizations, workspaces and objects`.

### Task 4: Configuração, inicialização, senhas, login e identidade

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/config.rs`, `rust/cube/cubeserver/src/rbac/identity.rs`, `rust/cube/cubeserver/src/rbac/handlers/auth.rs` (com o `mod.rs` de `rbac/handlers`)
- Modify:
  - `rust/cube/cubeserver/src/app.rs`: campo `rbac: Option<Arc<crate::rbac::Rbac>>` no `AppState`, rotas montadas só com `Some`;
  - `rust/cube/cubeserver/src/main.rs`: inicialização;
  - `rust/cube/cubeserver/src/services.rs`: `RequestContext` ganha `workspace: Option<String>`, `user: Option<String>` e `platform_admin: bool`;
  - `rust/cube/cubeserver/src/handlers/auth.rs`: o middleware preenche esses campos;
  - os 4 lugares que constroem `RequestContext` (`grep -rn "RequestContext {"`);
  - `Cargo.toml`: `argon2 = "0.5"`.
- Test: `rust/cube/cubeserver/tests/rbac_api.rs` (novo; reutiliza o padrão de `tests/playground.rs`: `build_app` e `send_json` com `oneshot`)

**Interfaces:**
- Produces:
```rust
pub struct RbacConfig { pub spicedb_endpoint: String, pub preshared_key: String, pub db_url: String, pub admins: Vec<String>, pub admin_password: Option<String>, pub token_ttl: Duration }
impl RbacConfig { pub fn from_env() -> Result<Option<Self>, String> }   // None quando CUBE_RBAC está ausente; Err quando "spicedb" sem as outras variáveis
pub struct Rbac { pub authz: Arc<dyn Authz>, pub store: Store, pub config: RbacConfig }
impl Rbac {
    pub async fn start(config: RbacConfig) -> Result<Self, String>;     // conecta, ensure_schema(include_str!("../../rbac/schema.zed")), bootstrap dos admins
    pub async fn with_parts(authz: Arc<dyn Authz>, store: Store, config: RbacConfig) -> Result<Self, String>; // testes
    pub async fn is_platform_admin(&self, user: &str) -> Result<bool, ApiError>;
    pub async fn require(&self, resource: &ObjectRef, permission: &str, user: &str) -> Result<(), ApiError>; // 403 / 503
    pub async fn require_visible(&self, resource: &ObjectRef, view_permission: &str, permission: &str, user: &str) -> Result<(), ApiError>; // 404 sem view, 403 sem permission
}
pub fn hash_password(password: &str) -> Result<String, ApiError>;      // 400 se < 10 caracteres
pub fn verify_password(hash: &str, password: &str) -> bool;
/// Resolve o usuário do token: 401 sem `sub`, usuário inexistente/desativado, ou `iat` < password_changed_at.
pub async fn resolve_identity(rbac: &Rbac, security_context: &Value) -> Result<(String, bool /*platform admin*/), ApiError>;
```
- **Inicialização:** em `main.rs`, `RbacConfig::from_env()` e, depois, `Rbac::start`. Um `Err` ou um bloco `tenants:` em `cube.yml` faz `eprintln!` com a mensagem e `exit(1)`.
- **Middleware:** com o RBAC ligado, chama `resolve_identity` em toda rota autenticada **exceto** `POST /v1/auth/login`. Esta é montada fora do middleware, porque é pública.
- **Tokens:** `POST /v1/auth/login` e `POST /v1/users/{id}/tokens` emitem com `AuthService::issue_token`, com as claims `{"sub": id, "iat": now}` e a validade `token_ttl`. Se o `issue_token` atual não aceitar validade, acrescente um parâmetro.

- [ ] **Step 1:** Testes em `tests/rbac_api.rs`, usando `FakeAuthz` e `Store` (`CUBE_TEST_PG_URL`, como na Tarefa 3), com o admin `admin` e a senha `admin-password-1`:
  - `login_returns_a_token_that_authenticates`.
  - `wrong_password_and_unknown_user_get_the_same_answer`: 401 e o mesmo corpo `{"error":"Invalid username or password"}`.
  - `short_passwords_are_refused`: criar um usuário com senha de 9 caracteres dá 400.
  - `a_token_issued_before_a_password_change_stops_working`: login → `PUT /v1/auth/password` → o token antigo dá 401 e um novo login funciona. **(Review Focus 2)**
  - `a_disabled_user_is_locked_out_at_once`: login → admin faz `PATCH disabled:true` → o token dá 401 e o login dá 401. **(Review Focus 2)**
  - `a_token_without_sub_is_refused`.
  - `startup_refuses_tenants_with_rbac`: chama a função de validação extraída de `main.rs`, `check_rbac_compatible(&CubeConfig) -> Result<(), String>`.
- [ ] **Step 2:** Rode `cargo test -p cubeserver --test rbac_api`. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** Rode de novo. Esperado: PASS. Rode também `cargo test -p cubeserver`, que deve continuar como antes (só a falha antiga do `athena`), provando que o RBAC desligado não muda nada.
- [ ] **Step 5:** Commit: `feat(cubeserver): RBAC start-up, password login and token identity`.

### Task 5: Usuários (gestão pelo admin)

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/handlers/users.rs`
- Test: `tests/rbac_api.rs`

**Interfaces:**
- Consumes: `Rbac::is_platform_admin`, `Store::{create_user, users, update_user, set_password_hash}`, `hash_password`.
- Produces: rotas `POST/GET /v1/users`, `PATCH /v1/users/{id}`, `PUT /v1/users/{id}/password` e `POST /v1/users/{id}/tokens`, todas exigindo platform admin. Respostas nunca incluem `password_hash`.

- [ ] **Step 1:** Testes:
  - `only_platform_admins_manage_users`: um usuário comum recebe 403 em cada rota.
  - `user_listing_never_shows_password_hashes`.
  - `a_service_token_authenticates_as_its_user`.
  - `invalid_user_ids_are_refused`: `"Ana"` e `"a b"` dão 400.
- [ ] **Step 2:** Rode. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** Rode. Esperado: PASS.
- [ ] **Step 5:** Commit: `feat(cubeserver): user management endpoints`.

### Task 6: Organizations, owners, workspaces e membros

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/handlers/organizations.rs`, `rust/cube/cubeserver/src/rbac/handlers/workspaces.rs`
- Test: `tests/rbac_api.rs`

**Interfaces:**
- Consumes: `Rbac::{require, require_visible}`, `Store::transaction`, `Authz::{touch, delete, delete_matching, lookup_resources, read}`.
- Produces: as rotas de Organizations e Workspaces do spec, com estas regras.
  - **Criação:** criar uma organização grava `organization:<id>#platform@platform:main` e `#owner@user:<owner>`. Criar um workspace grava `workspace:<id>#organization@organization:<org>`.
  - **Membros:** `PUT /v1/workspaces/{id}/members/{user}` apaga os três papéis do usuário naquele workspace e grava o novo, além de `organization#member`. `DELETE` remove o papel e, se o usuário não tiver mais papel em nenhum workspace da organização (via `read`), remove `organization#member`.
  - **Ordem das escritas e exclusões:** a do spec, "Consistência entre Postgres e SpiceDB".

- [ ] **Step 1:** Testes, montando o cenário com o admin, `ana` (owner da organização A), `bia` e `caio`:
  - `org_owner_creates_workspaces_and_sees_them`.
  - `organizations_list_only_what_the_user_can_view`: `caio` sem papel nenhum recebe `[]`.
  - `a_member_of_one_workspace_does_not_see_the_others`: `bia` é viewer de W1. `GET /v1/organizations/A/workspaces` devolve só W1, e `GET /v1/workspaces/W2` dá 404.
  - `role_change_replaces_the_old_role`: `bia` passa de editor para viewer. `GET members` mostra só `viewer`, e o fake nega `edit`. **(Review Focus 3)**
  - `another_orgs_workspace_is_404`: `ana`, owner de A, pede um workspace de B e recebe 404. **(Review Focus 4)**
  - `the_last_org_owner_cannot_be_removed`: 400.
  - `deleting_an_org_with_workspaces_is_refused`: 400.
  - `a_failed_spicedb_write_leaves_no_row`: o fake em modo de falha; `POST workspaces` dá 503 e `GET` (com o fake de volta) não mostra o workspace.
- [ ] **Step 2:** Rode. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** Rode. Esperado: PASS.
- [ ] **Step 5:** Commit: `feat(cubeserver): organization and workspace endpoints with roles`.

### Task 7: Conexões da organização e liberação por workspace

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/handlers/connections.rs`
- Modify:
  - `rust/cube/cubeserver/src/handlers/data_sources.rs`: expor como `pub(crate)` as funções `create`, `update` e `delete`, que já recebem `State` e o JSON, para serem chamadas daqui;
  - com o RBAC ligado, `v1_*` passam a exigir platform admin.
- Test: `tests/rbac_api.rs`, que precisa de um `cube.yml` temporário como em `tests/playground.rs` (`with_config_file`) e de conexões SQLite.

**Interfaces:**
- Consumes: as funções de `data_sources.rs`, `Store` (tabela `connections`) e `Authz`.
- Produces:
  - as rotas de Conexões do spec;
  - `pub async fn released_connections(rbac: &Rbac, workspace: &str) -> Result<BTreeSet<String>, ApiError>`, que lê as relações `connection:*#workspace@workspace:<id>` e é usada na Tarefa 8.

- [ ] **Step 1:** Testes:
  - `org_owner_adds_a_connection_and_releases_it`: `POST /v1/organizations/A/connections` (SQLite) → `PUT /v1/workspaces/W1/connections/x` → `GET /v1/workspaces/W1/connections` mostra `x`.
  - `a_connection_cannot_be_released_to_another_orgs_workspace`: 404.
  - `workspace_members_cannot_manage_connections`: 403 para o owner do workspace.
  - `assigning_an_existing_data_source_to_an_org`: `PUT /v1/connections/default/organization`, como platform admin.
  - `data_sources_routes_are_admin_only_with_rbac`: `ana` recebe 403 em `GET /v1/data-sources`.
  - `another_orgs_connection_is_404`. **(Review Focus 4)**
- [ ] **Step 2:** Rode. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** Rode. Esperado: PASS.
- [ ] **Step 5:** Commit: `feat(cubeserver): organization connections released per workspace`.

### Task 8: Objetos e acesso aos dados

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/handlers/objects.rs`, `rust/cube/cubeserver/src/rbac/enforce.rs`
- Modify:
  - `rust/cube/cubeserver/src/planner_adapter.rs`: tornar `DataSources`, `DataSources::load` e `query_members` `pub(crate)`. Acrescentar `pub(crate) fn data_sources_of(&self, members: &[String]) -> BTreeSet<String>`, que devolve **todas** as fontes, com `default` explícito, sem o erro de "várias fontes".
  - `rust/cube/cubeserver/src/app.rs`: `query_for` e `meta_for` envolvem o serviço com o RBAC quando `rbac` é `Some`.
  - As rotas só de admin do spec (`/v1/cubesql`, `/v1/convert-query`, pré-agregações, `/cube-system/*`).
  - `rust/cube/cubeserver/src/handlers/data_sources.rs`: `/v1/execute` com as regras do spec.
  - `rust/cube/cubeserver/src/ws/*`: o workspace do parâmetro `workspace` da autenticação.
- Test: `tests/rbac_api.rs`. O planner real precisa de banco, então use `QueryService` falso que devolve `{"data":[]}` e grava o pedido, além de um modelo YAML temporário com o cubo `a_orders` (`data_source: a_sqlite`) e o cubo `b_orders` (`data_source: b_sqlite`).

**Interfaces:**
- Consumes: `released_connections` (Tarefa 7), `Rbac::require`, `DataSources`.
- Produces:
```rust
pub struct RbacQueryService { inner: QueryServiceRef, rbac: Arc<Rbac>, model: Arc<ModelIndex> }   // impl QueryService
pub struct RbacMetaService  { inner: MetaServiceRef,  rbac: Arc<Rbac>, model: Arc<ModelIndex> }   // impl MetaService: filtra "cubes" do JSON
pub struct ModelIndex { /* DataSources + mtime do diretório; recarrega quando o mtime do diretório do modelo muda */ }
impl ModelIndex { pub fn new(schema_path: PathBuf, context: TemplateContext) -> Self; pub fn data_sources_of(&self, members: &[String]) -> Result<BTreeSet<String>, ApiError>; pub fn data_sources_of_cube(&self, cube: &str) -> BTreeSet<String> }
```
- **Membros da consulta:** os de cada `NormalizedQuery`, lidos de `measures`, `dimensions`, `segments`, `time_dimensions` e `filters` (recursivo em `and` e `or`).
- **Regra de acesso:**
  - platform admin passa sempre;
  - os outros precisam de `X-Cube-Workspace` (sem ele, 400 `"X-Cube-Workspace is required"`);
  - precisam também de `workspace#query`, e de que cada fonte esteja nas `released_connections`;
  - senão, 403 nomeando o cubo.
- **Meta:**
  - com o workspace: mantém cubos e views cujas fontes estão todas liberadas;
  - sem o workspace: mantém cubos e views cujas fontes estão em `lookup_resources("connection", "query", user)`.

- [ ] **Step 1:** Testes de objetos:
  - `editor_creates_and_updates_objects_viewer_only_reads`.
  - `stale_version_is_409`.
  - `object_listing_filters_by_type_and_omits_content`.
  - `raw_content_with_accept_header`: um objeto XML com `Accept: application/xml` devolve o XML puro com `Content-Type` XML.
  - `base64_content_roundtrips_binary`.
  - `another_orgs_object_is_404`. **(Review Focus 4)**
- [ ] **Step 2:** Testes de dados, com W1 liberando só `a_sqlite`:
  - `a_query_on_a_released_connection_runs`.
  - `a_query_touching_an_unreleased_cube_is_403`: `b_orders.count` em `measures` dá 403.
  - `a_forbidden_member_hidden_in_a_nested_or_filter_is_403`: o membro proibido aparece só em `filters: [{or: [{and: [{member: "b_orders.status", ...}]}]}]`. **(Review Focus 1)**
  - `a_view_over_an_unreleased_cube_is_403`. **(Review Focus 1)**
  - `meta_shows_only_released_cubes`.
  - `data_requests_without_a_workspace_are_400_for_members_and_fine_for_admins`.
  - `graphql_goes_through_the_same_check`: `POST /cube/graphql` com uma consulta a `b_orders` dá erro de permissão.
  - `cubesql_is_admin_only`.
  - `execute_needs_workspace_owner_and_a_released_connection`.
  - `spicedb_down_means_503_not_data`: `set_failing(true)`; `/v1/load` dá 503, `/v1/auth/login` dá 503 e `GET /v1/objects/{id}` dá 503. **(Review Focus 5)**
- [ ] **Step 3:** Rode. Esperado: FAIL.
- [ ] **Step 4:** Implemente.
- [ ] **Step 5:** Rode. Esperado: PASS. Rode também `cargo test -p cubeserver` completo, que deve ter só a falha antiga do `athena`.
- [ ] **Step 6:** Commit: `feat(cubeserver): objects and per-workspace data access`.

### Task 9: Playground no servidor com o RBAC

**Files:**
- Modify:
  - `rust/cube/cubeserver/src/handlers/playground.rs`;
  - `rust/cube/cubeserver/src/main.rs`: montar o Playground com a autenticação obrigatória quando o RBAC estiver ligado;
  - `rust/cube/cubeserver/src/app.rs`: as rotas `/playground/*` passam por um middleware que exige platform admin quando o RBAC está ligado. O `context` fica público.
- Test: `tests/rbac_api.rs`

**Interfaces:**
- Produces:
  - `GET /playground/context` com o RBAC ligado: sem `cubejsToken`, com `"authRequired": true` e `"rbac": true`;
  - `POST /playground/token`: 404;
  - as outras rotas `/playground/*`: 401 sem token e 403 para quem não é platform admin.

- [ ] **Step 1:** Testes:
  - `playground_context_hands_out_no_token_with_rbac`.
  - `playground_token_minting_is_gone_with_rbac`.
  - `playground_helpers_need_a_platform_admin`.
  - `playground_unchanged_without_rbac`: o contexto ainda traz o token.
- [ ] **Step 2:** Rode. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** Rode. Esperado: PASS.
- [ ] **Step 5:** Commit: `feat(cubeserver): Playground behind RBAC login`.

### Task 10: Playground no frontend: login, workspace e menu

**Files:**
- Create:
  - `packages/cubejs-playground/src/pages/Login/LoginPage.tsx`;
  - `packages/cubejs-playground/src/components/Header/WorkspaceSelector.tsx`;
  - `packages/cubejs-playground/src/shared/session.ts`: `getSession()`, `setSession({token, user, platformAdmin})`, `clearSession()`, `getWorkspace()`, `setWorkspace(id | null)`, com `sessionStorage` envolto em try/catch.
- Modify:
  - `src/App.tsx`: com `authRequired`, sem sessão, mostra o `LoginPage`. Com sessão, usa `session.token` como token do contexto;
  - `src/hooks/cubejs-api.ts`: passa `headers: { 'X-Cube-Workspace': id }` ao `cubejs(...)` quando há workspace;
  - `src/shared/helpers.ts` (`playgroundFetch`) e o `request` de `DataSourcesPage.tsx`: enviam `Authorization` e `X-Cube-Workspace`; um 401 chama `clearSession()` e recarrega;
  - `src/components/Header/Header.tsx`: usuário, **Sair** e `WorkspaceSelector`; Data Model e Data Source só para `platformAdmin`;
  - `src/pages/FrontendIntegrations/ApiConsole.tsx`: envia `X-Cube-Workspace` e mostra o cabeçalho no `curl`;
  - o botão "Add Security Context": oculto quando `rbac`.
- Test: `npx tsc --noEmit -p packages/cubejs-playground` (0 erros), mais a verificação no navegador da Tarefa 12.

**Interfaces:**
- Consumes:
  - `POST /v1/auth/login`, que devolve `{token, expiresAt, user: {id, name, platformAdmin}}`. **A Tarefa 4 precisa incluir `platformAdmin` na resposta;**
  - `GET /v1/organizations` e `GET /v1/organizations/{id}/workspaces`.
- Produces: o seletor guarda o workspace em `sessionStorage` e a página recarrega os dados ao trocar de workspace. Para platform admins, a opção "Todos (admin)" equivale a `null`.

- [ ] **Step 1:** Implemente `session.ts` e `LoginPage.tsx`: formulário antd com usuário e senha; um erro mostra a mensagem do servidor.
- [ ] **Step 2:** Integre `App.tsx`, o cabeçalho e o seletor.
- [ ] **Step 3:** Faça os fetches e o cliente Cube levarem os cabeçalhos.
- [ ] **Step 4:** `npx tsc --noEmit -p packages/cubejs-playground`. Esperado: 0 erros. Confira também com `grep` que nenhum `'/cubejs-api'` foi introduzido, porque o `install-playground.sh` rejeita esse texto.
- [ ] **Step 5:** Commit: `feat(playground): login, workspace selector and role-based menu`.

### Task 11: Documentação

**Files:**
- Modify: `/root/cube-rust/README.md`, com uma seção "RBAC": variáveis, primeiro login do admin, fluxo de exemplo em `curl` (criar usuário, organização, workspace, conexão, liberar, consultar com `X-Cube-Workspace`) e o aviso de que a API SQL da porta 15432 fica fora do RBAC.

- [ ] **Step 1:** Escreva a seção.
- [ ] **Step 2:** Não há commit no repositório (o arquivo está fora dele).

### Task 12: Implantação e verificação de ponta a ponta

**Files:** `/root/cube-rust/.env` (com backup), deploy como em Global Constraints.

- [ ] **Step 1:** Copie os arquivos para `/root/cube`, compile (`cargo build --release -p cubeserver --bin cube-server`), compile o Playground (`node_modules/.bin/vite build`) e rode `./install-playground.sh`.
- [ ] **Step 2:** **Pergunte ao usuário antes de ligar o RBAC**, porque isso muda o acesso de todos. Com o ok, descomente `CUBE_RBAC=spicedb` e defina `NODE_ENV=production` no `.env`; o usuário roda os comandos via `!` se o classificador bloquear.
- [ ] **Step 3:** Faça `./start-all.sh restart`. Esperado: SpiceDB `✓`, Cube pronto e `/readyz` 200.
- [ ] **Step 4:** Fluxo em `curl`, gravando as saídas:
  1. login do admin;
  2. criar os usuários `ana` (owner da organização) e `bia`;
  3. criar a organização `Teste` com `ana` como owner;
  4. `ana` cria um workspace e dá a `bia` o papel de viewer;
  5. o admin atribui `sofiadb` à organização, e `ana` a libera para o workspace;
  6. `bia` consulta `dfos_fibercable.count` com `X-Cube-Workspace` (200); a mesma consulta sem o cabeçalho dá 400;
  7. `bia` consulta `orders.count` (de `default`, não liberada): 403;
  8. `bia` tenta criar um objeto: 403. `ana` cria um pipeline JSON e `bia` o lê: 200.
- [ ] **Step 5:** No navegador (`playwright-cli`):
  - o login de `bia` mostra só Playground e Frontend Integrations;
  - o seletor de workspace funciona;
  - o query builder lista só os cubos do SofiaDB;
  - o login do admin mostra tudo.
- [ ] **Step 6:** Limpe os dados de teste (objetos, workspace, organização, usuários) e confirme que `start-all.sh status` está saudável.
- [ ] **Step 7:** Faça push da branch e abra o PR contra `main`. A descrição explica a dependência do PR #7 e a falha antiga do teste `athena`.

# Árvore do Data Model por Organization — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A página Data Model mostra Organization → Cubes / Tables / Files, e a pasta `model/<slug>/` define a organização dona de cada cubo.

**Architecture:** O `cubemodel` ganha uma política de posse (dados puros: slug → data sources) que o loader aplica antes da validação, recusando cubos fora do lugar sem derrubar o modelo. O RBAC monta essa política a partir do Postgres e a instala no processo, e cada recompilação a usa. Um índice do modelo (cacheado) alimenta duas rotas de admin novas, `/playground/tree` e `/playground/tree/{slug}`. A tela troca as abas por uma árvore com carregamento sob demanda.

**Tech Stack:** Rust (axum, tokio-postgres, serde), React 17 + antd 4 + TypeScript, vitest; Python 3 (sync do SofiaDB).

**Spec:** `docs/superpowers/specs/2026-11-20-playground-org-tree-design.md`

**Repositório:** `/home/antonio-rodrigues/Documents/GitHub/cube`, numa branch nova `feat/playground-org-tree` criada a partir de `feat/datasource-owner-organization`. Rust em `rust/cube/`, Playground em `packages/cubejs-playground/`. Deploy em `/root/cube-rust`.

## Global Constraints

- Só platform admins alcançam a página e as rotas `/playground/*` com o RBAC ligado. As rotas novas entram no grupo `helpers` de `handlers/playground.rs`, atrás de `require_platform_admin` + `auth_middleware`.
- Slug: `[a-z0-9][a-z0-9-]{0,62}`, imutável, único. Reservados: `_platform` e qualquer valor que comece com `_` ou `.`. As organizações existentes recebem `ON` → `on` e `Digital` → `digital`.
- Pasta de um arquivo = primeiro segmento do caminho relativo a `model/`. `_platform/` = data sources sem organização.
- Com o RBAC desligado, não há política, e tudo compila como hoje.
- Um cubo recusado nunca impede os outros de compilar.
- A lista de organizações não consulta nenhum data source (contagens: cubos, arquivos e connections).
- Paginação de `/playground/tree`: `limit` padrão 50, máximo 200.
- Textos de erro (exatos):
  - `the folder <seg>/ does not belong to any organization`
  - `data source <ds> is not a connection of <slug>`
  - `data source <ds> belongs to an organization; move this file to its folder`
  - `<kind> <other> belongs to <other-owner>` (kind ∈ `joins`, `extends`, `includes`)
  - `depends on rejected cube <other>`

## Review Focus

1. **Arquivo direto na raiz de `model/`** (ex.: `model/orders.yml`) com o RBAC ligado: recusado com "does not belong to any organization", e não tratado como `_platform`. O teste está na Task 1.
2. **Join para um cubo de outra organização que também foi recusado:** a cascata termina (sem laço infinito) e cada cubo aparece uma vez só nas recusas. O teste está na Task 1.
3. **`path` malicioso em `/playground/tree/{slug}/files/{*path}`** (`../`, absoluto, `%2e%2e`, symlink saindo da pasta): 400 ou 404, nunca o conteúdo. O teste está na Task 3.
4. **Connection movida de organização enquanto o servidor roda:** os cubos dela passam a ser recusados na próxima recompilação, sem reiniciar. O teste está na Task 4.
5. **Nome com acento ou só símbolos** (`"Ação & Cia"`, `"!!!"`): slugs `acao-cia` e `org`, com sufixo em caso de colisão. O teste está na Task 2.

---

### Task 1: Política de posse no `cubemodel`

**Files:**
- Create: `rust/cube/cubemodel/src/ownership.rs`
- Modify: `rust/cube/cubemodel/src/lib.rs` (`pub mod ownership;` e re-export)
- Modify: `rust/cube/cubemodel/src/model.rs` (`DataModel` ganha `pub rejected: Vec<Rejection>`)
- Modify: `rust/cube/cubemodel/src/loader.rs` (`load_sources`: aplicar a política depois de `resolve_extends` e antes de `validate`)
- Test: `rust/cube/cubemodel/tests/ownership.rs`

**Interfaces:**
- Produces:
  ```rust
  pub const PLATFORM_FOLDER: &str = "_platform";
  #[derive(Debug, Clone, Default, PartialEq)]
  pub struct OwnershipPolicy {
      pub organizations: BTreeMap<String /*slug*/, BTreeSet<String> /*data sources*/>,
      pub platform: BTreeSet<String>, // data sources sem organização ("default" incluído se for o caso)
      pub generation: u64,            // muda a cada nova política
  }
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub enum Owner { Organization(String), Platform, Unowned(String /*primeiro segmento, ou "" na raiz*/) }
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct Rejection { pub file: String, pub cube: String, pub message: String }
  impl OwnershipPolicy {
      pub fn owner_of_file(&self, file_name: &str) -> Owner;
      pub fn owner_of_data_source(&self, ds: &str) -> Option<Owner>;
  }
  pub fn set_policy(policy: Option<Arc<OwnershipPolicy>>);   // global do processo (RwLock)
  pub fn current_policy() -> Option<Arc<OwnershipPolicy>>;
  pub fn apply(model: &mut DataModel, policy: &OwnershipPolicy) -> Vec<Rejection>;
  ```
- `ModelLoader::load_sources` chama `apply` quando `current_policy()` é `Some`. Põe as recusas em `model.rejected` e escreve cada uma com `tracing::warn!` (ou `eprintln!` se o crate não usa tracing).

- [ ] **Step 1: Escrever os testes que falham** em `tests/ownership.rs`. A política dos testes: `digital = {sofiadb, example}`, `on = {on_pg}`, `platform = {default}`. O modelo é montado com `ModelLoader::new().load_sources(&[("digital/a.yml", ...), ...])` depois de `set_policy(Some(..))`. Os testes compartilham o global, então use um `static LOCK: Mutex<()>`.
  - `owner_of_file`: `"digital/sofiadb/x.yml"` → `Organization("digital")`, `"_platform/u.yml"` → `Platform`, `"orders.yml"` → `Unowned("")`, `"zzz/a.yml"` → `Unowned("zzz")`.
  - `a_cube_on_its_orgs_connection_is_kept`: `digital/a.yml` com `data_source: sofiadb` fica em `model.cubes` e `rejected` vem vazio.
  - `a_cube_on_another_orgs_connection_is_rejected`: `on/b.yml` com `data_source: sofiadb` → `rejected == [Rejection{file:"on/b.yml", cube:"b", message:"data source sofiadb is not a connection of on"}]`, e `b` fica fora de `model.cubes`.
  - `no_data_source_means_default`: `_platform/u.yml` sem `data_source` fica. O mesmo cubo em `digital/u.yml` é recusado com `data source default is not a connection of digital`.
  - `an_org_data_source_in_platform_is_rejected`: `_platform/f.yml` com `sofiadb` → `data source sofiadb belongs to an organization; move this file to its folder`.
  - `root_and_unknown_folders_are_rejected`: `orders.yml` → `the folder / does not belong to any organization`; `zzz/a.yml` → `the folder zzz/ does not belong to any organization`.
  - `a_join_across_organizations_rejects_the_joining_cube`: `digital/a` faz join com `on/c` (os dois válidos sozinhos) → `a` recusado com `joins c belongs to on`, e `c` fica.
  - `a_view_including_another_orgs_cube_is_rejected`: uma view em `digital/` com `join_path: c` → `includes c belongs to on`.
  - `rejection_cascades_once`: `digital/a` faz join com `digital/b`; `b` é recusado (data source errado); `b` também faz join com `a`. Resultado: `a` com `depends on rejected cube b`, `b` com a mensagem dele, cada um uma vez só, e o teste termina.
  - `the_rest_of_the_model_still_compiles`: com um cubo recusado, `load_sources` devolve `Ok` e os outros cubos estão em `model.cubes`.
  - `no_policy_changes_nothing`: com `set_policy(None)`, `orders.yml` na raiz fica e `rejected` vem vazio.

- [ ] **Step 2: Rodar e ver falhar.**
  Run: `cd rust/cube && cargo test -p cubemodel --test ownership`
  Expected: erro de compilação (`ownership` não existe).

- [ ] **Step 3: Implementar `ownership.rs` e ligar no loader.**
  O `apply` primeiro recusa por dono do arquivo e data source. Depois, repetindo até não mudar mais, recusa os cubos e views que:
  - referenciam por `joins[].name`, `extends` ou qualquer segmento de `cubes[].join_path` um cubo de outro dono (mensagem `<kind> <other> belongs to <slug|_platform>`);
  - referenciam um cubo já recusado (mensagem `depends on rejected cube <other>`).

  Os recusados saem de `model.cubes` e de `model.views`. Ordene as recusas por `(file, cube)`.

- [ ] **Step 4: Rodar e ver passar**, com o resto do crate intacto.
  Run: `cd rust/cube && cargo test -p cubemodel`
  Expected: PASS em tudo.

- [ ] **Step 5: Commit.** `git commit -m "feat(cubemodel): folder ownership policy rejects misplaced cubes"`

---

### Task 2: Slug da organização

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/slug.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (`pub mod slug;`; em `Rbac::start`, depois das migrações, chamar `store.backfill_slugs()`)
- Modify: `rust/cube/cubeserver/src/rbac/migrations.sql` (`ALTER TABLE organizations ADD COLUMN IF NOT EXISTS slug text;` e `CREATE UNIQUE INDEX IF NOT EXISTS organizations_slug ON organizations (slug);`)
- Modify: `rust/cube/cubeserver/src/rbac/store.rs` (`Organization.slug: String`; `create_organization(c, name, slug, created_by)`; `org_from` e todos os `SELECT` de organização incluem `slug`; `backfill_slugs`; `organization_by_slug`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/organizations.rs` (`CreateOrganization.slug: Option<String>`; o JSON de organização ganha `"slug"`; `update` não aceita nem altera `slug`)
- Modify: `rust/cube/cubeserver/src/rbac/fake.rs`, se ele espelhar `Organization`
- Test: testes de unidade em `slug.rs`; `db_test!` em `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Produces:
  ```rust
  pub fn slugify(name: &str) -> String;                       // nunca vazio: "org" como reserva
  pub fn validate_slug(s: &str) -> Result<(), String>;        // formato e reservados
  pub fn unique_slug(base: &str, taken: &dyn Fn(&str) -> bool) -> String; // base, base-2, base-3…
  // Store
  pub async fn organization_by_slug(&self, slug: &str) -> Result<Option<Organization>, StoreError>;
  pub async fn backfill_slugs(&self) -> Result<usize, StoreError>; // preenche NULL e depois SET NOT NULL
  ```
- O JSON de organização (`json!` em `organizations.rs:46`) passa a ter `{"id","name","slug","createdAt","createdBy"}`.

- [ ] **Step 1: Testes que falham.**
  - Unidade `slugify`: `"Digital"` → `"digital"`, `"ON"` → `"on"`, `"Ação & Cia"` → `"acao-cia"`, `"  Foo__Bar  "` → `"foo-bar"`, `"!!!"` → `"org"`, um nome de 100 caracteres → 63 caracteres sem `-` no fim.
  - Unidade `validate_slug`: rejeita `"_platform"`, `"_x"`, `".x"`, `"A"`, `""` e `"-a"`; aceita `"on"` e `"digital-2"`.
  - Unidade `unique_slug("digital", taken={"digital","digital-2"})` → `"digital-3"`.
  - `db_test! organizations_get_a_slug`: `POST /organizations {"name":"Digital","owner":…}` → `slug == "digital"`. Um segundo `"Digital"` → `"digital-2"`. `{"name":"X","slug":"_platform"}` → 400. `{"slug":"on"}` com `on` já usado → 409.
  - `db_test! renaming_keeps_the_slug`: `PATCH {"name":"Outro"}` → `slug` continua `"digital"`.
  - `db_test! backfill_fills_existing_rows`: insere uma organização por SQL com `slug` NULL, chama `store.backfill_slugs()` → 1, e a linha recebe o slug.

- [ ] **Step 2: Rodar e ver falhar.**
  Run: `cd rust/cube && cargo test -p cubeserver rbac::slug && CUBE_TEST_PG_URL=postgres://postgres:postgres@127.0.0.1:55432/postgres cargo test -p cubeserver --test rbac_api slug`
  Expected: erro de compilação, depois FAIL.
  (Confira a URL do Postgres de teste em `.env`/`cube-pg-test`; o banco de cada teste é descartável.)

- [ ] **Step 3: Implementar.** Acentos são removidos com decomposição NFD (crate `unicode-normalization`, se ainda não for dependência; senão, uma tabela pequena para o latim). `backfill_slugs` roda numa transação e ordena por `created_at`, para que a organização mais antiga fique com o slug sem sufixo.

- [ ] **Step 4: Rodar e ver passar** (os dois comandos do Step 2, mais `cargo test -p cubeserver --test rbac_api`).
  Expected: PASS em tudo.

- [ ] **Step 5: Commit.** `git commit -m "feat(rbac): immutable organization slug"`

---

### Task 3: Índice do modelo e as rotas da árvore

**Files:**
- Create: `rust/cube/cubeserver/src/model_index.rs`
- Create: `rust/cube/cubeserver/src/handlers/tree.rs`
- Modify: `rust/cube/cubeserver/src/lib.rs` (`pub mod model_index;`), `src/handlers/mod.rs` (`pub mod tree;`)
- Modify: `rust/cube/cubeserver/src/handlers/playground.rs` (registrar no `helpers`: `/playground/tree`, `/playground/tree/:slug`, `/playground/tree/:slug/files/*path`)
- Modify: `rust/cube/cubeserver/src/app.rs` (`AppState.model_index: Arc<ModelIndexCache>`, criado nos construtores de estado e nos helpers de teste)
- Test: `rust/cube/cubeserver/tests/playground.rs` (sem RBAC) e `tests/rbac_api.rs` (`db_test!`)

**Interfaces:**
- Consumes: `cubemodel::ownership::{current_policy, Owner, Rejection, OwnershipPolicy::owner_of_file}` (Task 1); `Store::organization_by_slug`, `Organization.slug` (Task 2); `Store::search_organizations` e `all_connections` (já existem).
- Produces:
  ```rust
  pub struct IndexedCube { pub name: String, pub title: Option<String>, pub kind: &'static str /*"cube"|"view"*/,
                           pub data_source: String, pub file: String, pub owner: Owner,
                           pub measures: usize, pub dimensions: usize }
  pub struct IndexedFile { pub file_name: String, pub owner: Owner }
  pub struct ModelIndex { pub cubes: Vec<IndexedCube>, pub files: Vec<IndexedFile>, pub rejected: Vec<(Owner, Rejection)> }
  impl ModelIndex { pub fn load(model_path: &Path) -> Result<ModelIndex, ModelError>; }
  pub struct ModelIndexCache { /* Mutex<Option<((Option<SystemTime>, u64), Arc<ModelIndex>)>> */ }
  impl ModelIndexCache { pub fn get(&self, model_path: &Path) -> Result<Arc<ModelIndex>, ModelError>; }
  // chave do cache: (newest_mtime(model_path), current_policy().map(|p| p.generation).unwrap_or(0))
  ```
- Sem política (RBAC desligado), todo cubo e arquivo tem `owner = Unowned("")`. A árvore mostra então um nó só, `{"slug":"_model","name":"Model"}`, e `/playground/tree/_model` devolve tudo.
- Respostas: exatamente os formatos JSON do spec (seção "Rotas novas"), com `counts: {cubes, files, connections}` e `errors: <n>` na lista, e `errors: [{file,cube,message}]` no detalhe. `/playground/tree` inclui `_platform` (`"name":"Platform"`) depois das organizações. O `total` conta as organizações sem o `_platform`.
- Arquivo: `GET /playground/tree/{slug}/files/{*path}` devolve `{"fileName","content","absPath"}`.

- [ ] **Step 1: Testes que falham.**
  - `playground.rs`, sem RBAC: `tree_without_rbac_is_one_model_node`: o fixture com 2 arquivos dá `organizations == [{slug:"_model", ...}]`, e `/playground/tree/_model` lista os 2 arquivos e os cubos deles.
  - `rbac_api.rs`, `db_test! tree_is_for_platform_admins`: um usuário comum leva 403 em `/playground/tree` e `/playground/tree/digital`.
  - `db_test! tree_groups_by_folder`: um tempdir com `digital/a.yml` (`data_source: sofiadb`), `on/b.yml` (`data_source: sofiadb`, recusado) e `_platform/u.yml`. As organizações Digital (dona de `sofiadb`) e ON são criadas e a política é instalada pela função da Task 4. Se a Task 4 ainda não existir, monte a `OwnershipPolicy` à mão e chame `set_policy`. O admin vê:
    - em `/playground/tree`: `digital` com `counts.cubes == 1` e `errors == 0`, `on` com `counts.cubes == 0` e `errors == 1`, e `_platform`;
    - em `/playground/tree/on`: `errors[0].message == "data source sofiadb is not a connection of on"`, e em `files` o `on/b.yml`.
  - `db_test! tree_search_and_paging`: com 3 organizações, `?q=dig` → só `digital`, e `?limit=1&offset=1` → 1 item, com `total == 3`.
  - `db_test! tree_unknown_slug_is_404`.
  - `db_test! file_paths_cannot_escape` (Review Focus 3): `files/../on/b.yml`, `files/%2e%2e/on/b.yml`, `files//etc/passwd` e um symlink `digital/link.yml -> ../on/b.yml` → status 400 ou 404, e o corpo não contém o YAML de `b`.

- [ ] **Step 2: Rodar e ver falhar.**
  Run: `cd rust/cube && cargo test -p cubeserver --test playground tree && CUBE_TEST_PG_URL=… cargo test -p cubeserver --test rbac_api tree`
  Expected: FAIL (404 nas rotas).

- [ ] **Step 3: Implementar.** `ModelIndex::load`:
  - usa `ModelLoader::new().without_validation()`, para que uma validação falha não esconda o índice; as recusas da Task 1 aparecem mesmo assim;
  - percorre os arquivos com a mesma regra de `collect_model_files` (`playground.rs`).

  `tables` no detalhe: reaproveite `db_schema`/`data_source_schema` de `playground.rs` só para as connections da organização (para `_platform`, `policy.platform`). A falha de uma connection vai em `errors` com `file: ""` e `cube: ""`. O caminho do arquivo é resolvido com `canonicalize` e precisa começar com `canonicalize(model/<slug>)`.

- [ ] **Step 4: Rodar e ver passar** (os comandos do Step 2, mais `cargo test -p cubeserver`).
  Expected: PASS.

- [ ] **Step 5: Commit.** `git commit -m "feat(playground): organization tree routes"`

---

### Task 4: Montar e instalar a política a partir do RBAC

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/ownership.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (`pub mod ownership;`)
- Modify: `rust/cube/cubeserver/src/main.rs` (depois de `start_rbac`, linha ~160, e **antes** de `ModelMetaService::load`, linha ~183: montar e instalar a política)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/connections.rs` (`assign_organization`, `unassign_organization`) e `organizations.rs` (`create`, `delete`): depois do commit, chamar `refresh_policy`
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Consumes: `Store::all_connections`, as organizações com slug (Task 2), `cubemodel::ownership::set_policy` (Task 1), `TenantRegistry::reload_all` (`tenants.rs:285`).
- Produces:
  ```rust
  pub async fn build_policy(store: &Store, data_source_names: &[String]) -> Result<OwnershipPolicy, StoreError>;
  // platform = data_source_names que não aparecem em connections; generation = um contador global incrementado
  pub async fn refresh_policy(state: &AppState) -> Result<Vec<Rejection>, ApiError>;
  // monta, instala, chama state.tenants.reload_all() (quando houver registry) e devolve as recusas de ModelIndex
  ```
- As respostas de `PUT /connections/{name}/organization` e `DELETE /connections/{name}/organization` continuam 204. Se a política nova recusar cubos daquela connection, escreve `tracing::warn!` com a lista de arquivos e o destino (`move <file> to <slug>/`). O spec pede "a resposta e o log"; com o 204 sem corpo, o aviso aparece na árvore (no `errors` da organização) e no log.
- `data_source_names` vem de `cube_config.data_sources.keys()` (já carregado em `main.rs`) e fica guardado no `AppState`/`Rbac` para o refresh.
- Se a política não puder ser montada no boot (Postgres fora), o servidor recusa subir, como o RBAC já faz (fail-closed).

- [ ] **Step 1: Testes que falham.**
  - `db_test! policy_follows_connections`: Digital com a connection `sofiadb` e o data source `default` em `data_source_names` dão `organizations["digital"] == {"sofiadb"}`, `organizations["on"] == {}` e `platform == {"default"}`.
  - `db_test! moving_a_connection_rejects_its_cubes_without_restart` (Review Focus 4): com a política instalada e `digital/a.yml` em `sofiadb`, `PUT /connections/sofiadb/organization {"organizationId": <ON>}` → 204. Depois disso, `/playground/tree/digital` traz `errors[0].message == "data source sofiadb is not a connection of digital"`, e `/cube/v1/meta` (como admin) não tem `a`.

- [ ] **Step 2: Rodar e ver falhar.**
  Run: `cd rust/cube && CUBE_TEST_PG_URL=… cargo test -p cubeserver --test rbac_api policy moving_a_connection`
  Expected: FAIL.

- [ ] **Step 3: Implementar.** No `main.rs`, `set_policy(Some(..))` tem que acontecer antes da primeira carga do modelo (meta, planner e SQL API carregam cada um o seu), para que todos vejam a mesma posse. Confira que o SQL API, que carrega por `ModelSource::dir`, também passa pelo `ModelLoader` e portanto pela política.

- [ ] **Step 4: Rodar e ver passar** (`cargo test -p cubeserver`, com e sem `CUBE_TEST_PG_URL`).
  Expected: PASS.

- [ ] **Step 5: Commit.** `git commit -m "feat(rbac): install the folder ownership policy and refresh it when connections move"`

---

### Task 5: `files` e `generate-schema` por organização

**Files:**
- Modify: `rust/cube/cubeserver/src/handlers/playground.rs` (`files`: cada item ganha `"organization"`)
- Modify: `rust/cube/cubeserver/src/handlers/scaffold.rs` (pasta de destino por organização; 400 com mistura)
- Test: `tests/playground.rs`, `tests/rbac_api.rs`

**Interfaces:**
- Consumes: `OwnershipPolicy::owner_of_file` e `owner_of_data_source` (Task 1).
- Produces:
  - `files[].organization`: o slug, `"_platform"` ou `null`.
  - `generate-schema` grava em `model/<slug>/<nome>.yml` ou `model/_platform/<nome>.yml`, criando a pasta se não existir. Tabelas de mais de um dono dão 400 `{"error":"The tables belong to more than one organization"}`. Sem política, grava na raiz como hoje.

- [ ] **Step 1: Testes que falham.**
  - `playground.rs`: `files_carry_null_organization_without_rbac`.
  - `db_test! generated_cubes_go_to_the_owners_folder`: com uma connection SQLite/DuckDB de teste da Digital (use o fixture de connection que `connections_scenario` já monta, `rbac_api.rs:1704`), `POST /playground/generate-schema` cria `digital/<cubo>.yml` e não cria nada na raiz.
  - `db_test! generate_refuses_mixed_owners`: tabelas de `default` e da Digital na mesma chamada → 400 com a mensagem acima.

- [ ] **Step 2: Rodar e ver falhar.** `cargo test -p cubeserver --test playground files_carry && CUBE_TEST_PG_URL=… cargo test -p cubeserver --test rbac_api generate`

- [ ] **Step 3: Implementar.**

- [ ] **Step 4: Rodar e ver passar** (`cargo test -p cubeserver`).

- [ ] **Step 5: Commit.** `git commit -m "feat(playground): files and generated cubes follow the organization folder"`

---

### Task 6: Lógica da árvore no Playground (módulo puro)

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/tree.ts`
- Test: `packages/cubejs-playground/src/__tests__/schema-tree.test.ts`

**Interfaces:**
- Consumes: os JSON das rotas da Task 3.
- Produces:
  ```ts
  export type OrgSummary = { id?: string; slug: string; name: string; counts: { cubes: number; files: number; connections: number }; errors: number };
  export type OrgDetail = { cubes: TreeCube[]; tables: Record<string, Record<string, Record<string, { name: string; type: string }[]>>>; files: { fileName: string }[]; errors: { file: string; cube: string; message: string }[] };
  export type TreeCube = { name: string; title?: string; type: 'cube' | 'view'; dataSource: string; file: string; members: { measures: number; dimensions: number } };
  export type NodeKey = string; // "org:<slug>" | "org:<slug>:cubes" | "cube:<slug>:<name>" | "tables:<slug>" | "table:<slug>:<ds>.<schema>.<table>" | "files:<slug>" | "file:<slug>:<fileName>"
  export function orgNodes(orgs: OrgSummary[]): DataNode[];          // nós antd sem filhos, isLeaf=false
  export function detailNodes(slug: string, d: OrgDetail): DataNode[]; // [Cubes, Tables, Files], com as contagens no título
  export function filterTree(nodes: DataNode[], term: string): DataNode[]; // mantém os ancestrais de quem casa, sem distinguir maiúsculas
  export function parseKey(key: NodeKey): { kind: 'org'|'cubes'|'cube'|'tables'|'table'|'files'|'file'; slug: string; rest?: string };
  export function buildQueryUrl(cube: { name: string; measures: { name: string }[]; dimensions: { name: string; primaryKey?: boolean }[] }): string;
  // "/build?query=" + encodeURIComponent(JSON.stringify({measures:[todas]})) ou, sem measure, {dimensions:[primeira que não é primaryKey, senão a primeira]}
  export function tablesFromSameOrg(checked: NodeKey[]): string | null; // o slug comum, ou null se houver mistura
  ```

- [ ] **Step 1: Testes que falham** (vitest):
  - `detailNodes` devolve 3 filhos com títulos `Cubes (1)`, `Tables (…)` e `Files (2)`, nessa ordem.
  - `filterTree(nodes, "FIB")` mantém `org:digital → cubes → cube:digital:dfos_fibercable` e tira o resto.
  - `parseKey("file:digital:digital/sofiadb/a.yml")` dá `{kind:"file", slug:"digital", rest:"digital/sofiadb/a.yml"}` (o caminho contém `:`? Ele é separado só nos dois primeiros `:`).
  - `buildQueryUrl`: um cubo com 2 measures dá `/build?query=` + o encode de `{"measures":["c.a","c.b"]}`; um cubo sem measures dá a primeira dimension que não é primary key.
  - `tablesFromSameOrg(["table:on:x.s.t","table:digital:y.s.t"])` → `null`.

- [ ] **Step 2: Rodar e ver falhar.** `cd packages/cubejs-playground && yarn unit src/__tests__/schema-tree.test.ts` → FAIL (módulo não existe).

- [ ] **Step 3: Implementar.**

- [ ] **Step 4: Rodar e ver passar.** O mesmo comando → PASS.

- [ ] **Step 5: Commit.** `git commit -m "feat(playground): pure helpers for the organization tree"`

---

### Task 7: `SchemaPage` com a árvore

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Schema/SchemaPage.tsx`
- Create: `packages/cubejs-playground/src/pages/Schema/CubeDetail.tsx` (painel do cubo)

**Interfaces:**
- Consumes: Task 6; `playgroundFetch` (`src/shared/helpers`); `/cube/v1/meta` pelo mesmo caminho que o Explore usa para o meta (o cliente do `AppContext`), para pegar measures, dimensions, segments e joins do cubo.
- Comportamento (do spec, seção "Playground (tela)"):
  - `Tree` com `loadData`: o nível 1 vem de `/playground/tree` (ao entrar, e de novo ao buscar com 2+ caracteres, com debounce de 300 ms) e o nível 2 de `/playground/tree/{slug}`.
  - Paginação: um nó "Carregar mais…" no fim do nível 1 quando `organizations.length < total`.
  - Clique, conforme o nó:
    - **cubo:** `CubeDetail`, com título, descrição, data source, arquivo, tabelas de measures (nome, tipo, título), dimensions (nome, tipo, PK) e segments, os joins, e o botão **Abrir no Playground** (`history.push(buildQueryUrl(...))`). Se o cubo não está no meta, mostra a recusa e desabilita o botão.
    - **arquivo:** `PrismCode` com o conteúdo vindo de `/playground/tree/{slug}/files/{path}`, mais o `Alert` com `absPath` (como hoje).
    - **organização com `errors > 0`:** a lista de erros num `Alert type="error"`.
  - Checkboxes só nos nós `table:*`. **Generate Data Model** fica desabilitado, com a dica "Escolha tabelas de uma única organização", quando `tablesFromSameOrg` dá `null`. Ao gerar, recarrega o detalhe daquela organização.
  - Com o RBAC desligado, o nó único `_model` vem da própria rota; não há caso especial na tela.

- [ ] **Step 1: Verificação de tipos e lint.** `cd packages/cubejs-playground && yarn tsc --noEmit && yarn lint` → sem erros novos.
- [ ] **Step 2: Testes existentes.** `yarn unit` → PASS (os snapshots de `access-layout`/`admin-sections` continuam).
- [ ] **Step 3: Build.** `yarn build:playground` → `build/` gerado sem erros.
- [ ] **Step 4: Verificação manual** com o servidor local da Task 8 ou com `cargo run`, logado como admin: abrir Digital (Cubes, Tables, Files), clicar num cubo, **Abrir no Playground**, e conferir que o construtor já vem com as measures.
- [ ] **Step 5: Commit.** `git commit -m "feat(playground): Data Model page as an organization tree"`

---

### Task 8: Migração no deploy, sync do SofiaDB e entrega

**Files (deploy `/root/cube-rust`, fora do git):**
- Modify: `/root/cube-rust/sync-sofiadb-model.py` (`OUT_DIR` dinâmico)
- Modify: `/root/cube-rust/README.md` (seções "SofiaDB tables", "The data model", RBAC: slug, pastas, recusas, árvore)
- Move: os arquivos de `/root/cube-rust/model/`

**Interfaces:**
- Consumes: `organizations.slug` e `connections` no Postgres `cube_app` (`docker exec cube-pg-test psql -U postgres -d cube_app`), mais o binário e o build das Tasks 1–7.
- `sync-sofiadb-model.py`:
  - nova função `owner_folder(connection="sofiadb") -> str`, que devolve o slug da organização dona ou `"_platform"`, lendo `CUBE_APP_DB_URL` do `.env` (via `psql`, como o script já faz chamadas externas, ou `psycopg` se já estiver disponível);
  - `OUT_DIR = os.path.join(MODEL_DIR, owner_folder(), "sofiadb")`;
  - a regra de "coberta à mão" (linha ~210) continua percorrendo `MODEL_DIR` inteiro e excluindo só `OUT_DIR`;
  - se o RBAC estiver desligado (`CUBE_RBAC` vazio), mantém `model/sofiadb/`.

- [ ] **Step 1: Backup.** `cp -a /root/cube-rust/model /root/cube-rust/model.bak-20261120-org` e `cp /root/cube-rust/sync-sofiadb-model.py /root/cube-rust/sync-sofiadb-model.py.bak-20261120-org`.
- [ ] **Step 2: Conferir o data source de cada arquivo** antes de mover: `grep -L data_source /root/cube-rust/model/*.yml` (os sem `data_source`, ou seja `default`, vão para `_platform/`) e `grep -l "data_source: *\(sofiadb\|example\)" /root/cube-rust/model/*.yml` (vão para `digital/`). Um arquivo que misture os dois tipos é dividido em dois, um por pasta.
- [ ] **Step 3: Build e troca do binário.** `cd /home/antonio-rodrigues/Documents/GitHub/cube/rust/cube && cargo build --release -p cubeserver`. Depois `cp /root/cube/rust/cube/target/release/cube-server /root/cube-rust/cube-server.bak-20261120-orgtree` e, por fim, copiar o binário novo para `/root/cube/rust/cube/target/release/cube-server` (o caminho que `start-all.sh` usa em `BINARY`).
- [ ] **Step 4: Playground.** `cd /root/cube-rust && ./install-playground.sh /home/antonio-rodrigues/Documents/GitHub/cube/packages/cubejs-playground/build`.
- [ ] **Step 5: Mover os arquivos e ajustar o sync.** `mkdir -p model/digital model/_platform model/on`. Mover conforme o Step 2, e `model/sofiadb` → `model/digital/sofiadb`. Aplicar a mudança do `sync-sofiadb-model.py`.
- [ ] **Step 6: Reiniciar e verificar.** `./start-all.sh restart`. Verificações:
  - `curl -s localhost:4000/readyz` → `{"health":"HEALTH"}`;
  - `./start-all.sh sync` → "no change";
  - `grep -i "rejected\|does not belong\|is not a connection" cube.log | tail` → vazio;
  - `/cube/v1/meta` como admin traz o mesmo conjunto de cubos de antes (compare os nomes com o backup: `diff <(antes) <(depois)`);
  - `/playground/tree` como admin → `digital`, `on` e `_platform` com as contagens esperadas.
- [ ] **Step 7: README do deploy** atualizado (pastas `model/<slug>/`, `_platform`, regras de recusa, onde ver erros, árvore do Data Model).
- [ ] **Step 8: Commit do repositório** (spec e plano já incluídos), se ainda faltar algo. O deploy não está em git.

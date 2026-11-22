# Modelo no RBAC e Data Model para donos — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Os YAML do modelo passam a morar só no `cube_app` (importados uma vez de `model/`). O platform admin e os donos de organização geram, editam, apagam (e o admin move) arquivos pela página Data Model, e nenhuma escrita é gravada sem que o modelo inteiro compile.

**Architecture:** As tabelas novas `model_files` e `model_state` guardam o modelo. A cada versão, o servidor materializa um retrato do banco num diretório privado (`<cache>/v<N>/`) e troca de forma atômica o link `<cache>/live`. `config.schema_path` e `cube_config.model_path` apontam para `live`, então meta, planner, tenants, árvore, gerador e API SQL seguem lendo uma pasta sem mudar o núcleo. Toda escrita passa por um `ModelWriter`: trava `model_state` na transação, compila em memória as linhas mais a mudança (`load_sources_with_policy`), recusa só problemas novos, grava, sobe a versão e materializa. As rotas ficam sob `/cube/v1`, e a permissão é checada por `visible_organization(..., "admin", ...)`.

**Tech Stack:** Rust (axum, tokio-postgres/deadpool, cubemodel), SpiceDB, React 18 + antd 4.16 + vitest, Python 3 (sync), bash (`start-all.sh`).

**Spec:** `docs/superpowers/specs/2026-11-22-data-model-donos-design.md`

## Global Constraints

- Com o RBAC desligado, nada muda: o modelo continua em `model/` e o disco continua editável.
- Com o RBAC ligado, **nenhum** componente lê `model/` depois da importação; só `<cache>/live`.
- O diretório de cache é `CUBE_MODEL_CACHE_DIR`, ou, se não estiver definido, `std::env::temp_dir()/cube-model-<PORT>`, criado com modo `0700`. Nunca é `model/`.
- Caminho de arquivo (`path`, relativo à organização):
  - sem `..`, sem barra no começo ou no fim, sem componente vazio ou `\`;
  - segmentos `[A-Za-z0-9._-]+`;
  - até 255 caracteres;
  - extensão `.yml`, `.yaml`, `.jinja` ou `.j2` (sem distinguir maiúsculas).
  - Só `.yml`/`.yaml` são editáveis.
- Fonte compilada de cada linha: `<slug>/<path>`, ou `_platform/<path>` quando `organization_id IS NULL`.
- `managed_by = 'sofiadb-sync'`: só a rota de sync escreve. `PUT`/`DELETE` → 400 com a mensagem do spec ("Este arquivo é gerado a partir do catálogo do SofiaDB…").
- Erros de versão → 409 "O arquivo mudou desde que você abriu; abra de novo".
- Nome de cubo já usado em outra organização (para quem não é admin) → 400 "O nome de cubo X já está em uso na plataforma; escolha outro", sem caminho.
- Erros antigos de outras organizações (para quem não é admin) → uma linha: "Há um problema no modelo fora da sua organização; avise o administrador".
- Apagar organização com arquivos → 400 "Mova ou apague os arquivos do modelo antes".
- Recarga por versão: intervalo `CUBEJS_MODEL_RELOAD_INTERVAL`, ou 5 s se não estiver definido; sempre ligada com RBAC.
- Usuário do sync: `CUBE_SYNC_USER` (padrão `sofiadb-sync`), token em `CUBE_SYNC_TOKEN`. As rotas de sync aceitam só esse usuário.
- Textos da interface em português; os de log e código em inglês, como no resto do projeto.
- Testes que instalam a política de posse global ficam em binários próprios (`tests/org_tree.rs`, `tests/model_api.rs`), serializados por um `Mutex` estático, como `org_tree.rs` já faz.
- Testes de banco usam `CUBE_TEST_PG_URL` e o `TestDb` de `tests/support/mod.rs`.

## Review Focus

1. **Importação interrompida** (o servidor morre entre o `COMMIT` e o `rename` da pasta). A próxima partida não importa de novo, porque o banco já tem linhas; só avisa que `model/` existe e não é usada. → teste `import_is_not_repeated_when_the_folder_survives` (Task 3).
2. **Arquivo com o mesmo nome em duas organizações** (`fatos.yml` na Digital e na ON). São linhas distintas, e as fontes não colidem (`digital/fatos.yml`, `on/fatos.yml`). → teste `same_path_in_two_organizations` (Task 1) e `sources_prefix_each_owner` (Task 2).
3. **YAML válido que cria um cubo com nome de outra organização.** A compilação falha por nome duplicado. Para o dono, a mensagem é a genérica, sem o caminho do outro arquivo. → teste `owner_sees_no_foreign_path_on_duplicate` (Task 11).
4. **Escrita enquanto o modelo atual já não compila.** Só o problema novo bloqueia. → teste `old_errors_do_not_block_a_write` (Task 5).
5. **Materialização concorrente com uma recarga.** O link `live` nunca aponta para um diretório incompleto. → teste `live_switches_only_to_complete_versions` (Task 2).

---

## Etapa 1 — o modelo no banco (admin)

### Task 1: Tabelas e consultas do modelo

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/migrations.sql` (append)
- Create: `rust/cube/cubeserver/src/rbac/model_store.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (`pub mod model_store;`)
- Modify: `rust/cube/cubeserver/src/rbac/store.rs` (`conn()` passa a `pub(crate)`; `delete_organization` recusa com arquivos)
- Test: `rust/cube/cubeserver/tests/model_store.rs` (binário novo)

**Interfaces:**
- Produces (in `rbac::model_store`):
  - `pub struct ModelFile { pub id: Uuid, pub organization_id: Option<Uuid>, pub path: String, pub content: String, pub version: i32, pub managed_by: Option<String>, pub created_at: DateTime<Utc>, pub created_by: String, pub updated_at: DateTime<Utc>, pub updated_by: String }`
  - `pub struct ModelRows { pub version: i64, pub files: Vec<ModelFile> }`
  - `pub enum ModelChange { Put { id: Option<Uuid>, organization_id: Option<Uuid>, path: String, content: String, expected_version: Option<i32>, managed_by: Option<String> }, Delete { id: Uuid, expected_version: Option<i32> }, Move { id: Uuid, organization_id: Option<Uuid> } }` (`Put` com `id: None` cria.)
  - `pub struct WriteOutcome { pub version: i64, pub changed: bool }`
  - `impl Store`:
    - `pub async fn model_rows(&self) -> Result<ModelRows, StoreError>`
    - `pub async fn model_version(&self) -> Result<i64, StoreError>`
    - `pub async fn model_file(&self, id: Uuid) -> Result<Option<ModelFile>, StoreError>`
    - `pub async fn model_is_empty(&self) -> Result<bool, StoreError>`
    - `pub async fn model_write<F, E>(&self, actor: &str, plan: F) -> Result<Result<WriteOutcome, E>, StoreError> where F: FnOnce(&ModelRows) -> Result<Vec<ModelChange>, E>`

    `model_write` faz, numa transação:
    1. `SELECT version FROM model_state FOR UPDATE`;
    2. lê as linhas;
    3. chama `plan`; com `Err`, faz `ROLLBACK` e devolve `Ok(Err(e))`;
    4. aplica as mudanças: `UPDATE … WHERE id=$1 AND version=$2`, e 0 linhas → `StoreError::Conflict("stale")`;
    5. se houve mudança, sobe `model_state.version` e faz o `COMMIT`. Uma lista vazia não sobe a versão (`changed: false`).
- `StoreError::Conflict("stale")` é o que as rotas mapeiam para 409. Um caminho repetido (índice único) → `StoreError::Conflict("path")`.

- [ ] **Step 1: Write the failing tests** em `tests/model_store.rs`:
  - `empty_model_has_version_zero`: `model_rows()` → `version == 0`, `files` vazio; `model_is_empty()` → `true`.
  - `write_creates_and_bumps_version`: um `Put` novo com `organization_id = Some(org)` → `WriteOutcome { version: 1, changed: true }`; `model_rows().files[0].path == "fatos.yml"`, `version == 1`, `created_by == actor`.
  - `stale_version_is_a_conflict`: `Put { id: Some(f), expected_version: Some(1) }` duas vezes → a segunda dá `Err(StoreError::Conflict(m))` com `m == "stale"`, e o conteúdo é o da primeira.
  - `plan_error_rolls_back`: `plan` devolve `Err("no")` → `Ok(Err("no"))` e `model_version() == 0`.
  - `empty_plan_does_not_bump`: `plan` devolve `vec![]` → `changed: false`, versão igual.
  - `same_path_in_two_organizations`: `fatos.yml` na org A e na org B → as duas linhas existem; `fatos.yml` de novo em A → `Conflict("path")`; `fatos.yml` em Platform (`None`) duas vezes → `Conflict("path")`.
  - `move_changes_owner`: `Move { organization_id: None }` → a linha fica com `organization_id == None` e `version` sobe 1.
  - `organization_with_files_cannot_be_deleted`: `delete_organization(org)` com um arquivo → `Err(StoreError::Conflict(_))`.
- [ ] **Step 2: Run** `cd rust/cube && CUBE_TEST_PG_URL=… cargo test -p cubeserver --test model_store`. Expected: não compila (`model_store` não existe).
- [ ] **Step 3: Implement** as tabelas, exatamente como no spec (`model_files`, `model_files_path_idx` com `coalesce`, `model_state` com uma linha). Adicionar `INSERT INTO model_state (id, version) VALUES (true, 0) ON CONFLICT DO NOTHING;`. Implementar também as funções acima e a checagem em `delete_organization`. Concorrência entre processos: só o `FOR UPDATE`; nada de advisory lock extra.
- [ ] **Step 4: Run** o mesmo comando. Expected: 8 passed.
- [ ] **Step 5: Commit** `feat(rbac): model files and model version in the RBAC store`.

### Task 2: Retrato do modelo e materialização

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/model_snapshot.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs`
- Test: unit tests no próprio arquivo (`#[cfg(test)]`, sem banco)

**Interfaces:**
- Consumes: `ModelRows`, `ModelFile` (Task 1).
- Produces:
  - `pub fn validate_path(path: &str) -> Result<(), String>`, com as regras de *Global Constraints*.
  - `pub fn is_editable(path: &str, managed_by: Option<&str>) -> bool`: `.yml`/`.yaml` e `managed_by` vazio.
  - `pub fn source_path(file: &ModelFile, slugs: &HashMap<Uuid, String>) -> Option<String>`: `None` se a organização não tem slug no mapa.
  - `pub fn sources(rows: &ModelRows, slugs: &HashMap<Uuid, String>) -> Vec<(String, String)>`: `(source_path, content)`, ordenado por caminho.
  - `pub fn materialize(cache_dir: &Path, version: i64, sources: &[(String, String)]) -> std::io::Result<PathBuf>`:
    1. escreve tudo em `cache_dir/v<version>.tmp/`;
    2. renomeia para `v<version>/`;
    3. cria `live.tmp` → `v<version>` e renomeia sobre `live` (troca atômica);
    4. apaga os `v*` exceto os dois mais novos.

    Devolve `cache_dir/live`. Com `v<version>` já existente, só religa o `live`.
  - `pub fn cache_dir_from_env(port: u16) -> PathBuf`, com as regras de *Global Constraints*.

- [ ] **Step 1: Write the failing tests**:
  - `validate_path_rules`: aceita `fatos.yml`, `sofiadb/dfos_a.yml`, `x.J2`; recusa `../a.yml`, `/a.yml`, `a//b.yml`, `a\\b.yml`, `a.txt`, `a b.yml` e um nome com 256 caracteres.
  - `sources_prefix_each_owner`: org A (`digital`) e Platform, ambas com `fatos.yml` → `["_platform/fatos.yml", "digital/fatos.yml"]`.
  - `materialize_writes_and_links`: `live` aponta para `v3`, e `live/digital/fatos.yml` tem o conteúdo.
  - `materialize_prunes_old_versions`: materializar v1, v2, v3 → só restam `v2` e `v3`.
  - `live_switches_only_to_complete_versions`: um `v5.tmp` incompleto deixado no diretório não é ligado; uma nova materialização da v5 refaz e liga.
  - `editable_only_plain_yaml`: `a.yml` → `true`; `a.jinja` → `false`; `a.yml` com `Some("sofiadb-sync")` → `false`.
- [ ] **Step 2: Run** `cargo test -p cubeserver rbac::model_snapshot`. Expected: FAIL (módulo vazio).
- [ ] **Step 3: Implement.** Use `std::os::unix::fs::symlink` e `std::fs::rename`. A troca do `live` é: criar o link com outro nome e renomear por cima.
- [ ] **Step 4: Run** de novo. Expected: 6 passed.
- [ ] **Step 5: Commit** `feat(rbac): model snapshot sources and atomic materialization`.

### Task 3: Importação na primeira partida

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/model_import.rs`
- Test: `rust/cube/cubeserver/tests/model_store.rs` (acrescentar)

**Interfaces:**
- Consumes: `Store::model_is_empty`, `Store::model_write` (Task 1); `sources` (Task 2); `Store::organization_by_slug`; `cubemodel::model_files(dir)`; `cubemodel::ModelLoader`.
- Produces:
  - `pub struct ImportReport { pub imported: usize, pub unowned: Vec<String>, pub renamed_to: Option<PathBuf> }`
  - `pub async fn import_if_empty(store: &Store, model_dir: &Path, today: &str) -> Result<ImportReport, String>`. `today` é `YYYYMMDD`, para o nome `model.imported-<today>`.

  Os passos seguem o spec (*Importação*):
  - organização pelo primeiro segmento; arquivos sem dono vão para Platform com o caminho inteiro;
  - marcador `# Generated by sync-sofiadb-model.py` → `managed_by = Some("sofiadb-sync")`;
  - `actor = "import"`, uma transação;
  - antes de renomear, compilar a pasta e as fontes do banco sem política e com o contexto padrão, e comparar os nomes de cubos e views. Se diferirem → `Err("the imported model lacks cubes: a, b")`, e a pasta não é renomeada;
  - depois do `COMMIT`: `rename(model, model.imported-<today>)`, criar `model/README` com "This folder is no longer read: with RBAC on the data model lives in the cube_app database (model_files). Export it with ./start-all.sh export-model.";
  - com o banco já preenchido: `imported: 0`, `renamed_to: None`, e um `tracing::warn!` se `model/` tiver arquivos de modelo.

- [ ] **Step 1: Write the failing tests**:
  - `import_moves_folders_into_rows`: uma pasta com `digital/fatos.yml`, `_platform/orders.yml`, `solto.yml` e `digital/sofiadb/dfos_a.yml` (com o marcador), mais uma org `digital`. Esperado: `imported == 4`; `unowned == ["solto.yml"]`; as linhas com os donos certos; `managed_by` só na do sync; `model_version() == 1`; `model.imported-20261122/` existe; `model/README` existe; `model/digital` não existe.
  - `import_is_not_repeated_when_the_folder_survives`: depois de importar, recriar `model/digital/x.yml` e chamar de novo → `imported == 0`, as linhas iguais, `x.yml` não entra.
  - `compare_cube_names_reports_missing` (unitário em `model_import.rs`): pasta com cubos `a` e `b`, banco só com `a` → `["b"]`; os dois iguais → `[]`; os dois com erro de compilação → `[]`.
  - `failed_compare_keeps_the_folder`: chamar o passo de conferência com um banco a que falta um cubo (pelo helper `compare_cube_names`, que o `import_if_empty` usa) → `Err` que cita o cubo; a pasta `model/` continua no lugar, sem `model.imported-*`.
- [ ] **Step 2: Run** `cargo test -p cubeserver --test model_store import`. Expected: FAIL.
- [ ] **Step 3: Implement** `import_if_empty` e `fn compare_cube_names(folder: &Result<DataModel, ModelError>, db: &Result<DataModel, ModelError>) -> Vec<String>`. A função devolve os nomes que estão na pasta e faltam no banco; um erro dos dois lados, ou nenhum, conta como igual.
- [ ] **Step 4: Run.** Expected: os testes novos passam, e os da Task 1 continuam passando.
- [ ] **Step 5: Commit** `feat(rbac): import the model folder into the RBAC store once`.

### Task 4: Partida, link `live` e recarga por versão

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/model_runtime.rs`
- Modify: `rust/cube/cubeserver/src/main.rs:160-190` (depois da política) e `:351-358` (laço de recarga)
- Modify: `rust/cube/cubeserver/src/app.rs` (`AppState` ganha `pub model_runtime: Option<Arc<ModelRuntime>>`)
- Modify: todos os construtores de `AppState` nos testes (`tests/*.rs`), com `model_runtime: None`
- Test: `rust/cube/cubeserver/tests/model_store.rs` (acrescentar)

**Interfaces:**
- Consumes: Tasks 1–3; `TenantRegistry::reload_all`; `cubeserver::rbac::ownership::build_policy`.
- Produces:
  - `pub struct ModelRuntime { /* store, cache_dir, live: PathBuf, seen: AtomicI64 */ }`
  - `impl ModelRuntime`:
    - `pub async fn start(store: Store, cache_dir: PathBuf) -> Result<Arc<Self>, String>`: lê as linhas e os slugs (`organizations`), materializa e guarda a versão.
    - `pub fn live_path(&self) -> &Path`
    - `pub fn version(&self) -> i64`
    - `pub async fn refresh(&self) -> Result<bool, StoreError>`: se `model_version()` difere da vista, lê as linhas, materializa e devolve `true`.
    - `pub fn spawn_loop(self: &Arc<Self>, interval: Duration, tenants: Arc<TenantRegistry>)`: a cada tick, `refresh()`, e com `true`, `tenants.reload_all()`.
- Em `main.rs`, com RBAC:
  1. `import_if_empty(&rbac.store, &cube_config.model_path, &today)`; um `Err` para a partida;
  2. `ModelRuntime::start`;
  3. `config.schema_path = live` e `cube_config.model_path = live`, **antes** de qualquer `load` do modelo;
  4. `println!("🗄️  Data model from the RBAC database (version {v}), served from {live}")`.

  O laço `tenants.spawn_reload_loop` continua (ele vê o `live` mudar pelo `model_stamp`), e o `ModelRuntime::spawn_loop` roda sempre com RBAC, com o intervalo de *Global Constraints*.

  A API SQL (`sql_api::start(&config.schema_path, …)`) lê o `live` na partida e mantém o modelo da partida até reiniciar, como hoje. O spec permite isso; a Task 10 documenta no README.

- [ ] **Step 1: Write the failing tests**:
  - `runtime_serves_rows_from_live`: um `Put` de `digital/fatos.yml` → `start` → `live_path()/digital/fatos.yml` existe.
  - `refresh_sees_writes_of_other_processes`: depois do `start`, um `model_write` direto no `Store` → `refresh()` dá `true`, o arquivo novo aparece em `live`; um segundo `refresh()` dá `false`.
  - `disk_folder_is_not_read`: um diretório `model/` com `digital/novo.yml` ao lado não aparece no `live`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement** `ModelRuntime` e a ligação em `main.rs`.
- [ ] **Step 4: Run** `cargo test -p cubeserver` (a suíte inteira, com `CUBE_TEST_PG_URL`). Expected: tudo verde.
- [ ] **Step 5: Commit** `feat(cubeserver): serve the model from the RBAC store through a private live folder`.

### Task 5: Validação de uma escrita

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/model_validate.rs`
- Test: unit tests no arquivo

**Interfaces:**
- Consumes: `cubemodel::ModelLoader::with_context(ctx).load_sources_with_policy(&sources, policy)`; `cubemodel::ownership::{OwnershipPolicy, Rejection}`.
- Produces:
  - `#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)] pub struct Problem { pub file: String, pub cube: String, pub message: String }`
  - `pub struct Check { pub problems: BTreeSet<Problem> }`, com todos os erros de compilação e recusas de um conjunto de fontes.
  - `pub fn check(sources: &[(String, String)], ctx: &cubemodel::TemplateContext, policy: Option<&OwnershipPolicy>) -> Check`. Compila **com validação** e põe cada erro de compilação como `Problem { file, cube: "", message }` (com `file` vazio quando o erro não diz) e cada recusa como `Problem`.
  - `pub fn new_problems(before: &Check, after: &Check) -> Vec<Problem>`, ordenado.
  - `pub fn fixed_count(before: &Check, after: &Check) -> usize`

- [ ] **Step 1: Write the failing tests**:
  - `valid_change_has_no_new_problems`: antes `{a.yml: cube a}`, depois mais `{b.yml: cube b}` → `new_problems` vazio.
  - `broken_yaml_is_new`: depois com `b.yml: "cubes: [ {"` → um problema com `file == "b.yml"`.
  - `old_errors_do_not_block_a_write`: antes com `x.yml` quebrado, depois com o mesmo `x.yml` mais `b.yml` válido → `new_problems` vazio.
  - `fixing_counts`: antes com `x.yml` quebrado, depois sem o erro → `fixed_count == 1`.
  - `ownership_rejection_is_new`: com uma política em que `on` não tem a connection `sofiadb`, um cubo `on/x.yml` com `data_source: sofiadb` → um problema de recusa com `cube == "x"`.
  - `deleting_a_joined_cube_is_new`: antes `a` com join para `b`, depois sem `b.yml` → um problema novo.
- [ ] **Step 2: Run** `cargo test -p cubeserver rbac::model_validate`. Expected: FAIL.
- [ ] **Step 3: Implement.** A política é passada por parâmetro, nunca a global, então estes testes não precisam de binário próprio.
- [ ] **Step 4: Run.** Expected: 6 passed.
- [ ] **Step 5: Commit** `feat(rbac): compare a model change against the current model`.

### Task 6: `ModelWriter`

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/model_writer.rs`
- Modify: `rust/cube/cubeserver/src/app.rs` (`AppState` ganha `pub model_writer: Option<Arc<ModelWriter>>`; os construtores dos testes recebem `None`)
- Modify: `rust/cube/cubeserver/src/main.rs` (cria o writer com o runtime, o registry e o contexto do tenant padrão)
- Test: `rust/cube/cubeserver/tests/model_api.rs` (binário novo; helpers de app copiados de `tests/org_tree.rs::env_with` para `tests/support/tree_env.rs`, que `org_tree.rs` também passa a usar)

**Interfaces:**
- Consumes: Tasks 1, 2, 4, 5.
- Produces:
  - `pub struct WriteError { pub status: StatusCode, pub message: String, pub problems: Vec<Problem> }`, com `impl From<WriteError> for ApiError`. O corpo da resposta é `{"error": message, "problems": [...]}`.
  - `pub struct Written { pub version: i64, pub changed: bool, pub fixed: usize }`
  - `impl ModelWriter`:
    - `pub fn new(store: Store, runtime: Arc<ModelRuntime>, tenants: Arc<TenantRegistry>, ctx: cubemodel::TemplateContext) -> Self`
    - `pub async fn write(&self, actor: &str, what: &str, changes: impl FnOnce(&ModelRows) -> Result<Vec<ModelChange>, WriteError>) -> Result<Written, WriteError>`

    O `write`:
    1. toma um `tokio::sync::Mutex` interno;
    2. chama `Store::model_write`. Dentro do `plan`: monta as fontes de antes e de depois (`sources` mais as mudanças), roda `check` dos dois lados com a política atual (`cubemodel::ownership::current_policy()`), e com `new_problems` não vazio → `Err(WriteError { status: 400, message: "The change does not compile", problems })`;
    3. `Conflict("stale")` → 409 com a mensagem de *Global Constraints*; `Conflict("path")` → 400 "Já existe um arquivo com esse caminho";
    4. depois do `COMMIT`: `runtime.refresh()` e `tenants.reload_all()`;
    5. `tracing::info!(actor, what, version, "model change")`.

- [ ] **Step 1: Write the failing tests** (`model_api.rs`, com a política instalada):
  - `write_applies_and_reloads`: um `Put` válido → `Written { changed: true, .. }`, e o `/cube/v1/meta` do admin tem o cubo novo sem esperar o laço.
  - `invalid_write_changes_nothing`: um `Put` com YAML quebrado → `status == 400`, `problems[0].file` termina em `.yml`, a versão não muda, e o `live` está igual.
  - `concurrent_writes_both_validate`: dois `write` em paralelo, cada um criando um arquivo válido → os dois `Ok`, a versão sobe 2, e os dois arquivos existem.
- [ ] **Step 2: Run** `cargo test -p cubeserver --test model_api`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: 3 passed; `--test org_tree` continua 10/10.
- [ ] **Step 5: Commit** `feat(rbac): validated, transactional model writes`.

### Task 7: Rotas de arquivo, mover e exportar

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/handlers/model.rs`
- Modify: `rust/cube/cubeserver/src/rbac/handlers/mod.rs` (rotas)
- Modify: `rust/cube/cubeserver/src/handlers/tree.rs` (o detalhe ganha `id`, `version`, `editable` por arquivo; `fn org_detail(state, owner) -> Value` passa a ser `pub(crate)` e é reaproveitada)
- Test: `rust/cube/cubeserver/tests/model_api.rs`

**Interfaces:**
- Consumes: Task 6; `visible_organization`, `caller`, `require_platform_admin` (de `rbac/handlers/connections.rs`).
- Produces, sob `{base}/v1`:
  - `GET /organizations/:id/model`, `GET /platform/model` → o detalhe do spec. Cada arquivo traz `{id, fileName, path, version, editable}`.
  - `GET|PUT|DELETE /organizations/:id/model/files/:file` e `GET|PUT|DELETE /platform/model/files/:file`:
    - corpo do `PUT` `{content, version}`; `DELETE` com `?version=`;
    - um arquivo de outra organização → 404;
    - não editável → 400 (mensagem do sync, ou "Arquivos .jinja e .j2 não são editáveis aqui");
    - sucesso → 200 com o detalhe novo; `DELETE` → 204.
  - `PUT /model/files/:file/organization` `{organizationId | null}` (admin) → 200 com `{version}`.
  - `GET /model/export` (admin) → `{"version": n, "files": [{"path": "<slug>/<path>", "content": "..."}]}`.
- Permissões: `/organizations/:id/...` com `visible_organization(rbac, id, "admin", user)`; `/platform/...` e `/model/...` com `require_platform_admin`.

- [ ] **Step 1: Write the failing tests**:
  - `admin_edits_a_file`: `PUT` com a versão lida → 200, e `GET` traz o conteúdo novo e `version + 1`.
  - `stale_put_is_409`: dois `PUT` com a mesma versão → o segundo dá 409 com a mensagem exata.
  - `sync_file_is_read_only`: `PUT` e `DELETE` num arquivo `managed_by` → 400 com a mensagem exata, também para o admin.
  - `delete_of_depended_file_is_400`: `a.yml` faz join com `b`, e `DELETE` de `b.yml` → 400 com `problems` não vazio.
  - `file_of_other_org_is_404`: `GET /organizations/A/model/files/<id de B>` → 404.
  - `move_revalidates`: mover um cubo `sofiadb` para uma org sem a connection → 400; para a dona → 200.
  - `export_lists_every_file`: `/model/export` tem `digital/fatos.yml` e `_platform/orders.yml`.
  - `non_admin_cannot_reach_platform`: um usuário sem papel → `/platform/model` 403 e `/model/export` 403.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `--test model_api` e `--test org_tree`. Expected: tudo verde.
- [ ] **Step 5: Commit** `feat(rbac): routes to read, edit, delete, move and export model files`.

### Task 8: Gerar para o banco

**Files:**
- Modify: `rust/cube/cubeserver/src/handlers/scaffold.rs`: dividir `generate` em `pub fn plan(model_dir: &Path, data_sources: &[(String, String)], request: &GenerateRequest, folder: Option<&str>) -> Result<Vec<PlannedFile>, ApiError>`, que só decide, e `fn write_files(...)`, que é o disco de hoje, usado sem RBAC.
- Modify: `rust/cube/cubeserver/src/handlers/playground.rs:386` (`generate_schema` com RBAC → `ModelWriter`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/model.rs` (`POST /organizations/:id/model/generate`, `POST /platform/model/generate`)
- Test: `rust/cube/cubeserver/tests/model_api.rs`; os testes de `scaffold.rs` continuam

**Interfaces:**
- Produces: `pub struct PlannedFile { pub name: String, pub file: String /* "<cubo>.yml" */, pub content: String, pub adopt_from: Option<String> /* fonte "<slug>/<path>" a apagar */ }`.
- A rota `generate`:
  - recebe `{tables: [[schema, table]]}`;
  - lê as colunas com `data_source_schema` das connections da organização;
  - uma tabela de connection de outra organização → 400 "A tabela X não é de uma conexão desta organização";
  - converte cada `PlannedFile` em `ModelChange::Put` (e `Delete` do adotado), num único `write`.
  - Responde `{"files": [{"fileName": "<slug>/<cubo>.yml", "content"}]}`, como hoje.

- [ ] **Step 1: Write the failing tests**:
  - `generate_writes_rows`: gerar `cluster_test.fatos` numa org de teste → uma linha `fatos.yml` nova, e o `/meta` do admin tem o cubo.
  - `generate_refuses_foreign_connection`: tabela de connection de outra org → 400 com a mensagem.
  - `generate_is_all_or_nothing`: duas tabelas, uma gerando nome já usado → 400, e nenhuma linha nova.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p cubeserver` (a suíte inteira). Expected: verde.
- [ ] **Step 5: Commit** `feat(cubeserver): Generate Data Model writes to the RBAC store`.

### Task 9: Rotas do sync

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/model_sync.rs` (o cálculo)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/model.rs` (`GET /model/sync/:connection/context`, `PUT /model/sync/:connection`)
- Modify: `rust/cube/cubeserver/src/rbac/config.rs` (`sync_user: String`, de `CUBE_SYNC_USER`, padrão `sofiadb-sync`)
- Test: unit tests em `model_sync.rs`; `tests/model_api.rs`

**Interfaces:**
- Produces:
  - `pub struct SyncContext { pub taken: BTreeSet<String>, pub covered: BTreeMap<String, String> }`, serializado como `{"taken": [...], "covered": {"db.table": "cube"}}`.
  - `pub fn sync_context(sources: &[(String, String)], managed: &HashSet<String>, connection: &str, default_db: &str) -> SyncContext`: a mesma regra de `hand_written()` do script (nomes de cubos e views; `sql_table`, ou o `FROM` do `sql`, dos cubos com `data_source == connection`), ignorando as fontes em `managed`.
  - A rota `PUT` recebe `{files: [{path, content}], context: SyncContext}`:
    - recalcula o contexto; se diferir do recebido → 409 "O modelo mudou durante a sincronização";
    - destino: a organização dona da connection, ou Platform;
    - substitui todas as linhas `managed_by='sofiadb-sync'` daquela connection, e move as que estiverem em outra organização;
    - um conjunto igual → `{"changed": false}`, sem subir a versão.
  - Responde `{changed, added, updated, removed}`.
- Permissão: `ctx.user == rbac.config.sync_user`; qualquer outro → 403 "Só o usuário de sincronização usa esta rota".

- [ ] **Step 1: Write the failing tests**:
  - Unitários: `context_names_and_covered` (um cubo à mão `eventos` com `sql_table: cluster_test.eventos_local` → `covered["cluster_test.eventos_local"] == "eventos"`, e `taken` contém `eventos`); `managed_sources_do_not_count`.
  - Integração:
    - `sync_replaces_only_managed_rows`;
    - `same_set_is_no_change` (versão igual);
    - `sync_moves_rows_with_the_connection`;
    - `broken_sync_file_changes_nothing` (400);
    - `stale_context_is_409`;
    - `only_the_sync_user` (admin → 403; usuário do sync → 200).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: verde.
- [ ] **Step 5: Commit** `feat(rbac): SofiaDB sync writes through the server`.

### Task 10: Script de sync, `start-all.sh` e README (deploy, fora do git)

**Files (deploy `/root/cube-rust`, com backup `*.bak-<data>` antes):**
- Modify: `/root/cube-rust/sync-sofiadb-model.py`
- Modify: `/root/cube-rust/start-all.sh`
- Modify: `/root/cube-rust/README.md`
- Test: `/root/cube-rust/tests/test_sync_rbac.py` (novo), `/root/cube-rust/tests/test_sync_stale.py` (continua)

**Interfaces:**
- Consumes: Task 9 (`GET …/context`, `PUT …`), Task 7 (`/model/export`), `POST /users`, `POST /users/{id}/tokens`.
- `sync-sofiadb-model.py` com `CUBE_RBAC=spicedb` (com `.strip()`):
  - `fetch_context() -> (set, dict)` substitui `hand_written()`;
  - `build()` usa o contexto;
  - `push(files, context) -> dict` faz o `PUT`;
  - não toca no disco;
  - servidor sem resposta → `log("server not answering; sync skipped")` e sai com 0;
  - 400/409 → registra o erro e sai com 1.
  - Sem RBAC, o comportamento de hoje.
- `start-all.sh`:
  - `cmd_start` roda `sync_model` depois de `start_server`;
  - `ensure_sync_token` (novo): se `CUBE_SYNC_TOKEN` faltar no `.env`, loga como admin, cria o usuário `sofiadb-sync` (senha aleatória de 32 caracteres, nunca impressa), cria o token de serviço e grava `CUBE_SYNC_TOKEN` no `.env` (com backup);
  - `export-model [pasta]` (novo): `GET /model/export` e grava `pasta/<path>` (padrão `model.export-<data>`);
  - o passo *Data model* passa a dizer "into the RBAC database".
- README: *SofiaDB tables*, *The data model* e RBAC, conforme o spec (*Deploy*). Inclui backup (`pg_dump cube_app`), `export-model`, a volta ao binário antigo e que editar `model/` não vale com RBAC.

- [ ] **Step 1: Write the failing test** `test_sync_rbac.py`: com um servidor HTTP falso (`http.server` numa thread) que responde ao `context` com `covered = {"dfos.a": "my_a"}`, `build()` não gera `dfos_a`, e um join para `a` aponta para `my_a`. O `push` envia `files` e `context`.
- [ ] **Step 2: Run** `python3 -I tests/test_sync_rbac.py sync-sofiadb-model.py`. Expected: FAIL.
- [ ] **Step 3: Implement** o script, depois `start-all.sh` (`bash -n start-all.sh` sem erro) e o README.
- [ ] **Step 4: Run** os dois testes Python. Expected: `PASS`.
- [ ] **Step 5:** Sem commit (deploy fora do git). Registrar no ledger os backups feitos.

## Etapa 2 — donos de organização

### Task 11: O que o dono vê dos erros e do gerador

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/model_redact.rs`
- Modify: `rust/cube/cubeserver/src/rbac/handlers/model.rs` (aplica quando `!is_platform_admin`)
- Modify: `rust/cube/cubeserver/src/handlers/scaffold.rs` (`plan` recebe `owner_only: bool`: com `true`, o `Adopt` só vale para fonte da mesma pasta e não do sync; senão, a mensagem genérica de nome usado)
- Test: unit tests; `tests/model_api.rs`

**Interfaces:**
- Produces: `pub fn redact(problems: &[Problem], new: &[Problem], own_folder: &str) -> Vec<Problem>`:
  - mantém os problemas cujo `file` começa com `"<own_folder>/"` ou é vazio, e os novos;
  - nos novos, troca um `file` de outra pasta por `"(arquivo de outra organização)"` e esse caminho dentro de `message`;
  - os demais viram um único `Problem { file: "", cube: "", message: "Há um problema no modelo fora da sua organização; avise o administrador" }`.

  Também: `pub fn duplicate_name_message(name: &str) -> String`.

- [ ] **Step 1: Write the failing tests**:
  - `redact_keeps_own_and_new`;
  - `redact_hides_foreign_path` (a mensagem não contém `digital/`);
  - `old_foreign_become_one_line`;
  - `owner_sees_no_foreign_path_on_duplicate` (integração: dono da ON cria `on/x.yml` com cubo `fatos` → 400, e o corpo não contém `digital`);
  - `owner_generate_does_not_adopt_foreign`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: verde.
- [ ] **Step 5: Commit** `feat(rbac): owners never see another organization's files in model errors`.

### Task 12: Membros do cubo nas rotas da árvore

**Files:**
- Modify: `rust/cube/cubeserver/src/model_index.rs` (`IndexedCube` ganha `pub measure_list: Vec<IndexedMember>`, `pub dimension_list: Vec<IndexedMember>`, `pub segment_list: Vec<IndexedMember>`)
- Modify: `rust/cube/cubeserver/src/handlers/tree.rs` (serializa em `cubes[].measures|dimensions|segments`; os contadores continuam em `members`)
- Test: `rust/cube/cubeserver/tests/playground.rs` (`tree_without_rbac_is_one_model_node` ganha asserts) e unit em `model_index.rs`

**Interfaces:**
- Produces: `pub struct IndexedMember { pub name: String /* "cube.member" */, pub r#type: String, pub title: Option<String>, pub primary_key: bool }`. JSON: `{name, type, title, primaryKey}`.

- [ ] **Step 1: Write the failing test**: um cubo com 1 measure `count`, 1 dimension `id` (PK) e 1 segment → `cubes[0].measures[0].name == "orders.count"` e `dimensions[0].primaryKey == true`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: verde.
- [ ] **Step 5: Commit** `feat(cubeserver): the tree routes list each cube's members`.

### Task 13: `ownedOrganizations` no login

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (`pub async fn owned_organizations(&self, user: &str) -> Result<Vec<Organization>, ApiError>`, via `authz.lookup_resources("organization", "admin", user)` + `store.organizations_by_ids`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/auth.rs:66` (resposta)
- Test: `tests/rbac_api.rs`

- [ ] **Step 1: Write the failing test** `login_lists_owned_organizations`: Ana, dona de "Teste" → `user.ownedOrganizations == [{id, slug: "teste", name: "Teste"}]`; Bia (viewer) → `[]`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `--test rbac_api`. Expected: verde.
- [ ] **Step 5: Commit** `feat(rbac): login lists the organizations the user owns`.

### Task 14: Playground — guarda, árvore do dono e painel sem `/meta`

**Files:**
- Modify: `packages/cubejs-playground/src/shared/auth.ts` (`Session.user.ownedOrganizations: { id: string; slug: string; name: string }[]`, padrão `[]`)
- Modify: `packages/cubejs-playground/src/hooks/rbac.ts` (`canModel: boolean` = `isAdmin || ownedOrganizations.length > 0`)
- Create: `packages/cubejs-playground/src/components/AdminOnly/OwnerOrAdmin.tsx`
- Modify: `packages/cubejs-playground/src/pages/index.tsx:74` (Data Model com `OwnerOrAdmin`)
- Modify: `packages/cubejs-playground/src/components/Sidebar/Sidebar.tsx:52` (`admin: true` vira `model: true`, filtrado por `canModel`)
- Create: `packages/cubejs-playground/src/pages/Schema/api.ts`
- Modify: `packages/cubejs-playground/src/pages/Schema/SchemaPage.tsx` (usa `api.ts`)
- Modify: `packages/cubejs-playground/src/pages/Schema/CubeDetail.tsx` (membros da rota; sem `metaCube`)
- Test: `packages/cubejs-playground/src/__tests__/schema-api.test.ts`, `schema-tree.test.ts`

**Interfaces:**
- `api.ts`:
  - `export function modelApi(rbac: Rbac): { list(q, offset): Promise<{organizations: OrgSummary[], total: number}>, detail(org: OrgSummary): Promise<OrgDetail>, file(org, id): Promise<FileContent>, save(org, id, content, version): Promise<OrgDetail>, remove(org, id, version): Promise<void>, generate(org, tables): Promise<void>, move(id, organizationId | null): Promise<void> }`
  - O admin usa `/playground/tree` para a lista; o dono monta a lista a partir de `ownedOrganizations`, filtrada por `q`.
  - O `detail` vai para `/cube/v1/organizations/:id/model`, ou `/platform/model` para `_platform`, com `authHeaders()`.
  - `OrgSummary` ganha `id`.
- `CubeDetail` lê `cube.measures|dimensions|segments` (Task 12). **Abrir no Playground** fica habilitado se houver pelo menos um membro e, para um dono, se a organização tiver workspace. Sem workspace, fica desabilitado com a dica "Crie um workspace nesta organização para consultar"; a lista vem de `GET /cube/v1/organizations/:id/workspaces`. Se a connection do cubo não estiver liberada para o workspace escolhido (`GET /cube/v1/workspaces/:id/connections`), o painel avisa "libere a conexão X para o workspace Y".
- O texto "This file can be edited at" sai.

- [ ] **Step 1: Write the failing tests**:
  - `owner list comes from ownedOrganizations and filters by q`;
  - `detail url for an org and for _platform`;
  - `canModel for admin, owner and member`.
- [ ] **Step 2: Run** `npx vitest run src/__tests__/schema-api.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npx vitest run && npx tsc --noEmit -p .`. Expected: verde.
- [ ] **Step 5: Commit** `feat(playground): organization owners open the Data Model page`.

### Task 15: Playground — editar, apagar, mover

**Files:**
- Create: `packages/cubejs-playground/src/pages/Schema/FileDetail.tsx`
- Create: `packages/cubejs-playground/src/pages/Schema/editor.ts` (estado puro do editor)
- Modify: `packages/cubejs-playground/src/pages/Schema/SchemaPage.tsx` (painel do arquivo usa `FileDetail`; o Generate do dono chama `api.generate`)
- Test: `packages/cubejs-playground/src/__tests__/schema-editor.test.ts`

**Interfaces:**
- `editor.ts`:
  - `export type EditorState = { mode: 'view' | 'edit' | 'saving'; draft: string; base: number; error: string | null; problems: Problem[]; stale: boolean }`
  - `export function reduce(s: EditorState, a: { type: 'edit' } | { type: 'change'; text: string } | { type: 'cancel' } | { type: 'save' } | { type: 'saved'; version: number; content: string } | { type: 'failed'; status: number; body: any }): EditorState`
  - Um `failed` com 409 → `stale: true` e mantém o `draft`; com 400 → `error` e `problems`, mantém o `draft`.
- `FileDetail`:
  - mostra `PrismCode` no modo `view`, com **Editar** e **Apagar** quando `editable`, e **Mover para…** só para o admin;
  - no modo `edit`, mostra um `Input.TextArea` monoespaçado, com altura de 24 linhas;
  - não editável → `Alert` com o motivo;
  - rodapé "Alterado por {updatedBy} em {data local}".

- [ ] **Step 1: Write the failing tests**:
  - `edit then cancel restores`;
  - `400 keeps draft and shows problems`;
  - `409 marks stale and keeps draft`;
  - `saved returns to view with new base`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npx vitest run && npx tsc --noEmit -p . && npx oxlint src/pages/Schema`. Expected: verde; `yarn build:playground` sem erro.
- [ ] **Step 5: Commit** `feat(playground): edit, delete and move model files in the Data Model page`.

### Task 16: Entrega no deploy (feita pelo controlador)

**Files:** deploy `/root/cube-rust` (fora do git).

- [ ] **Step 1: Backup:** `cp -a model/` e `pg_dump -Fc cube_app` para `backup-<data>-model-rbac/`.
- [ ] **Step 2: Build:** `cargo build --release -p cubeserver --bin cube-server`. Copiar o binário para `/root/cube/rust/cube/target/release/cube-server` com `cp` + `mv` atômico. Depois, `./install-playground.sh <build>`.
- [ ] **Step 3: Restart:** `./start-all.sh restart`. Expected no log:
  - "Data model from the RBAC database (version 1)";
  - `model.imported-<data>/` criado;
  - `CUBE_SYNC_TOKEN` gravado no `.env`.
- [ ] **Step 4: Verificar:**
  - `/meta` como admin com os mesmos 30 cubos de antes (`diff`);
  - `/playground/tree` com Digital 25, ON 0, Platform 5;
  - `./start-all.sh sync` → `changed: false`;
  - `./start-all.sh export-model /tmp/x` cria 30 arquivos;
  - a suíte de restrições (`verify_restrictions.sh`) dá 23/23.
- [ ] **Step 5: Dono de teste:** um usuário `e2e_owner`, dono só da ON. A ON não tem connection, então ela recebe uma connection temporária criada pela própria organização (`POST /organizations/{ON}/connections`, `e2e_on_sofia`, apontando para o nó SofiaDB `8202`; o servidor reinicia sozinho). As connections da Digital não são tocadas. No navegador, o dono:
  - vê só a ON;
  - gera um cubo de `e2e_on_sofia`;
  - edita, salvando primeiro com erro e depois certo;
  - apaga o arquivo;
  - não alcança Digital nem Platform.

  No fim, apagar o arquivo, a connection temporária e o vínculo de dono, e desativar `e2e_owner`.
- [ ] **Step 6: README** conferido e ledger atualizado.

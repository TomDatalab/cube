# Restrições de dados nas roles — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** As roles da organização passam a bloquear membros (colunas) e a filtrar linhas por atributos do usuário, em toda consulta que passa pelo `enforce.rs`, seguindo as dependências do modelo.

**Architecture:**
- Restrições (`restrictions jsonb` em `roles`) e atributos (`user_attributes`) ficam no Postgres do RBAC.
- Um grafo de dependências de membros é construído junto com o snapshot `DataSources`, a cada carga do modelo.
- O `RbacQueryService` reúne as restrições do usuário no workspace e reescreve cada `NormalizedQuery`: recusa membros bloqueados e acrescenta filtros. O `RbacMetaService` poda os membros bloqueados.
- O Playground ganha a edição dos atributos (Users) e das restrições (Roles).

**Tech Stack:** Rust (axum, tokio-postgres com `with-serde_json-1`, serde), SpiceDB (sem mudança de schema), React + antd 4.16.13, vitest.

**Spec:** `docs/superpowers/specs/2026-11-18-role-restricoes-de-dados-design.md`

## Global Constraints

- Platform admins passam sem restrição. Os papéis prontos (owner/editor/viewer) não têm restrições.
- Regra B: as restrições de todas as roles do usuário no workspace valem juntas. Filtros com E; membros bloqueados somados.
- Na dúvida, bloqueia: falha de SpiceDB, banco ou snapshot é 503; restrição que não dá para aplicar é 403; nunca roda sem a restrição.
- Mensagens de erro no padrão `Permission denied: ...` (403).
- A porta SQL do Postgres (`CUBEJS_PG_SQL_PORT`, login `CUBEJS_SQL_USER`) **não muda**.
- Nome de atributo: `a-z`, `0-9`, `_`, de 1 a 64 caracteres. Membro: `cube.member`, cada parte em `[A-Za-z0-9_]+`.
- Operadores de filtro: `equals` e `notEquals`. Fonte: exatamente um entre `attribute` e `values` (lista não vazia).
- Limites [decisão do plano]: até 50 `rowFilters`, 500 `blockedMembers` e 50 atributos por usuário, com até 100 valores de até 200 caracteres cada.
- Migrações idempotentes em `src/rbac/migrations.sql`.
- Frontend: antd 4.16.13 (`Modal visible`, `Form.List`, sem `Form.useWatch`), tokens `--pg-*`, sem violeta.
- Comandos de teste:
  - Rust: `cd rust/cube && CUBE_TEST_PG_URL=postgres://postgres:test@127.0.0.1:55432/postgres CARGO_TARGET_DIR=/root/cube/rust/cube/target cargo test -p cubeserver --lib --test rbac_api --test rbac_store`;
  - Playground, em `packages/cubejs-playground`: `node_modules/.bin/vitest run` e `node_modules/.bin/tsc --noEmit -p .`;
  - lint: `npx -y oxlint@1.82.0 <arquivos>`.
- Commits com `git -c "user.name=Tom DataLab" -c user.email=sofiadb@sofiadb.org commit`, terminando com `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Consulta por view cujo cube tem filtro, mas a view não expõe o membro filtrado:** deve ser 403, nunca rodar sem o filtro. Teste na Task 4: `a_view_without_the_filtered_member_is_refused`.
2. **Filtro do usuário com `or` tentando contornar o filtro injetado:** o filtro injetado fica no nível de cima (E) e não dentro do `or`. Teste na Task 4: `injected_filters_are_anded_outside_the_users_or`.
3. **Membro bloqueado usado só no filtro, na ordenação ou num segmento** (não em `measures`/`dimensions`): também é recusado. Teste na Task 4: `a_blocked_member_is_refused_wherever_the_query_names_it`.
4. **Membro calculado sobre um bloqueado, inclusive por uma cadeia de dois saltos:** `margin_pct → margin_ratio → margin` é recusado. Teste na Task 3: `closure_is_transitive`.
5. **Um usuário que perde a rule ou tem o atributo alterado:** a próxima consulta já reflete a mudança, porque não há cache de restrições. Teste na Task 4: `changing_attributes_takes_effect_on_the_next_query`.

---

### Task 1: Tipos de restrição, validação e armazenamento

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/restrictions.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (`pub mod restrictions;`)
- Modify: `rust/cube/cubeserver/src/rbac/migrations.sql` (no fim)
- Modify: `rust/cube/cubeserver/src/rbac/store.rs` (`Role`, `ROLE_COLS`, `role_from`, `create_role`, `update_role` no Store e no StoreTx; métodos de atributos)
- Test: `rust/cube/cubeserver/src/rbac/restrictions.rs` (`#[cfg(test)]`), `rust/cube/cubeserver/tests/rbac_store.rs`

**Interfaces:**
- Produces, em `rbac::restrictions`:
  ```rust
  #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase", deny_unknown_fields)]
  pub struct Restrictions { #[serde(default)] pub row_filters: Vec<RowFilter>, #[serde(default)] pub blocked_members: Vec<String> }
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase", deny_unknown_fields)]
  pub struct RowFilter { pub member: String, pub operator: RowOperator,
      #[serde(default, skip_serializing_if = "Option::is_none")] pub attribute: Option<String>,
      #[serde(default, skip_serializing_if = "Option::is_none")] pub values: Option<Vec<String>> }
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub enum RowOperator { Equals, NotEquals }
  pub type Attributes = std::collections::BTreeMap<String, Vec<String>>;
  impl Restrictions {
      /// Valida (400 via ApiError::bad_request) e normaliza: blocked_members sem repetição, em ordem.
      pub fn validated(self) -> Result<Restrictions, ApiError>;
      pub fn is_empty(&self) -> bool;
  }
  pub fn validate_attribute_name(name: &str) -> Result<(), ApiError>;
  pub fn validated_attributes(attributes: Attributes) -> Result<Attributes, ApiError>;
  ```
- Produces, no `store`:
  - `Role.restrictions: Restrictions`;
  - `create_role(org, name, permissions, restrictions: &Restrictions, created_by)` e `update_role(id, name, permissions, restrictions: &Restrictions)`;
  - `Store::user_attributes(&self, user: &str) -> Result<Attributes, StoreError>`;
  - `Store::set_user_attributes(&self, user: &str, attributes: &Attributes) -> Result<(), StoreError>`, que substitui o conjunto numa transação.
  - Um `restrictions` do banco que não desserializa vira `StoreError` (e daí 503), nunca "sem restrição".

- [ ] **Step 1: Testes de unidade em `restrictions.rs`:**
  - `restrictions_accept_the_spec_example`: o JSON do spec, com `rowFilters` (`attribute: "region"`, e `values: ["internal"]` com `notEquals`) e `blockedMembers`, desserializa e passa por `validated()`;
  - `a_filter_takes_exactly_one_value_source`: os dois campos, ou nenhum, dá erro;
  - `members_must_be_cube_dot_member`: `orders`, `orders.`, `a.b.c` e `or ders.x` dão erro;
  - `unknown_operators_and_fields_are_refused`: `"operator": "gt"` e um campo extra falham na desserialização;
  - `blocked_members_are_deduplicated`;
  - `limits_are_enforced`: 51 filtros dão erro;
  - `attribute_names_follow_the_identifier_rule`: `Region` e `a-b` dão erro, `cost_center` passa;
  - `attribute_values_are_bounded`: 101 valores dão erro.
- [ ] **Step 2:** rodar `cargo test -p cubeserver --lib restrictions` e ver falhar (módulo inexistente).
- [ ] **Step 3:** implementar os tipos e as validações acima.
- [ ] **Step 4: Migração, no fim do arquivo:**
  - `ALTER TABLE roles ADD COLUMN IF NOT EXISTS restrictions jsonb NOT NULL DEFAULT '{}'`;
  - `CREATE TABLE IF NOT EXISTS user_attributes (user_id text NOT NULL REFERENCES users (id) ON DELETE CASCADE, name text NOT NULL, values text[] NOT NULL, PRIMARY KEY (user_id, name))`.
  - Atualizar `Role`, `ROLE_COLS`, `role_from`, `create_role` e `update_role` (todas as chamadas em `handlers/roles.rs` passam `&Restrictions::default()` por enquanto) e os métodos de atributos.
- [ ] **Step 5: Testes em `tests/rbac_store.rs`:**
  - `role_restrictions_round_trip`: cria uma role com restrições, lê com `role()` e `roles_by_ids()`, e compara;
  - `user_attributes_are_replaced_as_a_set`: grava `{region:[Sul,Sudeste], cost_center:[1020]}`, depois só `{region:[Norte]}`, e lê `{region:[Norte]}`;
  - `deleting_a_user_deletes_their_attributes`.
- [ ] **Step 6:** rodar a suíte Rust completa (comando nas Global Constraints). Deve passar.
- [ ] **Step 7:** commit `feat(cubeserver): role restrictions and user attributes in the RBAC store`.

### Task 2: API de atributos e restrições

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/handlers/roles.rs` (`NewRole`/`RolePatch` com `restrictions: Option<Restrictions>`; `view()` devolve `"restrictions"`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/users.rs` (`attributes` e `set_attributes`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/mod.rs` (rota `/v1/users/:id/attributes` com GET e PUT)
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces: HTTP.
  - `GET /v1/users/{id}/attributes` → `{attributes: {name: [values]}}`.
  - `PUT /v1/users/{id}/attributes` com `{attributes: {...}}` → 200 com o conjunto salvo. 404 para usuário inexistente.
  - Ambas só para platform admin, com o mesmo guard das outras rotas `/v1/users`.
  - Roles: `POST` e `PATCH` aceitam `restrictions`; `GET`, `POST`, `PATCH` e a lista devolvem `restrictions` (sempre com as duas chaves).
  - Helpers de teste novos em `rbac_api.rs`: `set_attributes(env, token, user, Value) -> (StatusCode, Value)`.

- [ ] **Step 1: Testes:**
  - `user_attributes_are_set_and_read_by_platform_admins`: PUT e GET devolvem o mesmo conjunto. Um usuário comum recebe 403 nas duas rotas, e um usuário inexistente, 404;
  - `role_restrictions_are_saved_validated_and_returned`:
    - `create_role` com o exemplo do spec → 201, e o corpo traz `restrictions` igual ao enviado;
    - `patch_role` com `{"restrictions": {"blockedMembers": ["a_orders.status"]}}` → 200, e os `rowFilters` ficam vazios;
    - `{"restrictions": {"rowFilters": [{"member": "a_orders.status", "operator": "equals"}]}}` → 400;
    - um admin de organização (ana) pode editar as restrições; bia (viewer) recebe 403.
- [ ] **Step 2:** rodar só esses testes e ver falhar.
- [ ] **Step 3:** implementar. O `PATCH` sem `restrictions` mantém as atuais. Restrições não tocam o SpiceDB.
- [ ] **Step 4:** rodar a suíte Rust completa. Deve passar.
- [ ] **Step 5:** commit `feat(cubeserver): API for user attributes and role restrictions`.

### Task 3: Grafo de dependências de membros

**Files:**
- Create: `rust/cube/cubeserver/src/rbac/dependencies.rs`
- Modify: `rust/cube/cubeserver/src/rbac/mod.rs` (`pub mod dependencies;`)
- Modify: `rust/cube/cubeserver/src/planner_adapter.rs` (`DataSources` ganha `graph: MemberGraph`, construído em `DataSources::load` a partir do mesmo `model`, e `pub fn graph(&self) -> &MemberGraph`)
- Test: `#[cfg(test)]` em `dependencies.rs`

**Interfaces:**
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
  pub enum Dep { Member(String) /* "cube.member" de um cube, nunca de view */, Cube(String) /* o cube inteiro */ }
  impl Dep { pub fn cube(&self) -> &str; }
  #[derive(Debug, Default)]
  pub struct MemberGraph { /* privado */ }
  impl MemberGraph {
      pub fn build(model: &cubemodel::DataModel) -> MemberGraph;
      /// Fecho transitivo do que `name` lê: "cube.member", "view.member", "cube.member.granularity" ou só "cube".
      /// Inclui o próprio membro (resolvido para o de cube). Nome desconhecido → {Cube(primeira parte)}.
      pub fn closure(&self, name: &str) -> BTreeSet<Dep>;
      /// Membro de view → membro de cube; membro de cube → ele mesmo; None se desconhecido.
      pub fn source_member(&self, name: &str) -> Option<&str>;
      /// O membro de `view` que aponta para `cube_member`, se a view o expõe.
      pub fn view_member_for(&self, view: &str, cube_member: &str) -> Option<&str>;
      pub fn is_view(&self, name: &str) -> bool;
      /// Os membros (dimensões, medidas, segmentos) de um cube, para Dep::Cube e para o meta.
      pub fn members_of_cube(&self, cube: &str) -> Vec<&str>;
  }
  ```
- Regras de extração [decisão do plano, fiel ao spec]:
  - Cada membro de cube (dimensão, medida ou segmento) é serializado com `serde_json::to_value`. Todo texto entre `{` e `}` em qualquer string do JSON é uma referência. Isso cobre `sql`, `filters`, `case`, granularidades e o resto.
  - O `sql` e o `sql_table` do cube valem para **todos** os membros do cube.
  - Resolução de uma referência `r`, dentro do cube `C`:
    - `CUBE` → nada;
    - `CUBE.x` → `Member(C.x)` se `x` é membro de `C`, senão nada (é coluna física de `C`);
    - `x` (uma parte) → `Member(C.x)` se existe; senão `Cube(x)` se `x` é um cube; senão `Cube(C)`;
    - `a.b` → `Member(a.b)` se existe; senão `Cube(a)` se `a` é um cube; senão nada;
    - prefixos `FILTER_PARAMS.` e `SECURITY_CONTEXT.` / `COMPILE_CONTEXT.`: `FILTER_PARAMS.a.b...` → `Member(a.b)` se existe; os outros dois → nada;
    - `a.b.c...`, com mais partes → como `a.b`;
    - uma chave `{` sem `}` correspondente → `Cube(C)`.
  - `drill_members`, `title`, `description` e `meta` não têm chaves, então não geram referência.
  - Membro de view: `included_members[*].member_path` dá o membro de cube de origem. O fecho do membro de view é o fecho do membro de origem.
  - Fecho: busca em largura com conjunto de visitados, sem limite de profundidade; ciclos terminam.
  - `Dep::Cube(c)` não se expande.

- [ ] **Step 1: Testes (modelos YAML pequenos carregados com `cubemodel::ModelLoader` a partir de um `tempfile::tempdir()`):**
  - `a_member_depends_on_itself`: `orders.status` → `{Member(orders.status)}`;
  - `sql_references_are_followed`: `margin_pct` com `sql: "{margin} / {CUBE}.revenue"` → `{Member(orders.margin_pct), Member(orders.margin)}`;
  - `closure_is_transitive`: `margin_pct → {margin_ratio}` e `margin_ratio → {margin}` → os três;
  - `references_to_other_cubes`: `{customers.segment}` → `Member(customers.segment)`, e `{customers}.name` → `Cube(customers)`;
  - `measure_filters_are_followed`: `filters: [{sql: "{CUBE}.status = 'x' AND {customers.cpf} IS NOT NULL"}]` inclui `Member(customers.cpf)`;
  - `cube_sql_applies_to_every_member`: cube com `sql: "SELECT * FROM t JOIN {customers.sql()} c"` → todo membro tem `Cube(customers)`;
  - `view_members_resolve_to_their_cube_member`: `orders_view.status` → `Member(orders.status)`; `source_member` e `view_member_for(orders_view, orders.status) == Some(orders_view.status)`;
  - `unknown_names_depend_on_their_whole_cube`: `orders.nope` → `{Cube(orders)}`;
  - `cycles_terminate`: `a → {b}`, `b → {a}`.
- [ ] **Step 2:** rodar `cargo test -p cubeserver --lib dependencies` e ver falhar.
- [ ] **Step 3:** implementar `MemberGraph` e ligá-lo a `DataSources::load`.
- [ ] **Step 4:** rodar a suíte Rust completa. Deve passar, e os testes existentes de `DataSources` não mudam.
- [ ] **Step 5:** commit `feat(cubeserver): member dependency graph in the data-source snapshot`.

### Task 4: Aplicar as restrições nas consultas

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/restrictions.rs` (`Effective`, `restrict_query`)
- Modify: `rust/cube/cubeserver/src/rbac/enforce.rs`:
  - `members_of` vira `pub(super)`;
  - `query_access` passa a devolver também o workspace;
  - nova `restrictions_of`;
  - `RbacQueryService::authorize` devolve a requisição reescrita, e `load`, `sql` e `dry_run` usam essa versão.
- Modify: `rust/cube/cubeserver/tests/rbac_api.rs` (`DATA_MODEL` ganha `a_orders.region`, `a_orders.margin`, `a_orders.margin_pct` com `sql: "{margin} * 100"`, e uma view `a_view` com `a_orders.count` e `a_orders.status`, **sem** `region`)
- Test: `restrictions.rs` (unidade) e `tests/rbac_api.rs`

**Interfaces:**
- Consumes: Task 1 (`Restrictions`, `Attributes`, `Store::user_attributes`, `Role.restrictions`) e Task 3 (`DataSources::graph()`, `MemberGraph`, `Dep`).
- Produces:
  ```rust
  /// O que vale para um usuário num workspace (ou, para o meta sem workspace, em todos).
  #[derive(Debug, Clone, Default)]
  pub struct Effective { pub row_filters: Vec<RowFilter>, pub blocked: BTreeSet<String>, pub attributes: Attributes }
  impl Effective {
      pub fn combine<'a>(roles: impl IntoIterator<Item = &'a Restrictions>, attributes: Attributes) -> Effective;
      pub fn is_empty(&self) -> bool; // sem filtros e sem bloqueios
      /// O primeiro membro bloqueado no fecho de `name`, considerando que Dep::Cube(c) atinge qualquer bloqueado de `c`.
      pub fn blocked_dependency(&self, graph: &MemberGraph, name: &str) -> Option<String>;
  }
  pub fn restrict_query(query: NormalizedQuery, effective: &Effective, graph: &MemberGraph) -> Result<NormalizedQuery, ApiError>;
  ```
  Em `enforce.rs`:
  ```rust
  /// Rules do usuário no workspace (rule#member no SpiceDB, conferido com rules_by_ids e workspace_id),
  /// roles da organização dessas rules (role_id Some) e os atributos do usuário.
  /// `workspace: None` = todas as rules do usuário (usado pelo meta sem workspace).
  async fn restrictions_of(rbac: &Rbac, user: &str, workspace: Option<Uuid>) -> Result<Effective, ApiError>;
  ```
  - Nenhum cache [decisão do plano: mudanças valem na próxima consulta].
  - Platform admin não chama `restrictions_of`.
- O algoritmo de `restrict_query`:
  1. `names = members_of(&query)`.
  2. Para cada nome, se `effective.blocked_dependency(graph, name)` é `Some(b)`, dá 403:
     - `Permission denied: {b} is not available to you` quando `b` é o próprio membro (resolvido);
     - senão `Permission denied: {name} depends on {b}, which is not available to you`.
  3. `read_cubes` = os cubes de todos os `Dep` dos fechos de `names`. `direct` = algum nome cuja primeira parte não é view. `views` = as primeiras partes que são views.
  4. Para cada `RowFilter` cujo cube (`member` antes do `.`) está em `read_cubes`:
     - Se `graph.source_member(member)` é `None` (o membro saiu do modelo), dá 403 `Permission denied: a restriction of yours filters on {member}, which the model no longer has`. **Isso difere do spec**, que mandava ignorar com aviso: ignorar um filtro de linha abriria as linhas do cube. Ignorar só é seguro para membro bloqueado, que continua tratado como no spec.
     - `values` = os fixos ou `attributes[attribute]`. Sem o atributo ou com a lista vazia → 403 `Permission denied: you have no {attribute} attribute, which {cube} needs`.
     - Se `direct`, acrescentar ao `query.filters` (lista de cima, que é E) a folha `NormalizedFilterLeaf { member, operator: Equals | NotEquals, values: Some(values.map(Some)) }`.
     - Para cada view `v` em `views` que inclui algum membro do cube do filtro, `graph.view_member_for(v, member)`:
       - se existe, acrescentar a mesma folha com esse membro;
       - se não existe, 403 `Permission denied: view {v} does not expose {member}, which filters your rows`.
     - Se o cube está em `read_cubes` só por dependência, sem `direct` e sem uma view que o inclua, acrescentar a folha com o membro de cube [o planner fará o join ou falhará; as duas saídas são seguras].
  5. Os filtros do usuário nunca são reescritos, só acrescidos.
  6. Cada consulta de `NormalizedRequest.queries` passa pelo algoritmo.

- [ ] **Step 1: Testes de unidade em `restrictions.rs`** (com um `MemberGraph` construído do modelo de teste):
  - `no_restrictions_leave_the_query_untouched`;
  - `a_blocked_member_is_refused_wherever_the_query_names_it`: em `dimensions`, `filters` (dentro de um `or`), `order`, `segments` e `time_dimensions`;
  - `a_member_built_on_a_blocked_one_is_refused`: `a_orders.margin_pct` com `a_orders.margin` bloqueado → a mensagem com "depends on";
  - `a_row_filter_uses_the_users_attribute`: `region in [Sul, Sudeste]`, e a folha injetada é `a_orders.region equals ["Sul","Sudeste"]`;
  - `fixed_values_and_not_equals`;
  - `a_missing_or_empty_attribute_is_refused`;
  - `filters_only_apply_to_cubes_the_query_reads`: o filtro em `b_orders` não entra numa consulta só de `a_orders`;
  - `injected_filters_are_anded_outside_the_users_or`: a consulta com `filters: [{or: [...]}]` vira `[{or: [...]}, leaf]`;
  - `a_view_without_the_filtered_member_is_refused`: `a_view.count` com filtro em `a_orders.region` → 403 "does not expose";
  - `a_view_exposing_the_member_is_filtered_through_it`: com o filtro em `a_orders.status`, a folha é `a_view.status`;
  - `filters_from_several_roles_all_apply`: `Effective::combine` de duas roles soma filtros e bloqueios;
  - `a_filter_on_a_member_the_model_lost_is_refused`.
- [ ] **Step 2:** rodar e ver falhar.
- [ ] **Step 3:** implementar `Effective`, `restrict_query`, `restrictions_of` e a mudança em `authorize`.
- [ ] **Step 4: Testes de API em `rbac_api.rs`.** Usam `data_scenario`, uma role da organização A com restrições, uma rule em `w1` com essa role e bia como membro, e `load_in` com asserções sobre `d.queries.seen()`:
  - `a_restricted_role_filters_the_rows_that_reach_the_planner`: bia com `region=[Sul]` → a consulta gravada tem a folha `a_orders.region equals ["Sul"]`;
  - `a_restricted_role_blocks_members`: bia pede `a_orders.margin` → 403 e nada chega ao planner;
  - `restrictions_apply_even_with_an_unrestricted_role`: bia também é `viewer` (de `data_scenario`) e continua filtrada (regra B);
  - `platform_admins_are_not_restricted`;
  - `restrictions_stay_in_their_workspace`: a mesma rule em `w1` não afeta consultas de bia em `w2` (com `b_orders`, depois de dar `query` a ela em `w2`);
  - `changing_attributes_takes_effect_on_the_next_query`: troca `region` para `[Norte]` e a próxima consulta traz `["Norte"]`;
  - `restrictions_apply_on_sql_and_dry_run`: `/v1/sql` e `/v1/dry-run` também recebem a folha.
- [ ] **Step 5: Confirmar que nenhum cache fica antes do `query_for`.** Ler os handlers de `/v1/load`, `/v1/subscribe`, do GraphQL e do WebSocket (`src/handlers/`, `src/ws/`, `src/graphql_adapter.rs`) e confirmar que todos chamam `state.query_for(&ctx)` antes de qualquer cache de resultado. Se algum não chamar, parar e reportar.
- [ ] **Step 6:** rodar a suíte Rust completa. Deve passar.
- [ ] **Step 7:** commit `feat(cubeserver): apply role restrictions to every query`.

### Task 5: Meta sem os membros bloqueados

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/enforce.rs` (`meta_access` devolve também `Effective`, via `restrictions_of(user, Some(ws))` com workspace e `None` sem; `RbacMetaService::filter` poda os membros)
- Test: `rust/cube/cubeserver/tests/rbac_api.rs`

**Interfaces:**
- Consumes: Task 4 (`restrictions_of`, `Effective::blocked_dependency`).
- Produces: no `/v1/meta`, dentro de cada cube ou view, as listas `measures`, `dimensions` e `segments` perdem os itens cujo `name` tem `blocked_dependency` `Some`. Sem workspace, `Effective` só usa a soma dos `blocked` (os filtros são ignorados). O GraphQL, que é construído do meta filtrado (`graphql_for`), herda a poda sem mudança.

- [ ] **Step 1: Testes:**
  - `meta_hides_blocked_members_and_what_depends_on_them`: bia em `w1` não vê `a_orders.margin`, `a_orders.margin_pct` nem nada de `a_view` que dependa deles; continua vendo `a_orders.count`;
  - `meta_without_a_workspace_hides_members_blocked_anywhere`;
  - `row_filters_do_not_change_the_meta`.
- [ ] **Step 2:** rodar e ver falhar.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** rodar a suíte Rust completa. Deve passar.
- [ ] **Step 5:** commit `feat(cubeserver): hide blocked members from the meta`.

### Task 6: Membros do modelo por organização e restrições no grafo de acessos

**Files:**
- Modify: `rust/cube/cubeserver/src/rbac/handlers/roles.rs` (handler `model_members`)
- Modify: `rust/cube/cubeserver/src/rbac/handlers/mod.rs`. A rota é `GET /v1/organizations/:id/model-members` [decisão do plano: `members` já significa pessoas nas rotas de workspace; o spec chamava de `/members`].
- Modify: `rust/cube/cubeserver/src/rbac/access.rs` e `handlers/users.rs` (o grafo mostra as restrições)
- Test: `tests/rbac_api.rs` e unidade em `access.rs`

**Interfaces:**
- Consumes: Task 3 (`MemberGraph`) e `store.connections_of_organization`.
- Produces:
  - `GET /v1/organizations/{id}/model-members` → `{cubes: [{name, title, measures: [{name, title}], dimensions: [...], segments: [...]}]}`.
    - Só cubes (sem views) cujo `data_sources_of_cube` está contido nos nomes das conexões da organização.
    - Exige `admin` na organização, por `visible_organization(rbac, id, "admin", user)`. Platform admin também passa.
    - O handler usa `state.meta` (o meta sem filtro) e `state.query.data_sources()`; os tenants são recusados com RBAC, então há um só modelo.
  - Em `access.rs`, para a fonte de uma rule com role restrita, o texto ganha o sufixo ` (filters: orders.region = region; blocks: orders.margin)`. O formato vem de `fn restriction_note(r: &Restrictions) -> Option<String>`, que devolve `None` quando a role não tem restrições.

- [ ] **Step 1: Testes:**
  - `organization_model_members_list_only_its_cubes`: a organização A vê `a_orders` e `b_orders` (as conexões dela), não `c_orders` nem `a_view`; bia recebe 403;
  - unidade `a_restricted_rule_explains_its_restrictions`, em `access.rs`.
- [ ] **Step 2:** rodar e ver falhar.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** rodar a suíte Rust completa. Deve passar.
- [ ] **Step 5:** commit `feat(cubeserver): organization model members and restrictions in the access graph`.

### Task 7: Playground — API, atributos em Users e restrições em Roles

**Files:**
- Modify: `packages/cubejs-playground/src/pages/Admin/api.ts`:
  - tipos `RowFilter`, `Restrictions`, `Attributes` e `ModelCube`;
  - `OrgRole.restrictions`;
  - `userAttributes`, `setUserAttributes` e `modelMembers`;
  - `createRole`/`updateRole` aceitam `restrictions`.
- Create: `packages/cubejs-playground/src/pages/Admin/AttributesModal.tsx`
- Create: `packages/cubejs-playground/src/pages/Admin/RestrictionsFields.tsx`
- Modify: `packages/cubejs-playground/src/pages/Admin/UsersTab.tsx` (botão "Attributes" por usuário, que abre o modal)
- Modify: `packages/cubejs-playground/src/pages/Admin/RolesTab.tsx` (o modal inclui `RestrictionsFields`; a tabela ganha a coluna "Restrictions" com o resumo)
- Test: `packages/cubejs-playground/src/__tests__/admin-api.test.ts`

**Interfaces:**
- Consumes: HTTP das Tasks 2 e 6.
- Produces:
  ```ts
  export type RowFilter = { member: string; operator: 'equals' | 'notEquals'; attribute?: string; values?: string[] };
  export type Restrictions = { rowFilters: RowFilter[]; blockedMembers: string[] };
  export type Attributes = Record<string, string[]>;
  export type ModelCube = { name: string; title?: string; measures: ModelMember[]; dimensions: ModelMember[]; segments: ModelMember[] };
  export type ModelMember = { name: string; title?: string };
  export function restrictionSummary(r: Restrictions): string; // "2 filters · 3 blocked", "" se vazio; singular "1 filter", "1 blocked"
  // AdminApi
  userAttributes(user: string): Promise<Attributes>;
  setUserAttributes(user: string, attributes: Attributes): Promise<Attributes>;
  modelMembers(org: string): Promise<ModelCube[]>;
  ```
- UI:
  - `RestrictionsFields({ cubes: ModelCube[] })` fica dentro do `Form` da role, nos campos `restrictions.rowFilters` (`Form.List`) e `restrictions.blockedMembers`.
    - Cada filtro tem: membro (`Select` de dimensões); operador (`equals`/`notEquals`); fonte (`Radio`: "User attribute" ou "Fixed values"); e o valor (`Input` com o nome do atributo, ou `Select mode="tags"`).
    - Colunas bloqueadas: `Select mode="multiple"` com todos os membros, agrupado por cube.
    - Ao salvar, o formulário manda `attribute` **ou** `values`, conforme a fonte escolhida.
  - `AttributesModal({ api, user, onClose })` usa `Form.List` de pares nome → valores (`Select mode="tags"`) e salva com `setUserAttributes`.
  - Layout de referência: o esboço do spec, seção "Playground (Admin)".

- [ ] **Step 1: Testes em `admin-api.test.ts`:**
  - `userAttributes and setUserAttributes call the attributes route`: GET e PUT em `/v1/users/bia/attributes`, com corpo `{attributes}`;
  - `modelMembers calls the organization route`: `/v1/organizations/o1/model-members` → `cubes`;
  - `createRole sends restrictions`;
  - `restrictionSummary`: `{rowFilters:[f,f],blockedMembers:[a,b,c]}` → `"2 filters · 3 blocked"`, um de cada → `"1 filter · 1 blocked"`, vazio → `""`.
- [ ] **Step 2:** rodar `vitest run src/__tests__/admin-api.test.ts` e ver falhar.
- [ ] **Step 3:** implementar o `api.ts`, os dois componentes e as mudanças nas abas.
- [ ] **Step 4:** rodar `vitest run`, `tsc --noEmit -p .` e `oxlint@1.82.0` nos arquivos tocados. Tudo deve passar, e o oxlint não deve mostrar saída.
- [ ] **Step 5:** commit `feat(playground): edit user attributes and role restrictions`.

### Task 8: Documentação e deploy

**Files:**
- Modify: `/root/cube-rust/README.md`, fora do repositório, depois de copiá-lo para `README.md.bak-<data>`. Documentar:
  - as rotas novas;
  - o formato de `restrictions`;
  - a regra B;
  - o "na dúvida, bloqueia";
  - **que a porta SQL do Postgres não aplica as restrições das roles** (decisão do usuário).

- [ ] **Step 1:** rodar a suíte Rust, `vitest` e `tsc`. Tudo deve passar.
- [ ] **Step 2: Build e deploy** pelo fluxo usual:
  - fazer backup do binário em `/root/cube-rust/cube-server.bak-<data>-restrictions`;
  - `cargo build --release -p cubeserver --bin cube-server`;
  - `vite build`;
  - `/root/cube-rust/install-playground.sh <repo>/packages/cubejs-playground/build`;
  - `/root/cube-rust/start-all.sh restart`.
- [ ] **Step 3: Verificar no ambiente:**
  - `readyz` 200;
  - `\d roles` tem `restrictions`, e `user_attributes` existe;
  - `GET /cube/v1/users/x/attributes` sem token → 401.
- [ ] **Step 4:** atualizar o README e fazer o commit no repositório de tudo o que faltar.

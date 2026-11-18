# Restrições de dados nas roles (colunas e linhas) — design

Continuação de `2026-10-28-cube-rbac-spicedb-design.md` e do trabalho de rules e roles por organização (PR #10). Vale tudo o que está lá: SpiceDB, tabelas do RBAC, rotas `/v1/...`, rules, roles da organização e as seis permissões de workspace.

## Intenção

Hoje uma role só diz **o que a pessoa pode fazer no workspace**: `view`, `query`, `read_objects`, `write_objects`, `execute` e `manage`. Na consulta, o servidor só barra por workspace e por data source (`rbac/enforce.rs`). Ou a pessoa vê um cube inteiro, ou não vê nada dele.

O usuário quer que a role também limite **quais dados** a pessoa vê quando consulta:
1. **Colunas:** bloquear colunas (membros) de um cube para quem tem a role.
2. **Linhas:** filtrar automaticamente as linhas, com base em **atributos do usuário**, como a região dele (palavras do usuário: "o filtro depende de atributos do usuário, como região").

Um terceiro pedido, **bloquear objetos** (pastas, relatórios etc.), fica para um projeto à parte, depois deste. Os objetos hoje são uma lista plana por workspace, sem pastas e sem permissão por objeto, e isso precisa de um design próprio.

## Objetivos

1. **Atributos do usuário.** Cada usuário tem atributos com nome e uma lista de valores, por exemplo `region = [Sul, Sudeste]` e `cost_center = [1020]`.
   - São editados em Admin → Users.
   - Valem para o usuário em todas as organizações [decisão do Claude: os usuários são locais, sem SSO, e um valor por organização seria mais complexo sem pedido para isso].
2. **Filtros de linha na role.** Uma role da organização pode ter filtros do tipo "no cube `orders`, `orders.region` está em {atributo `region` do usuário}".
3. **Colunas bloqueadas na role.** Uma role da organização pode listar membros bloqueados: dimensões, medidas e segmentos de um cube.
4. **A restrição sempre vale** (decisão do usuário, opção B). Se a pessoa tem várias roles no mesmo workspace, todas as restrições de todas elas se aplicam juntas:
   - os filtros se combinam com E;
   - as colunas bloqueadas se somam.
   - Uma role sem restrições não remove as restrições de outra.
5. **Vale em todos os caminhos de dados com usuário do RBAC**, que já passam pelo `enforce.rs`: REST `/v1/load`, `/v1/sql`, `/v1/dry-run`, `/v1/subscribe`, GraphQL e WebSocket.
   - **Princípio (palavras do usuário):** "se o usuário possui acesso direto ao banco, o banco controla; se passa pelo nosso sistema, nós é que controlamos a exibição dos dados".
   - A SQL API pela porta do Postgres (`CUBEJS_PG_SQL_PORT`, login de serviço `CUBEJS_SQL_USER`) fica **como está**, sem as restrições das roles (decisão do usuário), porque ela não tem usuário nem workspace do RBAC. As conexões externas também ficam como estão.
6. **Na dúvida, bloqueia** (como o resto do `enforce.rs`):
   - usuário sem o atributo que o filtro usa não vê nenhuma linha;
   - restrição que não dá para aplicar com segurança recusa a consulta (403), em vez de rodá-la sem a restrição.
7. **O admin edita tudo pela tela**, sem mexer nos arquivos do modelo (motivo da escolha da abordagem 1).

## Não-objetivos

- Bloquear objetos, pastas e relatórios (próximo projeto).
- Mascarar valores (mostrar `***` em vez de recusar). O planner tem `masked_members` e isso pode vir depois, mas aqui uma coluna bloqueada é recusada e escondida.
- Implementar o `access_policy` dos YAML do Cube. O planner em Rust continua ignorando esse campo (`cubeplanner/src/model_check.rs`).
- Restrições nos papéis prontos (owner/editor/viewer). Eles continuam sem restrição.
- Atributos vindos de um provedor de identidade externo (SSO, claims de JWT).

## Anti-objetivos (seria um fracasso mesmo funcionando)

- **Uma consulta de um usuário do RBAC devolver linhas ou colunas que uma role dele restringe**, por qualquer caminho do nosso sistema: view, join, filtro, ordenação ou cache. A porta SQL de serviço fica fora (objetivo 5).
- Dar uma role a mais a alguém abrir dados que outra role dele restringe.
- Uma falha (SpiceDB, banco, snapshot do modelo) virar "sem restrição". Falha é 503, como hoje.

## Quem passa sem restrição

- **Platform admins:** já passam por tudo no `enforce.rs` (`Access::All`) e continuam assim.
- **Os papéis prontos não trazem restrições.**
- Todo o resto, inclusive admins de organização e quem tem `manage`, recebe as restrições das roles que tiver no workspace [decisão do Claude, coerente com a regra B: quem administra pode tirar a si mesmo da rule].

## Abordagens consideradas

1. **Restrições guardadas na role, aplicadas pelo servidor** — escolhida pelo usuário.
   - Ficam no banco do RBAC e são editadas em Admin → Roles.
   - O `enforce.rs` reescreve a consulta normalizada antes do planner e poda o `/v1/meta`.
   - A favor: tudo num lugar só, a regra B fica fácil de garantir, e o admin trabalha pela tela.
   - Contra: é um mecanismo próprio, não o padrão do Cube.
2. **Implementar o `access_policy` do Cube no planner.**
   - As políticas ficariam nos YAML do modelo.
   - Contra: quem edita o modelo é quem decide as restrições. O Cube combina políticas liberando o que qualquer uma permite (regra A, o oposto da escolhida). E dá muito mais trabalho no planner.
3. **As duas.**
   - Contra: dois sistemas para manter coerentes, sem modelos com `access_policy` que justifiquem.

## Desenho

### Dados (Postgres do RBAC, `migrations.sql`, idempotente)

- `user_attributes (user_id text REFERENCES users ON DELETE CASCADE, name text, values text[] NOT NULL, PRIMARY KEY (user_id, name))`.
  - O nome segue o formato de identificador (`a-z`, `0-9`, `_`, até 64 caracteres) [decisão do Claude].
  - A lista de valores pode ser vazia. Lista vazia equivale a não ter o atributo: a pessoa não vê nenhuma linha.
- Na tabela `roles`, uma coluna `restrictions jsonb NOT NULL DEFAULT '{}'` [decisão do Claude: as restrições são lidas sempre junto com a role, e nunca consultadas por partes]. Formato:

```json
{
  "rowFilters": [
    { "member": "orders.region", "operator": "equals", "attribute": "region" },
    { "member": "orders.status", "operator": "notEquals", "values": ["internal"] }
  ],
  "blockedMembers": ["orders.margin", "customers.cpf"]
}
```

  - `operator`: `equals` ou `notEquals`. Com `equals` e lista de valores, a linha passa se o membro for qualquer um deles.
  - A fonte dos valores é **um** destes campos:
    - `attribute`: o nome de um atributo do usuário;
    - `values`: valores fixos.
    [decisão do Claude: valores fixos custam pouco e cobrem casos como esconder status internos.]
  - `member` é sempre um membro de **cube**, nunca de view. A tela só oferece membros de cubes.
  - `blockedMembers` também lista membros de cube: dimensões, medidas e segmentos.
  - O servidor valida o formato ao salvar (400 para operador desconhecido, membro mal formado ou as duas fontes de valor juntas).
  - O servidor **não** exige que o membro exista no modelo na hora de salvar, porque o modelo pode mudar depois. Na consulta, uma restrição sobre um membro que não existe mais é ignorada com um aviso no log. Ela não pode abrir nada, porque o membro também não pode ser consultado.

### API

- `GET/PUT /v1/users/{id}/attributes`: só platform admins, como o resto de Users. O `PUT` recebe `{attributes: {name: [values]}}` e substitui o conjunto inteiro [decisão do Claude: simples para a tela].
- `POST /v1/organizations/{id}/roles` e `PATCH /v1/roles/{id}` aceitam `restrictions` além de `name` e `permissions`. O `GET` devolve as restrições.
- `GET /v1/users/{id}/access` (o grafo de acessos): passa a mostrar, para cada rule com role restrita, o que ela filtra e bloqueia, porque a pergunta "por que essa pessoa não vê isso?" é a mesma do grafo [decisão do Claude; dá para cortar se ficar grande].

### Na consulta (`rbac/enforce.rs`)

`query_access` já resolve o workspace e checa `workspace#query`. Depois disso:

1. **Reunir as restrições do usuário no workspace:**
   - as rules do workspace em que o usuário é membro (relação `rule#member` no SpiceDB, conferida com as linhas do banco, como o handler de access já faz);
   - as roles da organização dessas rules;
   - a soma de todas as restrições.
   - Mais os atributos do usuário.
   - Platform admin pula esta etapa.
2. **Colunas bloqueadas:**
   - Qualquer membro que a consulta lê é recusado com 403 se estiver bloqueado: medidas, dimensões, segmentos, dimensões de tempo, filtros, ordenação, join hints e masked members (a lista de `members_of`).
   - O mesmo vale se for um membro de view que aponta para um membro de cube bloqueado.
   - Mensagem: "Permission denied: orders.margin is not available to you".
3. **Filtros de linha:**
   - Para cada filtro cujo cube é lido pela consulta (algum membro dele está no fecho de dependências do passo 4, direto, por view ou por referência), acrescentar um filtro folha à consulta, ligado com E aos filtros do usuário.
     - Com `attribute`, os valores são os do usuário.
     - Sem o atributo, ou com lista vazia, a consulta é recusada com 403 ("you have no `region` attribute"). Recusar em vez de devolver vazio deixa claro por que não há dados [decisão do Claude].
   - Se a consulta lê o cube **por uma view**, o filtro vai no membro da view que aponta para o membro filtrado.
     - Se a view não expõe esse membro, a consulta é recusada (403). Na dúvida, bloqueia.
     - É o caso mais delicado do projeto. O plano precisa de testes dele.
   - A ordem importa: o filtro injetado não pode ser removido nem contornado por `or` do usuário. Por isso a consulta vira `E(filtros do usuário, filtros injetados)`, no nível de cima.
4. **Dependências do modelo** (decisão do usuário: o bloqueio e os filtros seguem as dependências, não só os nomes pedidos).
   - Cada membro lido pela consulta é expandido para o fecho transitivo dos membros de cube de que ele depende:
     - referências `{cube.member}`, `{member}` e `{CUBE}.member` no `sql`;
     - `filters` de medidas;
     - medidas calculadas (`type: number` sobre outras medidas);
     - `sql` de segmentos;
     - dimensões de subquery (`sub_query`);
     - membros de view, pelo membro de cube a que apontam.
   - **Bloqueio:** a consulta é recusada (403) se qualquer membro desse fecho estiver bloqueado. Exemplo: bloquear `orders.margin` recusa `orders.margin_pct`, que usa `{orders.margin}`. A mensagem nomeia o membro pedido e o bloqueado ("orders.margin_pct depends on orders.margin, which is not available to you").
   - **Filtros:** um cube "é lido pela consulta" se algum membro dele está no fecho. Um membro de `orders` que puxa `{customers.segment}` por subquery faz o filtro de `customers` valer.
   - **Meta:** esconde também os membros cujo fecho inclui um bloqueado.
   - **Na dúvida, bloqueia:** uma referência que não dá para resolver é tratada como dependência de **todo** o cube referenciado. Isso também vale para SQL que o parser não entende. Ou seja, se o cube tem qualquer membro bloqueado, a consulta é recusada.
   - Hoje não existe nada que calcule isso (`cubemodel/src/views.rs::resolve_reference` só resolve um nome). O grafo é construído junto com o snapshot (passo 5), a cada carga do modelo, e não a cada consulta.
5. **O snapshot `DataSources`** (`planner_adapter.rs`) já mapeia membro de view para o data source do cube de origem. Ele passa a guardar também:
   - membro de view → membro de cube;
   - para cada cube, os membros de view que apontam para seus membros.
   - o grafo de dependências do passo 4.
   - É construído no mesmo lugar e momento, então vale a mesma garantia de hoje: o que é checado é o que roda.
6. **Pre-aggregations:**
   - Os filtros injetados podem fazer a consulta não casar com uma pre-aggregation e ir para o banco. Isso é aceitável: corretude antes de velocidade.
   - Uma pre-aggregation que inclua um membro bloqueado não pode servir esse membro, e o passo 2 já cobre isso, porque a checagem é sobre a consulta.
7. **Cache de resultados:**
   - A consulta reescrita é a que chega ao planner e ao orquestrador, e a chave de cache vem dela. Usuários com atributos diferentes geram consultas diferentes e não compartilham resultado.
   - O plano precisa confirmar que nenhum cache fica antes do `enforce.rs`.

### `/v1/meta`

- Com workspace, além do filtro por data source de hoje, o meta remove os membros bloqueados, inclusive os membros de view que apontam para eles.
- Filtros de linha não mudam o meta.
- **Sem workspace** (o meta une as conexões de todos os workspaces do usuário), o meta esconde os membros bloqueados por **qualquer** role do usuário, em qualquer workspace [decisão do Claude: na dúvida, esconde; consultar exige workspace de qualquer jeito].

### SQL API

- **Porta do Postgres** (`main.rs` monta o `OrchestratedExecutor` com o `PlannerQueryService` sem RBAC, e `sql_api_config` não tem autenticador): **não muda** (decisão do usuário). Ela é um acesso de serviço, sem usuário do RBAC; quem tem esse login responde pelos dados. O README do deploy deve dizer isso claramente.
- `/v1/sql` (REST) passa pelo `enforce.rs` e recebe as restrições. `/v1/cubesql` e `/v1/convert-query` já são só para platform admins.
- Consultas com SQL próprio (member expressions, subquery joins) continuam recusadas para quem não é platform admin, como hoje.

### Playground (Admin)

- **Admin → Users:** cada usuário ganha uma seção "Atributos", com pares nome → valores. Os valores são editados como tags.
- **Admin → Roles:** o formulário da role ganha duas seções, abaixo das permissões.

```
Permissões ............ [x] view [x] query ...

Filtros de linha
  Membro            Operador   Valor
  [orders.region ▾] [equals ▾] (•) atributo do usuário [region      ]
                               ( ) valores fixos      [            ]
  [+ filtro]

Colunas bloqueadas
  [orders.margin ×] [customers.cpf ×]   [escolher membro ▾]
```

  - Quem edita roles é admin da organização, não necessariamente platform admin. A lista de membros vem de uma rota nova, `GET /v1/organizations/{id}/members`, com os cubes (não views) cujos data sources são conexões dessa organização, e nada de outras organizações [decisão do Claude].
    - O modelo é o mesmo que o `/v1/meta` usaria para quem chama, sem workspace (`runtime_for`).
  - A tabela de roles mostra, ao lado das permissões, um resumo como "2 filtros · 3 colunas".

## Restrições técnicas

- antd 4.16.13 (sem `Form.useWatch`; usar `Form.List` para os filtros), estilo do Playground (`--pg-*`, sem violeta).
- Escritas: transação no Postgres e, quando houver, escrita no SpiceDB antes do commit, como nos handlers atuais. Restrições e atributos não vão para o SpiceDB: ficam só no Postgres.
- Testes:
  - Rust: `rbac_api` com o FakeAuthz e o Postgres de teste;
  - injeção de filtros e bloqueio de colunas: testes de unidade sobre `NormalizedQuery`, mais testes de API ponta a ponta;
  - Playground: `vitest` e `tsc`.

## O que fica com quem constrói

- A forma exata de buscar as rules, roles e atributos do usuário por consulta, e se vale um cache curto por (usuário, workspace). Uma mudança de role ou atributo deve valer em até alguns segundos.
- Os nomes das funções e a divisão em arquivos dentro de `rbac/`.
- O texto exato das mensagens de erro, mantendo o padrão "Permission denied: ...".

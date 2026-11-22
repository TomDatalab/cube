# Árvore do Data Model por Organization — design

Data: 2026-11-20
Código: `/home/antonio-rodrigues/Documents/GitHub/cube` (branch `feat/datasource-owner-organization`)
Deploy: `/root/cube-rust` (modelo em `/root/cube-rust/model`, binário em `rust/cube/target/release/cube-server`)

## Intenção

A página **Data Model** do Playground (`packages/cubejs-playground/src/pages/Schema/SchemaPage.tsx`) hoje tem duas abas, *Tables* e *Files*, sem noção de organização e sem nenhuma visão dos cubos. Ela é usada pelos **platform admins** (e continua sendo só deles), que precisam enxergar e administrar o modelo de muitas organizações separadamente, de um jeito que continue usável quando houver muitas.

O que muda:

1. Uma árvore única substitui as abas: **Organization → Cubes / Tables / Files**.
2. Os arquivos do modelo passam a morar em `model/<org>/`, e **a pasta define a organização dona do cubo** (opção "B" da conversa).
3. Um nó **Cubes** novo: clicar num cubo mostra a definição e oferece **Abrir no Playground**.

## Objetivos

- **Quem acessa:** só platform admins, como hoje (`require_platform_admin` nas rotas `/playground/*` com RBAC ligado; `AdminOnly` na tela). A árvore lista todas as organizações, mais o nó **Platform** (`_platform`).
- Cada organização mostra três nós irmãos:
  - **Cubes**: os cubos e views cujo arquivo está em `model/<org>/`.
  - **Tables**: as tabelas das connections da organização (as de `connections.organization_id`), agrupadas por connection e schema. Continuam com checkbox e **Generate Data Model**.
  - **Files**: os arquivos de `model/<org>/`, com subpastas (por exemplo `sofiadb/`).
- **Escala:** a lista de organizações vem com contagens (cubos, arquivos e connections, tudo calculado do modelo carregado e do banco do app, sem conectar a nenhum data source) e sem conteúdo. O conteúdo de cada organização só é buscado quando o nó é aberto. Há uma busca no topo que filtra organizações, cubos, tabelas e arquivos já carregados e pergunta ao servidor pelos nomes de organização.
- **Cubo selecionado:** o painel da direita mostra título, descrição, data source, arquivo de origem, measures (nome, tipo, título), dimensions (nome, tipo, primary key), segments, joins (cubo alvo, relação) e, para view, os cubos que ela inclui. Também mostra o botão **Abrir no Playground**, que vai para o construtor de consultas (`/build`) com o cubo pré-selecionado (as measures, ou a primeira dimension se não houver measure).
- **Posse pela pasta:** um cubo em `model/<org>/` cujo `data_source` não é connection daquela organização é **recusado**: ele fica fora do modelo compilado e o erro aparece na árvore, no nó da organização, e no log. O resto do modelo continua servindo.
- **Generate Data Model** grava em `model/<org>/` da organização dona da connection, e não mais na raiz.

## Fora do escopo

- Editar cubos pela tela. Os arquivos continuam sendo editados no disco, e o servidor recarrega sozinho.
- Um modelo compilado por organização (opção "C"). Os nomes de cubo continuam **únicos na plataforma**.
- Pôr o SQL API (porta 15432) sob RBAC, que continua fora, como já documentado.
- Mudar quem vê os dados em `/meta` e `/load`. A regra atual (connections liberadas ao workspace) continua valendo. A pasta só passa a ter que concordar com ela.
- Abrir a página Data Model ou as rotas `/playground/*` para quem não é platform admin (owners e membros continuam sem acesso).

## Anti-objetivos (falha mesmo que "funcione")

- Alguém que não é platform admin alcançar as rotas novas. Elas ficam no mesmo grupo protegido por `require_platform_admin`.
- Um cubo aparecer na organização errada na árvore. A árvore precisa refletir a regra de posse que o servidor aplica.
- A página ficar lenta com centenas de organizações por carregar tudo de uma vez.
- Um arquivo mal colocado derrubar o modelo inteiro. Um cubo recusado não pode impedir os outros de compilar.
- Renomear uma organização quebrar a pasta dela.

## Restrições encontradas

- O servidor compila `model/` inteiro, com subpastas, como **um modelo só** (`tenants.rs`, `ModelMetaService::load_with_context`). As subpastas já funcionam (`model/sofiadb/`).
- O RBAC fica em `rust/cube/cubeserver/src/rbac/`. `enforce.rs` filtra `/meta` e `/load` pelos data sources que o workspace pode ler (`meta_access`, `check_queries`, o `MetaService` que poda cubos). `GET /organizations/{id}/model-members` já calcula os cubos de uma organização pelos data sources.
- `organizations` (`rbac/migrations.sql`) tem `id`, `name`, `created_at` e `created_by`, sem identificador curto.
- Rotas `/playground/*` (`handlers/playground.rs`) existem para `files`, `db-schema` e `generate-schema`. Com o RBAC ligado, todas passam por `auth_middleware` + `require_platform_admin`. A tela esconde a página de não-admins (`AdminOnly` em `index.tsx`, flag `admin` no Sidebar), e `IndexPage.tsx` não chama `playground/files` para não-admins. As rotas novas entram nesse mesmo grupo.
- Situação hoje: as organizações são **ON** e **Digital**. As connections `sofiadb` e `example` são da Digital. O data source `default` não tem organização (é da Platform).
- `sync-sofiadb-model.py` (no deploy) escreve em `model/sofiadb/` (`OUT_DIR`) e considera "coberta à mão" qualquer tabela que já tenha cubo fora dessa pasta.

## Abordagens consideradas

1. **A tela monta a árvore com as rotas existentes** (`/meta`, `playground/db-schema`, `playground/files`). É a mais rápida, mas carrega tudo de uma vez e `files` vaza os arquivos de todas as organizações. Descartada.
2. **Rota nova no servidor, por organização (escolhida).** O servidor sabe a organização de cada arquivo e cubo, aplica o RBAC e entrega uma organização por vez. Dá mais trabalho no Rust, mas escala e não vaza nada.
3. **Um modelo compilado por organização** ("C"). Isolaria até os nomes, mas multiplica compilador e cache por organização. Descartada por ora. A escolhida não fecha essa porta.

## Decisões

### Identificador da pasta: `slug`

- Coluna nova `organizations.slug text UNIQUE NOT NULL`, formato `[a-z0-9][a-z0-9-]{0,62}`, **imutável**. O `PATCH` de nome não a altera.
- Gerada na criação a partir do nome (minúsculas, sem acento, espaços e símbolos viram `-`), com sufixo `-2`, `-3`… em caso de colisão. `POST /organizations` aceita `slug` opcional para escolher.
- A migração preenche as existentes: `ON` → `on`, `Digital` → `digital`.
- Reservados: `_platform` e qualquer nome começando com `_` ou `.`.
- Aparece nas respostas de organização (`{id, name, slug, ...}`).

### Layout do modelo

```
model/
  _platform/        cubos dos data sources sem organização (ex.: default)
  digital/
    fatos.yml ...
    sofiadb/        gerado pelo sync-sofiadb-model.py
  on/
```

- A organização de um arquivo é o **primeiro segmento** do caminho relativo a `model/`.
- Um arquivo solto na raiz de `model/`, ou numa pasta que não corresponde a nenhum slug nem a `_platform`, fica **sem dono**: seus cubos são recusados com o erro "pasta não pertence a nenhuma organização". Com o RBAC desligado, a regra de posse não se aplica e tudo compila como hoje.

### Regra de posse (servidor)

- Ao compilar, cada cubo e view é marcado com a organização da sua pasta.
- **Cubo:** o `data_source` dele (ou `default`) precisa ser connection daquela organização. Em `_platform/`, precisa ser um data source sem organização. Se não for, o cubo é recusado.
- **View:** os cubos que ela inclui precisam ser todos da mesma organização da view.
- **Join** para cubo de outra organização: o cubo que declara o join é recusado.
- A recusa é feita antes de compilar (filtrando as definições), para não derrubar o modelo. Se a recusa de um cubo quebrar outro que dependia dele, esse outro também é recusado, com uma mensagem que diz o porquê.
- As recusas ficam guardadas junto do modelo carregado e são servidas pela rota da árvore (abaixo) e pelo log.
- **Quando uma connection muda de organização** (`PUT/DELETE /connections/{name}/organization`), os cubos dela ficam recusados até os arquivos serem movidos para a pasta nova. A resposta e o log dizem quais arquivos mover. Mover os arquivos automaticamente fica de fora; ver "Deixado ao builder".

### Rotas novas

Ambas ficam no grupo de rotas do Playground protegido por `require_platform_admin` (com o RBAC ligado). Não há filtro por usuário: o admin vê todas as organizações e `_platform`.

`GET /playground/tree?q=&limit=&offset=`

```json
{ "organizations": [
    { "id": "…", "slug": "digital", "name": "Digital",
      "counts": { "cubes": 27, "files": 31, "connections": 2 }, "errors": 0 } ],
  "total": 2 }
```

- Ordenada por nome. `q` filtra por nome ou slug. Paginada (padrão de 50).
- Para o admin, inclui `{ "slug": "_platform", "name": "Platform" }`.

`GET /playground/tree/{slug}`

```json
{ "cubes":  [ { "name": "fatos", "title": "…", "type": "cube|view", "dataSource": "sofiadb",
                "file": "digital/fatos.yml", "members": { "measures": 4, "dimensions": 9 } } ],
  "tables": { "sofiadb": { "dfos": { "fibercable": [ {"name":"…","type":"…"} ] } } },
  "files":  [ { "fileName": "digital/sofiadb/dfos_fibercable.yml" } ],
  "errors": [ { "file": "digital/x.yml", "cube": "x", "message": "…" } ] }
```

- `tables` reaproveita a lógica de `db_schema`, restrita às connections da organização (só aqui, ao expandir, os data sources são consultados). A falha de uma connection vai em `errors` sem derrubar o resto, como `db_schema` já faz. Tabelas, cubos e arquivos podem vir em chamadas separadas se a lentidão de um banco atrasar o resto (decisão do builder).
- `files` traz só o nome. O conteúdo vem sob demanda em `GET /playground/tree/{slug}/files/{path}`, com `path` validado como relativo, sem `..` e dentro da pasta.
- A definição de um cubo selecionado vem do `/cube/v1/meta` (measures, dimensions, segments, joins). O admin não precisa de `X-Cube-Workspace` e vê todos os cubos. Um cubo recusado não está no meta: o painel mostra a recusa em vez da definição e desabilita **Abrir no Playground**.

### Rotas existentes que mudam

- `GET /playground/files` e `GET /playground/db-schema`: o acesso não muda (admin). `files` passa a incluir o campo `organization` (slug ou `_platform`, ou `null` para arquivo sem dono).
- `POST /playground/generate-schema`: grava em `model/<slug da organização dona da connection>/`, ou em `_platform/` para data source sem organização. Recusa (400) tabelas de mais de uma organização na mesma chamada.

### Playground (tela)

- `SchemaPage.tsx` troca `Tabs` por uma `Tree` com carregamento sob demanda (`loadData` do antd): o nível 1 vem de `/playground/tree` e o nível 2 (Cubes/Tables/Files) de `/playground/tree/{slug}` ao expandir.
- A busca no topo filtra o que já foi carregado. Com pelo menos dois caracteres, também chama `/playground/tree?q=`.
- Painel da direita conforme o nó:
  - **Cubo:** a definição, mais **Abrir no Playground**.
  - **Arquivo:** o conteúdo (`PrismCode`, como hoje), mais o caminho absoluto.
  - **Organização com erros:** a lista de recusas.
  - **Nada selecionado:** o `Empty` de hoje.
- As tabelas continuam com checkbox, e **Generate Data Model** age só sobre as tabelas marcadas, que precisam ser de uma única organização (o botão desabilita, com dica, se houver mistura).
- Com o RBAC desligado: um nó único "Model", com Cubes/Tables/Files de `model/` inteiro (o comportamento de hoje reorganizado).

### Migração dos arquivos (deploy `/root/cube-rust`)

Feita uma vez, com backup de `model/` antes (`model.bak-20261120-org`):

| Origem | Destino | Por quê |
|---|---|---|
| `model/sofiadb/*` | `model/digital/sofiadb/` | connection `sofiadb` é da Digital |
| cubos em `model/*.yml` com `data_source: sofiadb` ou `example` (`fatos`, `fatos_local`, `eventos`, `pos_falha`, `dfos_schema_meta`, `main_exemplo` — conferir arquivo a arquivo) | `model/digital/` | connection da Digital |
| `users`, `orders`, `products`, `line_items`, `sales` (`default`) | `model/_platform/` | data source sem organização |
| `model/on/` | criada vazia | |

Depois de mover, confirmar que o servidor recarregou sem recusas e que `/meta` mostra os mesmos cubos de antes.

`sync-sofiadb-model.py` passa a consultar a organização dona da connection `sofiadb` (em `cube_app.connections` + `organizations.slug`) e a escrever em `model/<slug>/sofiadb/`. Sem dono, escreve em `model/_platform/sofiadb/`. A regra de "tabela coberta à mão" continua valendo para o modelo inteiro. O README do deploy é atualizado.

## Testes esperados

- Unidade (Rust): geração de slug (acentos, colisão, reservados), resolução de organização por caminho, as regras de posse (cubo, view, join, dependência em cascata), validação de `path`.
- Integração (`tests/rbac_api.rs` com o `fake.rs`): quem não é admin recebe 403 em `/playground/tree` e `/playground/tree/{slug}`. O admin vê todas as organizações e `_platform`, e cada cubo aparece na organização da sua pasta. `/tree/inexistente` dá 404. Um cubo mal colocado é recusado e aparece em `errors`, e o resto do modelo continua respondendo.
- Playground: a árvore carrega o nível 2 só ao expandir, a busca funciona e **Abrir no Playground** abre `/build` com o cubo.

## Decisões tomadas por mim (Claude) durante a revisão

- A lista de organizações não conta tabelas: isso exigiria conectar a todos os bancos de todas as organizações ao abrir a página. Ela conta cubos, arquivos e connections, e as tabelas aparecem ao expandir.

## Deixado ao builder

- O formato exato do estado inicial da consulta em `/build` (como o Explore lê a query da URL ou do estado).
- Ícones e textos da árvore.
- Onde, no pipeline de compilação, filtrar as definições recusadas, desde que antes de o erro derrubar o modelo.
- Cache das respostas de `/playground/tree/{slug}`, invalidado quando o modelo recarrega.
- Mover os arquivos automaticamente quando uma connection troca de organização, se ficar simples. Senão, o aviso basta.

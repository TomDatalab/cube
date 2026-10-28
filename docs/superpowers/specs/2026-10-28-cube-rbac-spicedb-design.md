# RBAC no Cube com SpiceDB — design

Data: 2026-10-28. Branch: `feat/rbac-spicedb`, a partir de `feat/playground-data-sources` (PR #7), de que depende.

## Intenção

O Cube (servidor Rust, `rust/cube/cubeserver`) passa a ser o backend de uma solução da equipe. No frontend há **Organizations**; dentro delas, **Workspaces**; dentro dos workspaces, acesso a **dados** e a **objetos** (pipelines parecidos com jobs do Databricks, relatórios etc.), guardados como documentos JSON, XML ou outros formatos.

O Cube precisa:

1. guardar Organizations, Workspaces e os objetos;
2. controlar quem pode fazer o quê, com o **SpiceDB** como fonte das permissões;
3. restringir conexões e dados por workspace.

Quem decidiu o quê: os papéis, a posse das conexões pela Organization e o "só guardar e devolver" dos objetos são decisões do usuário. Onde as coisas rodam e ficam guardadas (Docker, Postgres, portas) foi delegado ao Claude, e está marcado como **[decisão do Claude]**.

## Objetivos

- Endpoints REST para criar e gerenciar usuários e tokens, Organizations, Workspaces, membros e papéis, conexões, a liberação de conexões para workspaces e os objetos.
- Toda requisição de dados ou objetos é autorizada pelo SpiceDB, a partir do `sub` do token.
- O owner da Organization tem acesso a tudo dentro dela.
- Em cada workspace, os papéis são **owner**, **editor** e **viewer**.
- Conexões pertencem à Organization e são liberadas por workspace. Um usuário só consulta cubos cujo `data_source` foi liberado para o workspace em que está trabalhando.

## Não-objetivos

- **Provedor de identidade externo (OIDC etc.):** fica para depois. Por ora, o próprio Cube guarda usuários e senhas e emite o JWT HS256 atual, assinado com `CUBEJS_API_SECRET`. Um provedor externo depois só troca quem emite o token.
- **Executar pipelines:** o Cube só guarda e devolve os documentos.
- **Cubos ou modelo por workspace:** o modelo continua único. O isolamento é feito filtrando por conexão.
- **API SQL (porta 15432, protocolo Postgres):** autentica com usuário e senha fixos (`CUBEJS_SQL_USER`), não com token. Com o RBAC ligado, ela continua sendo um canal de administrador. Isso fica documentado; não é integrada ao RBAC nesta fase.
- **Telas de gestão no Playground** (organizations, workspaces, membros, objetos): ficam para depois. O frontend da equipe usa a API. O Playground ganha só o login, a escolha de workspace e o respeito às permissões (seção Playground).

## Anti-objetivos (falha mesmo que "funcione")

- Um usuário ver ou consultar dados, ou objetos, de um workspace ou Organization a que não tem acesso, por qualquer endpoint: REST, GraphQL, WebSocket, `/v1/meta`, `/v1/execute` ou `/v1/data-sources`.
- **Fail-open:** se o SpiceDB estiver fora do ar ou responder erro, o acesso é **negado** (503), nunca liberado.
- Quebrar instalações sem RBAC. Com o RBAC desligado (padrão), tudo continua como hoje.

## Restrições

- Servidor Rust (`cubeserver`, axum 0.7, tokio). As dependências preferidas já estão no workspace: `reqwest` 0.12, `tokio-postgres` 0.7, `deadpool-postgres` 0.14 e `serde_json`.
- Na instalação atual (`/root/cube-rust`, iniciada por `start-all.sh`):
  - o Postgres é o container `cube-pg-test` (postgres:16, porta 55432);
  - a porta 9090 está ocupada pelo Prometheus.
- O Playground só é montado sem autenticação obrigatória, em modo dev, e hoje emite um token sem `sub`.

## Decisões

### Ativação

- **[decisão do Claude]** Variável `CUBE_RBAC=spicedb` liga o RBAC. Sem ela, nada muda: os escopos e o comportamento atuais continuam.
- Com o RBAC ligado:
  - a conexão com o SpiceDB vem de `SPICEDB_ENDPOINT` (HTTP) e `SPICEDB_PRESHARED_KEY`;
  - o banco de metadados vem de `CUBE_APP_DB_URL` (Postgres).
- Se algum dos três estiver ausente ou inacessível na inicialização, o servidor **não sobe**: mostra um erro claro e não cai para o modo sem RBAC.
- **Escopos de API com o RBAC ligado** **[decisão do Claude]:** quem decide é o RBAC.
  - Os escopos continuam globais, como hoje (`CUBEJS_DEFAULT_API_SCOPES`, que não lê o token). A instalação deve listar `meta,data,sql,graphql,data_sources,execute`, para que um owner de workspace alcance `/v1/execute`.
  - Os endpoints novos de gestão (usuários, organizations, workspaces, conexões e objetos) não exigem escopo, só token e permissão no SpiceDB.
- **`tenants:` em `cube.yml`** **[decisão do Claude]:** com `CUBE_RBAC` ligado e um bloco `tenants:`, o servidor não sobe e mostra um erro claro. O RBAC assume um modelo único e nomes de conexão globais.

### Identidade: usuário e senha (decisão do usuário)

- **Login:** `POST /v1/auth/login` `{username, password}` → `{token, expiresAt, user}`.
  - O token é o JWT HS256 atual, com `sub` = id do usuário e validade padrão de 12 h (`CUBE_RBAC_TOKEN_TTL`).
  - No SpiceDB, o sujeito é `user:<id>`.
- **Senhas** **[decisão do Claude]:**
  - guardadas como hash **argon2id** (crate `argon2`), nunca em texto;
  - mínimo de 10 caracteres;
  - a verificação tem tempo constante, e cada falha espera 1 s antes de responder, para frear tentativas de força bruta;
  - a mensagem de erro é a mesma para usuário inexistente e senha errada.
- **Gestão de senhas:**
  - `PUT /v1/auth/password` `{currentPassword, newPassword}`: o próprio usuário troca a sua.
  - `PUT /v1/users/{id}/password` `{newPassword}`: um platform admin redefine a de outro usuário.
- **Desativação:** um usuário desativado (`disabled`) não faz login. Tokens já emitidos para ele param de valer na próxima requisição, porque o servidor confere a tabela `users` a cada requisição.
- **Token sem `sub`, com o RBAC ligado:** recebe 401 em todo endpoint protegido.
- **Tokens sem senha** **[decisão do Claude]:** `POST /v1/users/{id}/tokens` continua existindo, só para platform admins, para integrações de serviço que não fazem login interativo.
- **Platform admin** **[decisão do Claude]:** é quem cria usuários e Organizations.
  - Os primeiros vêm de `CUBE_RBAC_ADMINS`, uma lista separada por vírgulas, gravada no SpiceDB a cada inicialização (`platform:main#admin@user:<id>`).
  - Um admin listado que ainda não existe em `users` é criado com a senha de `CUBE_RBAC_ADMIN_PASSWORD`. O `start-all.sh` gera essa senha aleatoriamente na primeira vez, grava no `.env` e mostra uma única vez.
  - Remover um id da lista **não** revoga o acesso automaticamente.

### Esquema do SpiceDB

```zed
definition user {}

definition platform {
    relation admin: user
    permission manage = admin
}

definition organization {
    relation platform: platform
    relation owner: user
    // Quem tem qualquer papel em algum workspace da organização; serve para listar a organização.
    relation member: user

    permission admin = owner + platform->manage
    permission view = admin + member
    permission manage_connections = admin
}

definition workspace {
    relation organization: organization
    relation owner: user
    relation editor: user
    relation viewer: user

    permission manage = owner + organization->admin
    permission edit = editor + manage
    permission view = viewer + edit
    permission query = view
    permission execute = manage
}

definition connection {
    relation organization: organization
    // Workspaces para os quais a conexão foi liberada.
    relation workspace: workspace

    permission manage = organization->admin
    permission query = workspace->query + manage
}

definition object {
    relation workspace: workspace
    permission read = workspace->view
    permission write = workspace->edit
}
```

Regras de papel resultantes:

| Ação | org owner | ws owner | ws editor | ws viewer |
|---|---|---|---|---|
| Criar ou excluir workspace; gerenciar owners da organization | ✓ | | | |
| Criar, editar, excluir e liberar conexões da organization | ✓ | | | |
| Gerenciar membros e papéis do workspace; renomear o workspace | ✓ | ✓ | | |
| `/v1/execute` numa conexão liberada para o workspace | ✓ | ✓ | | |
| Criar, editar e excluir objetos | ✓ | ✓ | ✓ | |
| Ler objetos; consultar dados das conexões liberadas | ✓ | ✓ | ✓ | ✓ |

Ao dar qualquer papel num workspace, grava-se também `organization#member@user`. Ele é removido quando o usuário fica sem papel em todos os workspaces daquela organização.

### Onde as coisas ficam **[decisão do Claude]**

- **SpiceDB:**
  - container Docker `cube-spicedb` (imagem `authzed/spicedb`, versão fixada);
  - datastore Postgres: banco `spicedb` no container `cube-pg-test`, criado e migrado com `spicedb migrate head`;
  - portas só em 127.0.0.1: gRPC 50051, HTTP 18443 e métricas 19090, porque a 9090 está ocupada;
  - o Cube usa a **API HTTP** do SpiceDB via `reqwest`, sem gRPC nem código gerado a partir de proto;
  - o esquema acima é versionado no repositório e aplicado (`WriteSchema`) na inicialização do Cube, se diferente do atual.
- **Metadados e objetos:** banco `cube_app` no `cube-pg-test`, com tabelas criadas pelo próprio Cube na inicialização (migrações versionadas e idempotentes).
- **`start-all.sh`:** passa a criar os bancos, subir e migrar o SpiceDB antes do Cube, e mostrar o estado dele em `status`. As variáveis vão para o `.env`; a `SPICEDB_PRESHARED_KEY` é gerada aleatoriamente na primeira vez.

### Tabelas (`cube_app`)

- `users(id text pk, name text, email text, password_hash text, disabled bool, created_at timestamptz, password_changed_at timestamptz)`.
  - O `id` é o `sub` e o login, validado como `[a-z0-9._-]{1,64}`.
  - Os tokens emitidos antes de `password_changed_at` deixam de valer.
- `organizations(id uuid pk, name text, created_at, created_by text)`.
- `workspaces(id uuid pk, organization_id uuid fk, name text, created_at, created_by)`. Nome único dentro da organização.
- `connections(name text pk, organization_id uuid fk)`. O `name` é o nome do data source em `cube.yml`, que é global e único.
- `objects(id uuid pk, workspace_id uuid fk, type text, name text, content_type text, content bytea, version int, created_at, created_by, updated_at, updated_by)`.
  - O `type` é livre (`pipeline`, `report`…), validado como `[a-z0-9_-]{1,64}`.
  - O `content_type` é o MIME (`application/json`, `application/xml`, `text/plain`…).
  - O `content` é guardado como bytes, exatamente como veio.

Os papéis e as liberações **não** ficam em tabelas: a fonte é o SpiceDB.

### Consistência entre Postgres e SpiceDB

- **Criação:** abre transação no Postgres → insere → escreve os relacionamentos no SpiceDB → commit. Se a escrita no SpiceDB falhar, faz rollback.
- **Exclusão:** apaga os relacionamentos no SpiceDB primeiro (o recurso fica inacessível) e depois apaga no Postgres. Uma falha no meio deixa uma linha órfã inacessível, nunca um acesso indevido.
- **Excluir workspace:** apaga também os objetos dele e as liberações de conexão. Excluir uma organização exige que ela não tenha workspaces nem conexões.
- **Checagens:** usam consistência `fully_consistent`, para que o cliente leia o que acabou de escrever. A escala esperada é pequena.

### Endpoints (sob `{basePath}`, com token)

Todos respondem 401 sem token, 403 sem permissão e 404 quando o recurso não existe **ou** o usuário não pode vê-lo. O 404 serve para não revelar a existência de recursos alheios.

**Usuários e tokens** (platform admin):
- `POST /v1/users` `{id, name?, email?, password}`; `GET /v1/users`; `PATCH /v1/users/{id}` `{name?, email?, disabled?}`.
- `POST /v1/users/{id}/tokens` `{expiresIn?: "30d"}` → `{token}`, um JWT com `sub` = id, assinado com `CUBEJS_API_SECRET`.

**Organizations:**
- `POST /v1/organizations` `{name, owner}`: platform admin.
- `GET /v1/organizations`: as que o usuário pode ver (`LookupResources` com `view`).
- `GET` e `PATCH` `{name}` em `/v1/organizations/{id}`: `view` e `admin`, respectivamente.
- `DELETE /v1/organizations/{id}`: `admin`.
- `PUT` e `DELETE` em `/v1/organizations/{id}/owners/{user}`: `admin`. Não é possível remover o último owner.

**Workspaces:**
- `POST /v1/organizations/{id}/workspaces` `{name}`: `organization#admin`.
- `GET /v1/organizations/{id}/workspaces`: os workspaces que o usuário pode ver naquela organização.
- `GET`, `PATCH` e `DELETE` em `/v1/workspaces/{id}`: `view`, `manage` e `organization#admin`, respectivamente.
- `GET /v1/workspaces/{id}/members`: `view`.
- `PUT /v1/workspaces/{id}/members/{user}` `{role: owner|editor|viewer}`: `manage`. Substitui o papel anterior.
- `DELETE /v1/workspaces/{id}/members/{user}`: `manage`.

**Conexões:**
- `GET /v1/organizations/{id}/connections`: `admin`. A resposta inclui os workspaces para os quais cada conexão foi liberada.
- `POST /v1/organizations/{id}/connections`: mesmo corpo de `POST /v1/data-sources`; permissão `manage_connections`. Cria o data source (código existente) e grava `connections` e `connection#organization`.
- `PUT` e `DELETE` em `/v1/connections/{name}`: `connection#manage`. Reutilizam a edição e a exclusão existentes.
- `GET /v1/workspaces/{id}/connections`: `view`. Lista as conexões liberadas, sem as configurações.
- `PUT` e `DELETE` em `/v1/workspaces/{id}/connections/{name}`: `organization#admin`. A conexão e o workspace precisam ser da mesma organização.
- Conexões que já existem (`default`, `sofiadb`): um platform admin as atribui a uma organização com `PUT /v1/connections/{name}/organization` `{organizationId}`. Conexão sem organização: só platform admins a consultam.
- Com o RBAC ligado, `/v1/data-sources` (rotas atuais) fica restrito a platform admins.

**Objetos:**
- `POST /v1/workspaces/{id}/objects` `{type, name, contentType, content}`: `edit`. O `content` é texto ou, com `"encoding": "base64"`, binário. Grava `object#workspace`.
- `GET /v1/workspaces/{id}/objects?type=pipeline`: `view`. Devolve só os metadados, sem o conteúdo.
- `GET /v1/objects/{id}`: `read`. Devolve metadados e conteúdo. Com `Accept` igual ao `content_type` do objeto, devolve o conteúdo puro.
- `PUT /v1/objects/{id}` `{name?, contentType?, content?, version}`: `write`. Se `version` diferir da atual, responde **409**. Se for igual, grava e incrementa.
- `DELETE /v1/objects/{id}`: `write`.

**Outras rotas existentes, com o RBAC ligado** **[decisão do Claude]:**
- Ficam **só para platform admins**, porque executam SQL ou constroem dados sem passar pela checagem de cubos:
  - `POST /v1/cubesql`, `POST /v1/convert-query`;
  - `POST /v1/pre-aggregations/can-use` e `/v1/pre-aggregations/jobs`;
  - tudo em `/cube-system/v1/*`.
- `DELETE /v1/running-query/{id}`: exige só um token com `sub`. O id da consulta é aleatório e só quem a iniciou o conhece.

**Dados** (endpoints atuais):
- O workspace vem do cabeçalho `X-Cube-Workspace: <id>`. Ele é **obrigatório**, com o RBAC ligado, em `/v1/load`, `/v1/subscribe`, `/v1/sql`, `/v1/dry-run`, `/graphql`, `/v1/graphql-to-json`, WebSocket e `/v1/execute`.
  - No WebSocket, o workspace vai também no parâmetro `workspace` da autenticação.
- **Checagem:**
  1. `workspace#query` para o usuário.
  2. Os cubos usados pela consulta: membros em `measures`, `dimensions`, `segments`, `timeDimensions` e `filters`, incluindo `and`/`or` aninhados. Uma view é expandida nos cubos de origem.
  3. O `data_source` de cada cubo precisa estar liberado para o workspace (relação `connection:<ds>#workspace@workspace:<id>`).
  - Qualquer falha resulta em 403, informando o cubo sem acesso.
- **`/v1/meta`:**
  - Com `X-Cube-Workspace`: devolve só os cubos e views cujas conexões estão todas liberadas para o workspace.
  - Sem o cabeçalho: devolve só os cubos das conexões que o usuário pode consultar por algum workspace (`LookupResources connection#query`).
- **`/v1/execute`:**
  - Exige `workspace#execute` e, se `connection` for um nome, que a conexão esteja liberada para o workspace.
  - Conexão avulsa (objeto de configurações): só platform admin.

### Playground com o RBAC ligado (decisão do usuário: acesso controlado, com usuário e senha)

**No servidor:**
- O Playground pode ser montado mesmo com a autenticação obrigatória (`NODE_ENV=production`), porque deixa de distribuir tokens anônimos.
  - Com o RBAC ligado, a instalação passa a rodar assim.
  - Sem o RBAC, a regra atual (só em modo dev) continua.
- `GET /playground/context` continua público, mas **não traz `cubejsToken`** e informa `"authRequired": true`.
- `POST /playground/token` responde 404, porque não pode mais assinar conteúdo arbitrário.
- As outras rotas `/playground/*` passam a exigir `Authorization` com um token de usuário:
  - `files`, `db-schema`, `driver`, `generate-schema` e `data-sources/*`: **só platform admins**, porque leem o modelo e o banco e gravam arquivos;
  - essas rotas também deixam de aceitar a falha de autenticação "silenciosa" do modo dev.

**No frontend** (pacote `cubejs-playground`):
- **Tela de login** (usuário e senha) mostrada quando `authRequired` é verdadeiro e não há token válido guardado.
  - O token fica no `sessionStorage`, então fechar a aba sai do sistema.
  - Um 401 de qualquer chamada volta para o login.
- **Barra superior:** mostra o usuário, um botão **Sair** e um seletor de **workspace**, alimentado por `GET /v1/organizations` e pelos workspaces de cada uma.
  - Todas as chamadas de dados (query builder, `/v1/meta`, GraphQL, WebSocket, console de Frontend Integrations) levam `X-Cube-Workspace` com o workspace escolhido.
  - Platform admins podem escolher "Todos (admin)", que não envia o cabeçalho.
- **Menu por papel:**
  - quem não é platform admin vê só Playground e Frontend Integrations;
  - Data Model e Data Source aparecem só para platform admins.
- **Fica oculto com o RBAC ligado:** "Add Security Context", porque o token agora é o do usuário.

### Compatibilidade

- Sem `CUBE_RBAC`, nenhuma rota nova é montada e o Playground funciona como hoje.
- Platform admins ficam **dispensados** de `X-Cube-Workspace` e veem tudo.

## Deixado para quem implementar

- A organização do código: módulos, traits e como injetar o cliente do SpiceDB e o pool do Postgres no `AppState`, de forma que os testes usem implementações falsas sem SpiceDB real.
- Como extrair os cubos de uma consulta e expandir views, usando o modelo compilado que o servidor já tem.
- Formato exato das mensagens de erro, paginação das listagens (pode começar sem) e limites de tamanho dos objetos (o limite atual da requisição, 50mb, é aceitável).
- Testes: unitários e de integração com SpiceDB e Postgres falsos; além disso, um teste de ponta a ponta contra o SpiceDB real em Docker, rodado manualmente.
- A versão fixada da imagem do SpiceDB.

# Modelo no RBAC e Data Model para donos de organização — design

Data: 2026-11-22
Código: `/home/antonio-rodrigues/Documents/GitHub/cube` (a partir de `feat/playground-org-tree`, PR #14)
Deploy: `/root/cube-rust`
Base:
- `docs/superpowers/specs/2026-11-20-playground-org-tree-design.md` (a árvore por organização, só para admin)
- `docs/superpowers/specs/2026-11-19-conexoes-no-rbac-design.md` (o mesmo movimento feito com as conexões)

## Intenção

Duas mudanças que dependem uma da outra:

1. **Os arquivos YAML do modelo passam a morar no banco do RBAC (`cube_app`)**, como já aconteceu com as conexões. Hoje eles ficam na pasta `model/`. A pasta deixa de ser usada: o servidor compila o modelo a partir do banco, e o banco é o único lugar onde o modelo existe. O backup vai junto com o resto do RBAC (`pg_dump` do `cube_app`), e toda mudança passa pelo servidor, que pode checar permissão e validar.
2. **Os donos de organização** (permissão `admin` da organização no SpiceDB) passam a usar a página **Data Model** para a própria organização: ver os cubos, tabelas e arquivos dela, **gerar** cubos a partir das tabelas das suas connections, **editar** o YAML e **apagar** arquivos, sem depender do platform admin.

Com muitas organizações no mesmo servidor, o risco é um dono enxergar ou estragar o que é de outro. O design mantém duas garantias:
- cada dono só alcança os arquivos da própria organização;
- nenhuma escrita é gravada sem que o modelo inteiro compile antes.

## Objetivos

- **Fonte única:** com o RBAC ligado, o modelo vem só do `cube_app`. Nenhum componente lê `model/` do disco.
- **Importação na primeira partida:** se o banco não tem nenhum arquivo do modelo e `model/` tem, o servidor importa tudo, faz backup da pasta e para de usá-la (como fez com `cube.yml`/`.env` para as conexões).
- **Mudança visível na hora:** toda escrita grava numa transação, sobe a versão do modelo, e o servidor recarrega antes de responder. Outros processos e servidores percebem a versão nova e recarregam também.
- **Quem vê o quê:**
  - Um dono de organização vê na árvore só as organizações de que é dono, com Cubes, Tables e Files de cada uma.
  - O platform admin continua vendo todas, mais **Platform** (arquivos sem organização).
  - Membros de workspace que não são donos continuam sem a página.
- **Gerar, editar, apagar:**
  - O dono gera cubos YAML a partir de tabelas das connections da sua organização.
  - Edita um arquivo YAML da organização num editor na própria página.
  - Apaga um arquivo.
  - O admin faz o mesmo em qualquer organização e em Platform, e também **move** um arquivo de uma organização para outra.
- **Validação antes de gravar:** toda escrita é compilada primeiro, em memória, sobre o modelo atual mais a mudança. Ela só é gravada se não introduzir nenhum erro de compilação nem recusa de posse novos. Se falhar, a resposta é 400 com o erro e nada muda.
- **O sync do SofiaDB** (`sync-sofiadb-model.py`, a cada 5 minutos) passa a entregar os cubos que gera por uma rota do servidor, com a mesma validação.
- **Escala:** a lista de organizações é paginada, e o detalhe de cada uma só é carregado quando o nó é aberto, como hoje.

## Fora do escopo

- **RBAC desligado.** Sem RBAC não há `cube_app`, então o modelo continua em `model/`, como hoje, e editável no disco.
- Criar um arquivo vazio do zero ou renomear um arquivo. Arquivos nascem do **Generate Data Model** (ou do sync); um cubo novo pode ser escrito à mão dentro de um arquivo existente.
- Editar `.jinja`/`.j2` pela página. Eles são importados e compilam, mas ficam só leitura.
- Histórico de versões ou desfazer. Cada linha guarda quem mudou por último e quando; o log registra cada mudança.
- Membros de workspace (editor, viewer) e papéis de organização (`roles`) ganharem acesso à página. É só o dono e o admin.
- Pôr o SQL API (porta 15432) sob RBAC. Ele continua de fora, como já documentado.
- Um modelo compilado por organização (modo tenants). Continua um modelo só, com nomes de cubo únicos na plataforma.

## Anti-objetivos (falha mesmo que "funcione")

- **Duas fontes de verdade.** Qualquer caminho em que editar um arquivo no disco ainda mude o modelo com o RBAC ligado, ou em que o banco e uma pasta discordem.
- **Perder o modelo na importação.** A importação tem que ser completa ou não acontecer, com backup da pasta antes.
- Um dono **ler, listar ou deduzir** algo de outra organização: nomes de arquivo, nomes de cubo, tabelas, mensagens de erro de compilação que citem arquivos de outra organização.
- Uma escrita **derrubar o modelo** ou fazer o servidor parar de recarregar para todos.
- Duas edições simultâneas do mesmo arquivo se sobrescreverem em silêncio.
- O dono editar um cubo que o sync do SofiaDB reescreve e perder a edição sem aviso.

## Restrições encontradas

- **O núcleo lê o modelo de pastas.** Leem `config.schema_path` do disco:
  - o planner (`planner_adapter.rs`: `Model::from_dir_with_context`, `DataSources::load(path)`);
  - o meta (`meta_adapter.rs`: `ModelLoader::load_dir_with`, e a recarga pelo `newest_mtime`);
  - os tenant runtimes (`tenants.rs`);
  - o índice da árvore (`model_index.rs`);
  - o gerador (`scaffold.rs`, que procura cubos existentes no disco);
  - `/playground/files`;
  - a API SQL (`sql_api.rs`, `cubesqlbridge::ModelSource::Dir`).

  Só a validação já tem uma API em memória: `ModelLoader::load_sources_with_policy(&[(path, content)], policy)`.
- **A regra de posse usa o primeiro segmento do caminho** (`model/<slug>/…` → organização; `_platform/` → sem organização). É aplicada antes da compilação, e um cubo mal colocado é recusado sem derrubar o resto.
- **Permissão de dono:** `visible_organization(rbac, id, "admin", user)` (`rbac/handlers/organizations.rs`) já responde 404 a quem não vê a organização e 403 a quem vê mas não é dono. `GET /cube/v1/organizations` lista as organizações que o usuário vê, com `slug`. Pelo README (*Roles*), o dono da organização é dono de cada workspace dela.
- **As rotas `/playground/*`** estão todas atrás de `require_platform_admin` (`handlers/playground.rs`) e sem CORS. As rotas RBAC (`/cube/v1/...`) usam token Bearer, sem cookie, então não há CSRF.
- **Nomes de cubo são globais.** O gerador recusa um nome que já existe e hoje cita o caminho de quem já o usa ("…already exists in digital/fatos.yml"), o que vaza a outra organização.
- **A interface** só conhece `session.user.platformAdmin` (`hooks/rbac.ts`). Data Model, Data Source e Admin ficam atrás de `AdminOnly` (`pages/index.tsx`).
- **O painel do cube** lê measures e dimensions do `/cube/v1/meta` sem workspace, o que só o admin pode fazer.
- **Precedente das conexões:** importação única na partida, com `*.bak-<data>`, e daí em diante só pelo RBAC. Uma organização não pode ser apagada enquanto tiver workspaces ou connections (400).
- **Sync do SofiaDB hoje:** grava em `model/<slug>/sofiadb/` com um marcador no topo dos arquivos. Sem RBAC (fora do escopo), continua assim.

## Abordagens consideradas

1. **Banco como fonte, `model/` como espelho regravado pelo servidor.** É menos trabalho no núcleo, porque todo o resto continua lendo do disco. Mas ficam duas cópias, e quem editar a pasta perde a edição na próxima regravação. Descartada pelo usuário.
2. **Só no banco, carregado pelo servidor (escolhida pelo usuário).** O servidor importa `model/` uma vez e compila o modelo a partir das linhas do banco. A recarga segue uma versão no banco. A validação compila "linhas + mudança" em memória, e a gravação é uma transação. Dá mais trabalho no servidor, mas há um lugar só para o modelo.
3. **Guardar no banco só uma cópia de segurança.** Os arquivos continuariam mandando. Descartada: não é "como as conexões".

Sobre as permissões dos donos, a escolha anterior continua valendo: **modelo único validado antes de gravar**, com rotas por organização sob `/cube/v1/organizations/{id}/model`. As alternativas descartadas foram "um modelo compilado por organização" (muda o núcleo) e "abrir as rotas `/playground/*` com filtro" (mistura rotas de admin e de dono e herda as mensagens que citam caminhos).

## Decisões

### Banco (`cube_app`, `migrations.sql`)

```sql
CREATE TABLE IF NOT EXISTS model_files (
    id uuid PRIMARY KEY,
    organization_id uuid REFERENCES organizations (id),  -- NULL: Platform
    path text NOT NULL,          -- relativo à organização: 'fatos.yml', 'sofiadb/dfos_a.yml'
    content text NOT NULL,
    version integer NOT NULL DEFAULT 1,
    managed_by text,             -- NULL: editável; 'sofiadb-sync': do sync, só leitura
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by text NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS model_files_path_idx
    ON model_files (coalesce(organization_id, '00000000-0000-0000-0000-000000000000'), path);

CREATE TABLE IF NOT EXISTS model_state (
    id boolean PRIMARY KEY DEFAULT true CHECK (id),
    version bigint NOT NULL DEFAULT 0
);
```

- **`path`** é relativo à organização. É validado no servidor:
  - sem `..`, sem barra no começo ou no fim, sem componente vazio ou `\`;
  - segmentos `[A-Za-z0-9._-]`;
  - até 255 caracteres;
  - extensão `.yml`, `.yaml`, `.jinja` ou `.j2`, minúscula ou maiúscula, como o loader aceita.
- **A posse é a coluna `organization_id`**, não uma pasta. Para compilar, cada linha vira a fonte `<slug>/<path>` (ou `_platform/<path>`). Assim a regra de posse que já existe continua valendo sem mudança, e as mensagens e a árvore mostram o mesmo caminho de hoje.
- **`model_state.version`** sobe na mesma transação de toda escrita.
- Apagar uma organização passa a dar 400 também enquanto ela tiver arquivos do modelo ("Mova ou apague os arquivos do modelo antes").

### Carregar e recarregar

- Na partida com RBAC, o servidor lê todas as linhas de `model_files` e a versão, e compila. Daí em diante, o modelo servido é sempre o de **uma versão**.
- **Recarga:** a recarga atual pela data dos arquivos (`spawn_reload_loop`, `newest_mtime`) dá lugar a um laço que vê se `model_state.version` mudou. O intervalo é `CUBEJS_MODEL_RELOAD_INTERVAL`, o mesmo de hoje. Se mudou, lê as linhas e recompila; se a compilação falhar, continua servindo a anterior e registra o erro, como hoje.
- Depois de uma escrita própria, o servidor recarrega antes de responder, sem esperar o laço.
- **Todos os consumidores** (meta, planner, tenants, índice da árvore, gerador, `/playground/files`, API SQL) leem do mesmo **retrato do modelo** (`ModelSnapshot { version, files: Vec<(path, content)> }`), nunca do disco. Ver *Deixado ao builder* sobre como dar esse retrato às APIs que hoje pedem uma pasta.
- A API SQL passa a seguir a mesma recarga (hoje ela fica com o modelo e a posse da partida até reiniciar). Se isso se mostrar caro, o builder pode manter o comportamento atual e documentar, desde que ela leia do banco e não do disco.

### Importação (primeira partida com RBAC e com `model_files` vazia)

1. Ler todos os arquivos que o loader aceita em `model/` (`cubemodel::collect_files`).
2. Organização de cada um: o primeiro segmento do caminho.
   - `_platform/…` → `organization_id NULL`;
   - um slug existente → aquela organização;
   - na raiz ou numa pasta que não é de ninguém → Platform, com o caminho inteiro preservado e um aviso no log. Esses arquivos seguem recusados pela regra de posse, como hoje.
3. Um arquivo que começa com o marcador do `sync-sofiadb-model.py` entra com `managed_by = 'sofiadb-sync'`.
4. Tudo numa transação única, com `created_by = 'import'`, e `model_state.version = 1`.
5. Só depois do `COMMIT`: renomear `model/` para `model.imported-<data>` e criar um `model/README` curto dizendo onde o modelo está agora. Se a transação falhar, nada muda e o servidor não sobe, com o erro no log.
6. Conferir na própria partida que o modelo compilado do banco tem os mesmos cubos que o da pasta tinha. Se não tiver, o servidor não sobe e diz quais cubos faltam (o banco fica, para inspeção).
7. Com `model_files` já preenchida, a pasta é ignorada. Se ela ainda existir e tiver arquivos, o log avisa uma vez por partida que eles não são usados.

### Rotas de escrita e leitura do modelo

Todas sob `/cube/v1`, com token Bearer.

| Rota | Quem | O que faz |
|---|---|---|
| `GET /organizations/{id}/model` | dono ou admin | O detalhe da organização no formato de `GET /playground/tree/{slug}` (`cubes`, `tables`, `files`, `errors`). Cada arquivo traz `id`, `path`, `version`, `editable`. Cada cubo traz os membros (abaixo). |
| `GET /organizations/{id}/model/files/{fileId}` | dono ou admin | `path`, `content`, `version`, `editable`, `updatedAt`, `updatedBy`. |
| `PUT /organizations/{id}/model/files/{fileId}` `{content, version}` | dono ou admin | Substitui o conteúdo. Com uma `version` diferente da atual → 409 "O arquivo mudou desde que você abriu; abra de novo". |
| `DELETE /organizations/{id}/model/files/{fileId}?version=` | dono ou admin | Apaga, com a mesma regra de versão. |
| `POST /organizations/{id}/model/generate` `{tables: [[connection.schema, table]]}` | dono ou admin | Gera cubos YAML a partir de tabelas de connections **desta** organização; qualquer outra → 400. As colunas são lidas pelo servidor. Cria uma linha por cubo. |
| `GET /platform/model`, `…/files/{fileId}` (GET, PUT, DELETE), `…/generate` | admin | O mesmo, para Platform. |
| `PUT /model/files/{fileId}/organization` `{organizationId \| null}` | admin | Move um arquivo para outra organização ou para Platform, com validação. É o que resolve os cubos recusados depois que uma connection muda de organização. |
| `GET /model/sync/{connection}/context` e `PUT /model/sync/{connection}` `{files: [{path, content}]}` | só o usuário do sync (`CUBE_SYNC_USER`) | Substitui de uma vez todos os arquivos `managed_by = 'sofiadb-sync'` da organização dona da connection, com validação. Responde `{changed: bool, added, updated, removed}`. |

- `visible_organization(..., "admin", ...)` em toda rota de organização: 404 para quem não vê, 403 para quem vê sem ser dono. O dono nunca alcança Platform nem as rotas `/model/...` de admin.
- **Arquivos `managed_by = 'sofiadb-sync'`**: leitura sim. `PUT`/`DELETE` pelas rotas de arquivo → 400 "Este arquivo é gerado a partir do catálogo do SofiaDB e é reescrito a cada sincronização. Para personalizar uma tabela, escreva o cubo dela em outro arquivo da organização, com outro nome; na próxima sincronização o cubo gerado sai". Vale também para o admin. Só a rota de sync os escreve.
- `.jinja`/`.j2`: `editable: false`; `PUT` → 400.
- As rotas `/playground/tree*`, `/playground/files` e `/playground/generate-schema` continuam (só admin), mas passam a ler do retrato do modelo. `generate-schema` grava no banco pela mesma validação.

### Validação antes de gravar

Toda escrita (editar, apagar, gerar, mover, sync) passa pelo mesmo caminho, numa seção crítica única no servidor (um `Mutex`):

1. Montar as fontes do candidato: as linhas atuais do banco, com a mudança aplicada.
2. Compilar em memória, **com validação**, com o mesmo `COMPILE_CONTEXT` e a mesma política de posse do modelo servido. Com o RBAC ligado, o servidor se recusa a subir com `tenants:`, então há um contexto só.
3. Comparar com o modelo atual:
   - os erros de compilação;
   - as recusas de posse (arquivo, cubo, mensagem).

   Recusar (400) só o que é **novo**. Assim um problema que já existia em outra organização não bloqueia o dono, inclusive de consertar a própria parte. Se a escrita faz erros sumirem, a resposta diz quantos.
4. Se passar: numa transação do Postgres,
   - conferir a `version` da linha (`UPDATE … WHERE id = $1 AND version = $2`; 0 linhas → 409);
   - gravar;
   - subir `model_state.version`;
   - fazer o `COMMIT`.

   Depois, recarregar o modelo servido.
5. Registrar no log: usuário, organização, arquivo, ação, versão antiga → nova.

A resposta de sucesso traz o detalhe novo da organização, para a árvore se atualizar sem outra chamada.

Para os casos de dependência:
- **Apagar** um arquivo de que outro cubo da mesma organização depende (join, view) quebra a compilação do candidato e é recusado com o erro.
- Um cubo de **outra** organização não pode depender deste, porque a regra de posse já proíbe join e view entre organizações.

### O que o dono vê dos erros

- Erros **novos** causados pela escrita, por inteiro, e erros que citam arquivos da própria organização ou nenhum arquivo.
- Num erro novo que cite um arquivo de outra organização, o caminho é trocado por "(arquivo de outra organização)". O nome de um cubo de outra organização só aparece se o próprio dono o escreveu no YAML (por exemplo, um join recusado), o que não revela nada que ele não tenha digitado.
- Erros antigos de outras organizações: uma linha genérica, "Há um problema no modelo fora da sua organização; avise o administrador".
- **Nome de cubo já usado em outra organização:** 400 "O nome de cubo X já está em uso na plataforma; escolha outro", sem caminho. Dentro da própria organização, a mensagem pode citar o arquivo.
- O admin recebe sempre os erros completos.

### Gerar

- O servidor lista as tabelas só das connections da organização e gera com o `scaffold::generate` atual, adaptado para ler os cubos existentes do retrato do modelo e devolver linhas em vez de gravar arquivos. Os arquivos ficam na raiz da organização (`<cubo>.yml`).
- **"Adopt"** (reaproveitar um cubo gerado que lê a mesma tabela): para o dono, só quando o arquivo adotado é da organização dele e não é do sync. Senão, o cubo é recusado com a mensagem de nome já usado. O admin mantém o comportamento atual.
- Todos os arquivos de uma chamada entram na mesma validação e na mesma transação: ou todos entram ou nenhum.

### Sync do SofiaDB

- `sync-sofiadb-model.py` continua lendo o catálogo e gerando os YAML como hoje, mas, com o RBAC ligado, não escreve no disco. Ele envia o conjunto completo para `PUT /cube/v1/model/sync/sofiadb`.
- O token é um token de serviço do usuário `sofiadb-sync`, que não é admin (ver *Pontos levantados na revisão do design*), criado pelo `start-all.sh` na primeira vez e guardado no `.env` (`CUBE_SYNC_TOKEN`), como os outros segredos que ele gera.
- **"Coberta à mão":** antes de gerar, o script pede ao servidor os nomes já usados e as tabelas já cobertas (`GET /model/sync/{connection}/context`, calculado sobre o modelo inteiro, sem as linhas do sync). Com isso ele gera os nomes e os joins como hoje. O script não lê mais a pasta.
- O `--verify` (que conferia se o servidor aceitou os arquivos e voltava os antigos) deixa de ser necessário: o servidor valida antes de gravar. A rota responde 400 com o erro, e o script registra e sai com 1, com o modelo intacto.
- A organização de destino é a dona da connection no momento do sync (Platform se não tiver dono). Se a connection mudou de organização, a rota move as linhas do sync junto, na mesma transação.

### Quem é dono, na interface

- `POST /auth/login` e a sessão ganham `user.ownedOrganizations: [{id, slug, name}]`, as organizações em que o usuário tem `admin`. Para o platform admin pode vir vazia.
- O menu mostra **Data Model** para quem é platform admin **ou** dono de pelo menos uma organização. A guarda `AdminOnly` dessa rota vira `OwnerOrAdmin`. Data Source e Admin continuam só de admin.
- A lista é só para a interface. Toda rota checa a permissão de novo a cada requisição. Se a pessoa deixar de ser dona, a próxima chamada recebe 404/403 e a página tira a organização da árvore.

### Membros do cubo sem `/meta`

- Cada cubo de `GET …/model` (e de `GET /playground/tree/{slug}`) traz `measures`, `dimensions` e `segments` como listas `{name, type, title}` (dimensions com `primaryKey`), tiradas do modelo compilado.
- O painel do cube usa essas listas para todos, e o Data Model deixa de depender do `/meta`.

### Interface

- **Árvore:** para um dono, o nível 1 são as organizações de que é dono, e o nível 2 vem de `GET …/model`. Para o admin, o nível 1 continua vindo de `/playground/tree`, com Platform. Mesmos ícones, busca e reticências de hoje.
- **Arquivo selecionado** (quando `editable`): botões **Editar** e **Apagar**.
  - **Editar** troca o `PrismCode` por um editor de texto monoespaçado com **Salvar** e **Cancelar**. Um erro de validação aparece acima do editor, sem perder o texto.
  - **Apagar** pede confirmação com o nome do arquivo.
  - Um 409 mostra "O arquivo mudou desde que você abriu" com **Recarregar**.
  - No rodapé: "Alterado por <usuário> em <data>".
- **Arquivo do sync ou `.jinja`:** só leitura, com a explicação.
- **Admin:** no arquivo selecionado, também **Mover para…** (lista de organizações e Platform).
- **Cubo selecionado:** o painel usa os membros da rota. **Abrir no Playground** abre o construtor com o cubo, no workspace escolhido no seletor. Se a connection do cubo não está liberada para esse workspace, o painel avisa ("libere a conexão X para o workspace Y"). Se a organização não tem nenhum workspace, o botão fica desabilitado com "Crie um workspace nesta organização para consultar".
- **Generate Data Model**, para um dono, chama `POST …/model/generate` da organização das tabelas marcadas. A regra de "uma organização por vez" continua.
- O texto "This file can be edited at <caminho absoluto>" sai: o arquivo não está mais no disco.

### Deploy (`/root/cube-rust`)

- Antes de trocar o binário: backup de `model/` e `pg_dump` do `cube_app`.
- Na primeira partida, a importação acontece sozinha. Conferir: `/meta` com os mesmos 30 cubos, a árvore com as mesmas contagens, `model.imported-<data>` criado.
- `start-all.sh`: o passo *Data model* passa a chamar o sync em modo de envio, e cria o usuário e o token do sync se faltarem.
- O README do deploy muda as seções *SofiaDB tables*, *The data model* e RBAC: onde o modelo mora, como fazer backup e restaurar, que editar no disco não vale mais com o RBAC ligado, e as rotas novas.

### Pontos levantados na revisão do design (segunda rodada)

- **Mais leitores da pasta.** Além dos listados em *Restrições*, também leem `schema_path`:
  - `handlers/data_sources.rs` (`refuse_if_in_use` → `cubes_using`, a checagem antes de apagar uma connection);
  - `rbac/ownership.rs` (`model_index.get(schema_path)`).

  Todos passam a usar o retrato do modelo.
- **Recarga sempre ligada com RBAC.** Hoje a recarga só roda porque o `.env` define `CUBEJS_MODEL_RELOAD_INTERVAL=5`; sem a variável, fora do modo dev, ela é 0. Com o RBAC ligado, o laço de versão roda sempre, com 5 s quando a variável não estiver definida, para enxergar escritas de outros processos.
- **Mais de um servidor no mesmo `cube_app`.** O `Mutex` cobre só um processo. A transação de escrita trava a linha de `model_state` (`SELECT … FOR UPDATE`) e valida contra as linhas lidas **dentro** da transação, não contra o modelo em memória. Assim duas escritas de servidores diferentes nunca validam contra bases diferentes. O `Mutex` fica como otimização dentro do processo.
- **Sync: quem decide o que já tem cubo escrito à mão, e os nomes e joins.** O `build()` do script usa o resultado de `hand_written()` para três coisas:
  - não gerar tabelas cobertas;
  - manter os nomes gerados, inclusive os dos cubos de papel, longe de todos os nomes existentes (`taken`);
  - apontar os joins gerados para o cubo escrito à mão (`cube_of`).

  Por isso o servidor não pode só descartar cubos depois. Decisão: a geração continua no script, e o script pede o contexto ao servidor antes de gerar. A rota é `GET /cube/v1/model/sync/{connection}/context`, que responde `{taken: [nomes de cubos e views de todo o modelo, fora os do sync], covered: {"<db>.<tabela>": "<cubo>"}}`. É a mesma regra de hoje, calculada sobre o modelo inteiro: um cubo de outra organização ou de Platform continua contando como cobertura, como quando o script varria a pasta toda. Só o token do sync alcança essa rota, então ela não expõe nada a donos. A rota `PUT /model/sync/{connection}` confere de novo e recusa (400) se o contexto mudou entre as duas chamadas; o script tenta de novo na próxima rodada.
- **O que o token do sync pode fazer.** Ele não é admin. É o token de serviço de um usuário comum, `sofiadb-sync`, sem papel em nenhuma organização. As duas rotas de sync aceitam só o usuário cujo id está em `CUBE_SYNC_USER` (no `.env`, padrão `sofiadb-sync`), e esse usuário não alcança mais nada (toda outra rota responde como a qualquer usuário sem permissão). Não muda o esquema do SpiceDB. O `start-all.sh` cria o usuário e o token na primeira vez, com o login de admin que já usa a senha do `.env`, e guarda `CUBE_SYNC_TOKEN` no `.env`.
- **Ordem no `start-all.sh`.**
  - `start`: o sync passa a rodar **depois** de `start_server`, quando `/readyz` responde. Se o usuário e o token do sync faltarem, são criados nesse momento.
  - `sync` (o cron): se o servidor não responde, sai com 0 e registra "servidor fora do ar; sincronização pulada". Não é uma falha do sync, e o modelo no banco continua o mesmo.
- **Voltar ao binário antigo.** Fica um comando de exportação: `GET /cube/v1/model/export` (admin) devolve todos os arquivos como `{path, content}` com o caminho `<slug>/<path>`, e `./start-all.sh export-model [pasta]` grava isso numa pasta (padrão `model.export-<data>/`). Para voltar ao binário antigo: parar o servidor, exportar (ou usar a exportação mais recente), pôr a pasta como `model/`, e subir o binário antigo. O README do deploy descreve isso junto do backup por `pg_dump`.

## Testes esperados

- **Unidade (Rust):**
  - validação de `path`;
  - montagem das fontes `<slug>/<path>`;
  - comparação de erros e recusas antes/depois;
  - filtro e redação de erros para dono;
  - mensagem genérica de nome já usado;
  - detecção do marcador do sync na importação;
  - regra de "coberta à mão" a partir do retrato do modelo.
- **Integração (Postgres de teste, `fake.rs`):**
  - **importação:**
    - pasta com organização, `_platform`, raiz e arquivos do sync → linhas certas, `version = 1`, pasta renomeada;
    - segunda partida não importa de novo;
    - falha no meio → banco vazio e pasta intacta;
  - **recarga:** uma escrita feita direto no banco por outro processo, com a versão subindo, aparece no `/meta` dentro do intervalo;
  - **permissões:** dono de A 200 em A; 404 em B (não vê); 403 onde vê sem ser dono; membro de workspace 403; dono em `/platform/model` e `/model/...` 403/404;
  - **escrita:**
    - `PUT` com YAML inválido → 400 e nada muda;
    - `PUT` válido → 200 e o `/meta` do admin já mostra;
    - `PUT` com `version` velha → 409;
    - duas escritas concorrentes não se perdem;
  - **apagar:** de arquivo do qual outro cubo depende → 400; válido → 204 e o cubo some;
  - **gerar:**
    - com tabela de connection de outra organização → 400;
    - com nome já usado em outra organização → 400, sem caminho;
  - **arquivo do sync:** `PUT`/`DELETE` → 400;
  - **sync:**
    - substitui só as linhas do sync;
    - "no change" não sobe a versão;
    - YAML ruim → 400 e nada muda;
    - connection movida leva as linhas junto;
  - **mover:** move e revalida; dono → 403;
  - **apagar organização** com arquivos → 400;
  - **o disco:** com o RBAC ligado, criar um arquivo em `model/` não muda o modelo.
- **Playground (vitest):** a guarda `OwnerOrAdmin`, a árvore do dono, e os estados do editor (salvar, erro, 409, só leitura).
- **No deploy:** a importação real, depois um usuário de teste dono só da ON gerando, editando e apagando, e confirmando que não vê nada da Digital nem de Platform, inclusive no navegador.

## Decisões tomadas por mim (Claude)

- A posse é `organization_id`. O caminho mostrado (`<slug>/<path>`) é montado para compilar, para a regra de posse e as mensagens continuarem iguais.
- Arquivos sem dono na importação vão para Platform com o caminho inteiro, para nada se perder; eles continuam recusados como hoje.
- A importação confere os cubos antes de liberar a partida e só renomeia a pasta depois do `COMMIT`.
- Recarga por versão em `model_state`, com o mesmo intervalo de hoje. `LISTEN/NOTIFY` é opcional (ver abaixo).
- Só o problema **novo** bloqueia uma escrita (erros e recusas comparados antes/depois).
- O sync escreve pelo servidor, com um token de serviço, e o servidor aplica a regra de "coberta à mão".
- Mover arquivo entre organizações é do admin, porque sem a pasta não há mais `mv`.
- Os arquivos gerados ficam na raiz da organização (`<cubo>.yml`), e os do sync em `sofiadb/`.

## Deixado ao builder

- **Como dar o retrato do modelo às APIs que hoje pedem uma pasta** (`Model::from_dir_with_context`, `DataSources::load`, `ModelSource::Dir`): versões que aceitam fontes em memória, ou um diretório **privado** do servidor, regravado por inteiro a cada versão a partir do banco (fora de `model/`, por exemplo em `$XDG_RUNTIME_DIR` ou num `tempdir`, nunca editado por pessoas). A regra é que o banco seja a única fonte e que todos compilem o mesmo retrato.
- Usar `LISTEN/NOTIFY` além do laço de versão, se ajudar.
- Como guardar `ownedOrganizations` (no login, ou `GET /auth/me`).
- O componente do editor (um `TextArea` monoespaçado basta).
- Textos exatos das mensagens (em português, como o resto da página) e o formato do log de auditoria.
- A divisão em etapas no plano. Sugestão: primeiro o modelo no banco com as rotas de admin e o sync; depois os donos.

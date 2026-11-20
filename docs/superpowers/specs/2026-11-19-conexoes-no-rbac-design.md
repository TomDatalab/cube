# Conexões guardadas no RBAC, com id e desatribuição — design

Data: 2026-11-19

## Intenção

Hoje cada conexão (data source) é um bloco em `cube.yml` com host, porta,
banco, usuário e senha em `.env`; o RBAC (`cube_app`) só guarda a quem ela
pertence. O usuário quer que o RBAC seja a fonte única das conexões —
configuração e credenciais — que cada conexão tenha um id próprio, visível, e
que uma conexão possa sair da organização e voltar a ficar disponível para
ser associada a outra. A tela Data Source deve caber sem rolagem lateral.

## Decisões do usuário

- **Desatribuir:** a conexão sai **da organização** (não só dos workspaces):
  as liberações nos workspaces dela são retiradas e ela volta a ser Platform,
  pronta para ser arrastada para outra organização. Isso revoga a regra de
  2026-11-18 "nunca volta a ficar sem dono".
- **Id:** cada conexão ganha um UUID no RBAC. O nome continua sendo o que o
  modelo usa em `data_source:` e não muda.
- **Armazenamento:** tudo em `cube_app`; a senha (e qualquer outro segredo)
  cifrada com AES-256-GCM, chave mestra só no `.env`. `cube.yml`/`.env`
  deixam de declarar conexões; as atuais (inclusive `default`) são migradas
  na primeira partida.

## Abordagens

1. **Carregar do RBAC na partida, antes de declarar os data sources
   (escolhida).** O servidor já lê os data sources uma vez, na partida, e
   reinicia a si mesmo quando um muda. Basta trocar a origem: antes de
   `declare_data_sources`, ler as linhas de `connections`, decifrar os
   segredos e preencher `cube_config.data_sources`. Planner, drivers e o
   reinício existentes continuam iguais.
2. Recarregar conexões a quente, sem reinício: mexe no planner (dialetos por
   data source) e no orchestrator, que hoje assumem conjunto fixo. Muito
   maior, sem ganho pedido. Descartada.
3. RBAC como cópia e arquivos como fonte: não atende "armazenadas no RBAC".
   Descartada.

## Design

### Banco (`cube_app`, `migrations.sql`)

Tabela nova `data_sources` com a configuração de **todas** as conexões; a
tabela `connections` continua sendo só o dono (linha = pertence a uma
organização; sem linha = Platform), sem mudança de schema. Assim desatribuir
é apagar a linha de `connections`, e o código de organizações/liberações não
muda. (Decisão de implementação, 2026-11-19: alterar `connections` para
`organization_id NULL` espalharia `Option` por todo o RBAC já revisado.)

| coluna | |
|---|---|
| `id uuid UNIQUE NOT NULL DEFAULT gen_random_uuid()` | o id exibido |
| `name text PRIMARY KEY` | o nome de `data_source:` |
| `db_type text NOT NULL` | |
| `settings jsonb NOT NULL DEFAULT '{}'` | variáveis `CUBEJS_DB_*` não secretas, como o formulário as usa |
| `secrets bytea NULL` | JSON das variáveis secretas (as que `is_secret` reconhece: `PASS`, `KEY`, `TOKEN`...) cifrado: nonce de 12 bytes + texto cifrado AES-256-GCM |
| `created_at`, `updated_at timestamptz NOT NULL DEFAULT now()` | |

Nenhuma rota passa a exigir o id (todas seguem pelo nome); ele é exibido e
devolvido nas listagens.

### Chave mestra

`CUBE_RBAC_SECRET_KEY` no `.env`: 32 bytes em base64. Com RBAC ligado o
servidor não sobe sem ela (mesma regra das outras variáveis RBAC).
`start-all.sh` gera uma (`openssl rand -base64 32`) se faltar, antes de subir
o servidor, e avisa: perder a chave = perder as senhas guardadas.
Ficam no `.env` só as variáveis de *bootstrap* que não podem estar no próprio
RBAC: `CUBE_APP_DB_URL`, `SPICEDB_*`, `CUBE_RBAC*`, `CUBE_RBAC_SECRET_KEY`,
portas e segredos da API.

### Partida do servidor (RBAC ligado)

1. Carrega `cube.yml` como hoje.
2. Conecta em `cube_app`, aplica as migrações.
3. **Migração única:** cada data source de `cube.yml` sem linha em
   `connections` é importada (valores já resolvidos do ambiente; segredos
   cifrados; Platform, salvo as que já tinham linha de dono). Depois de
   importar, o bloco sai de `cube.yml` e as variáveis `CUBEJS_DS_<NOME>_*`
   saem do `.env`, com cópia `.bak-<data>` dos dois arquivos.
4. Monta `cube_config.data_sources` a partir de `connections` e segue para
   `declare_data_sources` e o resto, sem mudanças.
5. Falha em ler `cube_app` ou decifrar: o servidor não sobe (fail-closed,
   como o resto do RBAC).

Sem RBAC (`CUBE_RBAC` vazio) nada muda: arquivos como hoje.

### Data Source (criar / editar / excluir / testar)

`handlers/data_sources.rs` com RBAC ligado grava em `connections` em vez de
`cube.yml`/`.env`, e reinicia o servidor como hoje. Editar com senha vazia
mantém a atual. `GET` nunca devolve segredos (só `hasPassword`). A listagem
devolve `id`.

### Desatribuir

`DELETE /v1/connections/{name}/organization` → 204, só platform admin:
retira todas as liberações (`connection#released`) da conexão, apaga
`connection#organization` no SpiceDB e põe `organization_id = NULL`.
`409` se mudou de organização entre a leitura e a escrita (mesmo molde do
move). Conexão `created` pela organização também pode ser desatribuída e vira
Platform. `default` continua Platform para sempre.

### Tela Data Source

- **Sem rolagem lateral:** a tabela deixa de usar `scroll.x` e a coluna fixa.
  Colunas: alça, **Name** (nome em negrito + id em fonte mono pequena,
  copiável), **Type**, **Owner**, **Connection** (`user@host:port/database`,
  com reticências e o texto completo no title), **Status**, ações em ícones.
- **Painel Organizations (à direita):** cada conexão na organização aparece
  como tag com **×**; o × pede confirmação ("Remove sofiadb from Digital?
  Its releases to N workspaces are withdrawn and it goes back to Platform.")
  e chama o DELETE. Também dá para arrastar a tag de volta para uma área
  **Platform** no topo do painel, com a mesma confirmação.

### Scripts

`start-all.sh` e `sync-sofiadb-model.py` usam `SOFIADB_*` e `CUBEJS_DB_PASS`
para **subir e vigiar a infraestrutura** (cluster, contêiner Postgres,
SpiceDB), não como conexões do Cube; essas variáveis continuam no `.env` com
esse papel, e `cube.yml` deixa de referenciá-las. O README é atualizado.

## Fora do escopo

- Trocar as rotas para usar o id em vez do nome.
- Rotação da chave mestra.
- Recarregar conexões sem reiniciar.

## Testes

- Rust: cifrar/decifrar ida e volta; chave errada falha; migração importa e
  limpa arquivos (com backup) e é idempotente; carga de `connections` vira
  `data_sources`; desatribuir retira liberações e relação, 403 para não-admin,
  400 para `default`, 409 em corrida.
- Front: `dropAction` com Platform como alvo; formato da coluna Connection.
- Ponta a ponta: subir com `start-all.sh`, conferir `cube.yml`/`.env` sem
  conexões, consultas em `default` e `sofiadb` funcionando, desatribuir e
  reatribuir pela tela.

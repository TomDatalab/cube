# Models: montar um cubo arrastando tabelas

## Intenção

Na árvore do Data Model, cada organização ganha um grupo **Models**, logo abaixo de
**Cubes**. Um model é um desenho: o usuário arrasta tabelas para uma tela, liga
colunas de duas tabelas para criar um relacionamento e escolhe o tipo dele. O
model é salvo e pode ser reaberto. **Build cube** transforma o model num cubo
YAML, que aparece em **Cubes**.

Objetivo: quem não escreve YAML consegue montar um cubo sobre várias tabelas.

## Metas

- Grupo `Models (n)` em cada organização da árvore, com um item por model e a
  ação "Novo model".
- Editor de model numa tela (React Flow):
  - à esquerda, as tabelas das conexões da organização, arrastáveis;
  - no centro, cada tabela vira um nó com suas colunas, e cada coluna tem um
    ponto de ligação;
  - ligar uma coluna a outra abre um diálogo para escolher a cardinalidade
    (`1:1`, `1:N`, `N:1`) e o JOIN (`LEFT`, que é o padrão, ou `INNER`);
  - clicar numa linha permite editar ou apagar a ligação;
  - uma tabela é a **principal** (a primeira arrastada, trocável), e nela o
    usuário marca a chave primária;
  - botões Salvar e Build cube.
- **Build cube** gera um único cubo (opção B escolhida pelo usuário), com:
  - `sql` com os JOINs;
  - uma dimensão por coluna;
  - a medida `count`.
- Arquivo gerado `<slug>/<nome_do_cubo>.yml`, gravado pelo `ModelWriter`, que
  valida o modelo. O cubo aparece em Cubes e o editor oferece abri-lo.
- Reconstruir atualiza o mesmo arquivo.

## Fora do escopo

- Models na Plataforma (`_platform`): só organizações nesta versão.
- Gerar vários cubos com `joins` ou views (opção A, descartada).
- Medidas além de `count`, filtros e segmentos: o usuário edita o YAML depois,
  se quiser.
- Tabelas de conexões diferentes no mesmo model.

## Anti-metas

- **Medidas infladas sem aviso:** se um JOIN multiplica as linhas da tabela
  principal, `count` precisa continuar certo.
- **Edições perdidas em silêncio:** reconstruir não pode apagar, sem avisar, o
  que alguém editou à mão no arquivo.
- **Vazamento entre organizações:** nenhum model, tabela ou cubo de uma
  organização pode aparecer para outra.

## Restrições

- **Uma conexão por cubo:** um cubo lê uma só conexão. A conexão é a da primeira
  tabela do model, e tabelas de outra conexão são recusadas, na tela e no
  servidor.
- **Quem pode:** a permissão é a mesma das rotas de arquivos de modelo da
  organização (`visible_organization(..., "admin")`), ou seja, donos da
  organização e platform admins.
- **Tabelas e colunas:** vêm de `data_source_schema` da conexão. O build relê as
  colunas na hora, para não confiar no que a tela manda.
- **Convenções de geração:** os identificadores são citados como no scaffold
  (`quote` por tipo de banco), os tipos de coluna são mapeados por `cube_type`
  (colunas sem tipo Cube ficam de fora) e os nomes seguem `snake`.
- **Visual:** Tailwind v4 com prefixo `tw:` e componentes shadcn do Playground,
  com React 18.

## Abordagens consideradas

- **Saída do build:**
  - (A) um cubo por tabela, ligados por `joins`, mais uma view;
  - (B) um cubo só, com `sql` fazendo os JOINs.
  - **Escolhida: B**, pelo usuário. Ela tem o risco de duplicar medidas,
    tratado com `count_distinct` (ver abaixo).
- **Tela:**
  - React Flow (`@xyflow/react`, MIT, compatível com React 18);
  - SVG feito à mão.
  - **Escolhido: React Flow.** O trabalho fica na regra do model, não em
    arrasto, zoom e roteamento de linhas.
- **Onde o model fica:**
  - tabela `cube_models` no Postgres;
  - arquivo JSON na pasta da organização.
  - **Escolhida: Postgres.** Não aparece em Files, não confunde o loader nem a
    sincronização, e guarda as posições da tela.

## Desenho

### Dados (`migrations.sql`)

```
cube_models(
  id uuid PK,
  organization_id uuid FK organizations ON DELETE CASCADE,
  name text,
  definition jsonb,
  cube_name text NULL,          -- fixado no primeiro build
  cube_file_id uuid NULL,
  cube_built_hash text NULL,    -- sha256 do conteúdo gravado no último build
  version int,
  created_at, created_by, updated_at, updated_by,
  UNIQUE(organization_id, name)
)
```

Formato de `definition`:

```json
{
  "dataSource": "pg",
  "tables": [
    { "alias": "orders", "schema": "public", "table": "orders", "x": 0, "y": 0 }
  ],
  "primary": "orders",
  "primaryKey": "id",
  "joins": [
    {
      "from": { "alias": "orders", "column": "customer_id" },
      "to":   { "alias": "customers", "column": "id" },
      "relationship": "many_to_one",
      "join": "left"
    }
  ]
}
```

- **Aliases:** `alias` é único no model e permite usar a mesma tabela duas
  vezes (a segunda recebe `customers_2`).
- **Relationship:** lida de `from` para `to`, com os valores `one_to_one`,
  `one_to_many` ou `many_to_one`.

### API (rotas RBAC, em `handlers/cube_models.rs`)

- `GET /v1/organizations/:id/cube-models`: lista com
  `{id, name, cubeName?, updatedAt}`.
- `POST /v1/organizations/:id/cube-models`, com corpo `{name, definition}` →
  201.
- `GET /v1/cube-models/:id`.
- `PUT /v1/cube-models/:id`, com corpo `{name?, definition?, version}` (409 se
  a versão estiver velha).
- `DELETE /v1/cube-models/:id?version=`: apaga o model, mas não o cubo.
- `POST /v1/cube-models/:id/build`, com corpo `{force?: bool}` →
  `{fileName, content, cube}`, mais o detalhe novo da organização, como as
  outras escritas de modelo.

Sobre as respostas:
- Um model de outra organização responde 404, como os arquivos.
- O `org_detail` passa a trazer `models: [{id, name, cubeName}]`, para a árvore.

### Validação do model

Vale ao salvar (estrutura) e ao fazer o build (contra o banco). Erros em
português, com 400:

- **Aliases:** aliases únicos; `primary` precisa ser um deles.
- **Ligações:** toda ligação aponta para aliases e colunas existentes.
- **Grafo conexo:** toda tabela alcança a principal pelas ligações, e não há
  ciclos. Numa árvore de JOINs, cada tabela além da principal entra uma vez.
  - Uma segunda ligação entre os mesmos dois aliases é recusada (seria um
    ciclo).
  - Para ligar a mesma tabela por outro caminho, o usuário arrasta a tabela de
    novo, criando um novo alias (gerado automaticamente, sem edição).
    *(decisão minha)*
- **Conexão:** a conexão do model é da organização, e toda tabela é dessa
  conexão.
- **Chave primária, no build:**
  - `primaryKey` é obrigatório e precisa ser uma coluna da tabela principal;
  - o build relê as colunas da conexão, então tabelas e colunas que sumiram
    do banco são erro.

### Geração (opção B)

```yaml
# Built from model <name> (<id>): Build cube rewrites this file
cubes:
  - name: <snake(name)>
    data_source: <dataSource>      # omitido quando "default"
    sql: >
      SELECT "orders"."id" AS "orders_id", ..., "customers"."region" AS "customers_region"
      FROM "public"."orders" AS "orders"
      LEFT JOIN "public"."customers" AS "customers"
        ON "orders"."customer_id" = "customers"."id"
    dimensions:
      - name: orders_id
        sql: "{CUBE}.\"orders_id\""   # ou o equivalente citado para o banco
        type: number
        primary_key: true
      ...
    measures:
      - name: count
        type: count            # ou count_distinct, sql: orders_id
```

- **Ordem dos JOINs:** percorre a árvore a partir da principal. Uma ligação
  desenhada ao contrário (o `to` já está na árvore) é invertida, com a
  relationship espelhada (`one_to_many` ↔ `many_to_one`).
- **Colunas:** cada uma vira `<alias>_<coluna>` em `snake`. Colisões depois do
  `snake` ganham o sufixo `_2`.
- **Multiplicação de linhas:** acontece se algum JOIN da árvore for
  `one_to_many`, já orientado a partir da principal. Nesse caso `count` vira
  `count_distinct` da chave primária; sem multiplicação, fica `count` simples.
  - `one_to_one` e `many_to_one` não multiplicam.
  - Um `many_to_one` pendurado num ramo que já multiplicou não muda nada: a
    árvore já multiplicou.
  - `INNER` pode descartar linhas da principal, e isso é o que INNER
    significa. O diálogo explica isso numa linha. *(decisão minha)*
- **Chave primária:**
  - é uma única coluna da tabela principal (sem chave composta nesta versão);
  - o tipo vem de `cube_type`;
  - uma coluna sem tipo Cube é recusada como chave. *(decisão minha)*
- **Nome do cubo:** `cube_name = snake(name)` no primeiro build, e fica
  gravado. Renomear o model depois não muda o cubo nem o arquivo. *(decisão
  minha)*
- **Rebuild:**
  - se o arquivo `cube_file_id` existe e o conteúdo dele bate com
    `cube_built_hash`, ele é sobrescrito;
  - se o conteúdo mudou (editado à mão), a resposta é 409 com a mensagem
    "O cubo foi editado desde o último build"; a tela pergunta e reenvia com
    `force: true`;
  - a comparação é pelo conteúdo, não pela versão, para que movimentar ou
    regravar o arquivo sem mudar nada não dispare o 409 *(decisão minha)*;
  - se o arquivo sumiu, é criado de novo no mesmo caminho;
  - todo build bem-sucedido atualiza `cube_file_id` e `cube_built_hash`.
- **Model apagado:** o arquivo do cubo fica, e o comentário de cabeçalho é só
  um comentário. Não se usa `managed_by`, então o arquivo continua editável.
  *(decisão minha)*
- **Na lista e no `org_detail`:** `cubeName` vem da coluna `cube_name`, não do
  arquivo.
- **Nome do cubo duplicado:** o `ModelWriter` recusa e o erro aparece na tela.

### Playground

- **Árvore (`tree.ts`):**
  - grupo `org:<slug>:models` com título `Models (n)`, depois de Cubes;
  - folhas `model:<slug>:<id>`;
  - `parseKey` passa a conhecer os tipos `models` e `model`.
- **Página (`SchemaPage`):**
  - clicar num model, ou em "Novo model", abre `ModelEditor` na área da
    direita;
  - "Novo model" pede o nome (com o `NameDialog`).
- **`ModelEditor`:**
  - painel de tabelas com busca, arrastável (HTML5 drag até o canvas);
  - a fonte do painel é o `tables` que o `org_detail` já traz (todas as
    conexões da organização, lidas ao expandir); tabelas de outra conexão que
    não a do model aparecem desabilitadas *(decisão minha)*;
  - um model cuja conexão ou cujas tabelas sumiram abre mesmo assim, com
    aviso, e o build falha com o erro de validação *(decisão minha)*;
  - canvas React Flow com nós `TableNode`, que mostram o título, o selo de
    principal, as colunas com tipo e um handle de cada lado por coluna;
  - `onConnect` abre um diálogo de cardinalidade e JOIN;
  - cada aresta mostra um rótulo (`N:1 · LEFT`), e clicar nela abre o mesmo
    diálogo, com a opção de apagar;
  - na barra: nome, conexão, chave primária (um select com as colunas da
    principal), Salvar e Build cube;
  - aviso de alterações não salvas.
- **Depois do build:** a árvore recarrega a organização e um aviso oferece
  "Abrir cubo".

## A cargo de quem constrói

- Layout fino dos nós e do painel de tabelas.
- Mensagens exatas, além das citadas acima.
- Como reaproveitar `quote`, `cube_type` e `snake` do scaffold: torná-las
  `pub(crate)` é o esperado.

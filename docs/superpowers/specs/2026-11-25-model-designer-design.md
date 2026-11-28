# Models como ferramenta de modelagem

## Intenção

O editor de Models passa a ser uma ferramenta de modelagem: as tabelas no
canvas ficam mais legíveis, uma barra de ferramentas no topo reúne as ações
(nova tabela, cor, organizar, zoom...) e o usuário pode **projetar tabelas
novas** e **criá-las no banco** da conexão do model. Depois de criadas, elas
são tabelas comuns e entram no Build cube.

## Metas

- **Tabelas no canvas:**
  - cabeçalho com faixa da cor da tabela e ícone de tabela;
  - selo `principal` e selo `nova` (tabela projetada, ainda fora do banco);
  - por coluna, ícone do tipo (número, texto, data, booleano), 🔑 na chave
    primária e ícone de ligação nas colunas que têm link;
  - realce da linha sob o mouse, anel na tabela selecionada e sombra.
- **Barra de ferramentas**, acima do canvas:
  - Nova tabela;
  - Cor da tabela selecionada (paleta de 8 cores);
  - Editar tabela nova;
  - Remover a seleção do model;
  - Organizar (layout automático em camadas a partir da principal);
  - Zoom +, Zoom − e Ajustar à tela;
  - Criar no banco.
- **Nova tabela:** um diálogo com nome, schema (os da conexão, ou digitado) e
  colunas (nome, tipo, chave primária e "aceita nulo"). Também abre ao dar
  dois cliques numa tabela nova.
- **Criar no banco:**
  - mostra os `CREATE TABLE` no dialeto da conexão, pede confirmação e
    executa;
  - as tabelas criadas perdem o selo `nova`.

## Fora do escopo

- `ALTER TABLE` em tabelas existentes, `DROP TABLE`, índices e chaves
  estrangeiras no banco.
- Desfazer e refazer.
- Conexões que não sejam Postgres ou MySQL para "Criar no banco": as outras
  respondem 400 com a mensagem "Criar tabelas não é suportado para <tipo>".

## Anti-metas

- **SQL montado a partir de texto livre:**
  - nomes de tabela, schema e coluna seguem `[A-Za-z_][A-Za-z0-9_]*`, com no
    máximo 63 caracteres;
  - os tipos vêm de uma lista fechada;
  - o SQL é gerado no servidor.
- **Criar no banco sem permissão:** criar tabela pede a mesma permissão do
  `/v1/execute`.
- **Apagar ou sobrescrever dados:** uma tabela que já existe no banco não é
  recriada, e a resposta é 409.

## Restrições

- **Quem cria no banco (opção a, escolhida pelo usuário):**
  - precisa poder editar o model (`visible_organization(..., "admin")`);
  - e também precisa de `authorize_execute`: permissão "execute" no workspace
    do cabeçalho `X-Cube-Workspace`, com a conexão do model liberada para ele;
  - platform admins passam direto;
  - sem workspace escolhido, a resposta é 403 com uma mensagem dizendo para
    escolher um workspace com "execute".
- **Tipos portáveis**, mapeados por banco:

  | Tipo      | Postgres           | MySQL           |
  |-----------|--------------------|-----------------|
  | text      | `text`             | `text`          |
  | integer   | `integer`          | `int`           |
  | bigint    | `bigint`           | `bigint`        |
  | numeric   | `numeric(18,4)`    | `decimal(18,4)` |
  | boolean   | `boolean`          | `boolean`       |
  | date      | `date`             | `date`          |
  | timestamp | `timestamp`        | `datetime`      |

  `numeric(18,4)` e `decimal(18,4)` são decisão minha.
- **Visual:** Tailwind `tw:` e shadcn, sem cores fixas. As 8 cores da paleta
  são tokens CSS com valores de claro e escuro.

## Abordagens

- **(1) O servidor gera e executa o DDL** numa rota do model. **Escolhida.**
  - Valida nomes e tipos e cita os identificadores (`quote`), como no build.
  - Reaproveita a execução do `/v1/execute`.
  - O prévio usa a mesma rota com `dryRun`.
- **(2) A tela gera o SQL e chama o `/v1/execute`.** Descartada:
  - o SQL seria montado no navegador;
  - a rota aceita qualquer statement;
  - o model não saberia quais tabelas foram criadas.

## Desenho

### Definição do model (JSON, compatível com os models atuais)

`ModelTable` ganha dois campos opcionais:

- `color`: um nome da paleta (`slate`, `blue`, `green`, `amber`, `red`,
  `violet`, `pink`, `teal`). Sem cor, a tabela usa o neutro.
- `draft`: o projeto de uma tabela que ainda não existe no banco, no formato
  `{ columns: [{ name, type, nullable, primaryKey }] }`.

Validação ao salvar:
- colunas de nomes únicos, pelo menos uma;
- tipos da lista acima e nomes no padrão acima;
- duas tabelas novas não podem ter o mesmo `schema.table`.

O **build** recusa um model com tabela nova: "A tabela X ainda não existe no
banco: use Criar no banco".

### API

`POST /v1/cube-models/:id/create-tables`, com corpo `{ version, dryRun? }`.

- **Com `dryRun`:** responde `{ statements: [sql] }` e não executa nada.
- **Sem `dryRun`:**
  - confere as permissões acima;
  - relê o schema da conexão: uma tabela nova que já exista no banco gera 409
    "A tabela X já existe no banco";
  - executa um `CREATE TABLE` por tabela nova, em ordem, e para no primeiro
    erro;
  - as tabelas criadas perdem o `draft`, o model é gravado (com `version` + 1)
    e o cache de schema da conexão é limpo;
  - responde `{ model, created: [schema.table], error?, detail }`.
- **Versão:** uma `version` velha gera 409, como no PUT.

Um erro no meio deixa as tabelas já criadas marcadas como criadas. A tela
mostra quais foram criadas e qual falhou. *(decisão minha)*

### Playground

- **`modelGraph.ts`** ganha `addDraftTable`, `updateDraftTable`, `setColor` e
  `layout`.
- **`layout`:** camadas em BFS a partir da principal. Cada camada é uma
  coluna a 320px da anterior e as tabelas ficam a 40px uma da outra. As
  tabelas soltas vão para a última coluna. *(decisão minha)*
- **`ModelToolbar.tsx`:** a barra, com ícones lucide, dicas e
  `aria-label`s. "Criar no banco" abre um diálogo com o SQL do `dryRun`;
  "Criar" executa.
- **`TableDialog.tsx`:** o diálogo de nova tabela e de edição de tabela nova.
- **`TableNode.tsx`:** o visual acima. Uma tabela nova usa as colunas do
  `draft`.
- **Seleção:** a seleção do React Flow (clique) decide a tabela da cor e do
  "Remover".

## A cargo de quem constrói

- O layout fino da barra e do nó, os ícones exatos e os textos além dos
  citados.

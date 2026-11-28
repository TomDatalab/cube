# Model no estilo Redgate Data Modeler, com cubos

## Intenção

O editor de Model do Playground passa a funcionar como o Redgate Data
Modeler: um modelador de banco completo, com tabelas, ligações (FKs), views,
notas e áreas, edição de canvas (desfazer, copiar, alinhar...), painéis de
estrutura e propriedades, validação e exportação. Além do que o Redgate faz,
o usuário **cria cubos do Cube no mesmo canvas**, apoiados nas tabelas do
model.

Referência: o model "MySQL demo physical data model" na conta Redgate do
usuário, examinado em 2026-11-26 (só leitura).

**Regra de estilo do usuário:** as cores são as nossas (tokens CSS, shadcn,
paleta `TABLE_COLORS`, claro e escuro). **Todo o resto** (disposição da tela,
desenho das tabelas, ligações em pé de galinha, áreas tracejadas, painéis,
menus, atalhos, comportamento) segue o Redgate.

## Metas e fases

O trabalho é feito em 5 fases, nesta ordem, cada uma com o seu plano e
entregue (build, instalação, commit) antes da próxima.

### Fase 1 — Base do editor

**Layout da tela (como o Redgate):**
- Barra superior, em grupos separados por divisórias:
  1. Desfazer (Ctrl+Z) e refazer (Ctrl+Y / Ctrl+Shift+Z).
  2. Salvar (Ctrl+S), exportar imagem, exportar PDF, gerar SQL (Ctrl+G) e
     gerar documentação. Na fase 1 só "Salvar" funciona; os outros aparecem
     desabilitados, com dica "em breve", até as suas fases.
  3. Copiar (Ctrl+C), colar (Ctrl+V), colar como atalho, recortar (Ctrl+X) e
     excluir (Del).
  4. Menus Tamanho, Alinhar, Distribuir, Ordem e Ligações (detalhes abaixo).
  5. Zoom −, 1:1, zoom + e uma caixa de zoom (25, 50, 75, 100, 200, 400%).
  6. Mostrar/esconder painel esquerdo (Ctrl+Alt+N), painel direito
     (Ctrl+Alt+P) e barra superior (Ctrl+Alt+B).
  7. À direita, busca (Ctrl+F).
- Os botões ficam desabilitados quando a ação não se aplica à seleção, como
  no Redgate.
- **Paleta de ferramentas** em linha, flutuando no canto superior esquerdo do
  canvas (como no Redgate), com atalhos 1 a
  8: Selecionar (1), Selecionar área (2), Nova tabela (3), Nova ligação
  (4), Nova view (5), Nova nota (6), Nova área (7) e **Novo cubo (8)**. Na
  fase 1 funcionam 1, 2, 3 e 4; as outras aparecem desabilitadas até as
  suas fases.
- **Painel esquerdo** em sanfona com três seções: "Estrutura do model",
  "Explorador do model" (fase 5) e "Problemas" (fase 2).
  - A estrutura é uma árvore: Model › Tabelas (› colunas), Ligações, Notas,
    Views, Áreas e Cubos.
  - Clicar num item o seleciona e centraliza no canvas.
- **Painel direito** de propriedades, com o título conforme a seleção
  ("Propriedades do model", "Propriedades da tabela", "Propriedades da
  ligação"...). As seções são sanfonas com + e −.
  - Na fase 1 há: model (nome, descrição, conexão, criado e alterado por,
    tamanho), tabela (nome, comentário, cor, colunas) e ligação (os campos
    que já existem hoje).
- **Minimapa** no canto inferior direito, com o retângulo da área visível,
  que pode ser arrastado, e um botão para recolhê-lo.

**Comportamento:**
- **Seleção:** clique; Ctrl+clique soma à seleção (como no Redgate); a
  ferramenta "Selecionar área" (ou arrastar no vazio com ela ativa)
  seleciona por retângulo; Ctrl+A seleciona tudo. Um objeto selecionado
  mostra alças nos cantos para redimensionar.
- **Sem menu de botão direito**, como no Redgate: tudo fica na barra.
- **Desfazer e refazer:** guardam o histórico das alterações da definição
  (mover, editar, criar, excluir, colar, alinhar...), com até 100 passos.
  Uma sequência de arrasto conta como um passo. O histórico não sobrevive a
  recarregar a página.
- **Copiar, colar e recortar:**
  - copiar guarda as tabelas selecionadas e as ligações entre elas;
  - colar cria cópias deslocadas 20px, com nomes livres (`orders` vira
    `orders_1`). Colar uma tabela que existe no banco cria outro apelido da
    mesma tabela;
  - "Colar como atalho" mostra **a mesma tabela** em outro ponto do canvas.
    O atalho tem o mesmo nome, um ícone de atalho no cabeçalho e as mesmas
    ligações. Editar um edita o original. Excluir o atalho não exclui a
    tabela.
  - A área de transferência é interna à aba. Copiar entre abas ou models
    fica fora.
- **Menus de arranjo** (aplicam-se à seleção). Os itens foram conferidos no
  Redgate em modo de edição, numa cópia descartada do model de demonstração:
  - **Size:** Adjust to text (volta ao tamanho do conteúdo), Same width e
    Same height (pelo tamanho do primeiro selecionado). Redimensionar fixa
    o tamanho, guardado no model.
  - **Align:** Top, Center, Bottom (vertical) | Left, Center, Right
    (horizontal). Precisa de 2 ou mais itens.
  - **Distribution:** Justify, Top, Center, Bottom (vertical) | Justify,
    Left, Center, Right (horizontal). "Justify" deixa espaços iguais entre
    os itens. Os outros igualam a distância entre as bordas ou os centros.
    Precisa de 3 ou mais itens.
  - **Order:** fica desabilitado para tabelas no Redgate e serve para áreas
    e notas. *(decisão minha: Bring to front e Send to back, pelo `z`)*
  - **References:** Show all references, Add reference point, Reduce
    reference point e Swap reference orientation.
    - Pontos de referência são dobras do traçado. Ficam guardados na
      ligação e podem ser arrastados.
    - "Swap" troca o lado de onde a linha sai da tabela.
    - "Show all references" mostra as ligações escondidas.
- **Busca (Ctrl+F):** procura nomes de tabelas, colunas, ligações, views,
  notas, áreas e cubos. Lista os resultados, e Enter ou clique seleciona e
  centraliza.
- **Salvar:** Ctrl+S, como hoje (com `version`). Sair com alterações não
  salvas continua pedindo confirmação.
- **O que já existe e continua valendo:** criar tabela nova, cores, minimizar
  tabela, ligar arrastando ou clicando na coluna, organizar (layout
  automático), criar no banco e build.

**Desenho das tabelas (como o Redgate, com as nossas cores):**
- Caixa retangular de borda fina e cabeçalho com o nome centralizado em
  negrito. A cor escolhida pinta o cabeçalho.
- Cada coluna em três colunas alinhadas: nome, tipo e marcas `PK`, `FK`,
  `PK FK`, `N` (aceita nulo, quando ativado nas opções de formato).
- **Ligações em pé de galinha**, com traçado em ângulos retos: um traço no
  lado "um" e o pé de galinha no lado "muitos". Um círculo indica que o lado
  é opcional e um traço que é obrigatório.
- Uma tabela selecionada ganha borda de destaque (nosso token `ring`).

### Fase 2 — Modelagem de banco

- **Dialeto do model:** Postgres ou MySQL, o da conexão do model.
  - Os tipos de coluna passam a ser os do dialeto, numa lista fechada.
    Tipos com parâmetros (`varchar(n)`, `decimal(p,s)`, `char(n)`...) são
    validados por expressão regular no servidor.
  - Os 7 tipos portáveis de hoje são convertidos para o tipo do dialeto na
    primeira vez que o model é salvo com a versão nova.
- **Propriedades da tabela** (painel direito), em sanfonas:
  - Geral: nome e comentário.
  - Colunas: em grade com nome, tipo, N (aceita nulo) e PK. Cada linha tem
    uma alça de arrasto para reordenar e um "…" que abre os detalhes:
    comentário, valor padrão, expressão de check, auto incremento e, no
    MySQL, unsigned e zerofill.
  - Chave primária: nome e colunas.
  - Chaves únicas: nome e colunas, várias.
  - Índices: nome, colunas (com ASC/DESC) e único sim/não.
  - Checks: nome e expressão.
  - Scripts SQL adicionais: antes e depois do `CREATE`.
  - Formato: cor (a nossa paleta) e tamanho fixo.
  - Problemas da tabela: erros, avisos e dicas.
  - Um link "Prévia do SQL" no topo mostra o DDL da tabela.
- **Tabelas que existem no banco:** a estrutura (colunas e tipos) vem da
  conexão e é só leitura no painel. Dá para editar comentário, cor e
  formato. Alterar uma tabela existente (`ALTER TABLE`) fica fora do
  escopo.
- **Propriedades da ligação**, como no Redgate:
  - nome, comentário, cardinalidade (0..*, 0..1, 1..*, 1..1);
  - tabela primária e estrangeira, papel de cada lado e pares de colunas
    (várias colunas);
  - obrigatória sim/não;
  - ações ON UPDATE e ON DELETE (none, restrict, cascade, set null, set
    default).
  - **Nova ligação, como no Redgate** (conferido): arrastar com a
    ferramenta "Nova ligação" da tabela pai para a filha cria a ligação
    `<filha>_<pai>`, com cardinalidade 0..* e obrigatória.
    - Se a filha é uma tabela nova, ganha a coluna `<pai>_<pk>` com o tipo
      da PK do pai, marcada `FK`.
    - Se a filha já existe no banco, não ganha coluna. A ligação usa a
      coluna de mesmo nome, se houver, ou fica sem par até o usuário
      escolher no painel. Essa ligação fica só no model e não gera `ALTER
      TABLE`. *(decisão minha)*
    - O painel tem "Swap reference" (inverte pai e filha) e "+ Add" (mais
      um par de colunas).
    - Ligar clicando ou arrastando entre colunas, como hoje, continua
      funcionando e usa as colunas escolhidas.
  - O `join` (left/inner) e a `relationship` do Cube saem da ligação e vão
    para o join de cubos (fase 4).
- **Gerar SQL (Ctrl+G):** um diálogo com o script completo do model no
  dialeto: `CREATE TABLE`, chaves, índices, checks, FKs, views, mais os
  scripts adicionais do model e das tabelas. Opções: incluir `DROP`, só as
  tabelas novas, só a seleção. Permite copiar e baixar o `.sql`.
- **Criar no banco:** continua executando só as tabelas novas (com as suas
  chaves, índices, checks e FKs), com prévia e a mesma permissão de hoje.
- **Validação e Problemas:**
  - As regras são as do Redgate, agrupadas por tabela, coluna, chave única,
    check, índice, ligação, view e coluna de view. Exemplos: nome vazio,
    nome repetido, tabela sem colunas, PK que aceita nulo, ligação cujas
    colunas não batem.
  - Cada regra é um erro ou um aviso, e pode ser ligada ou desligada em
    "Configurações de validação", nas propriedades do model.
  - A validação roda no navegador a cada alteração. A seção "Problemas" do
    painel esquerdo lista tudo, e clicar num problema seleciona o objeto.
  - O servidor valida de novo o que gera SQL.
- **Propriedades do model:** scripts SQL adicionais (início e fim do
  script de criação e de exclusão), configurações de validação, formato
  (mostrar tipos, mostrar N e o tamanho de página da impressão) e
  configurações da geração de SQL.

### Fase 3 — Notas, áreas e views

- **Nota de texto:** caixa de texto livre no canvas, com cor e tamanho
  ajustável. É editada no painel e com dois cliques.
- **Área (subject area):**
  - retângulo com borda tracejada, fundo da nossa paleta (claro) e nome no
    canto inferior direito;
  - tem nome, comentário, cor de fundo e cor da borda;
  - mover a área move junto as tabelas, views, notas e cubos que estão
    inteiramente dentro dela;
  - fica sempre atrás dos outros objetos.
- **View:**
  - caixa de cantos arredondados e fundo azul claro (nosso token),
    como no Redgate;
  - propriedades: nome, comentário, consulta SQL (editor com números de
    linha, como o `NumberedCode` de hoje), colunas (nome e tipo),
    dependências (as tabelas e views que a consulta usa, escolhidas pelo
    usuário) e scripts adicionais;
  - entra no "Gerar SQL" como `CREATE VIEW`, e as dependências viram linhas
    tracejadas no canvas.

### Fase 4 — Cubos (abordagem A)

- O cubo é um tipo de objeto próprio no canvas. A caixa segue o desenho da
  tabela, com um ícone de cubo e a cor da nossa paleta, e mostra dimensões e
  medidas em duas seções.
- **Origem do cubo**, escolhida nas propriedades:
  - uma **tabela** do model (`sql_table`);
  - um **conjunto de tabelas ligadas**: uma tabela principal e as tabelas
    alcançadas pelas ligações, achatadas num `sql` com JOINs. É o build de
    hoje;
  - um **SQL** livre.
  - Uma linha tracejada liga o cubo às tabelas de onde ele lê.
- **Propriedades do cubo:**
  - nome, título e descrição;
  - dimensões (nome, SQL ou coluna, tipo, chave primária, título);
  - medidas (nome, tipo `count`, `sum`, `avg`, `min`, `max`,
    `count_distinct`..., SQL ou coluna, título, formato);
  - segmentos;
  - "Gerar membros" cria uma dimensão por coluna e uma medida `count`, como
    o build de hoje faz.
- **Joins entre cubos:** a ferramenta "Nova ligação" entre dois cubos cria um
  join do Cube (relationship `one_to_one`, `one_to_many` ou `many_to_one`, e
  SQL da condição montado a partir das colunas escolhidas). O join é
  desenhado como ligação com rótulo, no mesmo estilo de pé de galinha.
- **Build:** gera **um `.yml` por cubo** do model. O cubo removido do model
  tem o seu arquivo removido no próximo build. Os nomes dos cubos são únicos
  no model, e o servidor recusa o build se um nome colide com um cubo que
  não é do model.
- **Models que já existem:** na primeira vez que são abertos, o primary, o
  primaryKey e os joins de hoje viram um cubo de origem "tabelas ligadas"
  com o `cubeName` atual, e os joins viram ligações. O primeiro build depois
  disso gera o mesmo cubo de antes.

### Fase 5 — Explorador, exportação e colaboração

- **Explorador do model:** como no Redgate, uma consulta SQL sobre o próprio
  model. Há uma tabela virtual por tipo de objeto (tabelas, colunas, chaves
  primárias, chaves únicas, índices, checks, ligações, views, colunas de
  view, áreas, notas, cubos, dimensões e medidas). O resultado aparece em
  grade e oferece "Achar no diagrama" e "Exportar para CSV". A consulta
  roda no navegador.
- **Exportar imagem** (PNG e SVG) do diagrama inteiro ou da seleção.
- **Exportar PDF**, com tamanho de página, orientação e margens das
  propriedades do model, e prévia das quebras de página no canvas.
- **Gerar documentação:** um HTML com o model (tabelas, colunas, ligações,
  views, cubos e o diagrama), para baixar.
- **Edição por uma pessoa de cada vez:**
  - quem abre o model para editar fica com o controle;
  - quem abre depois vê "Modo só leitura · O documento está sendo editado
    por X" e um botão "Assumir o controle";
  - assumir o controle avisa quem tinha, que passa a só leitura, e o
    trabalho dele que não foi salvo é perdido;
  - o controle expira 2 minutos depois do último sinal do navegador de
    quem o tem.
- **Compartilhar:** abre o compartilhamento que o model já tem hoje pelas
  permissões da organização.

## Fora do escopo

- `ALTER TABLE`, `DROP` e qualquer mudança em tabela que já existe no banco.
  O "Gerar SQL" pode incluir `DROP` no script, mas o script só é baixado.
- Engenharia reversa do banco inteiro de uma vez. As tabelas continuam
  entrando uma a uma (ou em lote), como hoje.
- Models lógicos e conceituais, sequências e bancos além de Postgres e
  MySQL.
- Edição simultânea de várias pessoas no mesmo model.
- Histórico de versões do model e comentários.

## Anti-metas

- **Cores que não são as nossas:** nada de cor fixa em hex; tudo por token,
  com claro e escuro.
- **Perder trabalho:** desfazer funciona para tudo que altera a definição, e
  as alterações não salvas continuam pedindo confirmação.
- **Criar no banco sem permissão:** continua exigindo `authorize_execute`.
- **Regressão nos models de hoje:** o build de um model antigo, depois de
  convertido, gera o mesmo cubo.

## Restrições

- Frontend em `packages/cubejs-playground/src/pages/Schema/models`, com
  React Flow (`@xyflow/react` 12.3.6), Tailwind `tw:` e shadcn.
- Definição guardada como JSON em `cube_models` e validada pelo Rust
  (`rbac/cube_model.rs`, com `deny_unknown_fields`). Os campos novos são
  opcionais, e a definição antiga continua aceita.
- **SQL em texto livre:**
  - valor padrão, expressão de check, consulta da view, SQL do cubo e
    scripts adicionais são texto livre, como no Redgate. É uma mudança em
    relação ao design de 2026-11-25, que só aceitava nomes e tipos fechados;
  - esses textos só são executados em "Criar no banco", que exige
    `authorize_execute`, a mesma permissão que já executa SQL livre no
    `/v1/execute`;
  - nomes de tabela, coluna e objeto continuam no padrão
    `[A-Za-z_][A-Za-z0-9_]*`, e os tipos na lista fechada do dialeto.
- Dependências novas no Playground só quando não há alternativa no projeto.
  Elas são carregadas sob demanda (exportações e explorador), para não pesar
  a abertura do editor.

## Abordagens

Para os cubos, o usuário escolheu a **A**:

- **(A) Cubos no mesmo canvas.** **Escolhida.** O cubo é um objeto do
  diagrama com origem em tabelas do model, e os joins entre cubos são
  ligações. É o que mais se parece com o Redgate (onde a view é um objeto
  ao lado das tabelas) e mantém visível de onde cada cubo lê.
- **(B) Duas abas, "Banco" e "Cubos".** Descartada: separa o cubo da tabela
  de onde ele lê.
- **(C) "Virar cubo" em cada tabela.** Descartada: não cobre medidas nem
  cubos sobre SQL.

Ordem das fases: 1 → 2 → 3 → 4 → 5, aprovada pelo usuário.

## Desenho

### Definição do model (versão 2)

A definição ganha `formatVersion: 2`. Uma definição sem esse campo é a
versão 1 de hoje e é convertida ao abrir (no navegador) e ao ler (no
servidor). Campos novos, todos opcionais na gravação:

```ts
type ModelDefinitionV2 = {
  formatVersion: 2;
  dataSource: string;
  dialect?: 'postgres' | 'mysql';           // vem da conexão
  description?: string;
  tables: ModelTable[];                     // + comment, size, z, columns de projeto, keys, indexes, checks, scripts, shortcuts
  references: Reference[];                  // ligações físicas (FK), substituem joins
  views: ModelView[];
  notes: Note[];
  areas: Area[];
  cubes: CubeNode[];                        // fase 4
  cubeJoins: CubeJoin[];                    // fase 4
  settings?: { validation?: Record<string, boolean>; format?: {...}; sqlGeneration?: {...}; scripts?: {...} };
};
```

- **Ids estáveis:** cada objeto recebe um `id` curto (como o Redgate `t1`,
  `r3`, `v1`). Seleção, histórico e atalhos usam o id e não o nome.
- **Posição e tamanho:** `x`, `y`, `width?` e `height?` (ausentes = tamanho
  automático), e `z` para a ordem.
- **Atalho:** `shortcuts: [{ id, x, y }]` dentro da tabela.
- **Conversão da v1:**
  - cada `join` vira uma `reference`. A cardinalidade sai de
    `relationship`, e `join` left/inner vira "obrigatória" não/sim;
  - `primary`, `primaryKey` e o `cubeName` do model viram um cubo de origem
    "tabelas ligadas". Os `join` left/inner e as `relationship` originais
    ficam guardados nesse cubo, para o build gerar o mesmo YAML.

Os campos exatos de cada tipo ficam a cargo de quem constrói, desde que
cubram as propriedades listadas nas fases.

**Migração ao longo das fases** *(decisão minha)*:
- **Fase 1:** o servidor passa a aceitar `formatVersion: 2` com ids, `z`,
  `width`/`height` e `shortcuts`. `primary`, `primaryKey` e `joins` ficam
  como estão, e o build não muda.
- **Fase 2:** os `joins` viram `references`. Cada reference guarda, num
  campo provisório `cube: { relationship, join }`, o que o build ainda usa.
  O build lê `references` + `primary`/`primaryKey` e gera o mesmo YAML de
  antes.
- **Fase 4:** a conversão final cria o cubo "tabelas ligadas" com
  `primary`, `primaryKey`, o `cubeName` e os `cube` das references, e
  remove esses campos.
- Em todas as fases, um model de hoje continua abrindo e fazendo build.

**Conexões que não são Postgres nem MySQL** *(decisão minha)*:
- o model fica em modo "sem dialeto": os tipos são os 7 portáveis de hoje;
- Gerar SQL e Criar no banco ficam desabilitados, com dica;
- as regras de validação que dependem do dialeto ficam desligadas;
- o build e os cubos funcionam normalmente.

**Desfazer depois do servidor alterar o model** *(decisão minha)*: "Criar no
banco" limpa o histórico de desfazer, porque desfazer não pode devolver ao
estado de projeto uma tabela que já existe no banco. Salvar não limpa o
histórico.

**Build que remove o arquivo de um cubo** *(decisão minha)*: segue a mesma
regra de hoje. Se o arquivo foi editado à mão, o build responde 409, e só
remove com `force` (o mesmo diálogo "sobrescrever" de hoje).

### Servidor (Rust)

- `cube_model.rs` aceita a v1 e a v2. Valida nomes, tipos do dialeto,
  unicidade de nomes e referências a ids existentes.
- A **geração de DDL** sai de `create_statements` para um módulo próprio,
  usado por "Gerar SQL" (`POST /v1/cube-models/:id/sql`, que responde
  `{ script }`) e por "Criar no banco".
- **Build por cubo:** uma tabela nova `cube_model_cubes` (`model_id`,
  `cube_name`, a linha do arquivo e o sha), que substitui o `cube_name`
  único de `cube_models`. Um build grava, atualiza e remove os arquivos dos
  cubos do model numa única escrita do model.
- **Controle de edição (fase 5):** `cube_model_locks` (`model_id`,
  `user_id`, `heartbeat_at`), com as rotas `POST .../lock` (pegar ou
  renovar), `POST .../lock/take` e `DELETE .../lock`. O `PUT` do model
  recusa com 423 quem não tem o controle.

### Playground

- **Estado do editor** num store único com a definição, a seleção, o
  histórico (desfazer e refazer) e a área de transferência. As funções puras
  de `modelGraph.ts` continuam sendo as que alteram a definição.
- **Componentes:**
  - `ModelToolbar` refeita nos grupos do Redgate;
  - `ToolPalette`;
  - `StructurePanel` (sanfona);
  - `PropertiesPanel`, com um painel por tipo de objeto;
  - `SearchBox` e `Minimap` (o do React Flow, com as nossas cores);
  - os nós `TableNode`, `ViewNode`, `NoteNode`, `AreaNode` e `CubeNode`;
  - a aresta `ReferenceEdge`, com os símbolos de pé de galinha em SVG e o
    traçado em ângulos retos.
- **Atalhos de teclado** ficam desligados quando o foco está num campo de
  texto.
- Os testes seguem o padrão de `src/__tests__/model-editor.test.ts`
  (vitest): as funções puras e o comportamento de cada fase. Cada fase é
  verificada no navegador (playwright) antes da entrega.

## A cargo de quem constrói

- Os campos exatos dos tipos, os nomes dos componentes e a divisão em
  arquivos.
- Os ícones (lucide), desde que lembrem os do Redgate.
- A biblioteca da consulta do explorador (por exemplo `sql.js` sob demanda)
  e a da exportação de imagem e PDF.
- Os textos de interface ficam em inglês como o resto do Playground. Os
  nomes de botões e seções citados em português neste documento aparecem na
  tela com o nome em inglês do Redgate ("Model structure", "Table
  properties", "Take control"...). As mensagens de erro do servidor ficam em português, como
  hoje.

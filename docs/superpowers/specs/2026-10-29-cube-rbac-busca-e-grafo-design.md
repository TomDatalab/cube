# Busca no Admin e grafo de acessos do usuário — design

Continuação de `2026-10-28-cube-rbac-spicedb-design.md`. Vale tudo o que está lá: esquema do SpiceDB, tabelas `cube_app`, rotas `/v1/...` e regras de papel.

## Intenção

A página **Admin** do Playground (só para platform admins) hoje carrega tudo de uma vez:
- todos os usuários;
- todas as organizações;
- os workspaces de cada organização;
- na aba Users, os membros de **todos** os workspaces, para montar a árvore de arrastar e soltar.

O sistema vai receber **milhares de usuários** (palavras do usuário), e a página precisa continuar rápida e utilizável com esse volume.

Além disso, o usuário quer o que o Playground do OpenFGA oferece: abrir um usuário e ver **num grafo tudo o que ele pode fazer**, tanto as relações gravadas quanto os acessos que derivam delas. Para ele, a pergunta que mais importa é "por que essa pessoa consegue fazer isso?".

São dois trabalhos independentes, feitos nesta ordem:
1. **busca**, porque a lista de usuários, onde fica o botão do grafo, precisa funcionar com milhares de registros;
2. **grafo**.

## Objetivos

1. **Uma caixa de busca em cada aba** (decisão do usuário): Organizations, Workspaces, Users e a árvore de workspaces da aba Users. A busca é feita no servidor, com paginação.
2. **Campos da busca:**
   - usuários: username (que é o próprio id), nome e email;
   - organizações e workspaces: nome e id (UUID);
   - na aba Workspaces e na árvore da aba Users, a busca também casa o **nome da organização** [decisão do Claude]. Digitar "ON" mostra os workspaces da ON, e a árvore está agrupada por organização.
3. **Nenhuma tela do Admin carrega listas inteiras.** Em particular, a árvore da aba Users deixa de buscar os membros de todos os workspaces de uma vez.
4. **O seletor de workspace da barra superior também escala** (decisão do usuário: entra no escopo). Ele aparece em todas as páginas do Playground.
   - Hoje ele chama `GET /v1/organizations` e depois uma rota por organização.
   - Passa a ser um seletor com busca e paginação, alimentado por uma rota nova de "meus workspaces".
   - A rota funciona para qualquer usuário, não só para platform admins, e devolve só os workspaces que ele pode ver (`lookup_resources`), já com o nome da organização de cada um.
   - Continua oferecendo "Todos (admin)" a platform admins.
5. **Um botão Access** ao lado de "Reset password", em cada usuário, abre o grafo de acessos no **modelo C** (escolha do usuário entre três modelos):
   - o grafo em camadas, usuário → organizações → workspaces → conexões;
   - um painel "What \<user\> can do" ao lado.
6. **O grafo mostra as relações diretas e os acessos derivados** (decisão do usuário: "quero ver tudo que ela pode fazer").
7. **Clicar numa permissão do painel destaca no grafo** o caminho que a concede.

## Não-objetivos

- Uma busca única que encontre usuários, workspaces e organizações juntos. O usuário escolheu uma busca por aba.
- Um conector "SpiceDB" no Data Source. A conversa sobre ele foi substituída por este grafo.
- Editar acessos de dentro do grafo. Ele só mostra; as edições continuam nas abas.
- Grafo de quem acessa um recurso (a visão inversa). Aqui é só o grafo de um usuário.
- Mudar o esquema do SpiceDB ou as regras de papel.

## Anti-objetivos

- **O grafo não pode mentir.** Ele mostra o que o usuário **de fato consegue fazer**, nada a mais e nada a menos. Se o grafo e o comportamento real da API discordarem, o grafo está errado. Isso inclui os casos em que o servidor é mais restrito que o SpiceDB (ver "Quando o servidor restringe o SpiceDB").
- Uma tela do Admin, ou o seletor da barra superior, que fique lenta ou trave com milhares de usuários ou workspaces.
- Um platform admin cujo grafo vire uma "bola de pelo" com milhares de nós.

## Restrições

- **Interface:** React + antd 4.16 no pacote `cubejs-playground`, seguindo o estilo de `src/pages/Admin/*`.
  - O arrastar e soltar da aba Users usa o drag nativo do navegador (`USER_DRAG_TYPE` em `MembershipTree.tsx`) e precisa continuar funcionando com a busca.
  - O Playground já depende do `recharts`, mas ele não desenha grafos.
- **Servidor:** Rust, `cubeserver::rbac`. As permissões vêm do SpiceDB, pelo trait `Authz` (`check`, `lookup_resources`, `read`), e os cadastros vêm do Postgres (`cube_app`).
- **Rotas que já existem:** o formato das respostas que o Playground já usa não pode quebrar, mesmo depois que o seletor da barra superior trocar de rota. Os parâmetros de busca e paginação entram como opcionais. Sem eles, a resposta continua como hoje.
- **Visibilidade:**
  - a busca respeita as mesmas regras das listagens atuais (o que o usuário pode ver);
  - o grafo é só para platform admins, como o resto do Admin;
  - as rotas novas respondem 401 sem token e 403 sem permissão.
- **Commits e entrega:** commits na branch `feat/rbac-spicedb` (PR #8), em conventional commits. Ao final, a mudança é implantada em `/root/cube-rust` e verificada no navegador, como nas entregas anteriores.

## Decisões

### Busca (decisões do usuário: uma caixa por aba; campos acima)

- **Como a busca casa os termos [decisão do Claude]:** por trecho, sem diferenciar maiúsculas e minúsculas. Por exemplo, `ana` encontra `e2e-ana`, "Ana Souza" e `ana@x.com`. Num UUID, só casa um trecho do início ou o UUID inteiro.
- **Paginação [decisão do Claude]:**
  - 50 por página, em ordem alfabética (o username para usuários, o nome para os demais);
  - a resposta traz o total, para a tabela mostrar a paginação;
  - a busca dispara depois de uma pausa na digitação, sem um botão.
- **Aba Workspaces:**
  - a busca e o filtro por organização trabalham juntos;
  - a lista vem de **uma** chamada que pagina os workspaces de todas as organizações, em vez de uma chamada por organização como hoje.
- **Filtro de organização com muitas organizações [decisão do Claude]:** o seletor pesquisa enquanto você digita, em vez de listar todas.
- **Árvore da aba Users:**
  - a busca filtra os workspaces da árvore, e cada organização com resultados aparece com os seus;
  - os membros de um workspace só são carregados quando ele é aberto;
  - quando você solta um usuário num workspace, os membros dele também são carregados;
  - o número de membros ao lado de cada workspace pode sair junto com a página de workspaces, ou aparecer só depois de abrir o workspace. Fica a critério de quem implementar, desde que não exija ler os membros de todos os workspaces.
- **Diálogos que escolhem um usuário ou uma organização** (o owner de uma nova organização, o membro de um workspace, o seletor "Organization" de um novo workspace): também pesquisam no servidor enquanto você digita, em vez de receber a lista inteira.

### Grafo de acessos (modelo C, escolha do usuário)

O modelo aprovado está em https://claude.ai/artifact/Tkka575a24epJAoScwNkk9, opção C.

- **Onde fica:** o botão **Access**, ao lado de "Reset password", abre uma janela grande.
  - O grafo ocupa a esquerda e o painel "What \<user\> can do" a direita.
  - Numa tela estreita, o painel vai para baixo do grafo.
- **Nós:** usuário, organizações, workspaces e conexões.
  - Cada nó mostra o nome, o tipo e as permissões que o usuário tem ali, como `admin · view · connections`.
  - Conexões aparecem quando o usuário pode consultá-las ou administrá-las.
- **Arestas:**
  - **Linha cheia** para as relações gravadas no SpiceDB: `organization#owner`, `organization#member`, `workspace#owner|editor|viewer`.
  - **Linha tracejada** para os acessos derivados:
    - o owner de uma organização administra todos os workspaces dela (`admin ⇒ manage`);
    - uma conexão liberada para um workspace pode ser consultada por quem o vê (`released`);
    - o admin de uma organização gerencia as conexões dela.
- **Painel:** os recursos, cada um com as permissões que o usuário tem e o motivo de cada uma, por exemplo `manage` com o motivo "owner of ON", ou `query` com "released to Operations".
  - Clicar numa permissão destaca no grafo as arestas e os nós do caminho que a concede.
  - Quando há mais de um caminho, por exemplo editor do workspace **e** owner da organização, o painel mostra os dois motivos.
- **Permissões cobertas:** todas as do esquema, para cada recurso:
  - organização: `admin`, `view`, `manage_connections`;
  - workspace: `manage`, `edit`, `view`, `query`, `execute`;
  - conexão: `manage`, `query`.
- **Objetos [decisão do Claude]:** pipelines e afins não viram nós. A leitura e a gravação de objetos aparecem como permissões do workspace (`read objects` e `write objects`, que seguem `view` e `edit`).
- **Platform admin [decisão do Claude]:** um único nó "platform admin: full access", sem listar organizações e workspaces.
- **Muitas organizações e workspaces [decisão do Claude]:** acima de um limite de nós, a definir por quem implementar, as organizações entram recolhidas e se expandem com um clique.
- **Quando o servidor restringe o SpiceDB [decisão do Claude]:** o servidor nega algumas coisas que o SpiceDB concede, e o grafo mostra o efeito real, com o motivo:
  - **Conexão atribuída por um platform admin** (`origin: assigned` em `cube_app.connections`): o SpiceDB dá `connection#manage` ao admin da organização, mas `managed_connection` em `handlers/connections.rs` só deixa platform admins editarem ou excluírem a conexão. O grafo mostra `query`, se ela estiver liberada para algum workspace do usuário, e não mostra `manage`. No lugar, a conexão traz a nota "assigned by a platform admin: only platform admins edit it". No modelo, o `sofiadb · manage` do exemplo seria assim.
  - **Conexão sem organização:** só platform admins a consultam, então ela não aparece no grafo de quem não é platform admin.
  - **Liberação para um data source que não existe mais no `cube.yml`:** aparece com a nota "data source missing", sem `query`.
  - Por isso o grafo junta o SpiceDB com `cube_app.connections` e com os data sources configurados, e não lê só o SpiceDB.
- **Usuário desativado [decisão do Claude]:** o grafo mostra os acessos gravados com o aviso "disabled: these grants do not apply while the user is disabled".
- **Rota no servidor [decisão do Claude]:** o servidor monta o grafo inteiro numa rota nova, só para platform admins. Ela devolve os nós, as arestas e, para cada permissão, o motivo e o caminho. Assim o navegador não faz dezenas de chamadas. A forma de calcular, com `read` das relações do usuário mais as regras do esquema ou com `lookup_resources` por permissão, fica a critério de quem implementar, mas o resultado precisa bater com o que a API de fato permite (ver anti-objetivos).

## Deixado para quem implementar

- Os nomes e os formatos exatos das rotas e dos parâmetros: busca, paginação, a listagem de workspaces de todas as organizações e a rota do grafo.
- O índice no Postgres para a busca, se for necessário. O `pg_trgm` pode não estar instalado no container `cube-pg-test`. Na escala atual, alguns milhares de linhas, uma busca sem índice pode bastar. Quem implementar mede e decide.
- A biblioteca de desenho do grafo, ou SVG feito à mão, como no modelo. Uma biblioteca nova precisa de uma justificativa no PR.
- O limite de nós a partir do qual as organizações entram recolhidas.
- O desenho exato dos estados vazio, de carregamento e de erro.
- Como o seletor da barra superior lembra o workspace escolhido entre recargas, e o que acontece quando esse workspace não está na primeira página de resultados.
- Os testes:
  - testes de servidor para as rotas novas, contra o `FakeAuthz` e o Postgres de teste, como nos testes `rbac_api` atuais;
  - um teste que compare o grafo com o comportamento real em todos os papéis da tabela de regras: o `check` do SpiceDB mais as restrições do servidor para conexões atribuídas, sem organização ou sem data source;
  - testes `vitest` para a lógica pura do cliente.

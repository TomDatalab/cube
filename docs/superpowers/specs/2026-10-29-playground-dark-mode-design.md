# Dark mode do Playground — design

## Intenção

O usuário quer o **Playground inteiro** em modo escuro. Isso inclui:
- a barra superior e o login;
- Playground (construtor de consultas e resultados);
- Data Model;
- Data Source e o assistente de conexão;
- Frontend Integrations;
- Admin, com o grafo de acessos;
- a **aba Code** (vizard), que entra no escopo por decisão do usuário: "entra no dark mode".

O usuário escolheu a **paleta C**, "grafite com toque de roxo", entre três modelos (https://claude.ai/artifact/9mFdvfP8y9fz8myLz6s5KT). É um cinza-escuro levemente puxado para o roxo, neutro para ler tabelas e código por horas, sem perder a identidade da Cube.

## Objetivos

1. **Três modos de tema** (decisão do usuário: "os dois, botão na barra superior"):
   - **Sistema**, o padrão, segue `prefers-color-scheme` e acompanha a troca do sistema em tempo real, sem recarregar;
   - **Claro**;
   - **Escuro**.
2. **Um controle na barra superior**, visível em todas as páginas e também na tela de login, que alterna entre Sistema, Claro e Escuro.
3. **Todas as telas do Playground legíveis e coerentes no modo escuro.** Nenhum texto escuro sobre fundo escuro e nenhuma "ilha" branca, como cartões, tabelas, modais, menus suspensos, tooltips, popovers ou notificações.
4. **O modo claro continua idêntico ao de hoje.**

## Não-objetivos

- Redesenhar o Playground ou mudar o layout.
- Trocar ou atualizar o antd 4, o `@cube-dev/ui-kit` ou o styled-components.
- Temas além de claro e escuro.

## Anti-objetivos

- Um **flash de tela clara** ao abrir ou recarregar a página quando o tema escuro está ativo.
- Um modo claro que fique diferente do atual.
- Partes do Playground que só funcionam num dos temas: texto ilegível, ícones que somem, gráficos ou código com cores que não contrastam.

## Restrições

- **Frontend:** pacote `packages/cubejs-playground` (React 18, antd **4.16.13**, `@cube-dev/ui-kit` 0.52.3, styled-components 6, prismjs, recharts). Nenhuma dependência nova.
- **CSS do antd:** vem de um arquivo estático, o `public/antd.min.css`. É uma versão compilada com a cor primária roxa `#7a77ff`, e não a versão azul padrão do antd.
- **Cores do ui-kit:** o `@cube-dev/ui-kit` define as cores como variáveis CSS (`--dark-color`, `--white-color`, `--purple-color`, `--text-color`, `--light-color`, `--grey-light-color`, `--dark-02-color`…), a partir de `tokens.js`. As cores do construtor de consultas entram por `QUERY_BUILDER_COLOR_TOKENS`, no `ROOT_STYLES` do `<Root>` em `src/App.tsx`.
- **Cores fixas:** existem cerca de 61 cores fixas no código, espalhadas por 28 arquivos de `src/`, inclusive em `src/theme.ts`.
- **Verificações e entrega:**
  - `vitest`, o `oxlint` e o typecheck seguem a regra atual (nenhum erro novo em arquivo tocado);
  - commits na branch nova `feat/playground-dark-mode`, criada a partir da `main`, em conventional commits;
  - no fim, a mudança é implantada em `/root/cube-rust` e verificada no navegador, como nas entregas anteriores.

## Decisões

### Paleta C (decisão do usuário)

Os valores vêm do modelo aprovado:

| Papel | Escuro |
|---|---|
| Barra superior | `#17171f` |
| Fundo da página | `#1c1c24` |
| Painel / superfície (cartões, barras laterais, cabeçalho de tabela) | `#23232d` |
| Elevado (botões, nós do grafo, menus suspensos) | `#2b2b37` |
| Linhas e bordas | `#363644` |
| Texto | `#e7e7ef` |
| Texto secundário | `#9a9aaf` |
| Destaque (roxo da Cube) | `#8f8bff` |
| Destaque suave (seleção) | `rgba(143,139,255,.17)` |
| Sucesso | `#3fbf7f` |

**Cores semânticas [decisão do Claude]:** perigo, aviso e nota ficam nos tons do ui-kit, clareados só o necessário para ter contraste AA sobre `#1c1c24`.

### Como o tema é aplicado [decisão do Claude]

- **Atributo no `<html>`:** o tema efetivo é `data-theme="dark"` ou `"light"`. A preferência ("system", "light" ou "dark") fica guardada no `localStorage` do navegador. Se o armazenamento estiver bloqueado, o tema volta para "system" sem erro.
- **Sem flash:** um script pequeno e inline no `index.html`, executado antes de qualquer CSS, aplica o atributo e escolhe a folha de estilo do antd.
- **antd:** existem duas folhas de estilo, a clara (o `public/antd.min.css` atual) e uma **escura**. O script e o controle de tema trocam qual delas está ativa.
  - A escura é a `antd.dark` do antd 4, compilada com **as mesmas customizações da clara**: `variables.js`/`variables-esm.js` do pacote (raios, fonte Inter, `primary-1..6` e os tons de cor), remapeadas para a paleta C.
  - Os valores da paleta entram no compile como variáveis do antd: `body-background`, `component-background`, `border-color-base`, `text-color`, `text-color-secondary`, `primary-color` e as afins.
  - Assim as duas folhas só diferem nas cores. [decisão do Claude]
- **ui-kit e cores do construtor de consultas:** as variáveis `--*-color` **e as `--*-color-rgb`** são redefinidas sob `[data-theme="dark"]`, com os valores da paleta. As `-rgb` são os trios de que o ui-kit deriva as transparências de bordas e hovers. Todas existem nos dois temas, e só o tema escuro as sobrescreve.
- **Cores dos tipos de membro** no construtor de consultas (`QUERY_BUILDER_COLOR_TOKENS`: time dimension, measure, dimension, segment, filter, cada uma com tons pastel e fortes) [decisão do Claude]: cada tipo mantém o seu matiz. No escuro, os pastéis viram tons escuros e pouco saturados do mesmo matiz, para fundos de chip, e os fortes viram tons claros do mesmo matiz, para texto e ícones. A codificação por cor continua reconhecível.
- **CSS avulso** [decisão do Claude]: entram no escopo `@ant-design/compatible/assets/index.css`, `src/playground/index.less` e o `<style>` inline do `index.html`, que tem `#a9a9b6` fixo. Recebem sobrescritas sob `[data-theme="dark"]` ou passam a usar tokens.
- **Cores fixas no código:** cada uma passa a vir de um token, ou ganha a variante escura. Isso cobre `src/theme.ts`, os styled-components e os estilos inline, inclusive o grafo de acessos (`AccessModal.tsx`), que hoje usa só cores claras.
- **Aba Code (vizard)** (decisão do usuário: entra no dark mode):
  - **O que é:** um app à parte em `packages/cubejs-playground/vizard/`, com `@cube-dev/ui-kit` 0.33 e o editor Monaco. O Playground o abre em iframe (`/vizard/index.html#<config>`, em `src/components/Vizard/Vizard.tsx`).
  - **Na mesma origem:** pode ler a mesma preferência do `localStorage` e o `prefers-color-scheme`. Também acompanha as trocas feitas no Playground, pelo evento `storage` ou por `postMessage` do Playground (a escolha é de quem implementar).
  - **No escuro:** aplica a paleta C às variáveis do ui-kit dele e usa o tema `vs-dark` do Monaco, ajustado ao fundo da paleta.
  - **Sem flash:** o mesmo cuidado vale para o vizard.
  - **Build:** o build do vizard (`yarn build` em `vizard/`) usa as dependências já instaladas em `/root/cube/packages/cubejs-playground/vizard/node_modules`. O worktree de build não tem essas dependências. O resultado é instalado com `VIZARD_SOURCE=<build do vizard> ./install-playground.sh`.
- **Código (Prism):** um tema escuro para os blocos de código, aplicado quando o tema é escuro.
- **Gráficos (recharts) no construtor de consultas:** eixos, grade, legenda e tooltip usam cores do tema.

### Onde vale [decisão do Claude]

O dark mode vale para o **Playground como aplicação** (o `index.html` servido por `/root/cube-rust` ou pelo modo dev). O pacote também publica uma `lib/`, e o módulo de tema fica dentro de `src/`, com uma função de inicialização exportada. Mas fazer hosts externos que embutem os componentes funcionarem no escuro está fora do escopo: nesta instalação, ninguém os embute.

### O controle [decisão do Claude]

- **Preferência por navegador:** uma única chave no `localStorage`, que vale para qualquer usuário daquele navegador. Ela não depende da sessão do RBAC, e o controle funciona antes do login.
- **Na tela de login:** o mesmo controle aparece no topo da tela de login, como componente independente do contexto do app.

- **Lugar:** um seletor compacto na barra superior, à esquerda do seletor de workspace (ou no lugar dele, quando o RBAC está desligado).
- **Forma:** um ícone (sol, lua ou monitor) com um menu de três itens: "System", "Light" e "Dark". Os textos ficam em inglês, como o resto da interface.

## Deixado para quem implementar

- Como gerar o CSS escuro do antd com o roxo:
  - compilar `antd/dist/antd.dark.less` uma vez, com `modifyVars` da cor primária, e versionar o resultado em `public/`;
  - ou outra forma que dê o mesmo resultado sem dependência nova. O `less` já está instalado.
  - Documentar o comando usado num comentário ou no README do pacote.
- O mapeamento exato de cada `--*-color` do ui-kit para a paleta, conferido tela a tela.
- O desenho exato do ícone e do menu do controle.
- Testes:
  - `vitest` para a lógica pura (resolver o tema efetivo a partir da preferência e do sistema; ler e gravar a preferência com o armazenamento bloqueado);
  - verificação visual de cada página nos dois temas no navegador.

# Dark mode do Playground — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** o Playground inteiro, incluindo a aba Code (vizard), ganha um modo escuro com a paleta C, escolhido por um controle System/Light/Dark na barra superior e na tela de login, sem flash de tela clara e sem mudar o modo claro.

**Architecture:**
- **O tema efetivo** fica em `data-theme` no `<html>`, a partir da preferência guardada no `localStorage` (ou do `prefers-color-scheme`). Um script inline no `<head>` aplica o tema antes de qualquer CSS.
- **antd 4:** troca entre duas folhas estáticas. A clara é a atual. A escura é gerada uma vez a partir de `antd.dark.less`, com as mesmas customizações da clara remapeadas para a paleta.
- **ui-kit, tokens do construtor de consultas e cores fixas:** passam a ler variáveis CSS, que um bloco `[data-theme="dark"]` redefine.
- **Vizard:** lê a mesma preferência, por estar na mesma origem, e acompanha as trocas pelo evento `storage`.

**Tech Stack:** React 18, antd 4.16.13 (CSS estático), `@cube-dev/ui-kit` 0.52.3 (Playground) e 0.33 (vizard), styled-components 6, prismjs, recharts, Monaco (vizard), less 4 (já instalado), vitest.

**Spec:** `docs/superpowers/specs/2026-10-29-playground-dark-mode-design.md`

## Global Constraints

- **Paleta C, escuro:**

  | Papel | Valor |
  |---|---|
  | barra superior | `#17171f` |
  | fundo | `#1c1c24` |
  | painel | `#23232d` |
  | elevado | `#2b2b37` |
  | linhas e bordas | `#363644` |
  | texto | `#e7e7ef` |
  | texto secundário | `#9a9aaf` |
  | destaque | `#8f8bff` |
  | destaque suave | `rgba(143,139,255,.17)` |
  | sucesso | `#3fbf7f` |

  Perigo, aviso e nota ficam nos tons do ui-kit, clareados até contraste AA sobre `#1c1c24`.
- **Preferências:** `'system' | 'light' | 'dark'`. O padrão é `'system'`.
  - A chave no `localStorage` é `cube-playground-theme`, uma por navegador.
  - Armazenamento bloqueado conta como `'system'`, sem erro.
- **O tema efetivo** é aplicado como `document.documentElement.dataset.theme = 'light' | 'dark'`.
  - Com `'system'`, ele acompanha `prefers-color-scheme` em tempo real, sem recarregar.
- **Controle:** textos dos itens "System", "Light" e "Dark", em inglês.
  - Fica na barra superior, à esquerda do seletor de workspace (ou no lugar dele quando o RBAC está desligado), e também no topo da tela de login.
- **Modo claro:** continua visualmente idêntico ao de hoje.
  - `public/antd.min.css` não muda.
  - Toda variável e todo token mantém o valor atual fora de `[data-theme="dark"]`.
- **Sem flash:** o script de bootstrap fica no `<head>` antes de qualquer `<link rel="stylesheet">`, no `index.html` do Playground e no do vizard.
- **Dependências:** nenhuma nova. Só versão compilada da folha escura entra no repositório (`public/antd.dark.min.css`), com o comando de geração documentado.
- **Verificações e entrega:**
  - `vitest run` verde;
  - `oxlint` com saída 0 nos arquivos tocados;
  - nenhum erro de `tsc` que mencione um arquivo tocado (a base tem cerca de 130 erros TS2883 de ambiente, que não são nossos);
  - branch `feat/playground-dark-mode`, com conventional commits e `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`;
  - autor `Tom DataLab <sofiadb@sofiadb.org>` e `git -c safe.directory='*'`.

## Review Focus

1. **Armazenamento bloqueado** (`localStorage` lança exceção): o tema resolve para o do sistema e nada quebra. Teste na Task 1.
2. **O sistema troca de claro para escuro com a janela aberta e a preferência em `'system'`:** o Playground troca na hora. Com `'light'` ou `'dark'`, ignora a troca. Teste na Task 1.
3. **Recarregar com o escuro ativo:** nenhum quadro claro. O script vem antes das folhas de estilo, e a folha clara já começa desativada. Teste na Task 2, que lê o `index.html`.
4. **Uma cor fixa nova esquecida num componente:** deixaria uma "ilha" clara. O teste de guarda da Task 4 falha se aparecer uma cor literal fora dos arquivos de tema.
5. **Trocar o tema com um modal, menu suspenso ou popover aberto** (o antd os monta direto no `body`): eles também trocam. Verificação no navegador na Task 7.

---

### Task 1: Módulo de tema

**Files:**
- Create: `packages/cubejs-playground/src/shared/theme.ts`
- Create: `packages/cubejs-playground/src/shared/useTheme.ts`
- Test: `packages/cubejs-playground/src/__tests__/theme.test.ts`

**Interfaces:**
- Produces, em `theme.ts`:
  - `export type ThemePreference = 'system' | 'light' | 'dark'`;
  - `export type Theme = 'light' | 'dark'`;
  - `export const THEME_STORAGE_KEY = 'cube-playground-theme'`;
  - `export function resolveTheme(pref: ThemePreference, systemDark: boolean): Theme`;
  - `export function readPreference(storage?: Pick<Storage, 'getItem'>): ThemePreference`: um valor inválido, ausente ou uma exceção dá `'system'`;
  - `export function writePreference(pref: ThemePreference, storage?: Pick<Storage, 'setItem'>): void`: engole exceções;
  - `export function applyTheme(theme: Theme, doc: Document = document): void`:
    - grava `doc.documentElement.dataset.theme`;
    - ativa `link#antd-light` ou `link#antd-dark` pela propriedade `disabled`;
    - dispara `window.dispatchEvent(new CustomEvent('cube-theme', { detail: theme }))`.
- Produces, em `useTheme.ts`: `export function useTheme(): { preference: ThemePreference; theme: Theme; setPreference(p: ThemePreference): void }`.
  - Assina `matchMedia('(prefers-color-scheme: dark)')` (o `change`) e o evento `storage` da chave, para acompanhar outras abas.
  - Aplica `applyTheme` a cada mudança.

- [ ] **Step 1: Write the failing tests** em `theme.test.ts` (jsdom):
  - `resolveTheme('system', true) === 'dark'`, `resolveTheme('system', false) === 'light'`, e `'light'`/`'dark'` ignoram o sistema;
  - `readPreference` devolve `'system'` para `null`, para `'blue'` e quando `getItem` lança exceção. Devolve `'dark'` para `'dark'`;
  - `writePreference('dark', { setItem: () => { throw new Error() } })` não lança exceção;
  - `applyTheme('dark')` com `<link id="antd-light">` e `<link id="antd-dark">` no documento deixa `dataset.theme === 'dark'`, `antd-dark.disabled === false` e `antd-light.disabled === true`;
  - `useTheme`, renderizado com `react-dom` + `act`, no padrão de `src/__tests__/use-search.test.ts`:
    - com a preferência `'system'` e o `matchMedia` simulado mudando de `false` para `true`, `theme` passa a `'dark'` e `dataset.theme` também;
    - com `setPreference('light')`, um novo `change` do sistema não altera `theme`.
- [ ] **Step 2: Run them to see them fail.** `cd packages/cubejs-playground && node_modules/.bin/vitest run src/__tests__/theme.test.ts`. Expected: FAIL (módulo inexistente).
- [ ] **Step 3: Implement** `theme.ts` e `useTheme.ts`.
- [ ] **Step 4: Run them to see them pass.**
- [ ] **Step 5: Commit** `feat(playground): theme preference and resolution`

### Task 2: Folha escura do antd e bootstrap sem flash

**Files:**
- Create: `packages/cubejs-playground/variables-dark.js`: as mesmas chaves de `variables.js`, com os valores da paleta C. Inclui também `body-background`, `component-background`, `popover-background`, `border-color-base`, `border-color-split`, `text-color`, `text-color-secondary`, `heading-color`, `primary-color` (`#8f8bff`) e `layout-header-background` (`#17171f`).
- Create: `packages/cubejs-playground/scripts/build-antd-dark.mjs`: compila `antd/dist/antd.dark.less` com `less.render(..., { javascriptEnabled: true, compress: true, modifyVars })`, usando as variáveis de `variables-dark.js` sem o prefixo `@`. Grava `public/antd.dark.min.css`.
- Create: `packages/cubejs-playground/public/antd.dark.min.css`, gerado pelo script.
- Modify: `packages/cubejs-playground/package.json`: script `"build:antd-dark": "node scripts/build-antd-dark.mjs"`.
- Modify: `packages/cubejs-playground/index.html`:
  - `<link id="antd-light" rel="stylesheet" href="/antd.min.css">` e `<link id="antd-dark" rel="stylesheet" href="/antd.dark.min.css" disabled>`;
  - antes deles, um `<script>` inline que faz o mesmo que `readPreference`, `resolveTheme` e `applyTheme` (sem eventos) e nunca lança exceção;
  - o `#a9a9b6` do `<style>` inline passa a `var(--playground-muted, #a9a9b6)`.
- Test: `packages/cubejs-playground/src/__tests__/index-html.test.ts`

**Interfaces:**
- Consumes: `THEME_STORAGE_KEY` (Task 1). O valor é repetido literalmente no script inline, e o teste confere que é igual.
- Produces: os ids `antd-light` e `antd-dark` usados por `applyTheme`.

- [ ] **Step 1: Write the failing test** `index-html.test.ts`, que lê `index.html` como texto:
  - o primeiro `<script` do `<head>` aparece antes do primeiro `rel="stylesheet"`;
  - o script contém `'cube-playground-theme'`, igual a `THEME_STORAGE_KEY`;
  - existem `id="antd-light"` e `id="antd-dark"`, e o escuro começa com `disabled`.
  - Um segundo teste executa o script inline num documento jsdom, com o `localStorage` simulado em `'dark'`, e espera `dataset.theme === 'dark'`, a folha escura ativa e a clara desativada.
  - Um terceiro executa o script com `localStorage` lançando exceção e `matchMedia` escuro, e espera `'dark'`, sem exceção.
- [ ] **Step 2: Run it to see it fail.**
- [ ] **Step 3: Implement**
  - Gere a folha com `yarn build:antd-dark`.
  - Confira:
    - `grep -o '\.ant-btn-primary{[^}]*' public/antd.dark.min.css` contém `#8f8bff`;
    - a folha contém `#1c1c24`;
    - `public/antd.min.css` não mudou (`git diff --stat` não o lista).
- [ ] **Step 4: Run it to see it pass, then run `vitest run` inteiro.**
- [ ] **Step 5: Commit** `feat(playground): dark antd stylesheet and flash-free theme bootstrap`

### Task 3: Tokens escuros (ui-kit, construtor de consultas e CSS avulso)

**Files:**
- Create: `packages/cubejs-playground/src/theme/dark-tokens.ts`:
  - `export const UI_KIT_DARK: Record<string, string>`: para cada cor de `@cube-dev/ui-kit/es/tokens.js`, o valor escuro de `--<name>-color` e de `--<name>-color-rgb`, como trio `"r, g, b"`;
  - `export const DARK_CSS: string`: o bloco `:root[data-theme="dark"] { ... }` gerado a partir de `UI_KIT_DARK`, mais as sobrescritas de CSS avulso.
- Create: `packages/cubejs-playground/src/theme/ThemeStyles.tsx`: `createGlobalStyle` do styled-components com `DARK_CSS`, montado em `App.tsx` junto do `<GlobalStyles />`.
- Modify: `packages/cubejs-playground/src/QueryBuilderV2/color-tokens.ts`. Para cada tipo de membro (time dimension, measure, dimension, segment, filter):
  - os tokens passam a ser `var(--qb-<tipo>-<n>, <valor claro atual>)`;
  - `DARK_CSS` define `--qb-*`: os pastéis viram tons escuros e pouco saturados do mesmo matiz, e os fortes viram tons claros do mesmo matiz.
- Modify:
  - `src/playground/index.less` e as regras de `@ant-design/compatible/assets/index.css` usadas pelo app: sobrescritas em `DARK_CSS`, com seletores `[data-theme="dark"] .ant-...` para as classes compat que tenham fundo ou texto claros;
  - `--playground-muted`, definida nos dois temas.
- Test: `packages/cubejs-playground/src/__tests__/dark-tokens.test.ts`

**Interfaces:**
- Consumes: o atributo `data-theme` (Tasks 1 e 2).
- Produces: as variáveis `--<name>-color`, `--<name>-color-rgb`, `--qb-*` e `--playground-muted`. A Task 4 usa essas variáveis e as novas `--pg-bar`, `--pg-bg`, `--pg-surface`, `--pg-raised`, `--pg-line`, `--pg-text`, `--pg-muted`, `--pg-accent`, `--pg-accent-soft` e `--pg-success`.
  - `--pg-*` são definidas aqui nos dois temas. Os valores claros são os atuais, e os escuros são a paleta C.

- [ ] **Step 1: Write the failing tests** `dark-tokens.test.ts`:
  - toda cor exportada por `@cube-dev/ui-kit` (`tokens.js`) tem `--<name>-color` e `--<name>-color-rgb` em `UI_KIT_DARK`;
  - `DARK_CSS` contém `--pg-bg: #1c1c24`, `--pg-accent: #8f8bff` e `--pg-text: #e7e7ef`;
  - todo `--qb-*` usado em `color-tokens.ts` está definido em `DARK_CSS`;
  - contraste: `--pg-text`, `--pg-muted` e as cores de perigo, aviso e nota sobre `#1c1c24` dão razão de no mínimo 4.5, pela fórmula WCAG, implementada no teste.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them to see them pass; `vitest run` inteiro.**
- [ ] **Step 5: Commit** `feat(playground): dark tokens for the ui-kit, query builder and legacy CSS`

### Task 4: Cores fixas do código passam a usar tokens

**Files:**
- Modify:
  - `packages/cubejs-playground/src/theme.ts`: `colors.*` vira `var(--pg-*, <valor atual>)`;
  - os cerca de 28 arquivos de `src/` com cores literais (hex, `rgb()`, `rgba()`), cada uma trocada pelo token equivalente da Task 3;
  - inclui `src/pages/Admin/access/AccessModal.tsx` (cores do grafo: tipos de nó, destaque `#7a77ff` → `var(--pg-accent)`, fundo, linhas e textos);
  - inclui os gráficos (`src/QueryBuilderV2/components/ChartRenderer.tsx` e os componentes recharts do `PlaygroundQueryBuilder`): eixos, grade, legenda e tooltip passam a usar `var(--pg-text)`, `var(--pg-muted)`, `var(--pg-line)` e `var(--pg-raised)`.
- Modify: `packages/cubejs-playground/src/PrismCode.tsx` (e onde o CSS do Prism é carregado): um tema escuro do Prism sob `[data-theme="dark"]`, escrito em `DARK_CSS` com os tokens.
- Test: `packages/cubejs-playground/src/__tests__/no-hardcoded-colors.test.ts`

**Interfaces:**
- Consumes: as variáveis da Task 3.

- [ ] **Step 1: Write the failing test** `no-hardcoded-colors.test.ts`:
  - percorre `src/**/*.{ts,tsx,less,css}`, menos `src/__tests__`, `src/theme/`, `src/QueryBuilderV2/color-tokens.ts` e `src/theme.ts`;
  - falha listando arquivo:linha de cada literal `#rgb`, `#rrggbb` ou `rgba?(` que não esteja dentro de um `var(--…, <fallback>)`;
  - uma lista de exceções no topo do teste admite só casos justificados, com comentário, por exemplo um SVG de logo de marca.
- [ ] **Step 2: Run it to see it fail** (lista as ~61 ocorrências).
- [ ] **Step 3: Implement** a troca.
- [ ] **Step 4: Run it to see it pass; `vitest run`, `oxlint` e o gate do `tsc`.**
- [ ] **Step 5: Commit** `refactor(playground): colors come from theme tokens`

### Task 5: Controle de tema na barra e no login

**Files:**
- Create: `packages/cubejs-playground/src/components/ThemeSwitcher.tsx`
- Modify:
  - `packages/cubejs-playground/src/components/Header/Header.tsx`: o `ThemeSwitcher` fica à esquerda do `WorkspaceSelector`, e também aparece quando o RBAC está desligado;
  - `packages/cubejs-playground/src/pages/Login/LoginPage.tsx`: o `ThemeSwitcher` fica no topo da página, fora do card;
  - `packages/cubejs-playground/src/App.tsx`: monta `ThemeStyles` (Task 3).
- Test: `packages/cubejs-playground/src/__tests__/theme-switcher.test.ts`

**Interfaces:**
- Consumes: `useTheme()` (Task 1).
- Produces: `export function ThemeSwitcher(): JSX.Element`. É um `Dropdown` do antd com um botão de ícone (sol para `light`, lua para `dark`, monitor para `system`, de `@ant-design/icons`) e um menu com os itens "System", "Light" e "Dark", com o atual marcado.
  - Tem `data-testid="theme-switcher"` e `aria-label="Theme"`.
  - Não depende do contexto do app, para funcionar no login.

- [ ] **Step 1: Write the failing test** `theme-switcher.test.ts` (react-dom + `act`): renderiza `ThemeSwitcher`, abre o menu, clica em "Dark" e espera `localStorage['cube-playground-theme'] === 'dark'` e `document.documentElement.dataset.theme === 'dark'`. O mesmo para "System" com `matchMedia` claro, esperando `'light'`.
- [ ] **Step 2: Run it to see it fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run it to see it pass; `vitest run`, `oxlint` e o gate do `tsc`.**
- [ ] **Step 5: Commit** `feat(playground): theme switcher in the header and on the login page`

### Task 6: Aba Code (vizard) no tema

**Files:**
- Create: `packages/cubejs-playground/vizard/src/theme.ts`. É uma cópia mínima de `resolveTheme`, `readPreference`, `THEME_STORAGE_KEY` e `applyTheme`, sem os links do antd, porque o vizard não usa o antd. Fica com um comentário apontando para `src/shared/theme.ts`, que é a fonte.
  - Mais `export function watchTheme(onChange: (t: Theme) => void): () => void`, que assina o evento `storage` da chave e o `change` do `prefers-color-scheme`.
- Modify:
  - `packages/cubejs-playground/vizard/index.html`: o mesmo script de bootstrap no `<head>`, antes de qualquer CSS;
  - `packages/cubejs-playground/vizard/src/Vizard.tsx`: aplica o tema, e sob `[data-theme="dark"]` redefine as variáveis do ui-kit 0.33 com a paleta C (um `<style>` ou `styles` no `<Root>`);
  - `packages/cubejs-playground/vizard/src/monaco/config.ts`: um segundo tema `editor.defineTheme('cube-dark', { base: 'vs-dark', ... })` com `editor.background` `#23232d`, linhas `#363644` e cursor `#8f8bff`;
  - `vizard/src/Editor.tsx` usa `theme={theme === 'dark' ? 'cube-dark' : 'cube'}`.
- Build: `cd packages/cubejs-playground/vizard`.
  - O worktree não tem as dependências do vizard. Use as de `/root/cube/packages/cubejs-playground/vizard/node_modules`: crie um symlink `node_modules` → esse diretório, só durante o build, e remova depois, sem versioná-lo.
  - Rode `node_modules/.bin/tsc && node_modules/.bin/vite build`, ou o passo equivalente de `yarn build` sem `yarn prepare` se ele exigir rede.

**Interfaces:**
- Consumes: a chave `cube-playground-theme` e os valores de preferência (Task 1).

- [ ] **Step 1: Implement**
  - O vizard não tem configuração de testes, então a lógica pura fica na cópia mínima.
  - O teste dessa lógica é o da Task 1, sobre a fonte. Confira a cópia por leitura lado a lado.
- [ ] **Step 2: Build the vizard** e confira que `vizard/build/index.html` tem o script antes do CSS.
- [ ] **Step 3: Commit** `feat(playground): dark mode in the Code tab`, sem o `build/` do vizard, que já não é versionado.

### Task 7: Implantação e verificação no navegador

**Files:** nenhum no repositório. As capturas de tela vão para o scratchpad.

- [ ] **Step 1: Build**
  - `node_modules/.bin/vite build` do Playground no worktree `/home/antonio-rodrigues/Documents/GitHub/cube-rbac-build`, em `git checkout --detach <HEAD da branch>`;
  - o build do vizard (Task 6).
- [ ] **Step 2: Instalação**
  - `cd /root/cube-rust && VIZARD_SOURCE=<worktree>/packages/cubejs-playground/vizard/build ./install-playground.sh <worktree>/packages/cubejs-playground/build`;
  - depois `./start-all.sh status`. Expected: tudo ✓.
  - Não é preciso reiniciar o servidor, porque só os arquivos estáticos mudam.
  - Confira que `/root/cube-rust/playground/antd.dark.min.css` existe.
- [ ] **Step 3: Navegador** (`playwright-cli`, 1440×900). Capture claro e escuro de:
  - login;
  - Playground com uma consulta e o gráfico;
  - Data Model com código;
  - Data Source e o assistente de conexão;
  - Frontend Integrations;
  - aba Code;
  - Admin (as três abas e o grafo Access).

  Confira:
  - **no escuro:** nenhuma ilha branca, nenhum texto ilegível, e modais, menus suspensos e tooltips escuros (Review Focus 5);
  - **no claro:** igual ao de antes, comparando com capturas tiradas antes de instalar;
  - recarregar no escuro não mostra flash, numa gravação ou captura logo após o `goto`;
  - trocar o tema com a aba Code aberta muda também o editor, sem recarregar.
- [ ] **Step 4: Push** de `feat/playground-dark-mode` e abertura do PR contra a `main`.

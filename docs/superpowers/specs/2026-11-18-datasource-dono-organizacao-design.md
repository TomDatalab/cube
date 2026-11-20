# Datasource sempre com dono (organização) — design curto

Data: 2026-11-18

## Intenção

Todo datasource novo nasce com uma organização dona. Hoje o platform admin cria
o datasource na tela Data Source (ele nasce sem dono) e só depois o atribui em
Admin → Organizations; esquecer o segundo passo deixa o datasource solto, e as
organizações não veem os cubes dele.

## Regras (decididas com o usuário)

- **Criação:** escolher a organização dona é obrigatório. Não existe a opção
  "Platform" na criação.
- **Platform:** é só o rótulo dos datasources que já existem sem dono
  (`default`, `sofiadb`, `example`). Nenhuma migração: continuam sem linha em
  `connections`, aparecem como "Platform" e só platform admins os usam.
- **`default`:** é sempre Platform; não pode ser atribuído nem movido (regra
  atual mantida).
- **Nunca volta a ficar sem dono:** "Unassign" deixa de existir e vira
  "Move to…" (outra organização). Platform → organização é permitido
  (equivale à atribuição de hoje); organização → Platform não.

## Abordagens

1. **Platform implícito (escolhida):** "sem linha em `connections`" = Platform.
   Schema e SpiceDB não mudam. Custo: Platform não é um registro.
2. Platform explícito (organização especial ou coluna `owner`): migração e uma
   exceção em todo o RBAC recém-revisado, sem ganho visível. Descartada.

## Design

**Servidor (cubeserver, com RBAC ligado):**
- `POST /v1/data-sources` e `POST /playground/data-sources` exigem
  `organizationId`; sem ele, 400. Com ele, criam o datasource e a atribuição
  (`connections` com `origin: assigned` + `connection#organization`) numa
  operação só, com desfazer em caso de falha, no mesmo molde de
  `connections::create`. Organização inexistente: 400/404 conforme o handler
  de atribuição atual.
- Novo `PUT /v1/connections/{name}/organization` aceita trocar de organização
  quando a conexão já tem dona ("move"): numa transação, retira as liberações
  da organização antiga (`connection#workspace`), troca a
  `connection#organization` e atualiza a linha. Hoje esse caso responde 409.
- `DELETE /v1/connections/{name}/organization` (unassign) é removido; 405/404.
- `GET /v1/data-sources` passa a informar o dono de cada datasource
  (`organizationId` ou `null` = Platform), para a tela não precisar cruzar com
  `/v1/connections`.
- Com RBAC desligado, nada muda (não existem organizações).
- `POST /v1/organizations/{id}/connections` (criadas pela organização) já tem
  dono; não muda.

**Playground:**
- Data Source → New data source: campo obrigatório **Organization** (select das
  organizações, sem valor pré-selecionado). Na edição, o campo não aparece.
- Tabela de Data Source: coluna **Owner** (nome da organização ou tag
  "Platform") e filtro por ela.
- Admin → Organizations → Connections: "Unassign" vira **"Move to…"** (select
  das outras organizações); "Assign" continua oferecendo os datasources
  Platform, exceto `default`.

**Docs:** README do deploy em `/root/cube-rust` com a nova regra e rotas.

## Fica para o implementador

Mensagens de erro exatas; layout do select "Move to…"; se o filtro por Owner é
um filtro de coluna do antd ou um select acima da tabela.

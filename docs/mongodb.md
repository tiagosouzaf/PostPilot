# MongoDB no PostPilot

O PostPilot conecta em MongoDB (4.0 em diante, e Atlas) pelo driver
oficial. As janelas de consulta rodam comandos no estilo do mongosh e os
documentos aparecem numa grade; o Pesquisador de Objetos mostra bancos,
coleções e índices.

## Conectar

Em **Conectar ao Servidor**, escolha **MongoDB** em "Tipo de servidor".

| Campo | O que vai |
| --- | --- |
| Nome do servidor | `host` ou `host,porta` (27017 se faltar). Para um cluster do Atlas, `mongodb+srv://cluster0.abc.mongodb.net`. |
| Login e senha | O usuário do banco. Vazio num servidor sem autenticação. O usuário é conferido no banco `admin`. |
| Banco de dados | O banco em que as janelas abrem (`test` se faltar). |
| Criptografia | Sem criptografia, TLS confiando no certificado, ou TLS conferindo o certificado. Num cluster `mongodb+srv` é sempre TLS. |

Colar uma connection string (`mongodb://usuario:senha@host:27017/banco` ou
`mongodb+srv://...`) no nome do servidor preenche os campos.

A conexão pode ser salva no workspace e nos Servidores Registrados, com
pasta e ambiente, como as outras. A senha vai para o cofre, nunca para um
arquivo em texto puro.

A conexão fala com o servidor informado (um nó). Replica sets e clusters
com vários hosts na connection string ainda não entram, a não ser pelo
`mongodb+srv`. Usuários criados fora do banco `admin`, X.509, LDAP e
Kerberos também não.

## Janela de consulta

Os comandos são os do mongosh, um por linha (ou separados por `;`; uma
chamada pode continuar em várias linhas):

```js
use loja
show collections

db.pedidos.find({ total: { $gte: 100 }, "cliente.cidade": "Recife" })
  .sort({ data: -1 })
  .limit(50)

db.pedidos.aggregate([
  { $match: { pago: true } },
  { $group: { _id: "$cliente", total: { $sum: "$total" } } },
])

db.pedidos.updateMany({ pago: false }, { $set: { atrasado: true } })
```

Não é JavaScript: só chamadas com valores literais (sem variáveis, laços
ou funções). Os valores aceitam chaves sem aspas, aspas simples, vírgula
sobrando, `ObjectId("...")`, `ISODate("...")`, `NumberLong(...)`,
`NumberDecimal("...")`, `UUID("...")`, `/regex/i` e comentários `//` e
`/* */`.

| Onde | O que existe |
| --- | --- |
| Leitura | `find` (com `.sort`, `.limit`, `.skip`, `.projection`, `.hint`, `.count`, `.explain`), `findOne`, `countDocuments`, `estimatedDocumentCount`, `distinct`, `aggregate`, `getIndexes`, `stats` |
| Escrita | `insertOne`, `insertMany`, `updateOne`, `updateMany`, `replaceOne`, `deleteOne`, `deleteMany`, `createIndex`, `dropIndex`, `dropIndexes`, `drop` |
| Banco | `use`, `show dbs`, `show collections`, `db.createCollection`, `db.dropDatabase`, `db.stats`, `db.serverStatus`, `db.runCommand({...})`, `db.adminCommand({...})` |

O que não está na lista roda por `db.runCommand`, que aceita qualquer
comando do servidor.

- Os documentos de um `find` ou `aggregate` viram linhas: cada campo do
  primeiro nível é uma coluna. Documentos e listas de dentro aparecem como
  JSON na célula (o visualizador de JSON abre). As colunas saem dos
  primeiros 1.000 documentos; se um campo só aparece depois, uma mensagem
  avisa.
- Escritas respondem em Mensagens: "2 documento(s) inserido(s)".
- `updateOne`/`updateMany` sem operador (`$set`...) e `deleteMany()` sem
  filtro são erro, para ninguém trocar um documento inteiro ou apagar uma
  coleção por engano. Para apagar tudo, escreva `deleteMany({})`.
- Um erro para a execução e aponta a linha do comando.
- O seletor de bancos da barra (Ctrl+U) troca o banco da janela, como o
  `use`.
- Num servidor de um ambiente que pede confirmação (Produção), um texto com
  qualquer comando que escreve pede o "Execute" antes de rodar.

## Sugestões no editor

A lista abre enquanto você escreve (ou com Ctrl+Espaço):

- depois de `db.`, as coleções e views do banco e os métodos do `db`
  (uma coleção com nome fora do padrão entra como `getCollection("...")`);
- depois da coleção, os métodos (`find`, `aggregate`, `updateMany`...),
  com os argumentos ao lado; depois de um `find(...)`, `sort`, `limit`,
  `skip` e os outros;
- ao digitar `$`, os operadores que cabem ali: de filtro num `find`, de
  atualização no segundo argumento de um `updateOne`/`updateMany`,
  estágios e expressões num `aggregate`;
- onde vai um campo, os campos da coleção com o tipo, inclusive os de
  dentro (`"cliente.cidade"`), e dentro de `"$..."` num `aggregate`;
- `ObjectId`, `ISODate`, `NumberLong`, `NumberDecimal`, `true`, `false` e
  `null` onde vai um valor.

O MongoDB não tem schema: os campos vêm dos primeiros 50 documentos de
cada coleção, lidos uma vez por banco. Ctrl+Shift+R lê de novo.

## Pesquisador de Objetos

Servidor, bancos, coleções e views de cada banco, e os índices de cada
coleção (com as chaves). No menu de uma coleção:

- **Abrir os Primeiros 1000 Documentos** roda o `find` numa janela nova.
- **Gerar Script de CREATE da Coleção** escreve o `createCollection` e os
  `createIndex`.
- **Excluir Coleção...** (e **Excluir Índice...** num índice) abre o comando
  numa janela para você conferir e rodar; nada é apagado pelo menu.
- **Propriedades** (F4): documentos, tamanhos e índices.

## O que não existe para o MongoDB

Table Designer, diagramas, Schema Compare, Data Compare, Query Designer,
edição na grade, importação de CSV, plano de execução gráfico, Activity
Monitor, backup e as ferramentas do Claude avisam na status bar que não
estão disponíveis para o MongoDB.

## Testes

```sh
docker run -d -p 27117:27017 -e MONGO_INITDB_ROOT_USERNAME=root \
    -e MONGO_INITDB_ROOT_PASSWORD='PostPilot!2026' mongo:7
MONGODB_TEST_HOST=127.0.0.1 MONGODB_TEST_PORT=27117 MONGODB_TEST_USER=root \
    MONGODB_TEST_PASSWORD='PostPilot!2026' \
    cargo test -p pgstudio-mongodb -p pgstudio-ui-shell --test server --test mongodb
```

Sem `MONGODB_TEST_HOST` esses testes só avisam e passam.

# Consulta entre servidores

Um `SELECT` que junta dados de servidores diferentes: PostgreSQL com SQL
Server, SQL com MongoDB, Elasticsearch ou Redis, tudo na mesma consulta e
com o resultado na grade de sempre.

## Como usar

1. Guarde as conexões (no workspace ou nos Servidores Registrados), com a
   senha lembrada. O nome da conexão é o nome do servidor na consulta.
2. **Consulta > Consulta entre Servidores** (ou o botão na barra). A janela
   vira "entre servidores"; se estava vazia, entra um exemplo com os seus
   servidores. O mesmo comando desliga.
3. Escreva o `SELECT` e aperte F5. Não precisa estar conectado no
   Pesquisador de Objetos.

```sql
SELECT c.nome, sum(p.total) AS total
FROM "PG Vendas".loja.public.clientes c
JOIN financeiro.erp.dbo.pedidos p ON p.cliente_id = c.id
GROUP BY c.nome
ORDER BY total DESC;
```

## As fontes

Uma fonte entra onde o SQL aceita uma tabela (depois de `FROM` ou `JOIN`).

| O que | Como escrever |
| --- | --- |
| Tabela do PostgreSQL ou SQL Server | `servidor.banco.schema.tabela`, `servidor.schema.tabela` ou `servidor.tabela` |
| Coleção do MongoDB | `servidor.banco.coleção` ou `servidor.coleção` |
| Índice do Elasticsearch | `servidor.índice` (até 10.000 documentos) |
| Chaves do Redis | `servidor."usuario:*"` ou `servidor.3."usuario:*"`: uma linha por chave, com `key`, `type` e `value` |
| Consulta na língua do servidor | `servidor($$ ... $$)` ou `servidor.banco($$ ... $$)` |

O nome do servidor vai sem aspas quando é simples (`pg_vendas` também acha
"PG Vendas") ou entre aspas (`"PG Vendas"`) ou colchetes (`[PG Vendas]`).
Sem banco, vale o banco da conexão guardada.

Uma tabela escrita **sem servidor** (`documento`, `public.documento`) é do
servidor e do banco da janela, os que aparecem na barra.

Para ir além do que o PostPilot manda sozinho ao servidor (veja abaixo), use
a consulta dele:

```sql
SELECT c.nome, s.dispositivo, a.texto
FROM pg_vendas($$ SELECT id, nome FROM clientes WHERE ativo $$) c
JOIN mongo($$ db.sessoes.find({dispositivo: "celular"}) $$) s ON s.usuario = c.id
JOIN busca($$ GET /avaliacoes/_search
              {"query": {"term": {"nota": 5}}, "size": 1000} $$) a ON a.cliente_id = c.id
JOIN cache($$ HGETALL usuario:1 $$) h ON h.field = 'nome';
```

## O que roda em cada servidor

O PostPilot não baixa uma tabela inteira para depois filtrar. De uma tabela
do PostgreSQL ou do SQL Server ele pede ao servidor só o que a consulta usa:

- **Colunas**: só as que aparecem na consulta (`SELECT *` pede todas).
- **Filtros**: as condições do `WHERE` e do `ON` que falam de uma tabela só,
  com `=`, `<`, `>`, `IN`, `BETWEEN`, `IS NULL`, `AND` e `OR` contra números,
  textos e datas (`'2026-01-31'` ou `'2026-01-31 10:00:00'`).
- **Chaves do join**: com `a.x = b.y`, a tabela `b` é lida só com as chaves
  que `a` trouxe (até 50.000 chaves, em lotes).
- **Ordem e limite**: um `ORDER BY` pelas colunas de uma tabela com `LIMIT`
  lê só as primeiras linhas dela (1.000, e mais se o join não completar o
  limite).

```sql
-- Lê as 1.000 últimas linhas de documento e, de lotes, só as chaves delas.
SELECT d.caixa_id, l.id
FROM documento d
JOIN "Tiib@SaaS".lotes l ON l.chave_de_processamento = d.chave_processamento_tiib
ORDER BY d.id DESC
LIMIT 1;
```

A aba **Mensagens** mostra, enquanto roda, o que foi pedido a cada servidor
e quantas linhas vieram. O botão de cancelar para tudo na hora.

**Quando não dá para diminuir, a consulta para antes de baixar.** Se uma
tabela viria com mais de 200.000 linhas (pela estimativa do servidor), nada
é baixado e a mensagem diz quantas linhas viriam de qual servidor. Aí é
filtrar, juntar por uma chave com uma tabela filtrada ou escrever a consulta
do servidor. Para baixar mesmo assim (até 1.000.000 de linhas por tabela),
ponha o comentário `-- baixar tudo` na consulta.

Fica sem essa economia, por enquanto: coleções do MongoDB, índices do
Elasticsearch e chaves do Redis (use `servidor($$ ... $$)` para filtrar
neles), consultas com subconsulta, `WITH`, `UNION` ou a mesma tabela duas
vezes, `LIKE`, funções no filtro e `ORDER BY` por texto.

## Como funciona

O PostPilot lê cada fonte no servidor dela (até 8 ao mesmo tempo), guarda as
linhas num banco SQLite temporário no seu computador (em disco, apagado ao
terminar) e roda o seu `SELECT` nele. Por isso:

- **O SQL de fora é o do SQLite**: `||` para juntar texto, `LIMIT`,
  `strftime`, CTEs, funções de janela, `json_extract`. O SQL de dentro de
  `servidor($$ ... $$)` é o do servidor.
- **Documentos e hashes chegam como JSON** numa coluna: leia com
  `json_extract(coluna, '$.campo')`.
- **Só leitura.** Uma fonte que possa alterar o servidor é recusada antes de
  rodar (no PostgreSQL ela ainda roda numa transação `READ ONLY`), e o SQL
  de fora só aceita `SELECT`. Por isso não há confirmação de Produção.

## Limites

- **Volume**: os dados de cada fonte vêm para o seu computador. Uma tabela
  inteira com mais de 200.000 linhas não vem sem o comentário
  `-- baixar tudo`; com ele, e nas consultas `servidor($$ ... $$)`, cada
  fonte lê até 1.000.000 de linhas e as Mensagens avisam quando corta.
- **Filtros**: o `WHERE` de fora roda aqui também, depois de ler; o que vai
  para o servidor é só para ler menos.
- **Tipos**: os valores chegam como texto. Colunas só de números viram
  números (para comparar e somar entre servidores); o resto fica texto.
  Códigos com zero na frente (CEP, CPF) continuam texto. Datas ficam como o
  servidor escreve, e cada um escreve de um jeito: use `substr`, `date()` ou
  converta na consulta do servidor. Booleanos também (`t`/`f` no PostgreSQL,
  `1`/`0` no SQL Server). Decimais com mais de 15 dígitos ficam texto.
- **Texto** compara diferenciando maiúsculas (use `COLLATE NOCASE` ou
  `lower()`).
- Planos de execução, Alt+F1 e edição na grade não valem nessa janela.
- O IntelliSense sugere os servidores e, dos que têm conexão aberta, as
  tabelas e colunas do banco da conexão (`servidor.tabela`).

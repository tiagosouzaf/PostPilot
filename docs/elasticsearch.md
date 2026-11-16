# Elasticsearch no PostPilot

O PostPilot conecta em Elasticsearch (7 e 8, e o que fala a mesma API
REST) e roda requisições como no console do Kibana. As buscas aparecem
numa grade e o Pesquisador de Objetos mostra os índices com os campos.

## Conectar

Em **Conectar ao Servidor**, escolha **Elasticsearch** em "Tipo de
servidor".

| Campo | O que vai |
| --- | --- |
| Nome do servidor | `host` ou `host,porta` (9200 se faltar). |
| Login e senha | O usuário (`elastic`, por exemplo) e a senha. Com o login vazio, a senha é uma **chave de API** (a `encoded` que o Elasticsearch mostra ao criar). Os dois vazios num servidor sem segurança. |
| Criptografia | Sem criptografia (HTTP), HTTPS confiando no certificado, ou HTTPS conferindo o certificado. |

Colar o endereço (`https://usuario:senha@host:9200`) no nome do servidor
preenche os campos.

Uma instalação nova do Elasticsearch 8 já vem com HTTPS e um certificado
autoassinado: escolha a criptografia que confia no certificado. Se a
conexão cair logo de cara com "sem criptografia", é isso.

A conexão pode ser salva no workspace e nos Servidores Registrados, com
pasta e ambiente, como as outras. A senha ou a chave vai para o cofre,
nunca para um arquivo em texto puro.

A conexão fala com o endereço informado (um nó ou o balanceador). O Cloud
ID da Elastic Cloud não entra: use o endereço HTTPS do deployment.

## Janela de consulta

Cada requisição começa com o método e o caminho; as linhas seguintes são
o corpo em JSON. Linhas que começam com `#` ou `//` são comentários.

```
GET /_cat/indices?s=index

GET /pedidos/_search
{
  "query": { "match": { "cliente.nome": "ana" } },
  "sort": [{ "data": "desc" }]
}

POST /pedidos/_bulk
{ "index": { "_id": "1" } }
{ "numero": 1, "total": 10.5 }
```

A barra do começo do caminho pode faltar (`GET _search`) e o método pode
vir em minúsculas. Textos de três aspas (`"""..."""`) valem como no
Kibana. Num `_bulk` ou `_msearch`, cada JSON vai numa linha para o
servidor mesmo escrito em várias.

Como as respostas aparecem:

| Resposta | Na tela |
| --- | --- |
| Busca (`_search`) | Uma linha por documento: `_index`, `_id`, `_score` e um campo do `_source` por coluna (objetos e listas de dentro em JSON, que o visualizador de JSON abre). Uma grade a mais por agregação, com os buckets. Em Mensagens, o total encontrado e o tempo. |
| `_cat` | Uma grade, com as colunas na ordem do servidor (o PostPilot pede `format=json`; com `format=txt` ou `help` vem o texto). |
| `_sql` | A tabela como veio. |
| `_bulk` | Uma linha por operação, com o erro de cada uma quando há. |
| `_mget`, `_msearch` | Como uma busca. |
| Qualquer outro JSON | Uma linha por campo: `field` e `value`. |

- A busca traz o que o servidor mandar: 10 documentos se o corpo não tiver
  `size`. Para ir além de 10.000, use `search_after`.
- Um erro (HTTP 4xx ou 5xx) para a execução, mostra o motivo que o
  servidor deu e aponta a linha da requisição.
- Executar com um trecho selecionado roda só o trecho.
- Num servidor de um ambiente que pede confirmação (Produção), um texto com
  qualquer requisição que escreve pede o "Execute" antes de rodar. Só leem:
  `GET`, `HEAD` e os `POST` de `_search`, `_count`, `_msearch`, `_mget`,
  `_sql`, `_analyze`, `_explain`, `_validate`, `_field_caps` e parecidos.
- O seletor de bancos mostra só `cluster`: o Elasticsearch não tem bancos.

Os modelos (Ctrl+Alt+T) têm buscas, agregações, documentos, índices,
cluster e análise de texto prontos para adaptar.

## Pesquisador de Objetos

- **Índices**: cada índice com os campos do mapeamento e o tipo
  (`cliente.cidade (keyword)`, com os subcampos como `nome.keyword`).
  **Filtrar Índices...** no menu do grupo mostra só os que contêm um texto
  ou batem com um padrão (`logs-*`); o filtro fica salvo com a sessão.
- **Índices Ocultos**: os que começam com `.` (do próprio Elasticsearch,
  do Kibana e os de trás dos data streams).
- **Aliases** e **Data Streams**.

No menu de um índice (ou alias, ou data stream):

- **Abrir os Primeiros 1000 Documentos** roda a busca numa janela nova.
- **Gerar Script de CREATE do Índice** escreve o `PUT` com as
  configurações, o mapeamento e os aliases.
- **Excluir Índice...** abre o `DELETE` numa janela para você conferir e
  rodar; nada é apagado pelo menu.
- **Propriedades** (F4): saúde, documentos, tamanho, shards e réplicas.

## O que não existe para o Elasticsearch

Table Designer, diagramas, Schema Compare, Data Compare, Query Designer,
edição na grade, importação de CSV, plano de execução gráfico, Activity
Monitor, backup e as ferramentas do Claude avisam na status bar que não
estão disponíveis para o Elasticsearch. Sugestões no editor ainda não há.

## Testes

```sh
docker run -d -p 19200:9200 -e discovery.type=single-node \
    -e xpack.security.enabled=false elasticsearch:8.15.0
docker run -d -p 19201:9200 -e discovery.type=single-node \
    -e ELASTIC_PASSWORD='PostPilot!2026' elasticsearch:8.15.0
ELASTICSEARCH_TEST_HOST=127.0.0.1 ELASTICSEARCH_TEST_PORT=19200 \
    ELASTICSEARCH_TEST_TLS_PORT=19201 ELASTICSEARCH_TEST_PASSWORD='PostPilot!2026' \
    cargo test -p pgstudio-elasticsearch -p pgstudio-ui-shell --test server --test elasticsearch
```

Sem `ELASTICSEARCH_TEST_HOST` esses testes só avisam e passam. Com o disco
da máquina quase cheio o Elasticsearch não aloca os índices e as
requisições ficam esperando: acrescente
`-e cluster.routing.allocation.disk.threshold_enabled=false` aos dois
`docker run`.

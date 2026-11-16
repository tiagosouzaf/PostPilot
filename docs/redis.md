# Redis no PostPilot

O PostPilot conecta ao Redis pelo mesmo Connect to Server, pelos Servidores
Registrados e pelas conexões do workspace: escolha **Redis** em "Server
type". O plano está na Fase N do plano de implementação; as decisões, no
[ADR 0005](adr/0005-redis-nosql.md).

## Conectar

| Campo | No Redis |
| --- | --- |
| Server name | `host` ou `host,porta` (padrão 6379) |
| Login | opcional: o usuário de ACL (Redis 6+). Vazio é o usuário `default` |
| Password | a do `requirepass` ou a do usuário de ACL. Vazia se o servidor não pede |
| Database | `db0` a `db15` (ou quantos o servidor tiver) |
| Encryption | sem criptografia; TLS confiando no certificado; TLS conferindo o certificado |

Também dá para colar `redis://usuario:senha@host:6379/2` ou
`rediss://...` (TLS) no campo de connection string dos Servidores
Registrados. A senha vai para o cofre (do sistema ou do workspace), nunca
para um arquivo em texto puro.

Servidores antes do 6 (sem `HELLO`) funcionam em RESP2. Redis Cluster e
Sentinel ainda não: conecte direto num nó.

## Janela de consulta

Um comando por linha, como no redis-cli:

```
# comentário (também -- e //)
SET cliente:1 "João da Silva" EX 3600
HSET pedido:10 status pago total 99.90
HGETALL pedido:10
ZRANGE ranking 0 9 WITHSCORES
SCAN 0 MATCH cliente:* COUNT 100
```

- Aspas duplas aceitam `\n`, `\t`, `\"` e `\xHH`; aspas simples, só `\'`.
  Um texto entre aspas pode passar de uma linha.
- Cada resposta vira uma grade: `field`/`value` para hashes, `member`/`score`
  para sorted sets, uma coluna para listas e conjuntos. Listas dentro de
  listas saem como JSON na célula. Respostas de status (`OK`) e o número
  que um comando de escrita devolve (`(integer) 1`) vão para Messages.
- Um erro num comando aparece em Messages com a linha, e os comandos
  seguintes rodam.
- `SUBSCRIBE`, `PSUBSCRIBE` e `MONITOR` seguem mostrando o que chega até
  você cancelar a consulta (Alt+Break).
- O seletor de bancos da barra (Ctrl+U) troca entre `db0`, `db1`...
- Num servidor de um ambiente que pede confirmação (Produção), um texto com
  qualquer comando que não seja de leitura pede o "Execute" antes de rodar.

## Chaves no Pesquisador de Objetos

Cada banco (`db0 (48)`: o número é o total de chaves) abre com as chaves
dele, agrupadas pelo prefixo até o `:`: `cliente:* (3)`, e dentro dele
`cliente:1:* (2)` e assim por diante. Um prefixo com uma chave só não vira
grupo. Cada chave mostra o tipo (`string`, `hash`, `list`, `set`, `zset`,
`stream`...), e números dentro do nome ordenam como números (`sessao:2`
antes de `sessao:10`).

No menu de uma chave:

- **Abrir Valor** abre uma janela de consulta com o comando de leitura do
  tipo dela, já rodado: `GET`, `HGETALL`, `LRANGE 0 999`, `SMEMBERS`,
  `ZRANGE 0 999 WITHSCORES`, `XRANGE - + COUNT 1000`.
- **Excluir Chave...** abre o `DEL` numa janela para você conferir e rodar;
  nada é apagado pelo menu.
- **Propriedades** (F4): tipo, tempo para expirar (TTL), tamanho,
  codificação e memória.

No menu de um banco, **Filtrar Chaves...** mostra só as que contêm um texto
(ou casam com um padrão, como `user:*` ou `sess?o:1[0-9]`). O filtro aparece
como a primeira linha do banco e volta quando o app reabre.

As chaves são lidas com `SCAN`, que não trava o servidor. Cada leitura para
em 20.000 chaves: se o banco tem mais, as contagens saem com `+` e uma linha
avisa; o filtro estreita a busca. "Atualizar" relê depois de criar ou apagar
chaves.

## O que não existe para o Redis

Table Designer, diagramas, Schema Compare, Query Designer, plano de
execução, Activity Monitor, backup e as ferramentas do Claude avisam na
status bar que não estão disponíveis para o Redis.

## Testes

```
docker run -d -p 16379:6379 redis:7 redis-server --requirepass 'PostPilot!2026'
REDIS_TEST_HOST=127.0.0.1 REDIS_TEST_PORT=16379 REDIS_TEST_PASSWORD='PostPilot!2026' \
    cargo test -p pgstudio-redis -p pgstudio-ui-shell --test server --test redis
```

Sem `REDIS_TEST_HOST` esses testes só avisam. `REDIS_TEST_TLS_PORT` testa o
TLS e `REDIS_TEST_ACL_USER`/`REDIS_TEST_ACL_PASSWORD`, um usuário de ACL. Os
testes usam os bancos 14 e 15 e chaves com os prefixos `pptest:` e `ppui:`.

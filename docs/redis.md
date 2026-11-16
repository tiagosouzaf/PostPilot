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

# ADR 0005: Redis e bancos NoSQL no PostPilot

- **Status:** aceita (decisão técnica do Claude, a pedido do Tiago); em implementação na Fase N
- **Data:** 2026-11-16

## Contexto

O Tiago pediu que o PostPilot conecte a Redis e a bancos NoSQL, citando
MongoDB e Elasticsearch, com a lista aberta. Nenhum deles é relacional: não
há tabelas, colunas, schemas nem SQL. O ADR 0004 já deixou a interface
falando com o servidor por três traits (`Connector`,
`QueryService`/`QuerySession`, `MetadataProvider`) e com os recursos
ligados por motor (`Engine::supports`).

## Decisões

### 1. A experiência é traduzida, não copiada

Cada motor ganha o que faz sentido nele: conectar (com Servidores
Registrados, workspace, pastas, ambientes e a confirmação de Produção),
navegar no Pesquisador de Objetos e rodar comandos na janela de consulta
na língua do motor (comandos do Redis, consultas no estilo do mongosh,
requisições no estilo do console do Kibana). As ferramentas só relacionais
ficam desligadas por `Engine::supports` e avisam na status bar.

### 2. Os mesmos traits, com texto na língua do motor

`QuerySession::execute` recebe o texto da janela e devolve colunas e linhas
de texto. Isso serve a qualquer motor: a sessão do Redis lê um comando por
linha e transforma cada resposta numa grade. A interface não muda.

O que a interface pergunta ao servidor por SQL (lista de bancos, transação
aberta) vira, nos motores sem SQL, um comando do cliente que a sessão
responde sem mandar ao servidor com esse nome (`POSTPILOT DATABASES`,
`POSTPILOT INTRANSACTION`). `Engine::is_sql()` diz se o motor fala SQL.

### 3. `EngineConnector` no núcleo

O conector que escolhe pelo motor sai do crate do SQL Server e vai para o
`pgstudio-core`, com um registro por motor. Um motor novo é um crate
`pgstudio-<motor>` e uma linha no `main`.

### 4. Redis: cliente RESP próprio

| Opção | Prós | Contras |
| --- | --- | --- |
| **cliente próprio** (RESP2/RESP3 sobre tokio + rustls) | uns 300 linhas; devolve qualquer resposta de qualquer comando; nenhuma dependência nova | Cluster e Sentinel ficam por fazer |
| crate `redis` | Cluster e Sentinel prontos | API tipada por comando, que uma janela de consulta não usa; mais dependências e o provedor de criptografia padrão do rustls |

Escolhido o cliente próprio. A conexão pede RESP3 (`HELLO 3`), que traz
mapas e conjuntos nas respostas, e cai para RESP2 antes do Redis 6. O TLS
é o rustls com ring que o app já usa.

### 5. Confirmação de Produção por nome de comando

No Redis não há transação só de leitura para servir de rede. O
classificador conhece os comandos de leitura pelo nome; qualquer outro
(inclusive os de módulos que ele não conhece, `EVAL` e `MULTI`) conta como
escrita e pede confirmação.

## Consequências

- Arquivos de conexão ganham `"engine": "redis"`; os de PostgreSQL não
  mudam. A chave do servidor e da senha no cofre tem o prefixo `redis:`.
- MongoDB e Elasticsearch seguem o mesmo caminho (driver oficial do MongoDB;
  HTTP com o `ureq` no Elasticsearch).

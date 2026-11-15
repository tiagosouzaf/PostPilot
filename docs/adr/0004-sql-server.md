# ADR 0004: SQL Server no PostPilot

- **Status:** aceita (decisão técnica do Claude, a pedido do Tiago); em implementação na Fase S
- **Data:** 2026-11-15

## Contexto

O Tiago pediu que o PostPilot trabalhe também com o Microsoft SQL Server,
com a mesma experiência do PostgreSQL. A interface já fala com o banco por
três traits do núcleo (`Connector`, `QueryService`/`QuerySession`,
`MetadataProvider`); só o `main` conhece o driver. O que prende o app ao
PostgreSQL:

1. `ConnectionSettings` não diz o motor e assume a porta 5432.
2. Cerca de 80 consultas de catálogo do PostgreSQL escritas dentro da
   interface (lista de bancos, Activity Monitor, segurança, Schema Compare,
   propriedades, backup).
3. O dialeto do editor: `$$`, `::`, nomes que viram minúsculas,
   `search_path`, `EXPLAIN`.
4. A edição da grade, que identifica colunas por `oid` e `attnum`.

## Decisões

### 1. O motor fica nas configurações da conexão

`ConnectionSettings.engine` (`Engine::PostgreSql` ou `Engine::SqlServer`).
O PostgreSQL não é gravado (`skip_serializing_if`), então Registered
Servers, workspaces e conexões recentes antigos abrem iguais e os arquivos
novos de PostgreSQL não mudam. Porta, banco e usuário padrão vêm do motor
(1433, `master`, `sa` no SQL Server). A chave do servidor conectado (e da
senha no cofre) ganha o prefixo `sqlserver:` no SQL Server; a do PostgreSQL
fica como era, para achar as senhas já guardadas.

### 2. Recursos por motor, sem simular

`Engine::supports(Feature)` diz o que o app já sabe fazer em cada motor. Um
comando ou ação do Object Explorer que ainda não existe para o SQL Server
diz isso na status bar ("... ainda não está disponível para SQL Server") e
não manda SQL do PostgreSQL. A lista cresce marco a marco. O que não tem
equivalente (backup em arquivo local à la `pg_dump`, Extensions,
Materialized Views, tipos enum e range) não é imitado: a interface oferece o
que o SQL Server tem (backup no disco do servidor, Generate Scripts).

### 3. Driver: tiberius, no repositório, com o lote completo

Opções:

| Opção | Prós | Contras |
| --- | --- | --- |
| **tiberius** (Rust puro) | sem nada para instalar; rustls; SQL, NTLM (sspi-rs, também no Linux) e token do Entra ID | a API pública descarta mensagens e contagens |
| odbc-api + ODBC Driver 18 da Microsoft | driver oficial; Entra ID completo | exige unixODBC e o msodbcsql18 (no Arch, só pelo AUR); quebra o AppImage e o tar.gz que funcionam sozinhos |
| escrever o TDS do zero | controle total | meses de trabalho |

Escolhido: **tiberius 0.13** (MIT/Apache-2.0) em `vendor/tiberius`, com um
patch pequeno e isolado (`src/tds/stream/batch.rs` e `Client::batch`). O
`QueryStream` do tiberius só devolve metadados e linhas: perde o `PRINT`, o
`RAISERROR ... WITH NOWAIT`, as contagens de linhas (`(3 rows affected)`) e
todo erro depois do primeiro, que uma janela de consulta precisa mostrar na
ordem, como a aba Messages do SSMS. O `BatchStream` do patch devolve tudo em
`BatchItem`s. Também no vendor: rustls com ring (o tiberius usa aws-lc-rs
por padrão; o app inteiro usa ring). O tiberius 0.13 pede Rust 1.88, então a
MSRV do workspace sobe para 1.88.

Quando o tiberius publicar uma API com as mensagens, o vendor sai.

### 4. `GO` é do cliente

Como no SSMS e no sqlcmd, `GO` (sozinho na linha, com contagem opcional)
separa lotes no app; o servidor nunca o vê. Um erro num lote não impede os
seguintes. A linha de um erro é a do lote somada à posição do lote no
script, para o editor marcar a linha certa.

### 5. Um crate por motor

`pgstudio-mssql` implementa `Connector`, `QueryService`/`QuerySession` e,
depois, `MetadataProvider` para o SQL Server, como `pg-client` e
`pg-metadata` fazem para o PostgreSQL. O `main` escolhe o conector pelo
motor das configurações. A runtime Tokio (`DbRuntime`) é a mesma.

## Consequências

- Nada muda para quem usa só PostgreSQL; os testes atuais continuam valendo.
- Cada marco da Fase S liga um recurso em `Engine::supports` e traz testes
  contra um SQL Server 2022 real (em contêiner no ambiente de nuvem).
- O patch do tiberius é código nosso para manter; ele fica em um arquivo e
  num método, para ser fácil de levar a uma versão nova.

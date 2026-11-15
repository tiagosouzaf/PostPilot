# SQL Server no PostPilot

O PostPilot trabalha também com o Microsoft SQL Server (Fase S do plano;
decisões em [ADR 0004](adr/0004-sql-server.md)). A paridade com o PostgreSQL
chega marco a marco; o que ainda não existe para o SQL Server avisa na status
bar ("... ainda não está disponível para SQL Server") em vez de rodar.

## O que já funciona

| Recurso | Situação |
| --- | --- |
| Conectar (autenticação SQL, TLS, instância nomeada `servidor\INSTANCIA`, redirecionamento do Azure) | S1.2 |
| Janela de consulta: `GO` (com repetição `GO 5`), vários resultados, `(n rows affected)`, `PRINT` e `RAISERROR` na aba Messages, erros com a linha do script, cancelar | S1.2 |
| Object Explorer: servidor e bancos | S1.2 (pastas de cada banco no S2.1) |
| "Server type" no Connect to Server e nos Registered Servers; login `sa`, banco `master` e porta 1433 como padrão | S1.3 |
| Seletor de bancos da janela de consulta (`sys.databases` que o login abre) e aviso de transação aberta (`@@TRANCOUNT`) | S1.3 |
| Confirmação de comandos perigosos (DROP, DELETE sem WHERE...) lendo T-SQL, com `GO` | S1.3 |

Autenticação do Windows (Kerberos/NTLM) ainda não funciona: a biblioteca de
autenticação que o driver usa não compila junto com as outras dependências
do PostPilot. Por enquanto, use login do SQL Server.

### Criptografia

O campo "Encryption" muda de opções conforme o "Server type". No SQL Server
são as do SSMS:

| Opção | O que faz |
| --- | --- |
| Optional (login only) | só o login vai cifrado |
| Mandatory, trust server certificate | tudo cifrado, sem conferir o certificado (padrão) |
| Mandatory, verify certificate | tudo cifrado, conferindo certificado e nome do servidor |

O SQL Server instalado sem certificado próprio usa um autoassinado; com ele,
use uma das duas primeiras. No arquivo de conexões elas ficam como
`disable`, `require` e `verify-full`.

## Testes contra um SQL Server de verdade

Os testes de `crates/mssql/tests/server.rs` usam `MSSQL_TEST_HOST`,
`MSSQL_TEST_PORT`, `MSSQL_TEST_USER` (padrão `sa`) e `MSSQL_TEST_PASSWORD`;
sem `MSSQL_TEST_HOST` eles só avisam. Um SQL Server 2022 em contêiner:

```sh
docker run -d --name mssql -e ACCEPT_EULA=Y -e 'MSSQL_SA_PASSWORD=PostPilot!2026' \
    -e MSSQL_PID=Developer -p 14330:1433 mcr.microsoft.com/mssql/server:2022-latest
MSSQL_TEST_HOST=127.0.0.1 MSSQL_TEST_PORT=14330 MSSQL_TEST_PASSWORD='PostPilot!2026' \
    cargo test -p pgstudio-mssql
```

Para abrir o app já conectado (screenshots, demonstrações), o prefixo
`sqlserver:` em `PGSTUDIO_CONNECT`:

```sh
PGSTUDIO_CONNECT='sqlserver:127.0.0.1,14330' PGUSER=sa PGPASSWORD='PostPilot!2026' postpilot
```

`PGSTUDIO_CONNECT=dialog:sqlserver` abre o Connect to Server já com o SQL
Server escolhido.

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

### Criptografia

O campo "Encryption" do Connect to Server vale para os dois motores. No SQL
Server:

| Opção | O que faz |
| --- | --- |
| Disable | só o login vai cifrado, como o "Optional" do SSMS |
| Prefer | tudo cifrado, sem conferir o certificado ("Trust server certificate") |
| Require | tudo cifrado e o servidor precisa aceitar, sem conferir o certificado |
| Verify full | tudo cifrado, conferindo certificado e nome do servidor |

O SQL Server instalado sem certificado próprio usa um autoassinado; com ele,
use Disable, Prefer ou Require.

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

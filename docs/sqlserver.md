# SQL Server no PostPilot

Versões: SQL Server 2017 ou mais novo (e Azure SQL). O Object Explorer usa
`STRING_AGG`, que chegou no 2017.

O PostPilot trabalha também com o Microsoft SQL Server (Fase S do plano;
decisões em [ADR 0004](adr/0004-sql-server.md)). A paridade com o PostgreSQL
chega marco a marco; o que ainda não existe para o SQL Server avisa na status
bar ("... ainda não está disponível para SQL Server") em vez de rodar.

## O que já funciona

| Recurso | Situação |
| --- | --- |
| Conectar (autenticação SQL, TLS, instância nomeada `servidor\INSTANCIA`, redirecionamento do Azure) | S1.2 |
| Janela de consulta: `GO` (com repetição `GO 5`), vários resultados, `(n rows affected)`, `PRINT` e `RAISERROR` na aba Messages, erros com a linha do script, cancelar | S1.2 |
| Object Explorer como o do SSMS: bancos (os do sistema em "System Databases"), Tables, Views, Programmability (procedures, funções, tipos, sequences); em cada tabela, Columns (`id (PK, int, not null)`), Keys, Constraints, Triggers e Indexes; propriedades (F4) | S2.1 |
| Select Top 1000 Rows, Edit Top 200 Rows, Script as DROP, Script as CREATE de views, procedures, funções e triggers | S2.1 |
| Script as CREATE de tabela como o SSMS: colunas (identidade, calculadas), PRIMARY KEY e UNIQUE, índices (com INCLUDE e filtro), DEFAULT, FOREIGN KEY e CHECK | S2.2 |
| Editor em T-SQL: realce de `[nomes]`, `N'texto'`, `@variáveis`, `#temporárias` e palavras do T-SQL (`TOP`, `GO`, `PRINT`, `NVARCHAR`...); erro de colchete ou aspa sem fechar | S3.1 |
| IntelliSense: tabelas, views, colunas (com o tipo), procedures, funções e as funções internas do T-SQL; nomes sem diferenciar maiúsculas e com `[colchetes]` só quando precisa | S3.2 |
| "Server type" no Connect to Server e nos Registered Servers; login `sa`, banco `master` e porta 1433 como padrão | S1.3 |
| Seletor de bancos da janela de consulta (`sys.databases` que o login abre) e aviso de transação aberta (`@@TRANCOUNT`) | S1.3 |
| Confirmação de comandos perigosos (DROP, DELETE sem WHERE...) lendo T-SQL, com `GO` | S1.3 |
| Editar e excluir linhas na grade do resultado (SELECT de uma tabela com chave primária, ou de uma view sobre ela) | S1.4 |

Na grade, o PostPilot descobre de onde vem cada coluna com
`sys.dm_exec_describe_first_result_set`. O UPDATE e o DELETE não usam
`OUTPUT` (que falha em tabela com trigger): o valor gravado é lido de novo
pela chave. Colunas calculadas, de identidade, `rowversion` e binárias ficam
só leitura.

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

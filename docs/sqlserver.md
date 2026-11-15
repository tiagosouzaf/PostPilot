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
| Formatar Documento (Shift+Alt+F) em T-SQL: comandos sem `;`, `BEGIN ... END` e `TRY/CATCH` indentados, `GO` sozinho na linha, `CROSS APPLY`, `OUTPUT`, `MERGE` | S3.3 |
| Planos de execução: estimado (Ctrl+L, `SET SHOWPLAN_XML`, não roda o comando) e real (Ctrl+M, `SET STATISTICS XML`, junto com os resultados), com custo por operador, linhas reais e estimadas, predicados, avisos e índices sugeridos nas propriedades | S4.1 |
| Activity Monitor pelas DMVs (precisa de `VIEW SERVER STATE`): sessões, esperas, cadeias de bloqueio, locks, E/S por banco, consultas mais caras e Finalizar Processo (`KILL`). Não há "Cancelar Consulta": o SQL Server não cancela a consulta de outra sessão sem derrubá-la | S4.2 |
| Security no Object Explorer: Logins e Server Roles no servidor, Users e Database Roles em cada banco. Criar e alterar pela janela de propriedades (senha, banco padrão, política de senhas, desabilitar, login e schema do usuário, membros das roles), com Script e tudo numa transação. Só logins do SQL Server são criados (não os do Windows). | S4.3 |
| Permissões de banco, tabela, view, procedure e função (Properties): Grant, With Grant e Deny por usuário ou role, lidas de `sys.database_permissions`; o REVOKE de quem podia repassar usa `CASCADE` | S4.3b |
| Back Up e Restore (Tasks no banco): `BACKUP DATABASE/LOG` completo, diferencial ou de log, com copy-only, compressão e checksum; `RESTORE` lendo os backups do arquivo, para um banco novo (arquivos realocados para as pastas padrão) ou por cima de um existente. Progresso, Stop e Script. O arquivo `.bak` fica no disco do servidor: o SQL Server não tem um backup que venha para o computador de quem usa o app | S4.4 |
| Alt+F1 (Object Information) como o `sp_help`: o objeto, as colunas, os parâmetros, os índices, as constraints e quem aponta para ele por chave estrangeira; aceita `[dbo].[nome]` e avisa quando o nome não existe | S4.5 |
| Query Designer (Design Query in Editor): lê o T-SQL sem diferenciar maiúsculas, com `TOP (n)` e colchetes, e escreve de volta em T-SQL. `TOP ... PERCENT` e `WITH TIES` ficam só no texto. O modelo do banco (tabelas, colunas, chaves, checks, FKs e índices) agora é lido do SQL Server, base para o Table Designer, os diagramas e o Schema Compare | S4.6 |
| Table Designer (Design e New Table) e Database Diagrams: as edições viram T-SQL (`sp_rename`, `ALTER SCHEMA ... TRANSFER`, `ALTER COLUMN`, defaults como constraints) e são aplicadas numa transação. Chaves, índices, checks e FKs que impedem mudar o tipo ou o nome de uma coluna saem antes e voltam depois. Pôr ou tirar `IDENTITY` de uma coluna que já existe não é feito (o SQL Server exige recriar a tabela): o designer avisa e não salva. Índices recriados perdem as colunas `INCLUDE`, e colunas calculadas, o `PERSISTED` | S4.7 |
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

//! O que cada palavra-chave e função do SQL (PostgreSQL e T-SQL) faz.

use super::Doc;

/// Palavras-chave, em minúsculas.
pub const KEYWORDS: &[Doc] = &[
    (
        "abort",
        "Cancels the current transaction (same as ROLLBACK).",
        "Cancela a transação atual (igual ao ROLLBACK).",
    ),
    (
        "absolute",
        "In FETCH: goes to the row at an exact position of the cursor.",
        "No FETCH: vai para a linha de uma posição exata do cursor.",
    ),
    (
        "access",
        "Part of ACCESS METHOD and ACCESS SHARE/EXCLUSIVE lock modes.",
        "Parte de ACCESS METHOD e dos modos de bloqueio ACCESS SHARE/EXCLUSIVE.",
    ),
    (
        "action",
        "In a foreign key: NO ACTION fails when referenced rows still exist.",
        "Em uma chave estrangeira: NO ACTION dá erro se ainda há linhas referenciadas.",
    ),
    (
        "add",
        "In ALTER TABLE: adds a column or a constraint.",
        "No ALTER TABLE: adiciona uma coluna ou uma restrição.",
    ),
    (
        "admin",
        "WITH ADMIN OPTION: lets the member grant the role to others.",
        "WITH ADMIN OPTION: deixa o membro conceder a role a outros.",
    ),
    (
        "after",
        "In a trigger: runs after the change.",
        "Em um trigger: dispara depois da alteração.",
    ),
    (
        "aggregate",
        "CREATE AGGREGATE defines a new aggregate function.",
        "CREATE AGGREGATE define uma nova função de agregação.",
    ),
    (
        "all",
        "All rows or values: SELECT ALL, UNION ALL (keeps duplicates), > ALL (subquery).",
        "Todas as linhas ou valores: SELECT ALL, UNION ALL (mantém repetidas), > ALL (subconsulta).",
    ),
    (
        "also",
        "In a rule: runs the command in addition to the original.",
        "Em uma rule: executa o comando além do original.",
    ),
    (
        "alter",
        "Changes an existing object: ALTER TABLE, ALTER INDEX...",
        "Altera um objeto que existe: ALTER TABLE, ALTER INDEX...",
    ),
    (
        "always",
        "GENERATED ALWAYS: the column value is always produced by the database.",
        "GENERATED ALWAYS: o valor da coluna é sempre gerado pelo banco.",
    ),
    ("analyse", "Same as ANALYZE.", "Igual ao ANALYZE."),
    (
        "analyze",
        "Collects statistics of the tables for the planner. In EXPLAIN: runs the query and shows real times.",
        "Coleta estatísticas das tabelas para o planejador. No EXPLAIN: executa a consulta e mostra os tempos reais.",
    ),
    (
        "and",
        "Both conditions must be true.",
        "As duas condições precisam ser verdadeiras.",
    ),
    (
        "any",
        "True if the comparison holds for at least one value of the list or subquery.",
        "Verdadeiro se a comparação vale para pelo menos um valor da lista ou subconsulta.",
    ),
    (
        "array",
        "Builds an array: ARRAY[1, 2, 3] or ARRAY(subquery).",
        "Monta um array: ARRAY[1, 2, 3] ou ARRAY(subconsulta).",
    ),
    (
        "as",
        "Gives a name (alias) to a column or table, or introduces a definition.",
        "Dá um nome (apelido) a uma coluna ou tabela, ou introduz uma definição.",
    ),
    (
        "asc",
        "Ascending order (the default of ORDER BY).",
        "Ordem crescente (o padrão do ORDER BY).",
    ),
    (
        "asymmetric",
        "In BETWEEN: the bounds are used in the order written (the default).",
        "No BETWEEN: os limites valem na ordem escrita (o padrão).",
    ),
    (
        "at",
        "AT TIME ZONE converts a date and time to another time zone.",
        "AT TIME ZONE converte uma data e hora para outro fuso horário.",
    ),
    (
        "authorization",
        "Names the owner: CREATE SCHEMA ... AUTHORIZATION role.",
        "Indica o dono: CREATE SCHEMA ... AUTHORIZATION role.",
    ),
    (
        "before",
        "In a trigger: runs before the change.",
        "Em um trigger: dispara antes da alteração.",
    ),
    (
        "begin",
        "Starts a transaction (or a block of commands).",
        "Abre uma transação (ou um bloco de comandos).",
    ),
    (
        "between",
        "Value inside a range, including both ends: x BETWEEN 1 AND 10.",
        "Valor dentro de uma faixa, incluindo as duas pontas: x BETWEEN 1 AND 10.",
    ),
    ("bigint", "64-bit integer.", "Inteiro de 64 bits."),
    (
        "bigserial",
        "64-bit integer that numbers itself (auto-increment).",
        "Inteiro de 64 bits que se numera sozinho (autoincremento).",
    ),
    (
        "binary",
        "Binary data or format.",
        "Dados ou formato binário.",
    ),
    (
        "bit",
        "Bit string in PostgreSQL; 0, 1 or NULL in SQL Server.",
        "Sequência de bits no PostgreSQL; 0, 1 ou NULL no SQL Server.",
    ),
    ("boolean", "true, false or NULL.", "true, false ou NULL."),
    (
        "both",
        "TRIM(BOTH ...): removes from both ends.",
        "TRIM(BOTH ...): tira das duas pontas.",
    ),
    (
        "by",
        "Part of GROUP BY, ORDER BY and PARTITION BY.",
        "Parte de GROUP BY, ORDER BY e PARTITION BY.",
    ),
    (
        "cache",
        "In a sequence: how many numbers to reserve in memory.",
        "Em uma sequence: quantos números reservar na memória.",
    ),
    ("call", "Runs a procedure.", "Executa uma procedure."),
    (
        "called",
        "CALLED ON NULL INPUT: the function runs even with NULL arguments.",
        "CALLED ON NULL INPUT: a função executa mesmo com argumentos NULL.",
    ),
    (
        "cascade",
        "Applies to the dependent objects or rows too (DROP ... CASCADE, ON DELETE CASCADE).",
        "Aplica também aos objetos ou linhas dependentes (DROP ... CASCADE, ON DELETE CASCADE).",
    ),
    (
        "case",
        "Chooses a value by conditions: CASE WHEN ... THEN ... ELSE ... END.",
        "Escolhe um valor por condições: CASE WHEN ... THEN ... ELSE ... END.",
    ),
    (
        "cast",
        "Converts a value to another type: CAST(x AS integer).",
        "Converte um valor para outro tipo: CAST(x AS integer).",
    ),
    (
        "char",
        "Fixed-length text, padded with spaces.",
        "Texto de tamanho fixo, completado com espaços.",
    ),
    (
        "character",
        "Fixed-length text (CHARACTER VARYING is variable-length).",
        "Texto de tamanho fixo (CHARACTER VARYING é de tamanho variável).",
    ),
    (
        "check",
        "Constraint: the rows must satisfy a condition.",
        "Restrição: as linhas precisam atender a uma condição.",
    ),
    (
        "checkpoint",
        "Forces writing all changed data to disk now.",
        "Força a gravação em disco de todos os dados alterados agora.",
    ),
    ("close", "Closes a cursor.", "Fecha um cursor."),
    (
        "cluster",
        "Physically reorders a table by an index.",
        "Reordena fisicamente uma tabela por um índice.",
    ),
    (
        "coalesce",
        "Returns the first argument that is not NULL.",
        "Devolve o primeiro argumento que não é NULL.",
    ),
    (
        "collate",
        "Chooses the language rules to compare and sort text.",
        "Escolhe as regras de idioma para comparar e ordenar texto.",
    ),
    (
        "column",
        "A column of a table (ALTER TABLE ... ADD COLUMN).",
        "Uma coluna de uma tabela (ALTER TABLE ... ADD COLUMN).",
    ),
    (
        "comment",
        "COMMENT ON stores a description of an object.",
        "COMMENT ON guarda uma descrição de um objeto.",
    ),
    (
        "commit",
        "Confirms the transaction: the changes become permanent.",
        "Confirma a transação: as alterações ficam definitivas.",
    ),
    (
        "committed",
        "READ COMMITTED: each command sees only data already committed.",
        "READ COMMITTED: cada comando só enxerga dados já confirmados.",
    ),
    (
        "concurrently",
        "Creates or rebuilds an index without blocking writes to the table.",
        "Cria ou refaz um índice sem bloquear as gravações na tabela.",
    ),
    (
        "conflict",
        "INSERT ... ON CONFLICT: what to do when the row already exists (DO NOTHING or DO UPDATE).",
        "INSERT ... ON CONFLICT: o que fazer quando a linha já existe (DO NOTHING ou DO UPDATE).",
    ),
    (
        "constraint",
        "Names a rule of the table: primary key, foreign key, unique, check.",
        "Dá nome a uma regra da tabela: chave primária, chave estrangeira, unique, check.",
    ),
    (
        "constraints",
        "SET CONSTRAINTS chooses when deferrable constraints are checked.",
        "SET CONSTRAINTS escolhe quando as restrições adiáveis são verificadas.",
    ),
    (
        "copy",
        "Imports or exports the rows of a table from or to a file, quickly.",
        "Importa ou exporta as linhas de uma tabela de ou para um arquivo, rápido.",
    ),
    (
        "create",
        "Creates a new object: table, index, view, function...",
        "Cria um objeto novo: tabela, índice, view, função...",
    ),
    (
        "cross",
        "CROSS JOIN: every row of one table with every row of the other.",
        "CROSS JOIN: cada linha de uma tabela com cada linha da outra.",
    ),
    (
        "cube",
        "GROUP BY CUBE: totals for all combinations of the columns.",
        "GROUP BY CUBE: totais de todas as combinações das colunas.",
    ),
    (
        "current",
        "CURRENT ROW in a window frame; WHERE CURRENT OF cursor.",
        "CURRENT ROW na moldura de uma janela; WHERE CURRENT OF cursor.",
    ),
    ("current_date", "Today's date.", "A data de hoje."),
    ("current_time", "The current time of day.", "A hora atual."),
    (
        "current_timestamp",
        "The current date and time (of the start of the transaction, in PostgreSQL).",
        "A data e hora atuais (do início da transação, no PostgreSQL).",
    ),
    (
        "current_user",
        "The user whose permissions are in effect.",
        "O usuário cujas permissões estão valendo.",
    ),
    (
        "cursor",
        "A query read a few rows at a time (DECLARE ... CURSOR).",
        "Uma consulta lida aos poucos (DECLARE ... CURSOR).",
    ),
    (
        "cycle",
        "In a sequence: starts over after reaching the limit.",
        "Em uma sequence: recomeça depois de chegar ao limite.",
    ),
    (
        "data",
        "Part of WITH DATA, SET DATA TYPE and similar.",
        "Parte de WITH DATA, SET DATA TYPE e semelhantes.",
    ),
    (
        "database",
        "A database (CREATE, ALTER, DROP DATABASE).",
        "Um banco de dados (CREATE, ALTER, DROP DATABASE).",
    ),
    (
        "date",
        "A calendar date, without time.",
        "Uma data do calendário, sem hora.",
    ),
    (
        "day",
        "The day part of a date or interval.",
        "A parte do dia de uma data ou intervalo.",
    ),
    (
        "deallocate",
        "Releases a prepared statement or a cursor.",
        "Libera um comando preparado ou um cursor.",
    ),
    ("dec", "Same as DECIMAL.", "Igual ao DECIMAL."),
    (
        "decimal",
        "Exact number with a fixed number of decimal places.",
        "Número exato com casas decimais fixas.",
    ),
    (
        "declare",
        "Declares a cursor or (in T-SQL and PL/pgSQL) a variable.",
        "Declara um cursor ou (no T-SQL e no PL/pgSQL) uma variável.",
    ),
    (
        "default",
        "The value used when none is given.",
        "O valor usado quando nenhum é informado.",
    ),
    (
        "deferrable",
        "The constraint can be checked only at the end of the transaction.",
        "A restrição pode ser verificada só no fim da transação.",
    ),
    (
        "deferred",
        "INITIALLY DEFERRED: checks the constraint at the end of the transaction.",
        "INITIALLY DEFERRED: verifica a restrição no fim da transação.",
    ),
    (
        "delete",
        "Deletes rows of a table. Without WHERE, deletes all of them.",
        "Apaga linhas de uma tabela. Sem WHERE, apaga todas.",
    ),
    ("desc", "Descending order.", "Ordem decrescente."),
    (
        "distinct",
        "Removes duplicate rows from the result.",
        "Tira as linhas repetidas do resultado.",
    ),
    (
        "do",
        "Runs an anonymous code block; in ON CONFLICT, what to do.",
        "Executa um bloco de código anônimo; no ON CONFLICT, o que fazer.",
    ),
    (
        "domain",
        "A type based on another with its own rules (CREATE DOMAIN).",
        "Um tipo baseado em outro com regras próprias (CREATE DOMAIN).",
    ),
    (
        "double",
        "DOUBLE PRECISION: double-precision floating-point number.",
        "DOUBLE PRECISION: número de ponto flutuante de precisão dupla.",
    ),
    (
        "drop",
        "Deletes an object: table, index, view... Cannot be undone outside a transaction.",
        "Apaga um objeto: tabela, índice, view... Fora de uma transação não dá para desfazer.",
    ),
    (
        "each",
        "FOR EACH ROW / STATEMENT: how often the trigger runs.",
        "FOR EACH ROW / STATEMENT: quantas vezes o trigger dispara.",
    ),
    (
        "else",
        "The value or commands used when no condition is true.",
        "O valor ou os comandos usados quando nenhuma condição vale.",
    ),
    (
        "enable",
        "Turns on a trigger, a rule or row-level security.",
        "Liga um trigger, uma rule ou a segurança por linha.",
    ),
    (
        "end",
        "Ends a CASE or a block; in PostgreSQL alone, commits the transaction.",
        "Termina um CASE ou um bloco; sozinho, no PostgreSQL, confirma a transação.",
    ),
    (
        "enum",
        "A type with a fixed list of values (CREATE TYPE ... AS ENUM).",
        "Um tipo com uma lista fixa de valores (CREATE TYPE ... AS ENUM).",
    ),
    (
        "escape",
        "Sets the escape character of a LIKE pattern.",
        "Define o caractere de escape de um padrão do LIKE.",
    ),
    (
        "except",
        "The rows of the first query that are not in the second.",
        "As linhas da primeira consulta que não estão na segunda.",
    ),
    (
        "exclude",
        "Exclusion constraint, or rows to leave out of a window frame.",
        "Restrição de exclusão, ou linhas a deixar de fora da moldura de uma janela.",
    ),
    (
        "execute",
        "Runs a prepared statement, a procedure or dynamic SQL.",
        "Executa um comando preparado, uma procedure ou SQL dinâmico.",
    ),
    (
        "exists",
        "True if the subquery returns at least one row.",
        "Verdadeiro se a subconsulta devolve pelo menos uma linha.",
    ),
    (
        "explain",
        "Shows the execution plan of a query, without running it (unless with ANALYZE).",
        "Mostra o plano de execução de uma consulta, sem executá-la (a não ser com ANALYZE).",
    ),
    (
        "extension",
        "A package of extra features (CREATE EXTENSION).",
        "Um pacote de recursos extras (CREATE EXTENSION).",
    ),
    (
        "external",
        "Refers to something outside the database (external table, data source).",
        "Refere-se a algo fora do banco (tabela externa, fonte de dados).",
    ),
    (
        "extract",
        "Takes a part of a date: EXTRACT(year FROM d).",
        "Tira uma parte de uma data: EXTRACT(year FROM d).",
    ),
    (
        "false",
        "The boolean value false.",
        "O valor booleano falso.",
    ),
    (
        "fetch",
        "Reads rows of a cursor; FETCH FIRST n ROWS ONLY limits the result.",
        "Lê linhas de um cursor; FETCH FIRST n ROWS ONLY limita o resultado.",
    ),
    (
        "filter",
        "Aggregates only the rows that match: count(*) FILTER (WHERE ...).",
        "Agrega só as linhas que atendem: count(*) FILTER (WHERE ...).",
    ),
    (
        "first",
        "The first rows (FETCH FIRST) or NULLS FIRST.",
        "As primeiras linhas (FETCH FIRST) ou NULLS FIRST.",
    ),
    (
        "float",
        "Floating-point number (approximate).",
        "Número de ponto flutuante (aproximado).",
    ),
    (
        "following",
        "In a window frame: rows after the current one.",
        "Na moldura de uma janela: linhas depois da atual.",
    ),
    (
        "for",
        "Part of FOR UPDATE (locks the rows read), FOR EACH ROW, loops.",
        "Parte de FOR UPDATE (bloqueia as linhas lidas), FOR EACH ROW, laços.",
    ),
    (
        "force",
        "Forces a behavior (FORCE ROW LEVEL SECURITY, FORCE NOT NULL in COPY).",
        "Força um comportamento (FORCE ROW LEVEL SECURITY, FORCE NOT NULL no COPY).",
    ),
    (
        "foreign",
        "FOREIGN KEY: the value must exist in another table.",
        "FOREIGN KEY: o valor precisa existir em outra tabela.",
    ),
    (
        "from",
        "The tables the query reads.",
        "As tabelas que a consulta lê.",
    ),
    (
        "full",
        "FULL JOIN: all rows of both tables, matched where possible.",
        "FULL JOIN: todas as linhas das duas tabelas, casadas onde der.",
    ),
    (
        "function",
        "A function (CREATE, ALTER, DROP FUNCTION).",
        "Uma função (CREATE, ALTER, DROP FUNCTION).",
    ),
    (
        "functions",
        "ALL FUNCTIONS IN SCHEMA, in GRANT and REVOKE.",
        "ALL FUNCTIONS IN SCHEMA, no GRANT e no REVOKE.",
    ),
    (
        "generated",
        "A column computed from others or numbered by the database.",
        "Uma coluna calculada a partir de outras ou numerada pelo banco.",
    ),
    (
        "grant",
        "Gives permissions on an object to a user or role.",
        "Dá permissões em um objeto a um usuário ou role.",
    ),
    (
        "granted",
        "GRANTED BY: who gave the permission.",
        "GRANTED BY: quem deu a permissão.",
    ),
    (
        "group",
        "GROUP BY: groups the rows with equal values to compute totals.",
        "GROUP BY: agrupa as linhas de valores iguais para calcular totais.",
    ),
    (
        "grouping",
        "GROUPING SETS: several GROUP BY in one query.",
        "GROUPING SETS: vários GROUP BY em uma consulta.",
    ),
    (
        "having",
        "Filters the groups after GROUP BY (WHERE filters the rows before).",
        "Filtra os grupos depois do GROUP BY (o WHERE filtra as linhas antes).",
    ),
    (
        "hour",
        "The hour part of a time or interval.",
        "A parte da hora de um horário ou intervalo.",
    ),
    (
        "identity",
        "A column numbered automatically by the database.",
        "Uma coluna numerada automaticamente pelo banco.",
    ),
    (
        "if",
        "IF EXISTS / IF NOT EXISTS avoid errors; in T-SQL and PL/pgSQL, a condition.",
        "IF EXISTS / IF NOT EXISTS evitam erros; no T-SQL e no PL/pgSQL, uma condição.",
    ),
    (
        "ilike",
        "LIKE that ignores upper and lower case.",
        "LIKE que não diferencia maiúsculas de minúsculas.",
    ),
    (
        "immutable",
        "The function always returns the same result for the same arguments.",
        "A função sempre devolve o mesmo resultado para os mesmos argumentos.",
    ),
    (
        "in",
        "Equal to any value of the list or subquery.",
        "Igual a qualquer valor da lista ou subconsulta.",
    ),
    (
        "include",
        "Extra columns stored in the index, for index-only reads.",
        "Colunas extras guardadas no índice, para leituras só pelo índice.",
    ),
    (
        "index",
        "A structure that speeds up searches on columns (CREATE INDEX).",
        "Uma estrutura que acelera as buscas em colunas (CREATE INDEX).",
    ),
    (
        "inherit",
        "The role inherits the permissions of the roles it belongs to.",
        "A role herda as permissões das roles a que pertence.",
    ),
    (
        "initially",
        "When a deferrable constraint is checked: IMMEDIATE or DEFERRED.",
        "Quando uma restrição adiável é verificada: IMMEDIATE ou DEFERRED.",
    ),
    (
        "inner",
        "INNER JOIN: only the rows that match in both tables.",
        "INNER JOIN: só as linhas que casam nas duas tabelas.",
    ),
    (
        "inout",
        "A parameter that is both input and output.",
        "Um parâmetro que é de entrada e de saída.",
    ),
    (
        "insert",
        "Adds rows to a table.",
        "Adiciona linhas a uma tabela.",
    ),
    (
        "instead",
        "The trigger or rule runs in place of the original command.",
        "O trigger ou a rule executa no lugar do comando original.",
    ),
    ("int", "32-bit integer.", "Inteiro de 32 bits."),
    ("integer", "32-bit integer.", "Inteiro de 32 bits."),
    (
        "intersect",
        "Only the rows that are in both queries.",
        "Só as linhas que estão nas duas consultas.",
    ),
    (
        "interval",
        "A span of time: INTERVAL '2 days'.",
        "Um período de tempo: INTERVAL '2 days'.",
    ),
    (
        "into",
        "Where the rows go: INSERT INTO table, SELECT ... INTO.",
        "Para onde vão as linhas: INSERT INTO tabela, SELECT ... INTO.",
    ),
    (
        "is",
        "Tests NULL, TRUE or FALSE: x IS NULL, x IS NOT NULL.",
        "Testa NULL, TRUE ou FALSE: x IS NULL, x IS NOT NULL.",
    ),
    (
        "isnull",
        "Same as IS NULL (PostgreSQL).",
        "Igual a IS NULL (PostgreSQL).",
    ),
    (
        "isolation",
        "ISOLATION LEVEL: how much the transaction sees of the others.",
        "ISOLATION LEVEL: quanto a transação enxerga das outras.",
    ),
    (
        "join",
        "Combines the rows of two tables by a condition (ON).",
        "Combina as linhas de duas tabelas por uma condição (ON).",
    ),
    (
        "json",
        "JSON stored as text, exactly as written.",
        "JSON guardado como texto, exatamente como foi escrito.",
    ),
    (
        "jsonb",
        "JSON stored in a binary format: faster to query and can be indexed.",
        "JSON guardado em formato binário: mais rápido de consultar e pode ter índice.",
    ),
    (
        "key",
        "Part of PRIMARY KEY and FOREIGN KEY.",
        "Parte de PRIMARY KEY e FOREIGN KEY.",
    ),
    (
        "language",
        "The language a function is written in (sql, plpgsql...).",
        "A linguagem em que uma função é escrita (sql, plpgsql...).",
    ),
    (
        "last",
        "The last rows, or NULLS LAST.",
        "As últimas linhas, ou NULLS LAST.",
    ),
    (
        "lateral",
        "Lets a subquery in FROM use columns of the tables before it.",
        "Deixa uma subconsulta do FROM usar colunas das tabelas anteriores.",
    ),
    (
        "leading",
        "TRIM(LEADING ...): removes from the start.",
        "TRIM(LEADING ...): tira do começo.",
    ),
    (
        "left",
        "LEFT JOIN: all rows of the left table, with NULLs where the right has no match.",
        "LEFT JOIN: todas as linhas da tabela da esquerda, com NULL onde a da direita não casa.",
    ),
    (
        "level",
        "Part of ISOLATION LEVEL and ROW LEVEL SECURITY.",
        "Parte de ISOLATION LEVEL e ROW LEVEL SECURITY.",
    ),
    (
        "like",
        "Text that matches a pattern: % any text, _ one character.",
        "Texto que combina com um padrão: % qualquer texto, _ um caractere.",
    ),
    (
        "limit",
        "Returns at most n rows.",
        "Devolve no máximo n linhas.",
    ),
    (
        "listen",
        "Starts receiving the notifications of a channel (NOTIFY).",
        "Passa a receber as notificações de um canal (NOTIFY).",
    ),
    (
        "local",
        "Valid only for the current transaction (SET LOCAL).",
        "Vale só para a transação atual (SET LOCAL).",
    ),
    (
        "localtime",
        "The current time, without time zone.",
        "A hora atual, sem fuso horário.",
    ),
    (
        "localtimestamp",
        "The current date and time, without time zone.",
        "A data e hora atuais, sem fuso horário.",
    ),
    (
        "lock",
        "Locks a table for the rest of the transaction.",
        "Bloqueia uma tabela até o fim da transação.",
    ),
    (
        "logged",
        "A normal table, written to the WAL (the opposite of UNLOGGED).",
        "Uma tabela normal, gravada no WAL (o contrário de UNLOGGED).",
    ),
    (
        "matched",
        "In MERGE: WHEN MATCHED / WHEN NOT MATCHED.",
        "No MERGE: WHEN MATCHED / WHEN NOT MATCHED.",
    ),
    (
        "materialized",
        "A view whose result is stored and updated with REFRESH.",
        "Uma view cujo resultado fica guardado e é atualizado com REFRESH.",
    ),
    (
        "maxvalue",
        "The largest value of a sequence or partition.",
        "O maior valor de uma sequence ou partição.",
    ),
    (
        "merge",
        "Inserts, updates or deletes rows of a table by comparing with another source.",
        "Insere, atualiza ou apaga linhas de uma tabela comparando com outra origem.",
    ),
    (
        "minute",
        "The minute part of a time or interval.",
        "A parte dos minutos de um horário ou intervalo.",
    ),
    (
        "minvalue",
        "The smallest value of a sequence or partition.",
        "O menor valor de uma sequence ou partição.",
    ),
    (
        "mode",
        "The lock mode of LOCK TABLE.",
        "O modo de bloqueio do LOCK TABLE.",
    ),
    (
        "month",
        "The month part of a date or interval.",
        "A parte do mês de uma data ou intervalo.",
    ),
    (
        "move",
        "Moves a cursor without returning rows.",
        "Move um cursor sem devolver linhas.",
    ),
    (
        "natural",
        "NATURAL JOIN: joins by all columns with the same name.",
        "NATURAL JOIN: junta por todas as colunas de mesmo nome.",
    ),
    (
        "next",
        "The next row of a cursor.",
        "A próxima linha de um cursor.",
    ),
    (
        "no",
        "Negates an option: NO ACTION, WITH NO DATA.",
        "Nega uma opção: NO ACTION, WITH NO DATA.",
    ),
    ("none", "No value or option.", "Nenhum valor ou opção."),
    ("not", "Inverts a condition.", "Inverte uma condição."),
    (
        "nothing",
        "ON CONFLICT DO NOTHING: skips the row that already exists.",
        "ON CONFLICT DO NOTHING: pula a linha que já existe.",
    ),
    (
        "notify",
        "Sends a notification to the sessions listening to a channel.",
        "Envia uma notificação para as sessões que escutam um canal.",
    ),
    (
        "notnull",
        "Same as IS NOT NULL (PostgreSQL).",
        "Igual a IS NOT NULL (PostgreSQL).",
    ),
    (
        "nowait",
        "Fails right away instead of waiting for a lock.",
        "Dá erro na hora em vez de esperar um bloqueio.",
    ),
    (
        "null",
        "The absence of a value. Compare with IS NULL, never with = NULL.",
        "A ausência de valor. Compare com IS NULL, nunca com = NULL.",
    ),
    (
        "nullif",
        "Returns NULL if the two values are equal, otherwise the first.",
        "Devolve NULL se os dois valores são iguais, senão o primeiro.",
    ),
    (
        "nulls",
        "NULLS FIRST / NULLS LAST: where the NULLs go in the ordering.",
        "NULLS FIRST / NULLS LAST: onde ficam os NULL na ordenação.",
    ),
    (
        "numeric",
        "Exact number with the precision you choose (good for money).",
        "Número exato com a precisão que você escolher (bom para dinheiro).",
    ),
    (
        "of",
        "Part of FOR UPDATE OF table, INSTEAD OF, UPDATE OF column.",
        "Parte de FOR UPDATE OF tabela, INSTEAD OF, UPDATE OF coluna.",
    ),
    ("off", "Turns an option off.", "Desliga uma opção."),
    (
        "offset",
        "Skips the first n rows.",
        "Pula as primeiras n linhas.",
    ),
    (
        "oids",
        "Object identifiers (WITH OIDS, an old feature).",
        "Identificadores de objetos (WITH OIDS, um recurso antigo).",
    ),
    (
        "on",
        "The condition of a JOIN, the table of an index or trigger, or turns an option on.",
        "A condição de um JOIN, a tabela de um índice ou trigger, ou liga uma opção.",
    ),
    (
        "only",
        "Only this table, without the tables that inherit from it.",
        "Só esta tabela, sem as tabelas que herdam dela.",
    ),
    (
        "or",
        "At least one of the conditions must be true.",
        "Pelo menos uma das condições precisa ser verdadeira.",
    ),
    (
        "order",
        "ORDER BY: sorts the result.",
        "ORDER BY: ordena o resultado.",
    ),
    ("out", "An output parameter.", "Um parâmetro de saída."),
    (
        "outer",
        "Optional word of LEFT, RIGHT and FULL OUTER JOIN.",
        "Palavra opcional de LEFT, RIGHT e FULL OUTER JOIN.",
    ),
    (
        "over",
        "Turns a function into a window function: sum(x) OVER (PARTITION BY ...).",
        "Faz de uma função uma função de janela: sum(x) OVER (PARTITION BY ...).",
    ),
    (
        "overlaps",
        "True if two periods of time overlap.",
        "Verdadeiro se dois períodos de tempo se sobrepõem.",
    ),
    (
        "overriding",
        "OVERRIDING SYSTEM VALUE: inserts your own value in an identity column.",
        "OVERRIDING SYSTEM VALUE: insere um valor seu em uma coluna identity.",
    ),
    (
        "owned",
        "OWNED BY: ties a sequence to a column, or names the owner's objects.",
        "OWNED BY: liga uma sequence a uma coluna, ou indica os objetos de um dono.",
    ),
    (
        "owner",
        "OWNER TO: changes the owner of an object.",
        "OWNER TO: troca o dono de um objeto.",
    ),
    (
        "parallel",
        "Whether a function can run in parallel queries.",
        "Se uma função pode rodar em consultas paralelas.",
    ),
    (
        "partition",
        "PARTITION BY: splits the rows into groups for a window function, or a table into partitions.",
        "PARTITION BY: divide as linhas em grupos para uma função de janela, ou uma tabela em partições.",
    ),
    (
        "placing",
        "In OVERLAY: the text to put in place.",
        "No OVERLAY: o texto a colocar no lugar.",
    ),
    (
        "policy",
        "A row-level security rule (CREATE POLICY).",
        "Uma regra de segurança por linha (CREATE POLICY).",
    ),
    (
        "position",
        "POSITION(a IN b): where a text appears inside another.",
        "POSITION(a IN b): onde um texto aparece dentro de outro.",
    ),
    (
        "preceding",
        "In a window frame: rows before the current one.",
        "Na moldura de uma janela: linhas antes da atual.",
    ),
    (
        "precision",
        "DOUBLE PRECISION: double-precision floating-point number.",
        "DOUBLE PRECISION: número de ponto flutuante de precisão dupla.",
    ),
    (
        "prepare",
        "Prepares a statement to run several times with EXECUTE.",
        "Prepara um comando para executar várias vezes com EXECUTE.",
    ),
    (
        "primary",
        "PRIMARY KEY: the columns that identify each row (unique and not null).",
        "PRIMARY KEY: as colunas que identificam cada linha (únicas e não nulas).",
    ),
    (
        "prior",
        "The previous row of a cursor.",
        "A linha anterior de um cursor.",
    ),
    (
        "privileges",
        "ALL PRIVILEGES: every permission, in GRANT and REVOKE.",
        "ALL PRIVILEGES: todas as permissões, no GRANT e no REVOKE.",
    ),
    (
        "procedure",
        "A stored procedure, run with CALL (or EXEC in SQL Server).",
        "Uma stored procedure, executada com CALL (ou EXEC no SQL Server).",
    ),
    (
        "procedures",
        "ALL PROCEDURES IN SCHEMA, in GRANT and REVOKE.",
        "ALL PROCEDURES IN SCHEMA, no GRANT e no REVOKE.",
    ),
    (
        "publication",
        "The tables published for logical replication.",
        "As tabelas publicadas para replicação lógica.",
    ),
    (
        "range",
        "A window frame by value, or partitioning by ranges.",
        "Uma moldura de janela por valor, ou particionamento por faixas.",
    ),
    (
        "read",
        "READ ONLY / READ WRITE transactions; READ COMMITTED isolation.",
        "Transações READ ONLY / READ WRITE; isolamento READ COMMITTED.",
    ),
    (
        "real",
        "Single-precision floating-point number.",
        "Número de ponto flutuante de precisão simples.",
    ),
    (
        "recursive",
        "WITH RECURSIVE: a query that refers to itself (trees, hierarchies).",
        "WITH RECURSIVE: uma consulta que usa a si mesma (árvores, hierarquias).",
    ),
    (
        "references",
        "The table and column a foreign key points to.",
        "A tabela e a coluna para onde uma chave estrangeira aponta.",
    ),
    (
        "referencing",
        "In a trigger: names the tables of old and new rows.",
        "Em um trigger: dá nome às tabelas das linhas antigas e novas.",
    ),
    (
        "refresh",
        "REFRESH MATERIALIZED VIEW: recomputes the stored result.",
        "REFRESH MATERIALIZED VIEW: recalcula o resultado guardado.",
    ),
    ("reindex", "Rebuilds indexes.", "Refaz índices."),
    (
        "release",
        "RELEASE SAVEPOINT: discards a savepoint, keeping its changes.",
        "RELEASE SAVEPOINT: descarta um savepoint, mantendo as alterações dele.",
    ),
    (
        "rename",
        "RENAME TO: changes the name of an object or column.",
        "RENAME TO: troca o nome de um objeto ou coluna.",
    ),
    (
        "repeatable",
        "REPEATABLE READ: the transaction sees the data as of its start.",
        "REPEATABLE READ: a transação enxerga os dados como estavam no início dela.",
    ),
    (
        "replace",
        "CREATE OR REPLACE: creates the object or replaces its definition.",
        "CREATE OR REPLACE: cria o objeto ou substitui a definição dele.",
    ),
    (
        "replica",
        "REPLICA IDENTITY: how replication identifies the changed rows.",
        "REPLICA IDENTITY: como a replicação identifica as linhas alteradas.",
    ),
    (
        "reset",
        "Returns a setting to its default value.",
        "Volta uma configuração ao valor padrão.",
    ),
    (
        "restart",
        "Makes a sequence start again from a value.",
        "Faz uma sequence recomeçar de um valor.",
    ),
    (
        "restrict",
        "Refuses the DROP or DELETE if something depends on it.",
        "Recusa o DROP ou DELETE se algo depende dele.",
    ),
    (
        "return",
        "Returns a value from a function or leaves a procedure.",
        "Devolve um valor de uma função ou sai de uma procedure.",
    ),
    (
        "returning",
        "Returns the rows inserted, updated or deleted by the command.",
        "Devolve as linhas inseridas, atualizadas ou apagadas pelo comando.",
    ),
    (
        "returns",
        "The type a function returns.",
        "O tipo que uma função devolve.",
    ),
    (
        "revoke",
        "Takes permissions away from a user or role.",
        "Tira permissões de um usuário ou role.",
    ),
    (
        "right",
        "RIGHT JOIN: all rows of the right table, with NULLs where the left has no match.",
        "RIGHT JOIN: todas as linhas da tabela da direita, com NULL onde a da esquerda não casa.",
    ),
    (
        "role",
        "A user or a group of permissions (CREATE ROLE).",
        "Um usuário ou um grupo de permissões (CREATE ROLE).",
    ),
    (
        "rollback",
        "Cancels the transaction: undoes its changes.",
        "Cancela a transação: desfaz as alterações dela.",
    ),
    (
        "rollup",
        "GROUP BY ROLLUP: subtotals and a grand total.",
        "GROUP BY ROLLUP: subtotais e um total geral.",
    ),
    (
        "row",
        "A row; ROW(a, b) builds a composite value.",
        "Uma linha; ROW(a, b) monta um valor composto.",
    ),
    (
        "rows",
        "A window frame counted in rows; FETCH FIRST n ROWS.",
        "Uma moldura de janela contada em linhas; FETCH FIRST n ROWS.",
    ),
    (
        "rule",
        "A rule that rewrites commands on a table (CREATE RULE).",
        "Uma regra que reescreve os comandos sobre uma tabela (CREATE RULE).",
    ),
    (
        "savepoint",
        "Marks a point inside the transaction to roll back to.",
        "Marca um ponto dentro da transação para onde dá para voltar.",
    ),
    (
        "schema",
        "A namespace that groups tables and other objects.",
        "Um espaço de nomes que agrupa tabelas e outros objetos.",
    ),
    (
        "schemas",
        "Several schemas, in commands like IMPORT FOREIGN SCHEMA.",
        "Vários schemas, em comandos como IMPORT FOREIGN SCHEMA.",
    ),
    (
        "second",
        "The seconds part of a time or interval.",
        "A parte dos segundos de um horário ou intervalo.",
    ),
    (
        "security",
        "SECURITY DEFINER / INVOKER: with whose permissions a function runs.",
        "SECURITY DEFINER / INVOKER: com as permissões de quem a função executa.",
    ),
    (
        "select",
        "Reads rows: chooses the columns and expressions of the result.",
        "Lê linhas: escolhe as colunas e expressões do resultado.",
    ),
    (
        "sequence",
        "A generator of sequential numbers.",
        "Um gerador de números em sequência.",
    ),
    (
        "sequences",
        "ALL SEQUENCES IN SCHEMA, in GRANT and REVOKE.",
        "ALL SEQUENCES IN SCHEMA, no GRANT e no REVOKE.",
    ),
    (
        "serial",
        "32-bit integer that numbers itself (auto-increment).",
        "Inteiro de 32 bits que se numera sozinho (autoincremento).",
    ),
    (
        "serializable",
        "The strictest isolation: as if the transactions ran one at a time.",
        "O isolamento mais rígido: como se as transações rodassem uma por vez.",
    ),
    (
        "session",
        "Valid for the whole session (SET SESSION).",
        "Vale para a sessão inteira (SET SESSION).",
    ),
    (
        "session_user",
        "The user that logged in.",
        "O usuário que fez o login.",
    ),
    (
        "set",
        "Assigns values: the columns of an UPDATE, or a setting.",
        "Atribui valores: as colunas de um UPDATE, ou uma configuração.",
    ),
    (
        "setof",
        "The function returns several rows (RETURNS SETOF).",
        "A função devolve várias linhas (RETURNS SETOF).",
    ),
    (
        "sets",
        "GROUPING SETS: several GROUP BY in one query.",
        "GROUPING SETS: vários GROUP BY em uma consulta.",
    ),
    (
        "share",
        "A lock mode that allows reads but blocks writes.",
        "Um modo de bloqueio que permite leituras mas barra gravações.",
    ),
    (
        "show",
        "Shows the value of a setting.",
        "Mostra o valor de uma configuração.",
    ),
    (
        "similar",
        "SIMILAR TO: text that matches an SQL regular expression.",
        "SIMILAR TO: texto que combina com uma expressão regular do SQL.",
    ),
    ("smallint", "16-bit integer.", "Inteiro de 16 bits."),
    ("some", "Same as ANY.", "Igual ao ANY."),
    (
        "stable",
        "The function returns the same result within one command.",
        "A função devolve o mesmo resultado dentro de um comando.",
    ),
    (
        "start",
        "START TRANSACTION, or the first value of a sequence (START WITH).",
        "START TRANSACTION, ou o primeiro valor de uma sequence (START WITH).",
    ),
    (
        "statement",
        "FOR EACH STATEMENT: the trigger runs once per command.",
        "FOR EACH STATEMENT: o trigger dispara uma vez por comando.",
    ),
    (
        "statistics",
        "Statistics the planner uses to choose the plan.",
        "Estatísticas que o planejador usa para escolher o plano.",
    ),
    (
        "stored",
        "GENERATED ... STORED: the computed column is saved on disk.",
        "GENERATED ... STORED: a coluna calculada fica gravada em disco.",
    ),
    (
        "strict",
        "The function returns NULL if any argument is NULL.",
        "A função devolve NULL se algum argumento for NULL.",
    ),
    (
        "subscription",
        "Receives the changes of a publication (logical replication).",
        "Recebe as alterações de uma publication (replicação lógica).",
    ),
    (
        "symmetric",
        "BETWEEN SYMMETRIC: accepts the bounds in any order.",
        "BETWEEN SYMMETRIC: aceita os limites em qualquer ordem.",
    ),
    (
        "system",
        "Refers to the system itself (ALTER SYSTEM, TABLESAMPLE SYSTEM).",
        "Refere-se ao próprio sistema (ALTER SYSTEM, TABLESAMPLE SYSTEM).",
    ),
    (
        "table",
        "A table (CREATE, ALTER, DROP TABLE).",
        "Uma tabela (CREATE, ALTER, DROP TABLE).",
    ),
    (
        "tables",
        "ALL TABLES IN SCHEMA, in GRANT and REVOKE.",
        "ALL TABLES IN SCHEMA, no GRANT e no REVOKE.",
    ),
    (
        "tablesample",
        "Reads a random sample of the table.",
        "Lê uma amostra aleatória da tabela.",
    ),
    (
        "tablespace",
        "A place on disk where the data is stored.",
        "Um lugar no disco onde os dados ficam.",
    ),
    ("temp", "Same as TEMPORARY.", "Igual ao TEMPORARY."),
    (
        "temporary",
        "A table that exists only during the session.",
        "Uma tabela que só existe durante a sessão.",
    ),
    ("text", "Text of any length.", "Texto de qualquer tamanho."),
    (
        "then",
        "The value or commands used when the condition is true.",
        "O valor ou os comandos usados quando a condição vale.",
    ),
    (
        "ties",
        "WITH TIES: includes the rows tied with the last one.",
        "WITH TIES: inclui as linhas empatadas com a última.",
    ),
    (
        "time",
        "A time of day, without date.",
        "Um horário, sem data.",
    ),
    (
        "timestamp",
        "Date and time, without time zone (in SQL Server: a row version).",
        "Data e hora, sem fuso horário (no SQL Server: uma versão da linha).",
    ),
    (
        "timestamptz",
        "Date and time with time zone: a precise instant.",
        "Data e hora com fuso horário: um instante exato.",
    ),
    (
        "to",
        "The destination: GRANT ... TO, RENAME TO, COPY ... TO.",
        "O destino: GRANT ... TO, RENAME TO, COPY ... TO.",
    ),
    (
        "trailing",
        "TRIM(TRAILING ...): removes from the end.",
        "TRIM(TRAILING ...): tira do fim.",
    ),
    (
        "transaction",
        "A group of commands that take effect all together or not at all.",
        "Um grupo de comandos que valem todos juntos ou nenhum.",
    ),
    (
        "trigger",
        "Code that runs by itself when a table changes.",
        "Código que executa sozinho quando uma tabela muda.",
    ),
    (
        "trim",
        "Removes spaces (or other characters) from the ends of a text.",
        "Tira os espaços (ou outros caracteres) das pontas de um texto.",
    ),
    (
        "true",
        "The boolean value true.",
        "O valor booleano verdadeiro.",
    ),
    (
        "truncate",
        "Empties a table at once: faster than DELETE, without firing row triggers.",
        "Esvazia uma tabela de uma vez: mais rápido que o DELETE, sem disparar os triggers de linha.",
    ),
    (
        "type",
        "A data type (CREATE TYPE, ALTER COLUMN ... TYPE).",
        "Um tipo de dado (CREATE TYPE, ALTER COLUMN ... TYPE).",
    ),
    (
        "unbounded",
        "In a window frame: from the first or to the last row of the partition.",
        "Na moldura de uma janela: desde a primeira ou até a última linha da partição.",
    ),
    (
        "uncommitted",
        "READ UNCOMMITTED: can see data not committed yet (dirty reads).",
        "READ UNCOMMITTED: pode enxergar dados ainda não confirmados (leitura suja).",
    ),
    (
        "union",
        "Joins the rows of two queries, removing duplicates (UNION ALL keeps them).",
        "Junta as linhas de duas consultas, tirando as repetidas (UNION ALL mantém).",
    ),
    (
        "unique",
        "Constraint or index: the values cannot repeat.",
        "Restrição ou índice: os valores não podem repetir.",
    ),
    (
        "unknown",
        "IS UNKNOWN: the condition is neither true nor false (NULL).",
        "IS UNKNOWN: a condição não é verdadeira nem falsa (NULL).",
    ),
    (
        "unlisten",
        "Stops receiving the notifications of a channel.",
        "Deixa de receber as notificações de um canal.",
    ),
    (
        "unlogged",
        "A table not written to the WAL: faster, but emptied after a crash.",
        "Uma tabela que não vai para o WAL: mais rápida, mas é esvaziada depois de uma queda.",
    ),
    (
        "until",
        "VALID UNTIL: when the password of a role expires.",
        "VALID UNTIL: quando a senha de uma role expira.",
    ),
    (
        "update",
        "Changes the values of rows. Without WHERE, changes all of them.",
        "Altera os valores de linhas. Sem WHERE, altera todas.",
    ),
    (
        "user",
        "The current user; also CREATE USER.",
        "O usuário atual; também CREATE USER.",
    ),
    (
        "using",
        "The columns of a JOIN, the method of an index, or extra tables of a DELETE.",
        "As colunas de um JOIN, o método de um índice, ou tabelas extras de um DELETE.",
    ),
    (
        "uuid",
        "A universally unique identifier.",
        "Um identificador único universal.",
    ),
    (
        "vacuum",
        "Reclaims the space of deleted rows and updates statistics.",
        "Recupera o espaço das linhas apagadas e atualiza as estatísticas.",
    ),
    (
        "valid",
        "NOT VALID: adds a constraint without checking the existing rows.",
        "NOT VALID: cria uma restrição sem verificar as linhas que já existem.",
    ),
    (
        "values",
        "The rows of values to insert, or a table written by hand.",
        "As linhas de valores a inserir, ou uma tabela escrita à mão.",
    ),
    (
        "varchar",
        "Variable-length text, with a maximum.",
        "Texto de tamanho variável, com um máximo.",
    ),
    (
        "variadic",
        "The function accepts any number of arguments.",
        "A função aceita qualquer número de argumentos.",
    ),
    (
        "varying",
        "CHARACTER VARYING: variable-length text.",
        "CHARACTER VARYING: texto de tamanho variável.",
    ),
    (
        "verbose",
        "Shows more details (EXPLAIN VERBOSE, VACUUM VERBOSE).",
        "Mostra mais detalhes (EXPLAIN VERBOSE, VACUUM VERBOSE).",
    ),
    (
        "view",
        "A saved query used like a table.",
        "Uma consulta salva usada como uma tabela.",
    ),
    (
        "views",
        "Several views, in commands that list them.",
        "Várias views, em comandos que as listam.",
    ),
    (
        "volatile",
        "The function can return a different result at each call (the default).",
        "A função pode devolver um resultado diferente a cada chamada (o padrão).",
    ),
    (
        "when",
        "A condition of a CASE, a trigger or a MERGE.",
        "Uma condição de um CASE, de um trigger ou de um MERGE.",
    ),
    (
        "where",
        "Filters the rows: only the ones that satisfy the condition.",
        "Filtra as linhas: só as que atendem à condição.",
    ),
    (
        "window",
        "Names a window definition to use with OVER.",
        "Dá nome a uma definição de janela para usar com OVER.",
    ),
    (
        "with",
        "Defines named subqueries (CTE) before the query, or options of a command.",
        "Define subconsultas com nome (CTE) antes da consulta, ou opções de um comando.",
    ),
    (
        "within",
        "WITHIN GROUP (ORDER BY ...): the order for ordered-set aggregates.",
        "WITHIN GROUP (ORDER BY ...): a ordem para agregações de conjunto ordenado.",
    ),
    (
        "without",
        "WITHOUT TIME ZONE and similar options.",
        "WITHOUT TIME ZONE e opções semelhantes.",
    ),
    (
        "work",
        "Optional word after BEGIN, COMMIT and ROLLBACK.",
        "Palavra opcional depois de BEGIN, COMMIT e ROLLBACK.",
    ),
    (
        "write",
        "READ WRITE: the transaction can change data.",
        "READ WRITE: a transação pode alterar dados.",
    ),
    (
        "year",
        "The year part of a date or interval.",
        "A parte do ano de uma data ou intervalo.",
    ),
    (
        "zone",
        "TIME ZONE: the time zone.",
        "TIME ZONE: o fuso horário.",
    ),
    // Só do T-SQL.
    (
        "ansi_nulls",
        "SET ANSI_NULLS ON: = NULL is never true; use IS NULL.",
        "SET ANSI_NULLS ON: = NULL nunca é verdadeiro; use IS NULL.",
    ),
    (
        "apply",
        "CROSS APPLY / OUTER APPLY: runs a subquery or function for each row of the left table.",
        "CROSS APPLY / OUTER APPLY: executa uma subconsulta ou função para cada linha da tabela da esquerda.",
    ),
    (
        "backup",
        "Makes a backup of a database or of its log.",
        "Faz um backup de um banco ou do log dele.",
    ),
    ("break", "Leaves a WHILE loop.", "Sai de um laço WHILE."),
    (
        "browse",
        "FOR BROWSE: an old option to update data while reading.",
        "FOR BROWSE: uma opção antiga para atualizar dados durante a leitura.",
    ),
    (
        "bulk",
        "BULK INSERT: loads a file into a table, quickly.",
        "BULK INSERT: carrega um arquivo em uma tabela, rápido.",
    ),
    (
        "catch",
        "BEGIN CATCH: the commands that handle an error of the TRY block.",
        "BEGIN CATCH: os comandos que tratam um erro do bloco TRY.",
    ),
    (
        "clustered",
        "An index that defines the physical order of the table (one per table).",
        "Um índice que define a ordem física da tabela (um por tabela).",
    ),
    (
        "compute",
        "An old clause for subtotals; use ROLLUP.",
        "Uma cláusula antiga de subtotais; use ROLLUP.",
    ),
    (
        "contains",
        "Full-text search for words or phrases.",
        "Busca de texto completo por palavras ou frases.",
    ),
    (
        "containstable",
        "Full-text search that returns a table with the relevance of each row.",
        "Busca de texto completo que devolve uma tabela com a relevância de cada linha.",
    ),
    (
        "continue",
        "Goes to the next iteration of a WHILE loop.",
        "Vai para a próxima volta de um laço WHILE.",
    ),
    (
        "convert",
        "Converts a value to another type, with an optional style: CONVERT(type, value, style).",
        "Converte um valor para outro tipo, com um estilo opcional: CONVERT(tipo, valor, estilo).",
    ),
    (
        "datetime",
        "Date and time with a precision of about 3 ms. Prefer datetime2.",
        "Data e hora com precisão de cerca de 3 ms. Prefira datetime2.",
    ),
    (
        "datetime2",
        "Date and time with a precision of up to 100 ns.",
        "Data e hora com precisão de até 100 ns.",
    ),
    (
        "datetimeoffset",
        "Date and time with the time zone offset.",
        "Data e hora com a diferença de fuso horário.",
    ),
    (
        "dbcc",
        "Maintenance and checking commands (DBCC CHECKDB...).",
        "Comandos de manutenção e verificação (DBCC CHECKDB...).",
    ),
    (
        "deleted",
        "In a trigger or OUTPUT: the rows as they were before the change.",
        "Em um trigger ou OUTPUT: as linhas como eram antes da alteração.",
    ),
    (
        "deny",
        "Forbids a permission, even if a role grants it.",
        "Proíbe uma permissão, mesmo que uma role a conceda.",
    ),
    (
        "disk",
        "TO DISK / FROM DISK: the file of a backup.",
        "TO DISK / FROM DISK: o arquivo de um backup.",
    ),
    (
        "distributed",
        "BEGIN DISTRIBUTED TRANSACTION: a transaction across servers.",
        "BEGIN DISTRIBUTED TRANSACTION: uma transação entre servidores.",
    ),
    (
        "dump",
        "An old name of BACKUP.",
        "Um nome antigo do BACKUP.",
    ),
    (
        "errlvl",
        "An option of the sqlcmd tool.",
        "Uma opção da ferramenta sqlcmd.",
    ),
    (
        "exec",
        "Runs a procedure or dynamic SQL (short for EXECUTE).",
        "Executa uma procedure ou SQL dinâmico (forma curta do EXECUTE).",
    ),
    (
        "exit",
        "Leaves the script (sqlcmd).",
        "Sai do script (sqlcmd).",
    ),
    (
        "file",
        "A file of the database or of a backup.",
        "Um arquivo do banco ou de um backup.",
    ),
    (
        "fillfactor",
        "How full to leave each page of the index, in percent.",
        "Quão cheia deixar cada página do índice, em porcentagem.",
    ),
    (
        "freetext",
        "Full-text search by meaning, not only the exact words.",
        "Busca de texto completo pelo sentido, não só pelas palavras exatas.",
    ),
    (
        "freetexttable",
        "FREETEXT search that returns a table with the relevance of each row.",
        "Busca FREETEXT que devolve uma tabela com a relevância de cada linha.",
    ),
    (
        "go",
        "Separates batches: sends the commands above to the server. Not T-SQL itself.",
        "Separa lotes: envia os comandos acima ao servidor. Não é T-SQL em si.",
    ),
    ("goto", "Jumps to a label.", "Pula para um rótulo."),
    (
        "holdlock",
        "Table hint: keeps the lock until the end of the transaction.",
        "Dica de tabela: mantém o bloqueio até o fim da transação.",
    ),
    (
        "identity_insert",
        "SET IDENTITY_INSERT table ON: lets you insert your own values in the identity column.",
        "SET IDENTITY_INSERT tabela ON: deixa inserir valores seus na coluna identity.",
    ),
    (
        "identitycol",
        "Refers to the identity column of the table.",
        "Refere-se à coluna identity da tabela.",
    ),
    (
        "image",
        "Old binary type. Use varbinary(max).",
        "Tipo binário antigo. Use varbinary(max).",
    ),
    (
        "inserted",
        "In a trigger or OUTPUT: the rows as they are after the change.",
        "Em um trigger ou OUTPUT: as linhas como ficaram depois da alteração.",
    ),
    (
        "kill",
        "Ends a session by its ID.",
        "Encerra uma sessão pelo ID dela.",
    ),
    (
        "lineno",
        "An old option to set the line number.",
        "Uma opção antiga para definir o número da linha.",
    ),
    (
        "load",
        "An old name of RESTORE.",
        "Um nome antigo do RESTORE.",
    ),
    (
        "money",
        "Monetary value with 4 decimal places.",
        "Valor monetário com 4 casas decimais.",
    ),
    (
        "national",
        "NATIONAL CHARACTER: Unicode text (nchar, nvarchar).",
        "NATIONAL CHARACTER: texto Unicode (nchar, nvarchar).",
    ),
    (
        "nchar",
        "Fixed-length Unicode text.",
        "Texto Unicode de tamanho fixo.",
    ),
    (
        "nocheck",
        "Does not verify the existing rows, or turns a constraint off.",
        "Não verifica as linhas que já existem, ou desliga uma restrição.",
    ),
    (
        "nocount",
        "SET NOCOUNT ON: stops the \"rows affected\" messages.",
        "SET NOCOUNT ON: tira as mensagens de \"linhas afetadas\".",
    ),
    (
        "nolock",
        "Table hint: reads without locks, and may see uncommitted data.",
        "Dica de tabela: lê sem bloqueios, e pode enxergar dados não confirmados.",
    ),
    (
        "nonclustered",
        "An index separate from the data of the table.",
        "Um índice separado dos dados da tabela.",
    ),
    (
        "ntext",
        "Old Unicode text type. Use nvarchar(max).",
        "Tipo antigo de texto Unicode. Use nvarchar(max).",
    ),
    (
        "nvarchar",
        "Variable-length Unicode text.",
        "Texto Unicode de tamanho variável.",
    ),
    (
        "offsets",
        "An old option that returns the position of keywords.",
        "Uma opção antiga que devolve a posição das palavras-chave.",
    ),
    ("open", "Opens a cursor.", "Abre um cursor."),
    (
        "opendatasource",
        "Reads from another server with a connection string, without a linked server.",
        "Lê de outro servidor com uma string de conexão, sem linked server.",
    ),
    (
        "openquery",
        "Runs a query on a linked server.",
        "Executa uma consulta em um linked server.",
    ),
    (
        "openrowset",
        "Reads a file or another data source as a table.",
        "Lê um arquivo ou outra fonte de dados como uma tabela.",
    ),
    (
        "openxml",
        "Reads an XML document as a table.",
        "Lê um documento XML como uma tabela.",
    ),
    (
        "option",
        "Query hints: OPTION (RECOMPILE), OPTION (MAXDOP 1).",
        "Dicas da consulta: OPTION (RECOMPILE), OPTION (MAXDOP 1).",
    ),
    (
        "output",
        "Returns the rows changed by INSERT, UPDATE, DELETE or MERGE; or an output parameter.",
        "Devolve as linhas alteradas por INSERT, UPDATE, DELETE ou MERGE; ou um parâmetro de saída.",
    ),
    (
        "percent",
        "TOP n PERCENT: a percentage of the rows.",
        "TOP n PERCENT: uma porcentagem das linhas.",
    ),
    (
        "pivot",
        "Turns the values of a column into columns.",
        "Transforma os valores de uma coluna em colunas.",
    ),
    (
        "plan",
        "The execution plan (SET SHOWPLAN, query hints).",
        "O plano de execução (SET SHOWPLAN, dicas da consulta).",
    ),
    (
        "print",
        "Writes a message to the Messages tab.",
        "Escreve uma mensagem na aba Mensagens.",
    ),
    ("proc", "Short for PROCEDURE.", "Forma curta de PROCEDURE."),
    (
        "quoted_identifier",
        "SET QUOTED_IDENTIFIER ON: \"double quotes\" delimit names, not text.",
        "SET QUOTED_IDENTIFIER ON: \"aspas duplas\" delimitam nomes, não textos.",
    ),
    (
        "raiserror",
        "Raises an error with a message and a severity. In new code, prefer THROW.",
        "Gera um erro com mensagem e severidade. Em código novo, prefira THROW.",
    ),
    (
        "readtext",
        "Reads old text/image columns.",
        "Lê colunas text/image antigas.",
    ),
    (
        "readuncommitted",
        "Table hint: same as NOLOCK.",
        "Dica de tabela: igual ao NOLOCK.",
    ),
    (
        "reconfigure",
        "Applies the settings changed with sp_configure.",
        "Aplica as configurações alteradas com sp_configure.",
    ),
    (
        "replication",
        "NOT FOR REPLICATION: the rule does not apply to replication.",
        "NOT FOR REPLICATION: a regra não vale para a replicação.",
    ),
    (
        "restore",
        "Restores a database from a backup.",
        "Restaura um banco a partir de um backup.",
    ),
    (
        "revert",
        "Goes back to the previous user after EXECUTE AS.",
        "Volta ao usuário anterior depois de um EXECUTE AS.",
    ),
    (
        "rowcount",
        "SET ROWCOUNT n: stops after n rows; @@ROWCOUNT: rows of the last command.",
        "SET ROWCOUNT n: para depois de n linhas; @@ROWCOUNT: linhas do último comando.",
    ),
    (
        "rowguidcol",
        "Marks the uniqueidentifier column that identifies the row.",
        "Marca a coluna uniqueidentifier que identifica a linha.",
    ),
    (
        "rowlock",
        "Table hint: locks rows instead of pages or the table.",
        "Dica de tabela: bloqueia linhas em vez de páginas ou da tabela.",
    ),
    (
        "rowversion",
        "A number that changes by itself every time the row changes.",
        "Um número que muda sozinho toda vez que a linha muda.",
    ),
    (
        "save",
        "SAVE TRANSACTION: marks a point to roll back to.",
        "SAVE TRANSACTION: marca um ponto para onde dá para voltar.",
    ),
    (
        "securityaudit",
        "Reserved word of security auditing.",
        "Palavra reservada da auditoria de segurança.",
    ),
    (
        "semantickeyphrasetable",
        "Returns the key phrases of documents (semantic search).",
        "Devolve as frases-chave de documentos (busca semântica).",
    ),
    (
        "setuser",
        "Old way to impersonate a user. Use EXECUTE AS.",
        "Forma antiga de assumir outro usuário. Use EXECUTE AS.",
    ),
    ("shutdown", "Stops SQL Server.", "Para o SQL Server."),
    (
        "smalldatetime",
        "Date and time with a precision of one minute.",
        "Data e hora com precisão de um minuto.",
    ),
    (
        "smallmoney",
        "Monetary value of a smaller range.",
        "Valor monetário de faixa menor.",
    ),
    (
        "sql_variant",
        "A column that holds values of several types.",
        "Uma coluna que guarda valores de vários tipos.",
    ),
    (
        "system_user",
        "The login of the session.",
        "O login da sessão.",
    ),
    (
        "tablock",
        "Table hint: locks the whole table.",
        "Dica de tabela: bloqueia a tabela inteira.",
    ),
    (
        "textsize",
        "SET TEXTSIZE: the maximum size of text returned.",
        "SET TEXTSIZE: o tamanho máximo de texto devolvido.",
    ),
    (
        "throw",
        "Raises an error, or re-raises the current one inside a CATCH.",
        "Gera um erro, ou repassa o atual dentro de um CATCH.",
    ),
    ("tinyint", "Integer from 0 to 255.", "Inteiro de 0 a 255."),
    (
        "top",
        "Returns only the first n rows: SELECT TOP (10) ...",
        "Devolve só as primeiras n linhas: SELECT TOP (10) ...",
    ),
    (
        "tran",
        "Short for TRANSACTION.",
        "Forma curta de TRANSACTION.",
    ),
    (
        "try",
        "BEGIN TRY: commands whose errors go to the CATCH block.",
        "BEGIN TRY: comandos cujos erros vão para o bloco CATCH.",
    ),
    (
        "try_convert",
        "Like CONVERT, but returns NULL when the conversion fails.",
        "Como o CONVERT, mas devolve NULL quando a conversão falha.",
    ),
    (
        "tsequal",
        "Old comparison of timestamp values.",
        "Comparação antiga de valores timestamp.",
    ),
    (
        "uniqueidentifier",
        "A GUID: a 16-byte unique identifier.",
        "Um GUID: identificador único de 16 bytes.",
    ),
    (
        "unpivot",
        "Turns columns into rows.",
        "Transforma colunas em linhas.",
    ),
    (
        "updatetext",
        "Changes old text/image columns.",
        "Altera colunas text/image antigas.",
    ),
    (
        "updlock",
        "Table hint: takes update locks when reading, to change the rows afterwards.",
        "Dica de tabela: pega bloqueios de atualização ao ler, para alterar as linhas depois.",
    ),
    (
        "use",
        "Switches to another database.",
        "Troca para outro banco.",
    ),
    (
        "varbinary",
        "Variable-length binary data.",
        "Dados binários de tamanho variável.",
    ),
    (
        "waitfor",
        "Waits for a time (DELAY) or until an hour (TIME).",
        "Espera um tempo (DELAY) ou até um horário (TIME).",
    ),
    (
        "while",
        "Repeats commands while the condition is true.",
        "Repete comandos enquanto a condição vale.",
    ),
    (
        "writetext",
        "Writes old text/image columns.",
        "Grava colunas text/image antigas.",
    ),
    (
        "xact_abort",
        "SET XACT_ABORT ON: any error rolls back the whole transaction.",
        "SET XACT_ABORT ON: qualquer erro desfaz a transação inteira.",
    ),
    (
        "xml",
        "XML data; FOR XML returns the result as XML.",
        "Dados XML; FOR XML devolve o resultado como XML.",
    ),
];

/// Funções mais usadas do PostgreSQL e do SQL Server, em minúsculas.
pub const FUNCTIONS: &[Doc] = &[
    (
        "abs",
        "Absolute value of a number.",
        "Valor absoluto de um número.",
    ),
    (
        "age",
        "The interval between two dates (or between a date and today).",
        "O intervalo entre duas datas (ou entre uma data e hoje).",
    ),
    (
        "array_agg",
        "Aggregate: collects the values into an array.",
        "Agregação: junta os valores em um array.",
    ),
    (
        "array_length",
        "Number of elements of an array in a dimension.",
        "Número de elementos de um array em uma dimensão.",
    ),
    (
        "array_to_string",
        "Joins the elements of an array into a text, with a separator.",
        "Junta os elementos de um array em um texto, com um separador.",
    ),
    (
        "avg",
        "Aggregate: average of the values.",
        "Agregação: média dos valores.",
    ),
    (
        "bool_and",
        "Aggregate: true if all values are true.",
        "Agregação: verdadeiro se todos os valores são verdadeiros.",
    ),
    (
        "bool_or",
        "Aggregate: true if at least one value is true.",
        "Agregação: verdadeiro se pelo menos um valor é verdadeiro.",
    ),
    (
        "btrim",
        "Removes spaces (or the given characters) from both ends of a text.",
        "Tira os espaços (ou os caracteres informados) das duas pontas de um texto.",
    ),
    (
        "cast",
        "Converts a value to another type: CAST(x AS type).",
        "Converte um valor para outro tipo: CAST(x AS tipo).",
    ),
    (
        "ceil",
        "Rounds a number up.",
        "Arredonda um número para cima.",
    ),
    (
        "ceiling",
        "Rounds a number up.",
        "Arredonda um número para cima.",
    ),
    (
        "char_length",
        "Number of characters of a text.",
        "Número de caracteres de um texto.",
    ),
    (
        "charindex",
        "Position of a text inside another (0 when not found).",
        "Posição de um texto dentro de outro (0 quando não acha).",
    ),
    (
        "clock_timestamp",
        "The current date and time, changing even inside a command.",
        "A data e hora atuais, mudando mesmo dentro de um comando.",
    ),
    (
        "coalesce",
        "Returns the first argument that is not NULL.",
        "Devolve o primeiro argumento que não é NULL.",
    ),
    (
        "concat",
        "Joins texts; NULL arguments are ignored.",
        "Junta textos; argumentos NULL são ignorados.",
    ),
    (
        "concat_ws",
        "Joins texts with a separator between them.",
        "Junta textos com um separador entre eles.",
    ),
    (
        "convert",
        "Converts a value to another type, with an optional style: CONVERT(type, value, style).",
        "Converte um valor para outro tipo, com um estilo opcional: CONVERT(tipo, valor, estilo).",
    ),
    (
        "count",
        "Aggregate: number of rows (count(*)) or of values that are not NULL.",
        "Agregação: número de linhas (count(*)) ou de valores que não são NULL.",
    ),
    (
        "current_database",
        "Name of the current database.",
        "Nome do banco atual.",
    ),
    (
        "current_schema",
        "Name of the first schema of the search path.",
        "Nome do primeiro schema do caminho de busca.",
    ),
    (
        "currval",
        "The last value this session got from a sequence.",
        "O último valor que esta sessão pegou de uma sequence.",
    ),
    (
        "date_part",
        "A part of a date, as a number: date_part('year', d).",
        "Uma parte de uma data, como número: date_part('year', d).",
    ),
    (
        "date_trunc",
        "Truncates a date to a precision: date_trunc('month', d) gives the first day of the month.",
        "Trunca uma data em uma precisão: date_trunc('month', d) dá o primeiro dia do mês.",
    ),
    (
        "dateadd",
        "Adds an amount of a part (day, month...) to a date.",
        "Soma uma quantidade de uma parte (day, month...) a uma data.",
    ),
    (
        "datediff",
        "Difference between two dates, counted in a part (day, month...).",
        "Diferença entre duas datas, contada em uma parte (day, month...).",
    ),
    (
        "datename",
        "A part of a date as text (the name of the month, of the weekday).",
        "Uma parte de uma data como texto (o nome do mês, do dia da semana).",
    ),
    (
        "datepart",
        "A part of a date as a number.",
        "Uma parte de uma data como número.",
    ),
    (
        "day",
        "The day of the month of a date.",
        "O dia do mês de uma data.",
    ),
    (
        "db_name",
        "Name of the current database (or of the one with the given ID).",
        "Nome do banco atual (ou do banco com o ID informado).",
    ),
    (
        "dense_rank",
        "Window: position in the ordering, without gaps after ties.",
        "Janela: posição na ordenação, sem pular números depois de empates.",
    ),
    (
        "eomonth",
        "The last day of the month of a date.",
        "O último dia do mês de uma data.",
    ),
    (
        "exp",
        "e raised to the power of a number.",
        "e elevado a um número.",
    ),
    (
        "first_value",
        "Window: the first value of the frame.",
        "Janela: o primeiro valor da moldura.",
    ),
    (
        "floor",
        "Rounds a number down.",
        "Arredonda um número para baixo.",
    ),
    (
        "format",
        "Builds a text from a template. In PostgreSQL like printf (%s, %I, %L); in SQL Server formats dates and numbers.",
        "Monta um texto a partir de um modelo. No PostgreSQL como o printf (%s, %I, %L); no SQL Server formata datas e números.",
    ),
    (
        "gen_random_uuid",
        "Generates a random UUID.",
        "Gera um UUID aleatório.",
    ),
    (
        "generate_series",
        "Returns a series of numbers or dates, one per row.",
        "Devolve uma série de números ou datas, um por linha.",
    ),
    (
        "getdate",
        "The current date and time of the server.",
        "A data e hora atuais do servidor.",
    ),
    (
        "getutcdate",
        "The current date and time in UTC.",
        "A data e hora atuais em UTC.",
    ),
    (
        "greatest",
        "The largest of the arguments.",
        "O maior dos argumentos.",
    ),
    (
        "iif",
        "If the condition is true returns one value, otherwise another: IIF(condition, yes, no).",
        "Se a condição vale devolve um valor, senão outro: IIF(condição, sim, não).",
    ),
    (
        "initcap",
        "First letter of each word in uppercase.",
        "Primeira letra de cada palavra em maiúscula.",
    ),
    (
        "isnull",
        "Returns the second value when the first is NULL.",
        "Devolve o segundo valor quando o primeiro é NULL.",
    ),
    (
        "isnumeric",
        "1 if the text can be converted to a number.",
        "1 se o texto pode ser convertido em número.",
    ),
    (
        "json_agg",
        "Aggregate: collects the values into a JSON array.",
        "Agregação: junta os valores em um array JSON.",
    ),
    (
        "json_build_object",
        "Builds a JSON object from pairs of key and value.",
        "Monta um objeto JSON a partir de pares de chave e valor.",
    ),
    (
        "jsonb_agg",
        "Aggregate: collects the values into a JSONB array.",
        "Agregação: junta os valores em um array JSONB.",
    ),
    (
        "jsonb_array_elements",
        "Expands a JSON array into one row per element.",
        "Expande um array JSON em uma linha por elemento.",
    ),
    (
        "jsonb_build_object",
        "Builds a JSONB object from pairs of key and value.",
        "Monta um objeto JSONB a partir de pares de chave e valor.",
    ),
    (
        "jsonb_each",
        "Expands a JSON object into one row per key and value.",
        "Expande um objeto JSON em uma linha por chave e valor.",
    ),
    (
        "jsonb_pretty",
        "Formats a JSONB with indentation.",
        "Formata um JSONB com recuo.",
    ),
    (
        "jsonb_set",
        "Returns the JSONB with the value at a path replaced.",
        "Devolve o JSONB com o valor de um caminho trocado.",
    ),
    (
        "lag",
        "Window: the value of a previous row.",
        "Janela: o valor de uma linha anterior.",
    ),
    (
        "last_value",
        "Window: the last value of the frame.",
        "Janela: o último valor da moldura.",
    ),
    (
        "lastval",
        "The last value this session got from any sequence.",
        "O último valor que esta sessão pegou de qualquer sequence.",
    ),
    (
        "lead",
        "Window: the value of a following row.",
        "Janela: o valor de uma linha seguinte.",
    ),
    (
        "least",
        "The smallest of the arguments.",
        "O menor dos argumentos.",
    ),
    (
        "left",
        "The first n characters of a text.",
        "Os primeiros n caracteres de um texto.",
    ),
    (
        "len",
        "Number of characters of a text, without the trailing spaces.",
        "Número de caracteres de um texto, sem os espaços do fim.",
    ),
    (
        "length",
        "Number of characters of a text.",
        "Número de caracteres de um texto.",
    ),
    ("ln", "Natural logarithm.", "Logaritmo natural."),
    (
        "lower",
        "Converts a text to lowercase.",
        "Converte um texto para minúsculas.",
    ),
    (
        "lpad",
        "Pads a text on the left up to a length.",
        "Completa um texto à esquerda até um tamanho.",
    ),
    (
        "ltrim",
        "Removes the spaces from the start of a text.",
        "Tira os espaços do começo de um texto.",
    ),
    (
        "make_date",
        "Builds a date from year, month and day.",
        "Monta uma data a partir de ano, mês e dia.",
    ),
    (
        "max",
        "Aggregate: the largest value.",
        "Agregação: o maior valor.",
    ),
    (
        "md5",
        "MD5 hash of a text, in hexadecimal.",
        "Hash MD5 de um texto, em hexadecimal.",
    ),
    (
        "min",
        "Aggregate: the smallest value.",
        "Agregação: o menor valor.",
    ),
    ("mod", "Remainder of a division.", "Resto de uma divisão."),
    ("month", "The month of a date.", "O mês de uma data."),
    (
        "newid",
        "Generates a random GUID.",
        "Gera um GUID aleatório.",
    ),
    (
        "nextval",
        "Advances a sequence and returns the new value.",
        "Avança uma sequence e devolve o novo valor.",
    ),
    (
        "now",
        "The current date and time (of the start of the transaction).",
        "A data e hora atuais (do início da transação).",
    ),
    (
        "ntile",
        "Window: splits the rows into n groups of the same size.",
        "Janela: divide as linhas em n grupos do mesmo tamanho.",
    ),
    (
        "nullif",
        "Returns NULL if the two values are equal, otherwise the first.",
        "Devolve NULL se os dois valores são iguais, senão o primeiro.",
    ),
    (
        "object_id",
        "The ID of an object by its name (NULL when it does not exist).",
        "O ID de um objeto pelo nome (NULL quando não existe).",
    ),
    (
        "octet_length",
        "Number of bytes of a text.",
        "Número de bytes de um texto.",
    ),
    (
        "patindex",
        "Position of a pattern (with % and _) inside a text.",
        "Posição de um padrão (com % e _) dentro de um texto.",
    ),
    (
        "pg_size_pretty",
        "Formats a number of bytes as kB, MB, GB.",
        "Formata um número de bytes como kB, MB, GB.",
    ),
    (
        "pg_sleep",
        "Waits the given seconds.",
        "Espera os segundos informados.",
    ),
    (
        "pg_total_relation_size",
        "Disk space of a table with its indexes, in bytes.",
        "Espaço em disco de uma tabela com os índices dela, em bytes.",
    ),
    (
        "position",
        "Position of a text inside another.",
        "Posição de um texto dentro de outro.",
    ),
    (
        "power",
        "A number raised to a power.",
        "Um número elevado a uma potência.",
    ),
    (
        "random",
        "A random number from 0 to 1.",
        "Um número aleatório de 0 a 1.",
    ),
    (
        "rank",
        "Window: position in the ordering, with gaps after ties.",
        "Janela: posição na ordenação, pulando números depois de empates.",
    ),
    (
        "regexp_matches",
        "The parts of a text captured by a regular expression.",
        "As partes de um texto capturadas por uma expressão regular.",
    ),
    (
        "regexp_replace",
        "Replaces what matches a regular expression.",
        "Troca o que combina com uma expressão regular.",
    ),
    (
        "repeat",
        "Repeats a text n times.",
        "Repete um texto n vezes.",
    ),
    (
        "replace",
        "Replaces all occurrences of a text with another.",
        "Troca todas as ocorrências de um texto por outro.",
    ),
    ("reverse", "Reverses a text.", "Inverte um texto."),
    (
        "right",
        "The last n characters of a text.",
        "Os últimos n caracteres de um texto.",
    ),
    (
        "round",
        "Rounds a number to the given decimal places.",
        "Arredonda um número para as casas decimais informadas.",
    ),
    (
        "row_number",
        "Window: numbers the rows 1, 2, 3... in the order of the OVER.",
        "Janela: numera as linhas 1, 2, 3... na ordem do OVER.",
    ),
    (
        "row_to_json",
        "Converts a row into a JSON object.",
        "Converte uma linha em um objeto JSON.",
    ),
    (
        "rpad",
        "Pads a text on the right up to a length.",
        "Completa um texto à direita até um tamanho.",
    ),
    (
        "rtrim",
        "Removes the spaces from the end of a text.",
        "Tira os espaços do fim de um texto.",
    ),
    (
        "scope_identity",
        "The last identity value generated in this scope.",
        "O último valor identity gerado neste escopo.",
    ),
    (
        "setval",
        "Sets the current value of a sequence.",
        "Define o valor atual de uma sequence.",
    ),
    (
        "split_part",
        "The nth part of a text split by a separator.",
        "A enésima parte de um texto dividido por um separador.",
    ),
    ("sqrt", "Square root.", "Raiz quadrada."),
    (
        "string_agg",
        "Aggregate: joins the texts with a separator.",
        "Agregação: junta os textos com um separador.",
    ),
    (
        "string_to_array",
        "Splits a text into an array by a separator.",
        "Divide um texto em um array por um separador.",
    ),
    (
        "strpos",
        "Position of a text inside another (0 when not found).",
        "Posição de um texto dentro de outro (0 quando não acha).",
    ),
    (
        "stuff",
        "Deletes part of a text and inserts another in its place.",
        "Apaga um trecho de um texto e insere outro no lugar.",
    ),
    (
        "substr",
        "A part of a text: substr(text, start, length).",
        "Um trecho de um texto: substr(texto, início, tamanho).",
    ),
    (
        "substring",
        "A part of a text, from a position and with a length.",
        "Um trecho de um texto, a partir de uma posição e com um tamanho.",
    ),
    (
        "sum",
        "Aggregate: sum of the values.",
        "Agregação: soma dos valores.",
    ),
    (
        "sysdatetime",
        "The current date and time of the server, with more precision than GETDATE.",
        "A data e hora atuais do servidor, com mais precisão que o GETDATE.",
    ),
    (
        "to_char",
        "Formats a date or number as text: to_char(d, 'DD/MM/YYYY').",
        "Formata uma data ou número como texto: to_char(d, 'DD/MM/YYYY').",
    ),
    (
        "to_date",
        "Converts a text to a date, by a format.",
        "Converte um texto em data, por um formato.",
    ),
    (
        "to_json",
        "Converts a value to JSON.",
        "Converte um valor para JSON.",
    ),
    (
        "to_jsonb",
        "Converts a value to JSONB.",
        "Converte um valor para JSONB.",
    ),
    (
        "to_number",
        "Converts a text to a number, by a format.",
        "Converte um texto em número, por um formato.",
    ),
    (
        "to_timestamp",
        "Converts a text (by a format) or a Unix timestamp to date and time.",
        "Converte um texto (por um formato) ou um timestamp Unix em data e hora.",
    ),
    (
        "translate",
        "Replaces each character of a list with the matching one of another.",
        "Troca cada caractere de uma lista pelo correspondente de outra.",
    ),
    (
        "trim",
        "Removes spaces (or other characters) from the ends of a text.",
        "Tira os espaços (ou outros caracteres) das pontas de um texto.",
    ),
    (
        "trunc",
        "Cuts the decimal places of a number, without rounding.",
        "Corta as casas decimais de um número, sem arredondar.",
    ),
    (
        "try_cast",
        "Like CAST, but returns NULL when the conversion fails.",
        "Como o CAST, mas devolve NULL quando a conversão falha.",
    ),
    (
        "try_convert",
        "Like CONVERT, but returns NULL when the conversion fails.",
        "Como o CONVERT, mas devolve NULL quando a conversão falha.",
    ),
    (
        "unnest",
        "Expands an array into one row per element.",
        "Expande um array em uma linha por elemento.",
    ),
    (
        "upper",
        "Converts a text to uppercase.",
        "Converte um texto para maiúsculas.",
    ),
    (
        "version",
        "The version of the server.",
        "A versão do servidor.",
    ),
    ("year", "The year of a date.", "O ano de uma data."),
];

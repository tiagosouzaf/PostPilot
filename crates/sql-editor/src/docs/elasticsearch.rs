//! O que cada API, consulta, agregação e chave do Elasticsearch faz.

use super::Doc;

/// Pelo nome. Onde o mesmo nome quer dizer coisas diferentes (`terms`
/// consulta e agregação), a chave é `"detalhe:nome"`, com o detalhe que a
/// lista mostra em cinza (`API`, `consulta`, `agregação`, `tipo`, ou a API
/// de que o nome é uma parte, como em `_cat:indices`).
pub const DOCS: &[Doc] = &[
    // Métodos.
    (
        "DELETE",
        "Deletes: an index, a document, an alias...",
        "Apaga: um índice, um documento, um alias...",
    ),
    (
        "GET",
        "Reads: a search, a document, settings, the state of the cluster...",
        "Lê: uma busca, um documento, configurações, o estado do cluster...",
    ),
    (
        "HEAD",
        "Checks whether something exists, without returning the content.",
        "Verifica se algo existe, sem devolver o conteúdo.",
    ),
    (
        "PATCH",
        "Partially changes a resource (few APIs accept it).",
        "Altera parte de um recurso (poucas APIs aceitam).",
    ),
    (
        "POST",
        "Sends data: a search with a body, a new document, an action (_refresh, _reindex...).",
        "Envia dados: uma busca com corpo, um documento novo, uma ação (_refresh, _reindex...).",
    ),
    (
        "PUT",
        "Creates or replaces: an index, a document with an ID, a mapping, settings.",
        "Cria ou substitui: um índice, um documento com ID, um mapeamento, configurações.",
    ),
    // APIs.
    (
        "_aliases",
        "Adds and removes aliases of indices in a single atomic operation.",
        "Adiciona e remove aliases de índices em uma única operação atômica.",
    ),
    (
        "_alias",
        "Reads, creates or deletes an alias of the index.",
        "Lê, cria ou apaga um alias do índice.",
    ),
    ("_all", "All indices.", "Todos os índices."),
    (
        "_analyze",
        "Shows how an analyzer splits a text into terms.",
        "Mostra como um analisador quebra um texto em termos.",
    ),
    (
        "_bulk",
        "Runs many index, create, update and delete operations in one request (one JSON per line).",
        "Executa muitas operações de index, create, update e delete em uma requisição (um JSON por linha).",
    ),
    (
        "_cat",
        "Human-readable tables about the cluster (add ?v for the header).",
        "Tabelas fáceis de ler sobre o cluster (acrescente ?v para o cabeçalho).",
    ),
    (
        "_close",
        "Closes the index: it stops accepting reads and writes.",
        "Fecha o índice: ele deixa de aceitar leituras e gravações.",
    ),
    (
        "_cluster",
        "State, health and settings of the cluster.",
        "Estado, saúde e configurações do cluster.",
    ),
    (
        "_count",
        "Counts the documents that match a query.",
        "Conta os documentos que atendem a uma consulta.",
    ),
    (
        "_create",
        "Indexes a document only if the ID does not exist yet.",
        "Indexa um documento só se o ID ainda não existe.",
    ),
    (
        "_data_stream",
        "Lists, creates or deletes data streams.",
        "Lista, cria ou apaga data streams.",
    ),
    (
        "_delete_by_query",
        "Deletes all documents that match a query.",
        "Apaga todos os documentos que atendem a uma consulta.",
    ),
    (
        "_doc",
        "Reads, indexes or deletes one document by ID.",
        "Lê, indexa ou apaga um documento pelo ID.",
    ),
    (
        "_explain",
        "Explains why a document matches a query or not, and its score.",
        "Explica por que um documento atende ou não a uma consulta, e a pontuação dele.",
    ),
    (
        "_field_caps",
        "Shows the type of each field and whether it can be searched and aggregated.",
        "Mostra o tipo de cada campo e se ele pode ser buscado e agregado.",
    ),
    (
        "_forcemerge",
        "Merges the segments of the index to save space. Heavy: use on indices no longer written.",
        "Junta os segmentos do índice para ganhar espaço. Pesado: use em índices que não recebem mais gravações.",
    ),
    (
        "_index_template",
        "Templates applied to new indices by name pattern.",
        "Modelos aplicados a índices novos pelo padrão do nome.",
    ),
    (
        "_ingest",
        "Pipelines that transform documents before indexing.",
        "Pipelines que transformam os documentos antes de indexar.",
    ),
    (
        "_mapping",
        "Reads or changes the mapping: the fields of the index and their types.",
        "Lê ou altera o mapeamento: os campos do índice e os tipos deles.",
    ),
    (
        "_mget",
        "Reads several documents by ID in one request.",
        "Lê vários documentos pelo ID em uma requisição.",
    ),
    (
        "_msearch",
        "Runs several searches in one request.",
        "Executa várias buscas em uma requisição.",
    ),
    (
        "_nodes",
        "Information and statistics of the nodes of the cluster.",
        "Informações e estatísticas dos nós do cluster.",
    ),
    (
        "_open",
        "Reopens a closed index.",
        "Reabre um índice fechado.",
    ),
    (
        "_refresh",
        "Makes the recently indexed documents visible to searches now.",
        "Deixa os documentos recém-indexados visíveis para as buscas agora.",
    ),
    (
        "_reindex",
        "Copies documents from one index to another.",
        "Copia documentos de um índice para outro.",
    ),
    (
        "_rollover",
        "Creates a new index for the alias or data stream when the current one is too big or too old.",
        "Cria um índice novo para o alias ou data stream quando o atual está grande ou velho demais.",
    ),
    (
        "_search",
        "Searches the documents: query, sorting, pagination and aggregations.",
        "Busca os documentos: consulta, ordenação, paginação e agregações.",
    ),
    (
        "_security",
        "Users, roles and API keys.",
        "Usuários, roles e chaves de API.",
    ),
    (
        "_settings",
        "Reads or changes the settings of the index (replicas, refresh...).",
        "Lê ou altera as configurações do índice (réplicas, refresh...).",
    ),
    (
        "_snapshot",
        "Backup repositories and snapshots.",
        "Repositórios de backup e snapshots.",
    ),
    (
        "API:_source",
        "Returns only the original JSON of a document, without the metadata.",
        "Devolve só o JSON original de um documento, sem os metadados.",
    ),
    (
        "_sql",
        "Runs a query written in SQL.",
        "Executa uma consulta escrita em SQL.",
    ),
    (
        "_stats",
        "Statistics of the index (documents, size, searches, indexing).",
        "Estatísticas do índice (documentos, tamanho, buscas, indexação).",
    ),
    (
        "_tasks",
        "Lists the tasks running in the cluster.",
        "Lista as tarefas em execução no cluster.",
    ),
    (
        "_termvectors",
        "Shows the terms of a document and their statistics.",
        "Mostra os termos de um documento e as estatísticas deles.",
    ),
    (
        "_update",
        "Changes part of a document (doc) or runs a script on it.",
        "Altera parte de um documento (doc) ou executa um script nele.",
    ),
    (
        "_update_by_query",
        "Updates all documents that match a query.",
        "Atualiza todos os documentos que atendem a uma consulta.",
    ),
    (
        "_validate",
        "Checks whether a query is valid, without running it.",
        "Verifica se uma consulta é válida, sem executá-la.",
    ),
    // Partes do _cat.
    (
        "_cat:aliases",
        "Lists the aliases and the indices they point to.",
        "Lista os aliases e os índices para onde apontam.",
    ),
    (
        "_cat:allocation",
        "Shows the shards and the disk space of each node.",
        "Mostra os shards e o espaço em disco de cada nó.",
    ),
    (
        "_cat:count",
        "Number of documents of the cluster or of an index.",
        "Número de documentos do cluster ou de um índice.",
    ),
    (
        "_cat:health",
        "Health of the cluster (green, yellow, red).",
        "Saúde do cluster (green, yellow, red).",
    ),
    (
        "_cat:indices",
        "Lists the indices with health, documents and size.",
        "Lista os índices com saúde, documentos e tamanho.",
    ),
    (
        "_cat:master",
        "Shows which node is the master.",
        "Mostra qual nó é o mestre.",
    ),
    (
        "_cat:nodes",
        "Lists the nodes with memory, CPU and roles.",
        "Lista os nós com memória, CPU e papéis.",
    ),
    (
        "_cat:pending_tasks",
        "Cluster changes waiting to run.",
        "Alterações do cluster esperando para executar.",
    ),
    (
        "_cat:plugins",
        "Plugins installed on each node.",
        "Plugins instalados em cada nó.",
    ),
    (
        "_cat:recovery",
        "Shard recoveries in progress and finished.",
        "Recuperações de shards em andamento e concluídas.",
    ),
    (
        "_cat:repositories",
        "Snapshot repositories.",
        "Repositórios de snapshots.",
    ),
    (
        "_cat:segments",
        "Segments of the shards of the indices.",
        "Segmentos dos shards dos índices.",
    ),
    (
        "_cat:shards",
        "Lists the shards, their state and the node each one is on.",
        "Lista os shards, o estado deles e o nó em que cada um está.",
    ),
    (
        "_cat:snapshots",
        "Snapshots of a repository.",
        "Snapshots de um repositório.",
    ),
    (
        "_cat:tasks",
        "Tasks running in the cluster.",
        "Tarefas em execução no cluster.",
    ),
    ("_cat:templates", "Index templates.", "Modelos de índices."),
    (
        "_cat:thread_pool",
        "Thread pools of each node (active, queued, rejected).",
        "Pools de threads de cada nó (ativas, na fila, rejeitadas).",
    ),
    // Partes do _cluster.
    (
        "_cluster:allocation",
        "With /explain: tells why a shard is unassigned or where it is.",
        "Com /explain: diz por que um shard está sem nó ou onde ele está.",
    ),
    (
        "_cluster:health",
        "Health of the cluster (green, yellow, red) and shard counts.",
        "Saúde do cluster (green, yellow, red) e contagem de shards.",
    ),
    (
        "_cluster:pending_tasks",
        "Cluster changes waiting to run.",
        "Alterações do cluster esperando para executar.",
    ),
    (
        "_cluster:reroute",
        "Moves shards between nodes by hand, or retries failed allocations.",
        "Move shards entre nós à mão, ou tenta de novo alocações que falharam.",
    ),
    (
        "_cluster:settings",
        "Reads or changes the settings of the whole cluster.",
        "Lê ou altera as configurações do cluster inteiro.",
    ),
    (
        "_cluster:state",
        "The full internal state of the cluster (large).",
        "O estado interno completo do cluster (grande).",
    ),
    (
        "_cluster:stats",
        "Statistics of the whole cluster.",
        "Estatísticas do cluster inteiro.",
    ),
    (
        "_ingest:pipeline",
        "Reads, creates or deletes an ingest pipeline.",
        "Lê, cria ou apaga um pipeline de ingestão.",
    ),
    (
        "_nodes:hot_threads",
        "Shows the threads using the most CPU on each node.",
        "Mostra as threads que mais usam CPU em cada nó.",
    ),
    (
        "_nodes:stats",
        "Statistics of each node (JVM, disk, indexing, searches).",
        "Estatísticas de cada nó (JVM, disco, indexação, buscas).",
    ),
    (
        "_nodes:usage",
        "How many times each API was called on each node.",
        "Quantas vezes cada API foi chamada em cada nó.",
    ),
    (
        "_security:_authenticate",
        "Shows the user of this connection and its roles.",
        "Mostra o usuário desta conexão e as roles dele.",
    ),
    (
        "_security:api_key",
        "Creates, lists or invalidates API keys.",
        "Cria, lista ou invalida chaves de API.",
    ),
    (
        "_security:role",
        "Reads, creates or deletes roles.",
        "Lê, cria ou apaga roles.",
    ),
    (
        "_security:user",
        "Reads, creates or deletes users.",
        "Lê, cria ou apaga usuários.",
    ),
    (
        "_validate:query",
        "Checks whether a query is valid, without running it.",
        "Verifica se uma consulta é válida, sem executá-la.",
    ),
    // Parâmetros.
    (
        "expand_wildcards=",
        "Which indices a * matches: open, closed, hidden, all.",
        "Quais índices um * alcança: open, closed, hidden, all.",
    ),
    (
        "format=",
        "Format of the answer: json, yaml, text...",
        "Formato da resposta: json, yaml, text...",
    ),
    (
        "from=",
        "How many results to skip (pagination).",
        "Quantos resultados pular (paginação).",
    ),
    (
        "h=",
        "Columns of a _cat table, separated by commas.",
        "Colunas de uma tabela do _cat, separadas por vírgula.",
    ),
    (
        "ignore_unavailable=true",
        "Ignores indices that do not exist or are closed, instead of failing.",
        "Ignora índices que não existem ou estão fechados, em vez de dar erro.",
    ),
    (
        "pretty",
        "Formats the JSON of the answer with line breaks.",
        "Formata o JSON da resposta com quebras de linha.",
    ),
    (
        "q=",
        "A query in the Lucene syntax, in the URL (field:value).",
        "Uma consulta na sintaxe do Lucene, na URL (campo:valor).",
    ),
    (
        "refresh=true",
        "Makes the change visible to searches right away.",
        "Deixa a alteração visível para as buscas na hora.",
    ),
    (
        "routing=",
        "Value that chooses the shard of the document.",
        "Valor que escolhe o shard do documento.",
    ),
    (
        "s=",
        "Columns to sort a _cat table by.",
        "Colunas para ordenar uma tabela do _cat.",
    ),
    (
        "size=",
        "How many results to return.",
        "Quantos resultados devolver.",
    ),
    (
        "timeout=",
        "How long to wait for the operation (30s, 1m).",
        "Quanto tempo esperar pela operação (30s, 1m).",
    ),
    (
        "v",
        "Shows the header of a _cat table.",
        "Mostra o cabeçalho de uma tabela do _cat.",
    ),
    // Chaves do corpo de uma busca.
    (
        "_source",
        "Which fields of the original document come back (false returns none).",
        "Quais campos do documento original voltam (false não devolve nenhum).",
    ),
    (
        "aggs",
        "Aggregations: totals, averages and groups computed over the documents found.",
        "Agregações: totais, médias e grupos calculados sobre os documentos encontrados.",
    ),
    (
        "collapse",
        "Returns only the best document for each value of a field.",
        "Devolve só o melhor documento de cada valor de um campo.",
    ),
    (
        "explain",
        "Includes in each result how its score was computed.",
        "Inclui em cada resultado como a pontuação dele foi calculada.",
    ),
    ("fields", "List of fields.", "Lista de campos."),
    (
        "from",
        "How many results to skip (pagination).",
        "Quantos resultados pular (paginação).",
    ),
    (
        "highlight",
        "Marks in the text the excerpts that matched the search.",
        "Marca no texto os trechos que combinaram com a busca.",
    ),
    (
        "knn",
        "Search by vector similarity (nearest neighbors).",
        "Busca por semelhança de vetores (vizinhos mais próximos).",
    ),
    (
        "min_score",
        "Drops the results with a score below this value.",
        "Descarta os resultados com pontuação abaixo deste valor.",
    ),
    (
        "post_filter",
        "Filters the results after the aggregations are computed.",
        "Filtra os resultados depois de calcular as agregações.",
    ),
    (
        "profile",
        "Includes how long each part of the search took.",
        "Inclui quanto tempo cada parte da busca levou.",
    ),
    (
        "query",
        "The query: which documents to find.",
        "A consulta: quais documentos encontrar.",
    ),
    (
        "runtime_mappings",
        "Fields computed by a script only for this search.",
        "Campos calculados por um script só para esta busca.",
    ),
    (
        "script_fields",
        "Extra fields computed by a script for each result.",
        "Campos extras calculados por um script para cada resultado.",
    ),
    (
        "search_after",
        "Continues after the last result of the previous page (deep pagination).",
        "Continua depois do último resultado da página anterior (paginação profunda).",
    ),
    (
        "size",
        "How many results to return (default 10).",
        "Quantos resultados devolver (padrão 10).",
    ),
    ("sort", "Order of the results.", "Ordem dos resultados."),
    (
        "suggest",
        "Suggests corrections for misspelled terms.",
        "Sugere correções para termos escritos errado.",
    ),
    (
        "timeout",
        "How long to wait for the operation (30s, 1m).",
        "Quanto tempo esperar pela operação (30s, 1m).",
    ),
    (
        "track_total_hits",
        "Counts the exact total of results (true) instead of stopping at 10,000.",
        "Conta o total exato de resultados (true) em vez de parar em 10.000.",
    ),
    // Consultas.
    (
        "consulta:bool",
        "Combines queries with must, filter, should and must_not.",
        "Combina consultas com must, filter, should e must_not.",
    ),
    (
        "consulta:constant_score",
        "Wraps a filter and gives the same score to every document.",
        "Envolve um filtro e dá a mesma pontuação a todos os documentos.",
    ),
    (
        "consulta:dis_max",
        "Uses the best score among several queries.",
        "Usa a melhor pontuação entre várias consultas.",
    ),
    (
        "consulta:exists",
        "The field has a value.",
        "O campo tem valor.",
    ),
    (
        "consulta:function_score",
        "Changes the score of the documents with functions.",
        "Altera a pontuação dos documentos com funções.",
    ),
    (
        "consulta:fuzzy",
        "Terms similar to the one searched, tolerating typos.",
        "Termos parecidos com o buscado, tolerando erros de digitação.",
    ),
    (
        "consulta:geo_distance",
        "Points within a distance of a place.",
        "Pontos dentro de uma distância de um lugar.",
    ),
    (
        "consulta:ids",
        "Documents with the given IDs.",
        "Documentos com os IDs informados.",
    ),
    (
        "consulta:match",
        "Full-text search: analyzes the text and finds documents with its words.",
        "Busca de texto: analisa o texto e acha documentos com as palavras dele.",
    ),
    (
        "consulta:match_all",
        "All documents.",
        "Todos os documentos.",
    ),
    (
        "consulta:match_phrase",
        "The words in the same order, as a phrase.",
        "As palavras na mesma ordem, como uma frase.",
    ),
    (
        "consulta:match_phrase_prefix",
        "A phrase whose last word is only the beginning (search as you type).",
        "Uma frase cuja última palavra é só o começo (busca enquanto digita).",
    ),
    (
        "consulta:multi_match",
        "A match on several fields at once.",
        "Um match em vários campos de uma vez.",
    ),
    (
        "consulta:nested",
        "Searches inside the objects of a nested field.",
        "Busca dentro dos objetos de um campo nested.",
    ),
    (
        "consulta:prefix",
        "Terms that start with the given text.",
        "Termos que começam com o texto informado.",
    ),
    (
        "consulta:query_string",
        "A query in the Lucene syntax (AND, OR, field:value). Fails on a syntax error.",
        "Uma consulta na sintaxe do Lucene (AND, OR, campo:valor). Dá erro se a sintaxe estiver errada.",
    ),
    (
        "consulta:range",
        "Values in a range (gt, gte, lt, lte): numbers, dates.",
        "Valores em uma faixa (gt, gte, lt, lte): números, datas.",
    ),
    (
        "consulta:regexp",
        "Terms that match a regular expression.",
        "Termos que combinam com uma expressão regular.",
    ),
    (
        "consulta:simple_query_string",
        "A simpler query syntax that never fails: + AND, | OR, - NOT.",
        "Uma sintaxe de consulta mais simples que nunca dá erro: + E, | OU, - NÃO.",
    ),
    (
        "consulta:term",
        "The exact value, without analysis (use on keyword, numbers, dates).",
        "O valor exato, sem análise (use em keyword, números, datas).",
    ),
    (
        "consulta:terms",
        "Any of the exact values of a list.",
        "Qualquer um dos valores exatos de uma lista.",
    ),
    (
        "consulta:wildcard",
        "Terms that match a pattern with * and ?.",
        "Termos que combinam com um padrão com * e ?.",
    ),
    // Dentro de um bool.
    (
        "filter",
        "Conditions the documents must match, without affecting the score (and cached).",
        "Condições que os documentos precisam atender, sem afetar a pontuação (e com cache).",
    ),
    (
        "minimum_should_match",
        "How many of the should clauses must match.",
        "Quantas das cláusulas should precisam valer.",
    ),
    (
        "must",
        "Conditions the documents must match; they count for the score.",
        "Condições que os documentos precisam atender; contam para a pontuação.",
    ),
    (
        "must_not",
        "Conditions the documents must not match.",
        "Condições que os documentos não podem atender.",
    ),
    (
        "should",
        "Optional conditions: the ones that match raise the score.",
        "Condições opcionais: as que valem aumentam a pontuação.",
    ),
    // Dentro de um range.
    (
        "format",
        "Format of the dates (yyyy-MM-dd, epoch_millis...).",
        "Formato das datas (yyyy-MM-dd, epoch_millis...).",
    ),
    ("gt", "Greater than.", "Maior que."),
    ("gte", "Greater than or equal to.", "Maior ou igual a."),
    ("lt", "Less than.", "Menor que."),
    ("lte", "Less than or equal to.", "Menor ou igual a."),
    (
        "time_zone",
        "Time zone of the dates (-03:00, America/Sao_Paulo).",
        "Fuso horário das datas (-03:00, America/Sao_Paulo).",
    ),
    // Agregações.
    (
        "agregação:aggs",
        "Aggregations inside each group of this aggregation.",
        "Agregações dentro de cada grupo desta agregação.",
    ),
    ("agregação:avg", "Average of a field.", "Média de um campo."),
    (
        "agregação:cardinality",
        "Approximate number of distinct values of a field.",
        "Número aproximado de valores distintos de um campo.",
    ),
    (
        "agregação:composite",
        "Groups by several fields, with pagination over all groups.",
        "Agrupa por vários campos, com paginação por todos os grupos.",
    ),
    (
        "agregação:date_histogram",
        "Groups by time interval (day, month...).",
        "Agrupa por intervalo de tempo (dia, mês...).",
    ),
    (
        "agregação:date_range",
        "Groups by ranges of dates.",
        "Agrupa por faixas de datas.",
    ),
    (
        "agregação:extended_stats",
        "Stats plus variance and standard deviation.",
        "O stats mais variância e desvio padrão.",
    ),
    (
        "agregação:filter",
        "Only the documents that match a query.",
        "Só os documentos que atendem a uma consulta.",
    ),
    (
        "agregação:filters",
        "One group for each query.",
        "Um grupo para cada consulta.",
    ),
    (
        "agregação:histogram",
        "Groups by numeric interval.",
        "Agrupa por intervalo numérico.",
    ),
    (
        "agregação:max",
        "Largest value of a field.",
        "Maior valor de um campo.",
    ),
    (
        "agregação:min",
        "Smallest value of a field.",
        "Menor valor de um campo.",
    ),
    (
        "agregação:missing",
        "The documents without a value in the field.",
        "Os documentos sem valor no campo.",
    ),
    (
        "agregação:nested",
        "Aggregates the objects of a nested field.",
        "Agrega os objetos de um campo nested.",
    ),
    (
        "agregação:percentiles",
        "Percentiles of a field (median, p95, p99).",
        "Percentis de um campo (mediana, p95, p99).",
    ),
    (
        "agregação:range",
        "Groups by ranges of values.",
        "Agrupa por faixas de valores.",
    ),
    (
        "agregação:significant_terms",
        "Terms unusually frequent in the result compared to the whole index.",
        "Termos mais frequentes no resultado do que no índice inteiro.",
    ),
    (
        "agregação:stats",
        "Count, minimum, maximum, average and sum of a field.",
        "Contagem, mínimo, máximo, média e soma de um campo.",
    ),
    ("agregação:sum", "Sum of a field.", "Soma de um campo."),
    (
        "agregação:terms",
        "Groups by the values of a field, most frequent first (like GROUP BY).",
        "Agrupa pelos valores de um campo, do mais frequente para o menos (como um GROUP BY).",
    ),
    (
        "agregação:top_hits",
        "The best documents of each group.",
        "Os melhores documentos de cada grupo.",
    ),
    (
        "agregação:value_count",
        "Number of values of a field.",
        "Número de valores de um campo.",
    ),
    // Dentro de uma agregação.
    (
        "calendar_interval",
        "Calendar interval: minute, hour, day, week, month, quarter, year.",
        "Intervalo de calendário: minute, hour, day, week, month, quarter, year.",
    ),
    ("field", "The field to use.", "O campo a usar."),
    (
        "fixed_interval",
        "Interval of fixed length: 30s, 15m, 12h, 7d.",
        "Intervalo de duração fixa: 30s, 15m, 12h, 7d.",
    ),
    (
        "interval",
        "Size of each group of the histogram.",
        "Tamanho de cada grupo do histograma.",
    ),
    (
        "min_doc_count",
        "Hides the groups with fewer documents than this.",
        "Esconde os grupos com menos documentos que isto.",
    ),
    (
        "missing",
        "Value to use for the documents without the field.",
        "Valor a usar para os documentos sem o campo.",
    ),
    (
        "order",
        "Order of the groups (asc or desc).",
        "Ordem dos grupos (asc ou desc).",
    ),
    (
        "script",
        "A script (in Painless) that computes the value or changes the document.",
        "Um script (em Painless) que calcula o valor ou altera o documento.",
    ),
    // Mapeamento.
    (
        "analyzer",
        "The analyzer that splits the text into terms.",
        "O analisador que quebra o texto em termos.",
    ),
    (
        "copy_to",
        "Copies the value of the field to another field.",
        "Copia o valor do campo para outro campo.",
    ),
    (
        "ignore_above",
        "Does not index texts longer than this.",
        "Não indexa textos maiores que isto.",
    ),
    (
        "index",
        "The index (in a mapping: whether the field can be searched).",
        "O índice (em um mapeamento: se o campo pode ser buscado).",
    ),
    (
        "null_value",
        "Value indexed in place of null.",
        "Valor indexado no lugar de null.",
    ),
    (
        "properties",
        "The fields of the index or of an object, each with its type.",
        "Os campos do índice ou de um objeto, cada um com o tipo dele.",
    ),
    (
        "type",
        "The type (of a field in a mapping, or of a multi_match).",
        "O tipo (de um campo em um mapeamento, ou de um multi_match).",
    ),
    (
        "dynamic",
        "What to do with fields not in the mapping: true adds, false ignores, strict rejects.",
        "O que fazer com campos fora do mapeamento: true adiciona, false ignora, strict recusa.",
    ),
    (
        "runtime",
        "Fields computed by a script at search time.",
        "Campos calculados por um script na hora da busca.",
    ),
    // Tipos de campo.
    (
        "tipo:binary",
        "Binary data in Base64, not searchable.",
        "Dados binários em Base64, sem busca.",
    ),
    ("tipo:boolean", "true or false.", "true ou false."),
    (
        "tipo:byte",
        "Integer from -128 to 127.",
        "Inteiro de -128 a 127.",
    ),
    ("tipo:date", "Date and time.", "Data e hora."),
    (
        "tipo:dense_vector",
        "Vector of numbers, for similarity search (knn).",
        "Vetor de números, para busca por semelhança (knn).",
    ),
    (
        "tipo:double",
        "Double-precision decimal number.",
        "Número decimal de precisão dupla.",
    ),
    (
        "tipo:flattened",
        "A whole JSON object indexed as a single field.",
        "Um objeto JSON inteiro indexado como um campo só.",
    ),
    (
        "tipo:float",
        "Single-precision decimal number.",
        "Número decimal de precisão simples.",
    ),
    (
        "tipo:geo_point",
        "A point: latitude and longitude.",
        "Um ponto: latitude e longitude.",
    ),
    (
        "tipo:geo_shape",
        "A shape: line, polygon...",
        "Uma forma: linha, polígono...",
    ),
    ("tipo:integer", "32-bit integer.", "Inteiro de 32 bits."),
    ("tipo:ip", "IPv4 or IPv6 address.", "Endereço IPv4 ou IPv6."),
    (
        "tipo:keyword",
        "Exact text, not analyzed: for filters, sorting and aggregations (codes, status, e-mail).",
        "Texto exato, sem análise: para filtros, ordenação e agregações (códigos, status, e-mail).",
    ),
    ("tipo:long", "64-bit integer.", "Inteiro de 64 bits."),
    (
        "tipo:nested",
        "List of objects searched independently of each other.",
        "Lista de objetos buscados de forma independente uns dos outros.",
    ),
    (
        "tipo:object",
        "A JSON object with its own fields.",
        "Um objeto JSON com os próprios campos.",
    ),
    (
        "tipo:scaled_float",
        "Decimal stored as an integer with a scaling factor (good for prices).",
        "Decimal guardado como inteiro com um fator de escala (bom para preços).",
    ),
    (
        "tipo:short",
        "Integer from -32,768 to 32,767.",
        "Inteiro de -32.768 a 32.767.",
    ),
    (
        "tipo:text",
        "Analyzed text, for full-text search.",
        "Texto analisado, para busca de texto.",
    ),
    // Outras chaves do corpo.
    (
        "doc",
        "The fields to change in the document.",
        "Os campos a alterar no documento.",
    ),
    (
        "doc_as_upsert",
        "Uses doc as the new document when it does not exist.",
        "Usa o doc como documento novo quando ele não existe.",
    ),
    (
        "upsert",
        "The document to insert when it does not exist.",
        "O documento a inserir quando ele não existe.",
    ),
    (
        "conflicts",
        "What to do on a version conflict: abort or proceed.",
        "O que fazer em um conflito de versão: abort ou proceed.",
    ),
    (
        "dest",
        "The index that receives the documents.",
        "O índice que recebe os documentos.",
    ),
    (
        "source",
        "Where the documents come from (in a script: its code).",
        "De onde vêm os documentos (em um script: o código dele).",
    ),
    (
        "actions",
        "The list of alias changes to apply together.",
        "A lista de alterações de aliases a aplicar juntas.",
    ),
    (
        "create",
        "Bulk action: indexes a document only if the ID does not exist.",
        "Ação do bulk: indexa um documento só se o ID não existe.",
    ),
    (
        "delete",
        "Bulk action: deletes a document.",
        "Ação do bulk: apaga um documento.",
    ),
    (
        "update",
        "Bulk action: changes part of a document.",
        "Ação do bulk: altera parte de um documento.",
    ),
    (
        "fetch_size",
        "How many rows to return per page.",
        "Quantas linhas devolver por página.",
    ),
    (
        "params",
        "Values for the parameters of the query or script.",
        "Valores para os parâmetros da consulta ou do script.",
    ),
    ("text", "The text to analyze.", "O texto a analisar."),
    (
        "tokenizer",
        "The tokenizer that splits the text.",
        "O tokenizador que quebra o texto.",
    ),
    (
        "docs",
        "The documents to read, each with _index and _id.",
        "Os documentos a ler, cada um com _index e _id.",
    ),
    (
        "ids",
        "The IDs of the documents to read.",
        "Os IDs dos documentos a ler.",
    ),
    (
        "aliases",
        "The aliases of the new index.",
        "Os aliases do índice novo.",
    ),
    (
        "mappings",
        "The fields of the new index and their types.",
        "Os campos do índice novo e os tipos deles.",
    ),
    (
        "settings",
        "The settings of the index (shards, replicas, analysis).",
        "As configurações do índice (shards, réplicas, análise).",
    ),
    (
        "post_tags",
        "What to put after each highlighted excerpt (default </em>).",
        "O que pôr depois de cada trecho destacado (padrão </em>).",
    ),
    (
        "pre_tags",
        "What to put before each highlighted excerpt (default <em>).",
        "O que pôr antes de cada trecho destacado (padrão <em>).",
    ),
    (
        "analysis",
        "Custom analyzers, tokenizers and filters of the index.",
        "Analisadores, tokenizadores e filtros próprios do índice.",
    ),
    (
        "number_of_replicas",
        "How many copies of each shard (can be changed later).",
        "Quantas cópias de cada shard (pode mudar depois).",
    ),
    (
        "number_of_shards",
        "In how many shards the index is split (fixed at creation).",
        "Em quantos shards o índice é dividido (fixo na criação).",
    ),
    (
        "refresh_interval",
        "How often new documents become visible to searches (default 1s).",
        "De quanto em quanto tempo os documentos novos ficam visíveis para as buscas (padrão 1s).",
    ),
    (
        "lang",
        "The language of the script (default painless).",
        "A linguagem do script (padrão painless).",
    ),
    (
        "add",
        "Adds an alias to an index.",
        "Adiciona um alias a um índice.",
    ),
    (
        "remove",
        "Removes an alias from an index.",
        "Remove um alias de um índice.",
    ),
    ("remove_index", "Deletes an index.", "Apaga um índice."),
    ("alias", "The name of the alias.", "O nome do alias."),
    (
        "is_write_index",
        "Makes this index the one that receives the writes of the alias.",
        "Faz deste índice o que recebe as gravações do alias.",
    ),
    ("_id", "The ID of the document.", "O ID do documento."),
    (
        "_index",
        "The index of the document.",
        "O índice do documento.",
    ),
    (
        "path",
        "The path of the nested field.",
        "O caminho do campo nested.",
    ),
    (
        "default_operator",
        "Whether all words must match (and) or any of them (or).",
        "Se todas as palavras precisam combinar (and) ou qualquer uma (or).",
    ),
    // Valores.
    (
        "asc",
        "Ascending order: smallest first.",
        "Ordem crescente: do menor para o maior.",
    ),
    (
        "desc",
        "Descending order: largest first.",
        "Ordem decrescente: do maior para o menor.",
    ),
    (
        "and",
        "All words must match.",
        "Todas as palavras precisam combinar.",
    ),
    (
        "or",
        "Any of the words may match.",
        "Qualquer uma das palavras pode combinar.",
    ),
    ("day", "One group per day.", "Um grupo por dia."),
    ("hour", "One group per hour.", "Um grupo por hora."),
    ("minute", "One group per minute.", "Um grupo por minuto."),
    ("month", "One group per month.", "Um grupo por mês."),
    (
        "quarter",
        "One group per quarter.",
        "Um grupo por trimestre.",
    ),
    ("week", "One group per week.", "Um grupo por semana."),
    ("year", "One group per year.", "Um grupo por ano."),
    (
        "brazilian",
        "Analyzer for Brazilian Portuguese: removes common words and reduces words to their root.",
        "Analisador para o português do Brasil: tira palavras comuns e reduz as palavras à raiz.",
    ),
    (
        "english",
        "Analyzer for English: removes common words and reduces words to their root.",
        "Analisador para o inglês: tira palavras comuns e reduz as palavras à raiz.",
    ),
    (
        "keyword",
        "Analyzer that keeps the whole text as a single term.",
        "Analisador que mantém o texto inteiro como um termo só.",
    ),
    (
        "portuguese",
        "Analyzer for Portuguese: removes common words and reduces words to their root.",
        "Analisador para o português: tira palavras comuns e reduz as palavras à raiz.",
    ),
    (
        "simple",
        "Analyzer that splits at anything that is not a letter and lowercases.",
        "Analisador que quebra em tudo o que não é letra e passa para minúsculas.",
    ),
    (
        "standard",
        "The default analyzer: splits into words and lowercases.",
        "O analisador padrão: quebra em palavras e passa para minúsculas.",
    ),
    (
        "whitespace",
        "Analyzer that splits only at spaces.",
        "Analisador que quebra só nos espaços.",
    ),
];

//! O que cada comando e subcomando do Redis faz.

use super::Doc;

/// Comandos, pelo nome. Os subcomandos entram como `"COMANDO SUB"`.
pub const DOCS: &[Doc] = &[
    (
        "ACL",
        "Manages users and their permissions (access control lists).",
        "Gerencia os usuários e as permissões deles (listas de controle de acesso).",
    ),
    (
        "APPEND",
        "Appends a value to the end of a string key and returns the new length.",
        "Acrescenta um valor ao fim de uma chave de texto e devolve o novo tamanho.",
    ),
    (
        "AUTH",
        "Authenticates the connection with a password (and a user name, with ACLs).",
        "Autentica a conexão com uma senha (e um usuário, com ACLs).",
    ),
    (
        "BGREWRITEAOF",
        "Rewrites the append-only file in the background to make it smaller.",
        "Reescreve o arquivo AOF em segundo plano para deixá-lo menor.",
    ),
    (
        "BGSAVE",
        "Saves a snapshot of the data to disk in the background.",
        "Salva uma cópia dos dados em disco em segundo plano.",
    ),
    (
        "BITCOUNT",
        "Counts the bits set to 1 in a string.",
        "Conta os bits ligados (1) de um texto.",
    ),
    (
        "BITPOS",
        "Finds the position of the first bit set to 0 or 1 in a string.",
        "Acha a posição do primeiro bit 0 ou 1 de um texto.",
    ),
    (
        "BLPOP",
        "Removes and returns the first element of a list, waiting until one exists or the timeout expires.",
        "Tira e devolve o primeiro elemento de uma lista, esperando até existir um ou o tempo acabar.",
    ),
    (
        "BRPOP",
        "Removes and returns the last element of a list, waiting until one exists or the timeout expires.",
        "Tira e devolve o último elemento de uma lista, esperando até existir um ou o tempo acabar.",
    ),
    (
        "CLIENT",
        "Inspects and manages the connections of the clients.",
        "Consulta e gerencia as conexões dos clientes.",
    ),
    (
        "CLUSTER",
        "Shows information about a Redis Cluster and its nodes.",
        "Mostra informações de um Redis Cluster e dos nós dele.",
    ),
    (
        "COMMAND",
        "Describes the commands the server knows.",
        "Descreve os comandos que o servidor conhece.",
    ),
    (
        "CONFIG",
        "Reads and changes the configuration of the server.",
        "Lê e altera a configuração do servidor.",
    ),
    (
        "COPY",
        "Copies the value of a key to another key.",
        "Copia o valor de uma chave para outra chave.",
    ),
    (
        "DBSIZE",
        "Returns the number of keys in the current database.",
        "Devolve o número de chaves do banco atual.",
    ),
    (
        "DEBUG",
        "Debugging commands of the server. Do not use in production.",
        "Comandos de depuração do servidor. Não use em produção.",
    ),
    (
        "DECR",
        "Subtracts 1 from the number stored in a key.",
        "Subtrai 1 do número guardado em uma chave.",
    ),
    (
        "DECRBY",
        "Subtracts the given amount from the number stored in a key.",
        "Subtrai a quantidade informada do número guardado em uma chave.",
    ),
    (
        "DEL",
        "Deletes one or more keys. Returns how many were deleted.",
        "Apaga uma ou mais chaves. Devolve quantas foram apagadas.",
    ),
    (
        "DISCARD",
        "Cancels the transaction started with MULTI.",
        "Cancela a transação aberta com MULTI.",
    ),
    (
        "DUMP",
        "Returns the value of a key in the serialized format of Redis (for RESTORE).",
        "Devolve o valor de uma chave no formato serializado do Redis (para o RESTORE).",
    ),
    (
        "ECHO",
        "Returns the given message.",
        "Devolve a mensagem informada.",
    ),
    (
        "EVAL",
        "Runs a Lua script on the server.",
        "Executa um script Lua no servidor.",
    ),
    (
        "EVALSHA",
        "Runs a Lua script already loaded on the server, by its SHA1 hash.",
        "Executa um script Lua já carregado no servidor, pelo hash SHA1 dele.",
    ),
    (
        "EXEC",
        "Runs all commands queued since MULTI.",
        "Executa todos os comandos enfileirados desde o MULTI.",
    ),
    (
        "EXISTS",
        "Tells how many of the given keys exist.",
        "Diz quantas das chaves informadas existem.",
    ),
    (
        "EXPIRE",
        "Sets the time to live of a key, in seconds.",
        "Define o tempo de vida de uma chave, em segundos.",
    ),
    (
        "EXPIREAT",
        "Sets the moment a key expires, as a Unix timestamp in seconds.",
        "Define o momento em que uma chave expira, como um timestamp Unix em segundos.",
    ),
    (
        "EXPIRETIME",
        "Returns the moment a key expires, as a Unix timestamp in seconds.",
        "Devolve o momento em que uma chave expira, como um timestamp Unix em segundos.",
    ),
    (
        "FCALL",
        "Calls a function loaded with FUNCTION LOAD.",
        "Chama uma função carregada com FUNCTION LOAD.",
    ),
    (
        "FLUSHALL",
        "Deletes all keys of ALL databases. Cannot be undone.",
        "Apaga todas as chaves de TODOS os bancos. Não dá para desfazer.",
    ),
    (
        "FLUSHDB",
        "Deletes all keys of the current database. Cannot be undone.",
        "Apaga todas as chaves do banco atual. Não dá para desfazer.",
    ),
    (
        "FUNCTION",
        "Manages the function libraries loaded on the server.",
        "Gerencia as bibliotecas de funções carregadas no servidor.",
    ),
    (
        "GEOADD",
        "Adds places (longitude, latitude, name) to a geospatial index.",
        "Adiciona lugares (longitude, latitude, nome) a um índice geoespacial.",
    ),
    (
        "GEODIST",
        "Returns the distance between two places of a geospatial index.",
        "Devolve a distância entre dois lugares de um índice geoespacial.",
    ),
    (
        "GEOPOS",
        "Returns the longitude and latitude of places of a geospatial index.",
        "Devolve a longitude e a latitude de lugares de um índice geoespacial.",
    ),
    (
        "GEOSEARCH",
        "Finds the places inside a radius or a box of a geospatial index.",
        "Acha os lugares dentro de um raio ou de um retângulo de um índice geoespacial.",
    ),
    (
        "GET",
        "Returns the string value of a key, or nil when it does not exist.",
        "Devolve o valor de texto de uma chave, ou nil quando ela não existe.",
    ),
    (
        "GETBIT",
        "Returns the bit at a position of a string.",
        "Devolve o bit de uma posição de um texto.",
    ),
    (
        "GETDEL",
        "Returns the value of a key and deletes it.",
        "Devolve o valor de uma chave e a apaga.",
    ),
    (
        "GETEX",
        "Returns the value of a key and changes its expiration.",
        "Devolve o valor de uma chave e muda a expiração dela.",
    ),
    (
        "GETRANGE",
        "Returns a part of a string, between two positions.",
        "Devolve um trecho de um texto, entre duas posições.",
    ),
    (
        "GETSET",
        "Sets a new value and returns the old one. Deprecated: use SET with GET.",
        "Grava um valor novo e devolve o antigo. Obsoleto: use SET com GET.",
    ),
    (
        "HDEL",
        "Deletes fields of a hash.",
        "Apaga campos de um hash.",
    ),
    (
        "HELLO",
        "Starts the conversation with the server and chooses the protocol version (RESP2 or RESP3).",
        "Inicia a conversa com o servidor e escolhe a versão do protocolo (RESP2 ou RESP3).",
    ),
    (
        "HEXISTS",
        "Tells whether a field exists in a hash.",
        "Diz se um campo existe em um hash.",
    ),
    (
        "HGET",
        "Returns the value of a field of a hash.",
        "Devolve o valor de um campo de um hash.",
    ),
    (
        "HGETALL",
        "Returns all fields and values of a hash.",
        "Devolve todos os campos e valores de um hash.",
    ),
    (
        "HINCRBY",
        "Adds an integer to the number in a field of a hash.",
        "Soma um inteiro ao número de um campo de um hash.",
    ),
    (
        "HINCRBYFLOAT",
        "Adds a decimal number to the number in a field of a hash.",
        "Soma um número decimal ao número de um campo de um hash.",
    ),
    (
        "HKEYS",
        "Returns the names of all fields of a hash.",
        "Devolve os nomes de todos os campos de um hash.",
    ),
    (
        "HLEN",
        "Returns the number of fields of a hash.",
        "Devolve o número de campos de um hash.",
    ),
    (
        "HMGET",
        "Returns the values of several fields of a hash.",
        "Devolve os valores de vários campos de um hash.",
    ),
    (
        "HMSET",
        "Sets several fields of a hash. Deprecated: use HSET.",
        "Grava vários campos de um hash. Obsoleto: use HSET.",
    ),
    (
        "HRANDFIELD",
        "Returns random fields of a hash.",
        "Devolve campos aleatórios de um hash.",
    ),
    (
        "HSCAN",
        "Walks through the fields of a hash a few at a time, without blocking the server.",
        "Percorre os campos de um hash aos poucos, sem travar o servidor.",
    ),
    (
        "HSET",
        "Sets one or more fields of a hash, creating it if needed.",
        "Grava um ou mais campos de um hash, criando o hash se preciso.",
    ),
    (
        "HSETNX",
        "Sets a field of a hash only if the field does not exist yet.",
        "Grava um campo de um hash só se o campo ainda não existe.",
    ),
    (
        "HSTRLEN",
        "Returns the length of the value of a field of a hash.",
        "Devolve o tamanho do valor de um campo de um hash.",
    ),
    (
        "HVALS",
        "Returns all values of a hash.",
        "Devolve todos os valores de um hash.",
    ),
    (
        "INCR",
        "Adds 1 to the number stored in a key.",
        "Soma 1 ao número guardado em uma chave.",
    ),
    (
        "INCRBY",
        "Adds the given amount to the number stored in a key.",
        "Soma a quantidade informada ao número guardado em uma chave.",
    ),
    (
        "INCRBYFLOAT",
        "Adds a decimal number to the number stored in a key.",
        "Soma um número decimal ao número guardado em uma chave.",
    ),
    (
        "INFO",
        "Shows information and statistics of the server (memory, clients, replication...).",
        "Mostra informações e estatísticas do servidor (memória, clientes, replicação...).",
    ),
    (
        "KEYS",
        "Lists all keys that match a pattern. Slow on large databases: prefer SCAN.",
        "Lista todas as chaves que combinam com um padrão. Lento em bancos grandes: prefira SCAN.",
    ),
    (
        "LASTSAVE",
        "Returns when the data was last saved to disk, as a Unix timestamp.",
        "Devolve quando os dados foram salvos em disco pela última vez, como um timestamp Unix.",
    ),
    (
        "LATENCY",
        "Shows the latency spikes the server recorded.",
        "Mostra os picos de latência que o servidor registrou.",
    ),
    (
        "LINDEX",
        "Returns the element at a position of a list.",
        "Devolve o elemento de uma posição de uma lista.",
    ),
    (
        "LINSERT",
        "Inserts an element in a list before or after another element.",
        "Insere um elemento em uma lista antes ou depois de outro elemento.",
    ),
    (
        "LLEN",
        "Returns the number of elements of a list.",
        "Devolve o número de elementos de uma lista.",
    ),
    (
        "LMOVE",
        "Moves an element from one end of a list to an end of another list.",
        "Move um elemento de uma ponta de uma lista para uma ponta de outra lista.",
    ),
    (
        "LPOP",
        "Removes and returns the first elements of a list.",
        "Tira e devolve os primeiros elementos de uma lista.",
    ),
    (
        "LPOS",
        "Returns the position of an element in a list.",
        "Devolve a posição de um elemento em uma lista.",
    ),
    (
        "LPUSH",
        "Adds elements to the start of a list, creating it if needed.",
        "Adiciona elementos ao começo de uma lista, criando a lista se preciso.",
    ),
    (
        "LPUSHX",
        "Adds elements to the start of a list only if the list exists.",
        "Adiciona elementos ao começo de uma lista só se a lista existe.",
    ),
    (
        "LRANGE",
        "Returns the elements of a list between two positions (0 -1 returns all).",
        "Devolve os elementos de uma lista entre duas posições (0 -1 devolve todos).",
    ),
    (
        "LREM",
        "Removes the elements of a list equal to a value.",
        "Remove os elementos de uma lista iguais a um valor.",
    ),
    (
        "LSET",
        "Replaces the element at a position of a list.",
        "Troca o elemento de uma posição de uma lista.",
    ),
    (
        "LTRIM",
        "Keeps only the elements of a list between two positions.",
        "Mantém só os elementos de uma lista entre duas posições.",
    ),
    (
        "MEMORY",
        "Shows how the server and the keys use memory.",
        "Mostra como o servidor e as chaves usam a memória.",
    ),
    (
        "MGET",
        "Returns the values of several keys at once.",
        "Devolve os valores de várias chaves de uma vez.",
    ),
    (
        "MODULE",
        "Manages the modules loaded on the server.",
        "Gerencia os módulos carregados no servidor.",
    ),
    (
        "MONITOR",
        "Shows every command the server receives, in real time. Heavy for the server.",
        "Mostra todo comando que o servidor recebe, em tempo real. Pesa no servidor.",
    ),
    (
        "MOVE",
        "Moves a key to another database.",
        "Move uma chave para outro banco.",
    ),
    (
        "MSET",
        "Sets several keys at once.",
        "Grava várias chaves de uma vez.",
    ),
    (
        "MSETNX",
        "Sets several keys at once, only if none of them exists.",
        "Grava várias chaves de uma vez, só se nenhuma delas existe.",
    ),
    (
        "MULTI",
        "Starts a transaction: the next commands are queued until EXEC.",
        "Abre uma transação: os próximos comandos ficam na fila até o EXEC.",
    ),
    (
        "OBJECT",
        "Shows internal details of a key (encoding, idle time...).",
        "Mostra detalhes internos de uma chave (codificação, tempo parada...).",
    ),
    (
        "PERSIST",
        "Removes the expiration of a key, so it lasts forever.",
        "Tira a expiração de uma chave, que passa a durar para sempre.",
    ),
    (
        "PEXPIRE",
        "Sets the time to live of a key, in milliseconds.",
        "Define o tempo de vida de uma chave, em milissegundos.",
    ),
    (
        "PFADD",
        "Adds elements to a HyperLogLog (approximate count of distinct values).",
        "Adiciona elementos a um HyperLogLog (contagem aproximada de valores distintos).",
    ),
    (
        "PFCOUNT",
        "Returns the approximate number of distinct elements of a HyperLogLog.",
        "Devolve o número aproximado de elementos distintos de um HyperLogLog.",
    ),
    (
        "PFMERGE",
        "Merges several HyperLogLogs into one.",
        "Junta vários HyperLogLogs em um só.",
    ),
    (
        "PING",
        "Tests the connection: the server answers PONG.",
        "Testa a conexão: o servidor responde PONG.",
    ),
    (
        "PSETEX",
        "Sets a value with a time to live in milliseconds. Deprecated: use SET with PX.",
        "Grava um valor com tempo de vida em milissegundos. Obsoleto: use SET com PX.",
    ),
    (
        "PSUBSCRIBE",
        "Listens to the channels whose names match a pattern.",
        "Escuta os canais cujos nomes combinam com um padrão.",
    ),
    (
        "PTTL",
        "Returns the remaining time to live of a key, in milliseconds.",
        "Devolve o tempo de vida que resta de uma chave, em milissegundos.",
    ),
    (
        "PUBLISH",
        "Sends a message to a channel.",
        "Envia uma mensagem para um canal.",
    ),
    (
        "PUBSUB",
        "Shows the state of the channels (active channels, number of subscribers).",
        "Mostra o estado dos canais (canais ativos, número de assinantes).",
    ),
    ("QUIT", "Closes the connection.", "Fecha a conexão."),
    (
        "RANDOMKEY",
        "Returns a random key of the database.",
        "Devolve uma chave aleatória do banco.",
    ),
    (
        "RENAME",
        "Renames a key, replacing the destination if it exists.",
        "Renomeia uma chave, substituindo o destino se ele existe.",
    ),
    (
        "RENAMENX",
        "Renames a key only if the new name does not exist.",
        "Renomeia uma chave só se o novo nome não existe.",
    ),
    (
        "REPLICAOF",
        "Makes the server a replica of another one, or a primary again (NO ONE).",
        "Faz o servidor virar réplica de outro, ou voltar a ser primário (NO ONE).",
    ),
    (
        "ROLE",
        "Tells whether the server is a primary or a replica.",
        "Diz se o servidor é primário ou réplica.",
    ),
    (
        "RPOP",
        "Removes and returns the last elements of a list.",
        "Tira e devolve os últimos elementos de uma lista.",
    ),
    (
        "RPUSH",
        "Adds elements to the end of a list, creating it if needed.",
        "Adiciona elementos ao fim de uma lista, criando a lista se preciso.",
    ),
    (
        "RPUSHX",
        "Adds elements to the end of a list only if the list exists.",
        "Adiciona elementos ao fim de uma lista só se a lista existe.",
    ),
    (
        "SADD",
        "Adds members to a set, creating it if needed.",
        "Adiciona membros a um conjunto, criando o conjunto se preciso.",
    ),
    (
        "SAVE",
        "Saves the data to disk and blocks the server until it finishes. Prefer BGSAVE.",
        "Salva os dados em disco e trava o servidor até terminar. Prefira BGSAVE.",
    ),
    (
        "SCAN",
        "Walks through the keys of the database a few at a time, without blocking the server.",
        "Percorre as chaves do banco aos poucos, sem travar o servidor.",
    ),
    (
        "SCARD",
        "Returns the number of members of a set.",
        "Devolve o número de membros de um conjunto.",
    ),
    (
        "SCRIPT",
        "Manages the Lua scripts loaded on the server.",
        "Gerencia os scripts Lua carregados no servidor.",
    ),
    (
        "SDIFF",
        "Returns the members of the first set that are not in the others.",
        "Devolve os membros do primeiro conjunto que não estão nos outros.",
    ),
    (
        "SDIFFSTORE",
        "Stores in a key the difference between sets.",
        "Guarda em uma chave a diferença entre conjuntos.",
    ),
    (
        "SELECT",
        "Switches to another database, by its number.",
        "Troca para outro banco, pelo número dele.",
    ),
    (
        "SET",
        "Sets the string value of a key. Options: EX/PX (expiration), NX (only if new), XX (only if exists).",
        "Grava o valor de texto de uma chave. Opções: EX/PX (expiração), NX (só se for nova), XX (só se existe).",
    ),
    (
        "SETBIT",
        "Sets or clears the bit at a position of a string.",
        "Liga ou desliga o bit de uma posição de um texto.",
    ),
    (
        "SETEX",
        "Sets a value with a time to live in seconds. Deprecated: use SET with EX.",
        "Grava um valor com tempo de vida em segundos. Obsoleto: use SET com EX.",
    ),
    (
        "SETNX",
        "Sets a value only if the key does not exist. Deprecated: use SET with NX.",
        "Grava um valor só se a chave não existe. Obsoleto: use SET com NX.",
    ),
    (
        "SETRANGE",
        "Overwrites part of a string, starting at a position.",
        "Sobrescreve um trecho de um texto, a partir de uma posição.",
    ),
    (
        "SINTER",
        "Returns the members that are in all the given sets.",
        "Devolve os membros que estão em todos os conjuntos informados.",
    ),
    (
        "SINTERSTORE",
        "Stores in a key the intersection of sets.",
        "Guarda em uma chave a interseção de conjuntos.",
    ),
    (
        "SISMEMBER",
        "Tells whether a value is a member of a set.",
        "Diz se um valor é membro de um conjunto.",
    ),
    (
        "SLOWLOG",
        "Shows the commands that took longer than the configured limit.",
        "Mostra os comandos que demoraram mais que o limite configurado.",
    ),
    (
        "SMEMBERS",
        "Returns all members of a set.",
        "Devolve todos os membros de um conjunto.",
    ),
    (
        "SMISMEMBER",
        "Tells, for each value, whether it is a member of a set.",
        "Diz, para cada valor, se ele é membro de um conjunto.",
    ),
    (
        "SMOVE",
        "Moves a member from one set to another.",
        "Move um membro de um conjunto para outro.",
    ),
    (
        "SORT",
        "Returns the elements of a list, set or sorted set in order.",
        "Devolve os elementos de uma lista, conjunto ou conjunto ordenado em ordem.",
    ),
    (
        "SPOP",
        "Removes and returns random members of a set.",
        "Tira e devolve membros aleatórios de um conjunto.",
    ),
    (
        "SRANDMEMBER",
        "Returns random members of a set, without removing them.",
        "Devolve membros aleatórios de um conjunto, sem tirá-los.",
    ),
    (
        "SREM",
        "Removes members from a set.",
        "Remove membros de um conjunto.",
    ),
    (
        "SSCAN",
        "Walks through the members of a set a few at a time, without blocking the server.",
        "Percorre os membros de um conjunto aos poucos, sem travar o servidor.",
    ),
    (
        "STRLEN",
        "Returns the length of the string value of a key.",
        "Devolve o tamanho do valor de texto de uma chave.",
    ),
    (
        "SUBSCRIBE",
        "Listens to the messages of one or more channels.",
        "Escuta as mensagens de um ou mais canais.",
    ),
    (
        "SUNION",
        "Returns the members of all the given sets together.",
        "Devolve os membros de todos os conjuntos informados juntos.",
    ),
    (
        "SUNIONSTORE",
        "Stores in a key the union of sets.",
        "Guarda em uma chave a união de conjuntos.",
    ),
    (
        "SWAPDB",
        "Swaps two databases: each one gets the keys of the other.",
        "Troca dois bancos: cada um fica com as chaves do outro.",
    ),
    (
        "TIME",
        "Returns the current time of the server.",
        "Devolve a hora atual do servidor.",
    ),
    (
        "TOUCH",
        "Updates the last access time of keys.",
        "Atualiza a hora do último acesso de chaves.",
    ),
    (
        "TTL",
        "Returns the remaining time to live of a key, in seconds (-1: no expiration, -2: no key).",
        "Devolve o tempo de vida que resta de uma chave, em segundos (-1: sem expiração, -2: não existe).",
    ),
    (
        "TYPE",
        "Returns the type of the value of a key (string, list, set, zset, hash, stream).",
        "Devolve o tipo do valor de uma chave (string, list, set, zset, hash, stream).",
    ),
    (
        "UNLINK",
        "Deletes keys like DEL, but frees the memory in the background.",
        "Apaga chaves como o DEL, mas libera a memória em segundo plano.",
    ),
    (
        "UNWATCH",
        "Stops watching all keys watched with WATCH.",
        "Deixa de vigiar todas as chaves vigiadas com WATCH.",
    ),
    (
        "WAIT",
        "Waits until the previous writes reach the given number of replicas.",
        "Espera as gravações anteriores chegarem ao número de réplicas informado.",
    ),
    (
        "WATCH",
        "Watches keys: the transaction (MULTI/EXEC) fails if one of them changes.",
        "Vigia chaves: a transação (MULTI/EXEC) falha se uma delas mudar.",
    ),
    (
        "XACK",
        "Confirms that a consumer group processed messages of a stream.",
        "Confirma que um grupo de consumidores processou mensagens de um stream.",
    ),
    (
        "XADD",
        "Adds an entry to a stream (use * to generate the ID).",
        "Adiciona uma entrada a um stream (use * para gerar o ID).",
    ),
    (
        "XDEL",
        "Deletes entries of a stream by ID.",
        "Apaga entradas de um stream pelo ID.",
    ),
    (
        "XGROUP",
        "Manages the consumer groups of a stream.",
        "Gerencia os grupos de consumidores de um stream.",
    ),
    (
        "XINFO",
        "Shows information about a stream, its groups and consumers.",
        "Mostra informações de um stream, dos grupos e dos consumidores dele.",
    ),
    (
        "XLEN",
        "Returns the number of entries of a stream.",
        "Devolve o número de entradas de um stream.",
    ),
    (
        "XPENDING",
        "Lists the messages of a consumer group delivered but not confirmed yet.",
        "Lista as mensagens de um grupo de consumidores entregues mas ainda não confirmadas.",
    ),
    (
        "XRANGE",
        "Returns the entries of a stream between two IDs (- + returns all).",
        "Devolve as entradas de um stream entre dois IDs (- + devolve todas).",
    ),
    (
        "XREAD",
        "Reads new entries of one or more streams, optionally waiting for them.",
        "Lê entradas novas de um ou mais streams, podendo esperar por elas.",
    ),
    (
        "XREADGROUP",
        "Reads entries of a stream as a consumer of a group.",
        "Lê entradas de um stream como um consumidor de um grupo.",
    ),
    (
        "XREVRANGE",
        "Returns the entries of a stream between two IDs, newest first.",
        "Devolve as entradas de um stream entre dois IDs, da mais nova para a mais antiga.",
    ),
    (
        "XTRIM",
        "Shortens a stream by removing the oldest entries.",
        "Encurta um stream tirando as entradas mais antigas.",
    ),
    (
        "ZADD",
        "Adds members with a score to a sorted set, or updates their scores.",
        "Adiciona membros com uma pontuação a um conjunto ordenado, ou atualiza a pontuação deles.",
    ),
    (
        "ZCARD",
        "Returns the number of members of a sorted set.",
        "Devolve o número de membros de um conjunto ordenado.",
    ),
    (
        "ZCOUNT",
        "Counts the members of a sorted set with a score between two values.",
        "Conta os membros de um conjunto ordenado com pontuação entre dois valores.",
    ),
    (
        "ZINCRBY",
        "Adds an amount to the score of a member of a sorted set.",
        "Soma uma quantidade à pontuação de um membro de um conjunto ordenado.",
    ),
    (
        "ZMSCORE",
        "Returns the scores of several members of a sorted set.",
        "Devolve as pontuações de vários membros de um conjunto ordenado.",
    ),
    (
        "ZPOPMAX",
        "Removes and returns the members with the highest scores of a sorted set.",
        "Tira e devolve os membros de maior pontuação de um conjunto ordenado.",
    ),
    (
        "ZPOPMIN",
        "Removes and returns the members with the lowest scores of a sorted set.",
        "Tira e devolve os membros de menor pontuação de um conjunto ordenado.",
    ),
    (
        "ZRANDMEMBER",
        "Returns random members of a sorted set.",
        "Devolve membros aleatórios de um conjunto ordenado.",
    ),
    (
        "ZRANGE",
        "Returns the members of a sorted set between two positions, lowest score first.",
        "Devolve os membros de um conjunto ordenado entre duas posições, da menor pontuação para a maior.",
    ),
    (
        "ZRANGEBYSCORE",
        "Returns the members of a sorted set with a score between two values. Deprecated: use ZRANGE with BYSCORE.",
        "Devolve os membros de um conjunto ordenado com pontuação entre dois valores. Obsoleto: use ZRANGE com BYSCORE.",
    ),
    (
        "ZRANK",
        "Returns the position of a member in a sorted set, lowest score first.",
        "Devolve a posição de um membro em um conjunto ordenado, da menor pontuação para a maior.",
    ),
    (
        "ZREM",
        "Removes members from a sorted set.",
        "Remove membros de um conjunto ordenado.",
    ),
    (
        "ZREMRANGEBYRANK",
        "Removes the members of a sorted set between two positions.",
        "Remove os membros de um conjunto ordenado entre duas posições.",
    ),
    (
        "ZREMRANGEBYSCORE",
        "Removes the members of a sorted set with a score between two values.",
        "Remove os membros de um conjunto ordenado com pontuação entre dois valores.",
    ),
    (
        "ZREVRANGE",
        "Returns the members of a sorted set between two positions, highest score first. Deprecated: use ZRANGE with REV.",
        "Devolve os membros de um conjunto ordenado entre duas posições, da maior pontuação para a menor. Obsoleto: use ZRANGE com REV.",
    ),
    (
        "ZREVRANGEBYSCORE",
        "Returns the members of a sorted set with a score between two values, highest first. Deprecated: use ZRANGE with BYSCORE REV.",
        "Devolve os membros de um conjunto ordenado com pontuação entre dois valores, da maior para a menor. Obsoleto: use ZRANGE com BYSCORE REV.",
    ),
    (
        "ZREVRANK",
        "Returns the position of a member in a sorted set, highest score first.",
        "Devolve a posição de um membro em um conjunto ordenado, da maior pontuação para a menor.",
    ),
    (
        "ZSCAN",
        "Walks through the members of a sorted set a few at a time, without blocking the server.",
        "Percorre os membros de um conjunto ordenado aos poucos, sem travar o servidor.",
    ),
    (
        "ZSCORE",
        "Returns the score of a member of a sorted set.",
        "Devolve a pontuação de um membro de um conjunto ordenado.",
    ),
    (
        "JSON.DEL",
        "Deletes a JSON document, or the part of it at a path (RedisJSON).",
        "Apaga um documento JSON, ou a parte dele em um caminho (RedisJSON).",
    ),
    (
        "JSON.GET",
        "Returns a JSON document, or the parts of it at the given paths (RedisJSON).",
        "Devolve um documento JSON, ou as partes dele nos caminhos informados (RedisJSON).",
    ),
    (
        "JSON.MGET",
        "Returns the value at a path of several JSON documents (RedisJSON).",
        "Devolve o valor de um caminho de vários documentos JSON (RedisJSON).",
    ),
    (
        "JSON.SET",
        "Sets a JSON document, or the value at a path of it (RedisJSON).",
        "Grava um documento JSON, ou o valor de um caminho dele (RedisJSON).",
    ),
    (
        "JSON.TYPE",
        "Returns the type of the JSON value at a path (RedisJSON).",
        "Devolve o tipo do valor JSON de um caminho (RedisJSON).",
    ),
    // Subcomandos.
    (
        "ACL LIST",
        "Lists the users with their rules, in the format of the ACL file.",
        "Lista os usuários com as regras deles, no formato do arquivo de ACL.",
    ),
    (
        "ACL USERS",
        "Lists the names of the users.",
        "Lista os nomes dos usuários.",
    ),
    (
        "ACL WHOAMI",
        "Returns the user of this connection.",
        "Devolve o usuário desta conexão.",
    ),
    (
        "ACL GETUSER",
        "Shows the rules of a user.",
        "Mostra as regras de um usuário.",
    ),
    (
        "ACL SETUSER",
        "Creates a user or changes its rules.",
        "Cria um usuário ou altera as regras dele.",
    ),
    (
        "ACL DELUSER",
        "Deletes users and closes their connections.",
        "Apaga usuários e fecha as conexões deles.",
    ),
    (
        "ACL CAT",
        "Lists the command categories, or the commands of a category.",
        "Lista as categorias de comandos, ou os comandos de uma categoria.",
    ),
    (
        "ACL LOG",
        "Shows the recent commands and logins denied by the ACLs.",
        "Mostra os comandos e logins negados pelas ACLs recentemente.",
    ),
    (
        "ACL GENPASS",
        "Generates a strong random password.",
        "Gera uma senha aleatória forte.",
    ),
    (
        "ACL LOAD",
        "Reloads the users from the ACL file.",
        "Recarrega os usuários do arquivo de ACL.",
    ),
    (
        "ACL SAVE",
        "Saves the current users to the ACL file.",
        "Salva os usuários atuais no arquivo de ACL.",
    ),
    (
        "CLIENT LIST",
        "Lists the open connections.",
        "Lista as conexões abertas.",
    ),
    (
        "CLIENT INFO",
        "Shows information about this connection.",
        "Mostra informações desta conexão.",
    ),
    (
        "CLIENT ID",
        "Returns the ID of this connection.",
        "Devolve o ID desta conexão.",
    ),
    (
        "CLIENT GETNAME",
        "Returns the name of this connection.",
        "Devolve o nome desta conexão.",
    ),
    (
        "CLIENT SETNAME",
        "Gives a name to this connection (shown in CLIENT LIST).",
        "Dá um nome a esta conexão (aparece no CLIENT LIST).",
    ),
    (
        "CLIENT KILL",
        "Closes the connection of a client.",
        "Fecha a conexão de um cliente.",
    ),
    (
        "CLIENT PAUSE",
        "Suspends the commands of all clients for some time.",
        "Suspende os comandos de todos os clientes por um tempo.",
    ),
    (
        "CLIENT UNPAUSE",
        "Resumes the clients suspended by CLIENT PAUSE.",
        "Retoma os clientes suspensos pelo CLIENT PAUSE.",
    ),
    (
        "CLIENT NO-EVICT",
        "Tells whether this connection can be closed when client memory runs out.",
        "Diz se esta conexão pode ser fechada quando a memória dos clientes acabar.",
    ),
    (
        "CLIENT TRACKING",
        "Turns on notices of changed keys, for client-side caching.",
        "Liga os avisos de chaves alteradas, para cache no cliente.",
    ),
    (
        "CLUSTER INFO",
        "Shows the state of the cluster.",
        "Mostra o estado do cluster.",
    ),
    (
        "CLUSTER NODES",
        "Lists the nodes of the cluster and their slots.",
        "Lista os nós do cluster e os slots deles.",
    ),
    (
        "CLUSTER SLOTS",
        "Shows which node serves each range of slots. Deprecated: use CLUSTER SHARDS.",
        "Mostra qual nó atende cada faixa de slots. Obsoleto: use CLUSTER SHARDS.",
    ),
    (
        "CLUSTER SHARDS",
        "Shows the shards of the cluster with their slots and nodes.",
        "Mostra os shards do cluster com os slots e os nós deles.",
    ),
    (
        "CLUSTER KEYSLOT",
        "Returns the slot a key belongs to.",
        "Devolve o slot a que uma chave pertence.",
    ),
    (
        "CLUSTER MYID",
        "Returns the ID of this node.",
        "Devolve o ID deste nó.",
    ),
    (
        "CLUSTER COUNTKEYSINSLOT",
        "Counts the keys of a slot.",
        "Conta as chaves de um slot.",
    ),
    (
        "COMMAND COUNT",
        "Returns how many commands the server has.",
        "Devolve quantos comandos o servidor tem.",
    ),
    (
        "COMMAND DOCS",
        "Returns the documentation of commands.",
        "Devolve a documentação de comandos.",
    ),
    (
        "COMMAND INFO",
        "Returns details of commands (arguments, flags, keys).",
        "Devolve detalhes de comandos (argumentos, flags, chaves).",
    ),
    (
        "COMMAND LIST",
        "Lists the names of the commands.",
        "Lista os nomes dos comandos.",
    ),
    (
        "COMMAND GETKEYS",
        "Shows which arguments of a command are keys.",
        "Mostra quais argumentos de um comando são chaves.",
    ),
    (
        "CONFIG GET",
        "Returns configuration parameters (accepts patterns like max*).",
        "Devolve parâmetros da configuração (aceita padrões como max*).",
    ),
    (
        "CONFIG SET",
        "Changes a configuration parameter without restarting.",
        "Altera um parâmetro da configuração sem reiniciar.",
    ),
    (
        "CONFIG REWRITE",
        "Writes the current configuration to the redis.conf file.",
        "Grava a configuração atual no arquivo redis.conf.",
    ),
    (
        "CONFIG RESETSTAT",
        "Resets the statistics shown by INFO.",
        "Zera as estatísticas mostradas pelo INFO.",
    ),
    (
        "DEBUG OBJECT",
        "Shows internal details of a key.",
        "Mostra detalhes internos de uma chave.",
    ),
    (
        "DEBUG SLEEP",
        "Blocks the server for the given seconds.",
        "Trava o servidor pelos segundos informados.",
    ),
    (
        "DEBUG JMAP",
        "Internal debugging subcommand.",
        "Subcomando interno de depuração.",
    ),
    (
        "FUNCTION LIST",
        "Lists the loaded function libraries.",
        "Lista as bibliotecas de funções carregadas.",
    ),
    (
        "FUNCTION LOAD",
        "Loads a function library.",
        "Carrega uma biblioteca de funções.",
    ),
    (
        "FUNCTION DELETE",
        "Deletes a function library.",
        "Apaga uma biblioteca de funções.",
    ),
    (
        "FUNCTION FLUSH",
        "Deletes all function libraries.",
        "Apaga todas as bibliotecas de funções.",
    ),
    (
        "FUNCTION STATS",
        "Shows the function running now and the engines available.",
        "Mostra a função em execução e os motores disponíveis.",
    ),
    (
        "FUNCTION DUMP",
        "Returns all libraries in a serialized format (for FUNCTION RESTORE).",
        "Devolve todas as bibliotecas em formato serializado (para o FUNCTION RESTORE).",
    ),
    (
        "FUNCTION RESTORE",
        "Loads the libraries from a FUNCTION DUMP.",
        "Carrega as bibliotecas de um FUNCTION DUMP.",
    ),
    (
        "FUNCTION KILL",
        "Stops the function running now.",
        "Interrompe a função em execução.",
    ),
    (
        "LATENCY LATEST",
        "Shows the latest latency spike of each event.",
        "Mostra o último pico de latência de cada evento.",
    ),
    (
        "LATENCY HISTORY",
        "Shows the history of latency spikes of an event.",
        "Mostra o histórico de picos de latência de um evento.",
    ),
    (
        "LATENCY RESET",
        "Clears the recorded latency spikes.",
        "Limpa os picos de latência registrados.",
    ),
    (
        "LATENCY DOCTOR",
        "Analyzes the latency and gives advice.",
        "Analisa a latência e dá conselhos.",
    ),
    (
        "LATENCY GRAPH",
        "Draws a text graph of the latency of an event.",
        "Desenha um gráfico em texto da latência de um evento.",
    ),
    (
        "MEMORY USAGE",
        "Returns the bytes a key uses.",
        "Devolve os bytes que uma chave ocupa.",
    ),
    (
        "MEMORY STATS",
        "Shows details of the memory use of the server.",
        "Mostra detalhes do uso de memória do servidor.",
    ),
    (
        "MEMORY DOCTOR",
        "Analyzes the memory use and gives advice.",
        "Analisa o uso de memória e dá conselhos.",
    ),
    (
        "MEMORY PURGE",
        "Asks the allocator to give unused memory back to the system.",
        "Pede ao alocador para devolver ao sistema a memória sem uso.",
    ),
    (
        "MEMORY MALLOC-STATS",
        "Shows the internal statistics of the memory allocator.",
        "Mostra as estatísticas internas do alocador de memória.",
    ),
    (
        "MODULE LIST",
        "Lists the loaded modules.",
        "Lista os módulos carregados.",
    ),
    (
        "MODULE LOAD",
        "Loads a module from a file.",
        "Carrega um módulo de um arquivo.",
    ),
    (
        "MODULE UNLOAD",
        "Unloads a module.",
        "Descarrega um módulo.",
    ),
    (
        "OBJECT ENCODING",
        "Returns how the value of a key is stored internally.",
        "Devolve como o valor de uma chave é guardado internamente.",
    ),
    (
        "OBJECT FREQ",
        "Returns the access frequency of a key (with an LFU policy).",
        "Devolve a frequência de acesso de uma chave (com política LFU).",
    ),
    (
        "OBJECT IDLETIME",
        "Returns the seconds since a key was last accessed.",
        "Devolve os segundos desde o último acesso a uma chave.",
    ),
    (
        "OBJECT REFCOUNT",
        "Returns the number of references to the value of a key.",
        "Devolve o número de referências ao valor de uma chave.",
    ),
    (
        "PUBSUB CHANNELS",
        "Lists the channels with subscribers.",
        "Lista os canais com assinantes.",
    ),
    (
        "PUBSUB NUMSUB",
        "Returns the number of subscribers of channels.",
        "Devolve o número de assinantes de canais.",
    ),
    (
        "PUBSUB NUMPAT",
        "Returns the number of pattern subscriptions.",
        "Devolve o número de assinaturas por padrão.",
    ),
    (
        "PUBSUB SHARDCHANNELS",
        "Lists the shard channels with subscribers.",
        "Lista os canais de shard com assinantes.",
    ),
    (
        "PUBSUB SHARDNUMSUB",
        "Returns the number of subscribers of shard channels.",
        "Devolve o número de assinantes de canais de shard.",
    ),
    (
        "SCRIPT LOAD",
        "Loads a Lua script and returns its SHA1 hash (for EVALSHA).",
        "Carrega um script Lua e devolve o hash SHA1 dele (para o EVALSHA).",
    ),
    (
        "SCRIPT EXISTS",
        "Tells whether scripts are loaded, by SHA1 hash.",
        "Diz se scripts estão carregados, pelo hash SHA1.",
    ),
    (
        "SCRIPT FLUSH",
        "Removes all loaded scripts.",
        "Remove todos os scripts carregados.",
    ),
    (
        "SCRIPT KILL",
        "Stops the script running now.",
        "Interrompe o script em execução.",
    ),
    (
        "SLOWLOG GET",
        "Returns the latest slow commands.",
        "Devolve os últimos comandos lentos.",
    ),
    (
        "SLOWLOG LEN",
        "Returns how many slow commands are recorded.",
        "Devolve quantos comandos lentos estão registrados.",
    ),
    (
        "SLOWLOG RESET",
        "Clears the slow command log.",
        "Limpa o registro de comandos lentos.",
    ),
    (
        "XGROUP CREATE",
        "Creates a consumer group for a stream.",
        "Cria um grupo de consumidores em um stream.",
    ),
    (
        "XGROUP DESTROY",
        "Deletes a consumer group.",
        "Apaga um grupo de consumidores.",
    ),
    (
        "XGROUP SETID",
        "Changes the last ID delivered to a consumer group.",
        "Muda o último ID entregue a um grupo de consumidores.",
    ),
    (
        "XGROUP CREATECONSUMER",
        "Creates a consumer in a group.",
        "Cria um consumidor em um grupo.",
    ),
    (
        "XGROUP DELCONSUMER",
        "Removes a consumer from a group.",
        "Remove um consumidor de um grupo.",
    ),
    (
        "XINFO STREAM",
        "Shows information about a stream.",
        "Mostra informações de um stream.",
    ),
    (
        "XINFO GROUPS",
        "Lists the consumer groups of a stream.",
        "Lista os grupos de consumidores de um stream.",
    ),
    (
        "XINFO CONSUMERS",
        "Lists the consumers of a group.",
        "Lista os consumidores de um grupo.",
    ),
];

/// O `HELP` de qualquer comando com subcomandos.
pub const HELP: (&str, &str) = (
    "Lists the subcommands of this command.",
    "Lista os subcomandos deste comando.",
);

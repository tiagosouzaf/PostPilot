//! O que cada método, operador e estágio do MongoDB faz.

use super::Doc;

/// Pelo nome. Onde o mesmo nome quer dizer coisas diferentes (`$set` de
/// um `update` e de um `aggregate`), a chave é `"detalhe:nome"`, com o
/// detalhe que a lista mostra em cinza.
pub const DOCS: &[Doc] = &[
    // Começo de um comando.
    (
        "db",
        "The current database. Follow with a collection (db.orders.find()) or a database method.",
        "O banco atual. Continue com uma coleção (db.pedidos.find()) ou um método do banco.",
    ),
    (
        "show collections",
        "Lists the collections of the current database.",
        "Lista as coleções do banco atual.",
    ),
    (
        "show dbs",
        "Lists the databases of the server.",
        "Lista os bancos do servidor.",
    ),
    (
        "use",
        "Switches to another database.",
        "Troca para outro banco.",
    ),
    (
        "collections",
        "Lists the collections of the current database.",
        "Lista as coleções do banco atual.",
    ),
    (
        "dbs",
        "Lists the databases of the server.",
        "Lista os bancos do servidor.",
    ),
    // Métodos de uma coleção.
    (
        "aggregate",
        "Runs a pipeline of stages ($match, $group, $sort...) that transform the documents step by step.",
        "Executa uma sequência de estágios ($match, $group, $sort...) que transformam os documentos passo a passo.",
    ),
    (
        "countDocuments",
        "Counts the documents that match the filter.",
        "Conta os documentos que atendem ao filtro.",
    ),
    (
        "createIndex",
        "Creates an index on the given fields (1 ascending, -1 descending).",
        "Cria um índice nos campos informados (1 crescente, -1 decrescente).",
    ),
    (
        "deleteMany",
        "Deletes all documents that match the filter.",
        "Apaga todos os documentos que atendem ao filtro.",
    ),
    (
        "deleteOne",
        "Deletes the first document that matches the filter.",
        "Apaga o primeiro documento que atende ao filtro.",
    ),
    (
        "distinct",
        "Returns the distinct values of a field.",
        "Devolve os valores distintos de um campo.",
    ),
    (
        "drop",
        "Deletes the collection with all its documents and indexes.",
        "Apaga a coleção com todos os documentos e índices dela.",
    ),
    (
        "dropIndex",
        "Deletes an index of the collection.",
        "Apaga um índice da coleção.",
    ),
    (
        "dropIndexes",
        "Deletes all indexes of the collection, except the _id one.",
        "Apaga todos os índices da coleção, menos o do _id.",
    ),
    (
        "estimatedDocumentCount",
        "Returns the approximate number of documents, from the collection metadata (fast).",
        "Devolve o número aproximado de documentos, pelos metadados da coleção (rápido).",
    ),
    (
        "find",
        "Returns the documents that match the filter. The second argument chooses the fields.",
        "Devolve os documentos que atendem ao filtro. O segundo argumento escolhe os campos.",
    ),
    (
        "findOne",
        "Returns the first document that matches the filter.",
        "Devolve o primeiro documento que atende ao filtro.",
    ),
    (
        "getIndexes",
        "Lists the indexes of the collection.",
        "Lista os índices da coleção.",
    ),
    (
        "insertMany",
        "Inserts several documents.",
        "Insere vários documentos.",
    ),
    ("insertOne", "Inserts one document.", "Insere um documento."),
    (
        "replaceOne",
        "Replaces the whole first document that matches the filter.",
        "Substitui inteiro o primeiro documento que atende ao filtro.",
    ),
    (
        "stats",
        "Shows statistics (size, number of documents, indexes).",
        "Mostra estatísticas (tamanho, número de documentos, índices).",
    ),
    (
        "updateMany",
        "Changes all documents that match the filter, with operators like $set.",
        "Altera todos os documentos que atendem ao filtro, com operadores como $set.",
    ),
    (
        "updateOne",
        "Changes the first document that matches the filter, with operators like $set.",
        "Altera o primeiro documento que atende ao filtro, com operadores como $set.",
    ),
    // Métodos do banco.
    (
        "adminCommand",
        "Runs a command on the admin database.",
        "Executa um comando no banco admin.",
    ),
    (
        "createCollection",
        "Creates a collection (needed only for special options; inserting already creates one).",
        "Cria uma coleção (só é preciso para opções especiais; inserir já cria).",
    ),
    (
        "dropDatabase",
        "Deletes the current database with everything in it.",
        "Apaga o banco atual com tudo o que há nele.",
    ),
    (
        "getCollection",
        "Returns a collection by name, for names that cannot be written after db. (with dashes or spaces).",
        "Devolve uma coleção pelo nome, para nomes que não dá para escrever depois de db. (com hífen ou espaço).",
    ),
    (
        "getCollectionNames",
        "Lists the names of the collections of the database.",
        "Lista os nomes das coleções do banco.",
    ),
    (
        "runCommand",
        "Runs a raw server command on the current database.",
        "Executa um comando direto do servidor no banco atual.",
    ),
    (
        "serverStatus",
        "Shows the state of the server (connections, memory, operations).",
        "Mostra o estado do servidor (conexões, memória, operações).",
    ),
    (
        "version",
        "Returns the version of the server.",
        "Devolve a versão do servidor.",
    ),
    // Depois de um find(...).
    (
        "collation",
        "Sets the language rules to compare and sort text.",
        "Define as regras de idioma para comparar e ordenar texto.",
    ),
    (
        "count",
        "Counts the documents of the query.",
        "Conta os documentos da consulta.",
    ),
    (
        "explain",
        "Shows how the server runs the query (indexes used, documents examined).",
        "Mostra como o servidor executa a consulta (índices usados, documentos examinados).",
    ),
    (
        "hint",
        "Forces the query to use an index.",
        "Obriga a consulta a usar um índice.",
    ),
    (
        "limit",
        "Returns at most n documents.",
        "Devolve no máximo n documentos.",
    ),
    (
        "maxTimeMS",
        "Stops the query if it takes longer than the given milliseconds.",
        "Interrompe a consulta se ela passar dos milissegundos informados.",
    ),
    (
        "projection",
        "Chooses which fields come back (1 includes, 0 excludes).",
        "Escolhe quais campos voltam (1 inclui, 0 exclui).",
    ),
    (
        "skip",
        "Skips the first n documents.",
        "Pula os primeiros n documentos.",
    ),
    (
        "sort",
        "Sorts the documents (1 ascending, -1 descending).",
        "Ordena os documentos (1 crescente, -1 decrescente).",
    ),
    // Operadores de filtro.
    (
        "$all",
        "The array has all the given values.",
        "A lista tem todos os valores informados.",
    ),
    (
        "$and",
        "All the conditions of the list must be true.",
        "Todas as condições da lista precisam valer.",
    ),
    (
        "$elemMatch",
        "At least one element of the array matches all the conditions.",
        "Pelo menos um elemento da lista atende a todas as condições.",
    ),
    ("$eq", "Equal to the value.", "Igual ao valor."),
    (
        "$exists",
        "The field exists (true) or does not exist (false).",
        "O campo existe (true) ou não existe (false).",
    ),
    (
        "$expr",
        "Uses aggregation expressions in the filter, to compare fields of the same document.",
        "Usa expressões de agregação no filtro, para comparar campos do mesmo documento.",
    ),
    ("$gt", "Greater than the value.", "Maior que o valor."),
    (
        "$gte",
        "Greater than or equal to the value.",
        "Maior ou igual ao valor.",
    ),
    (
        "$in",
        "Equal to any of the values of the list.",
        "Igual a qualquer um dos valores da lista.",
    ),
    ("$lt", "Less than the value.", "Menor que o valor."),
    (
        "$lte",
        "Less than or equal to the value.",
        "Menor ou igual ao valor.",
    ),
    (
        "$mod",
        "The remainder of the division of the field by a number equals a value: [divisor, remainder].",
        "O resto da divisão do campo por um número é igual a um valor: [divisor, resto].",
    ),
    ("$ne", "Not equal to the value.", "Diferente do valor."),
    (
        "$nin",
        "Not equal to any of the values of the list.",
        "Diferente de todos os valores da lista.",
    ),
    (
        "$nor",
        "None of the conditions of the list is true.",
        "Nenhuma das condições da lista vale.",
    ),
    ("$not", "Inverts a condition.", "Inverte uma condição."),
    (
        "$options",
        "Options of $regex: i ignores case, m multiline, s dot matches line breaks.",
        "Opções do $regex: i ignora maiúsculas, m várias linhas, s o ponto casa com quebras de linha.",
    ),
    (
        "$or",
        "At least one of the conditions of the list is true.",
        "Pelo menos uma das condições da lista vale.",
    ),
    (
        "$regex",
        "The text matches a regular expression.",
        "O texto combina com uma expressão regular.",
    ),
    (
        "$size",
        "The array has exactly this number of elements.",
        "A lista tem exatamente este número de elementos.",
    ),
    (
        "$text",
        "Text search on the fields of a text index.",
        "Busca de texto nos campos de um índice de texto.",
    ),
    (
        "$type",
        "The field is of the given BSON type (\"string\", \"int\", \"date\"...).",
        "O campo é do tipo BSON informado (\"string\", \"int\", \"date\"...).",
    ),
    // Operadores de um update.
    (
        "$addToSet",
        "Adds a value to an array only if it is not there yet.",
        "Adiciona um valor a uma lista só se ele ainda não está nela.",
    ),
    (
        "$currentDate",
        "Sets the field to the current date.",
        "Grava no campo a data atual.",
    ),
    (
        "$each",
        "With $push or $addToSet: adds each value of a list.",
        "Com $push ou $addToSet: adiciona cada valor de uma lista.",
    ),
    (
        "$inc",
        "Adds an amount to the number of the field (negative subtracts).",
        "Soma uma quantidade ao número do campo (negativo subtrai).",
    ),
    (
        "atualização:$max",
        "Updates the field only if the new value is greater than the current one.",
        "Atualiza o campo só se o valor novo for maior que o atual.",
    ),
    (
        "atualização:$min",
        "Updates the field only if the new value is less than the current one.",
        "Atualiza o campo só se o valor novo for menor que o atual.",
    ),
    (
        "$mul",
        "Multiplies the number of the field.",
        "Multiplica o número do campo.",
    ),
    (
        "$pop",
        "Removes the first (-1) or the last (1) element of an array.",
        "Remove o primeiro (-1) ou o último (1) elemento de uma lista.",
    ),
    (
        "$pull",
        "Removes from an array the elements that match a condition.",
        "Remove de uma lista os elementos que atendem a uma condição.",
    ),
    (
        "atualização:$push",
        "Adds a value to the end of an array.",
        "Adiciona um valor ao fim de uma lista.",
    ),
    ("$rename", "Renames a field.", "Renomeia um campo."),
    (
        "atualização:$set",
        "Sets the value of fields, creating the ones that do not exist.",
        "Grava o valor de campos, criando os que não existem.",
    ),
    (
        "$setOnInsert",
        "Sets fields only when the update inserts a new document (upsert).",
        "Grava campos só quando a atualização insere um documento novo (upsert).",
    ),
    (
        "atualização:$unset",
        "Removes fields from the document.",
        "Remove campos do documento.",
    ),
    // Estágios de um aggregate.
    (
        "$addFields",
        "Adds computed fields to the documents.",
        "Adiciona campos calculados aos documentos.",
    ),
    (
        "$bucket",
        "Groups the documents in ranges of a value.",
        "Agrupa os documentos em faixas de um valor.",
    ),
    (
        "$count",
        "Returns the number of documents that reached this stage.",
        "Devolve o número de documentos que chegaram a este estágio.",
    ),
    (
        "$facet",
        "Runs several pipelines over the same documents, each with its own result.",
        "Executa várias sequências de estágios sobre os mesmos documentos, cada uma com o próprio resultado.",
    ),
    (
        "$group",
        "Groups the documents by a key (_id) and computes totals ($sum, $avg...) for each group.",
        "Agrupa os documentos por uma chave (_id) e calcula totais ($sum, $avg...) de cada grupo.",
    ),
    (
        "$limit",
        "Passes on at most n documents.",
        "Deixa passar no máximo n documentos.",
    ),
    (
        "$lookup",
        "Joins documents of another collection (like a LEFT JOIN).",
        "Junta documentos de outra coleção (como um LEFT JOIN).",
    ),
    (
        "$match",
        "Keeps only the documents that match a filter (like find).",
        "Mantém só os documentos que atendem a um filtro (como o find).",
    ),
    (
        "$merge",
        "Writes the result into a collection, merging with what is there.",
        "Grava o resultado em uma coleção, mesclando com o que já existe.",
    ),
    (
        "$out",
        "Writes the result into a collection, replacing it.",
        "Grava o resultado em uma coleção, substituindo-a.",
    ),
    (
        "$project",
        "Chooses, renames or computes the fields of the documents.",
        "Escolhe, renomeia ou calcula os campos dos documentos.",
    ),
    (
        "$replaceRoot",
        "Replaces each document with one of its subdocuments.",
        "Troca cada documento por um subdocumento dele.",
    ),
    (
        "$sample",
        "Picks documents at random.",
        "Sorteia documentos.",
    ),
    (
        "estágio:$set",
        "Adds computed fields to the documents (same as $addFields).",
        "Adiciona campos calculados aos documentos (igual ao $addFields).",
    ),
    (
        "$skip",
        "Skips the first n documents.",
        "Pula os primeiros n documentos.",
    ),
    (
        "$sort",
        "Sorts the documents (1 ascending, -1 descending).",
        "Ordena os documentos (1 crescente, -1 decrescente).",
    ),
    (
        "$sortByCount",
        "Groups by a value and sorts the groups by size, largest first.",
        "Agrupa por um valor e ordena os grupos pelo tamanho, do maior para o menor.",
    ),
    (
        "$unionWith",
        "Adds the documents of another collection to the result.",
        "Acrescenta ao resultado os documentos de outra coleção.",
    ),
    (
        "estágio:$unset",
        "Removes fields from the documents.",
        "Remove campos dos documentos.",
    ),
    (
        "$unwind",
        "Turns each element of an array into a separate document.",
        "Transforma cada elemento de uma lista em um documento separado.",
    ),
    // Expressões de um aggregate.
    (
        "$add",
        "Adds numbers, or adds milliseconds to a date.",
        "Soma números, ou soma milissegundos a uma data.",
    ),
    (
        "$arrayElemAt",
        "Returns the element at a position of an array.",
        "Devolve o elemento de uma posição de uma lista.",
    ),
    ("$avg", "Average of the values.", "Média dos valores."),
    ("$concat", "Joins texts.", "Junta textos."),
    (
        "$cond",
        "If the condition is true returns one value, otherwise another: [if, then, else].",
        "Se a condição vale devolve um valor, senão outro: [se, então, senão].",
    ),
    (
        "$dateToString",
        "Formats a date as text.",
        "Formata uma data como texto.",
    ),
    (
        "$dayOfMonth",
        "Day of the month of a date (1 to 31).",
        "Dia do mês de uma data (1 a 31).",
    ),
    (
        "$divide",
        "Divides one number by another.",
        "Divide um número por outro.",
    ),
    (
        "$filter",
        "Keeps the elements of an array that match a condition.",
        "Mantém os elementos de uma lista que atendem a uma condição.",
    ),
    (
        "$first",
        "First value of the group (or of an array).",
        "Primeiro valor do grupo (ou de uma lista).",
    ),
    (
        "$ifNull",
        "Returns the value, or a replacement when it is null or missing.",
        "Devolve o valor, ou um substituto quando ele é nulo ou não existe.",
    ),
    (
        "$last",
        "Last value of the group (or of an array).",
        "Último valor do grupo (ou de uma lista).",
    ),
    (
        "$map",
        "Applies an expression to each element of an array.",
        "Aplica uma expressão a cada elemento de uma lista.",
    ),
    ("$max", "Largest of the values.", "Maior dos valores."),
    ("$min", "Smallest of the values.", "Menor dos valores."),
    (
        "$month",
        "Month of a date (1 to 12).",
        "Mês de uma data (1 a 12).",
    ),
    ("$multiply", "Multiplies numbers.", "Multiplica números."),
    (
        "$push",
        "Collects the values of the group into an array.",
        "Junta os valores do grupo em uma lista.",
    ),
    (
        "$round",
        "Rounds a number to the given decimal places.",
        "Arredonda um número para as casas decimais informadas.",
    ),
    (
        "$subtract",
        "Subtracts two numbers, or two dates (result in milliseconds).",
        "Subtrai dois números, ou duas datas (resultado em milissegundos).",
    ),
    (
        "$sum",
        "Sum of the values ({ $sum: 1 } counts the documents).",
        "Soma dos valores ({ $sum: 1 } conta os documentos).",
    ),
    (
        "$toDate",
        "Converts a value to a date.",
        "Converte um valor para data.",
    ),
    (
        "$toDecimal",
        "Converts a value to a decimal number.",
        "Converte um valor para número decimal.",
    ),
    (
        "$toInt",
        "Converts a value to an integer.",
        "Converte um valor para inteiro.",
    ),
    (
        "$toLower",
        "Converts a text to lowercase.",
        "Converte um texto para minúsculas.",
    ),
    (
        "$toString",
        "Converts a value to text.",
        "Converte um valor para texto.",
    ),
    (
        "$toUpper",
        "Converts a text to uppercase.",
        "Converte um texto para maiúsculas.",
    ),
    ("$year", "Year of a date.", "Ano de uma data."),
    (
        "$set",
        "Sets the value of fields.",
        "Grava o valor de campos.",
    ),
    ("$unset", "Removes fields.", "Remove campos."),
    // Valores.
    (
        "ISODate",
        "A date and time, written in the ISO 8601 format.",
        "Uma data e hora, escrita no formato ISO 8601.",
    ),
    (
        "NumberDecimal",
        "An exact decimal number (Decimal128), good for money.",
        "Um número decimal exato (Decimal128), bom para dinheiro.",
    ),
    ("NumberLong", "A 64-bit integer.", "Um inteiro de 64 bits."),
    (
        "ObjectId",
        "The unique identifier MongoDB generates for the _id of a document.",
        "O identificador único que o MongoDB gera para o _id de um documento.",
    ),
    (
        "UUID",
        "A universally unique identifier.",
        "Um identificador único universal.",
    ),
    (
        "false",
        "The boolean value false.",
        "O valor booleano falso.",
    ),
    ("null", "The absence of a value.", "A ausência de valor."),
    (
        "true",
        "The boolean value true.",
        "O valor booleano verdadeiro.",
    ),
];

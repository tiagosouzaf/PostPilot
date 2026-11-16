# Sugestões da IA no editor

Com uma IA conectada (Ferramentas > Provedores de IA: o Claude Code
instalado, o ChatGPT pela conta ou um provedor por chave), o PostPilot lê
a consulta que você está escrevendo e propõe o que dá para fazer com ela.
Funciona nos cinco bancos: PostgreSQL, SQL Server, MongoDB, Redis e
Elasticsearch.

## Como aparece

Você escreve normalmente. Quando para de digitar por um instante, a IA lê
o comando em que o cursor está. No canto de baixo do editor aparece um
aviso: "A IA está lendo a consulta…" e depois "5 sugestões da IA".

**Ctrl+.** (ou um clique no aviso, ou Editar > IntelliSense > Sugestões da
IA) abre a lista embaixo do cursor, como a do IntelliSense. Cada linha diz
o que fazer; embaixo ficam o porquê e o código pronto.

| Tecla | O que faz |
| --- | --- |
| ↑ ↓ | Escolhe a sugestão |
| Enter ou Tab | Aplica |
| Esc | Fecha a lista |
| Ctrl+Z | Desfaz a sugestão aplicada, num passo só |

Há três tipos de sugestão:

- **Aplicar** troca o comando inteiro pela versão sugerida (o JOIN, o
  filtro, a correção).
- **Acrescentar depois da consulta** põe outro comando embaixo do seu (um
  `CREATE INDEX`, um `EXPLAIN`, um `TTL`).
- **Só um conselho** não mexe no texto (um aviso de `DELETE` sem `WHERE`,
  por exemplo).

## O que a IA propõe

| Assunto | Exemplos |
| --- | --- |
| Junção | JOIN pela chave estrangeira das tabelas citadas; `$lookup` no MongoDB |
| Filtro | intervalo de datas, `EXISTS`, `$gte`/`$in`/`$regex`, `term` em vez de `match` num campo keyword |
| Colunas | nomear as colunas em vez de `SELECT *`; projeção; `_source` |
| Resumo | `GROUP BY`, `$group`, agregações `terms`/`date_histogram` |
| Ordem e limite | `ORDER BY` com `LIMIT`/`TOP`, `.sort().limit()`, `size` |
| Desempenho | filtro que usa índice, `SCAN` em vez de `KEYS`, `bool.filter` |
| Índice | o índice que falta para o filtro e a ordenação |
| Conserto | erro de sintaxe, nome errado, tipo trocado |
| Segurança | `UPDATE`/`DELETE` sem `WHERE`, `FLUSHDB`, `_delete_by_query` |
| Completar | o resto de um comando pela metade |
| Jeito melhor | uma forma mais limpa de escrever a mesma coisa |

## O que é enviado

Vão para a IA o texto do comando do cursor e o que o catálogo do
IntelliSense sabe das tabelas citadas e das vizinhas: nomes, tipos, chaves
e índices (coleções e campos no MongoDB, índices e mapeamento no
Elasticsearch, nomes e tipos das chaves no Redis). **Os dados das tabelas
não são enviados e nada roda no servidor**: a IA só lê texto.

Num ambiente com a regra "IA: bloqueada" (Servidores Registrados >
Ambientes), nada é enviado.

## Não atrapalha a digitação

A pergunta sai em segundo plano, só depois de uma pausa (pouco mais de um
segundo), uma de cada vez. Digitar de novo descarta as sugestões do texto
antigo. A resposta de cada comando fica guardada: voltar a um comando já
lido não pergunta outra vez. Com o Claude Code, a resposta costuma levar
de 10 a 15 segundos.

## Ligar e desligar

Ferramentas > Opções > IA > Provedores de IA, "Sugerir o que fazer com a
consulta no editor":

- **Enquanto eu digito** (o padrão);
- **Só quando eu pedir**: a IA só lê quando você aperta Ctrl+. (gasta
  menos do plano ou dos créditos);
- **Nunca**.

Ctrl+Shift+R (Atualizar Cache Local) faz a IA esquecer as respostas
guardadas e reler chaves e índices.

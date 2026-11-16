# Consultas de diagnóstico

Consultas prontas para entender o que está pesando num servidor: o que roda
agora, quem bloqueia quem, tempo e custo das consultas. Um clique abre uma
janela de consulta e já traz o resultado.

## Onde ficam

- **Toolbar**: o botão "Diagnóstico" lista as consultas prontas para o
  servidor conectado. Clicou, rodou.
- **Exibir > Consultas de Diagnóstico**: abre o Template Explorer na pasta
  "Diagnóstico". O duplo clique roda; o botão "Executar" da prévia também.
  "Inserir" e "Nova Janela" só põem o texto, para adaptar.

Cada execução abre uma janela de consulta comum: dá para editar, rodar de
novo com F5, salvar no workspace ou como trecho, e ela volta ao reabrir o
app. As execuções entram no Histórico de Consultas.

## O que tem

PostgreSQL e SQL Server têm a pasta "Diagnóstico":

| Consulta | Responde |
| --- | --- |
| Diagnóstico Completo | Quatro resultados: visão geral, o que roda agora, quem bloqueia quem e as consultas mais caras |
| Quem Bloqueia Quem | Árvore de bloqueio: na margem quem segura o lock, embaixo quem espera, com o tempo e o objeto |
| Consultas Mais Pesadas Rodando Agora | Tempo, espera, transação e, no SQL Server, CPU, leituras e memória |
| Consultas Mais Caras (Histórico) | Tempo total, % do total, chamadas, média, máximo, linhas, leituras |
| Consultas Mais Lentas em Média | O mesmo histórico, pela média (com pelo menos 5 execuções) |
| Transações Longas ou Esquecidas | Transações abertas, as mais antigas primeiro, e quantas sessões cada uma bloqueia |
| Tabelas Lidas sem Índice (PostgreSQL) | Tabelas com mais leitura sequencial |
| O Que o Servidor Mais Espera (SQL Server) | Estatísticas de espera, sem as esperas ociosas |
| Conexões por Aplicação | Quem está conectado, por banco, usuário, aplicação e cliente |

Nos outros motores, os modelos de servidor que só leem também rodam com um
clique (Redis: informações, clientes e comandos lentos; MongoDB: status e
operações em andamento; Elasticsearch: saúde, nós, shards e tarefas).

## Regras

- Todas só leem. Um teste confere isso (`ready_templates_only_read`), então
  rodam sem pedir confirmação, mesmo em Produção.
- No PostgreSQL, o histórico vem da extensão `pg_stat_statements`. Num banco
  sem ela, a consulta não falha: a parte do histórico dá lugar a uma linha
  que explica como ligar (`shared_preload_libraries` e `create extension`).
- No SQL Server, as consultas leem as DMVs e pedem `VIEW SERVER STATE`.
- O SQL fica em `crates/ui-shell/src/templates/diagnostics/*.sql`; a lista,
  em `templates/builtin.rs` (`.ready()` marca o que roda com um clique).

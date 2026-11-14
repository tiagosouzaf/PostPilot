# ADR 0003: Claude no PostPilot (MCP, plugin e painel)

- **Status:** aceita (Tiago, 2026-11-14); implementada na Fase 7 (marcos 7.1 a 7.6)
- **Data:** 2026-11-14

## Contexto

O Tiago pediu "conexão com o Claude, plugin mesmo": uma tela do Claude dentro
do app, e o Claude Code do terminal mexendo no PostPilot pelo MCP. O Claude
precisa ver o mesmo que o usuário (servidores conectados, catálogo, janelas
de consulta e resultados) e agir no app aberto, não numa conexão paralela.
Isso traz três perguntas: por onde o Claude chega ao app, como se autentica, e
o que ele pode fazer sem perguntar.

## Decisões

### 1. Um conjunto de ferramentas, três portas

As ferramentas ficam numa crate sem GUI, `pgstudio-automation`
(`tools.rs`): 12 ferramentas com esquema JSON (`list_servers`,
`list_databases`, `list_objects`, `describe_table`, `describe_function`,
`run_query`, `list_query_windows`, `get_query_window`, `edit_query_window`,
`open_query_window`, `execute_query_window`, `get_query_results`). O app as
atende num único lugar (`ui-shell/src/claude.rs`), no frame do egui, pelo
mesmo estado que a interface usa. Quem chama não importa:

- **`postpilot mcp`** (7.2): servidor MCP por stdio, JSON-RPC escrito à mão
  (`initialize`, `ping`, `tools/list`, `tools/call`; versões 2025-06-18,
  2025-03-26 e 2024-11-05). Sem SDK: são quatro métodos, e o binário não
  ganha um runtime a mais. As chamadas rodam em paralelo, para uma
  confirmação pendente não segurar as outras.
- **Plugin do Claude Code** (7.3): `claude-plugin/` com o `.mcp.json` que
  chama `postpilot mcp`, uma skill e os comandos `/postpilot:explain`,
  `optimize` e `fix`; `.claude-plugin/marketplace.json` na raiz do
  repositório e em `/usr/share/postpilot` nos pacotes Linux.
- **Painel do Claude** (7.4) e **ações rápidas** (7.5): o próprio app abre o
  Claude Code e passa a ele o mesmo `postpilot mcp`.

### 2. Canal local entre o `postpilot mcp` e o app

O `postpilot mcp` é outro processo (o Claude Code o abre). Ele fala com o app
aberto por TCP em `127.0.0.1`, porta aleatória, uma linha JSON de ida e uma
de volta. O app grava porta, pid e um token de 32 bytes em
`<config>/PostPilot/mcp-endpoint.json` com permissão 0600 (escrita num
temporário e renomeada); o token é comparado em tempo constante. Outro
usuário da máquina não lê o arquivo e não acerta o token.

Escolhemos TCP local e não socket Unix ou named pipe porque funciona igual no
Linux e no Windows, sem dependência nova. O canal só existe com
**Ferramentas > Permitir o Claude Code (MCP)** ligado (desligado por padrão;
a escolha fica em `claude.json`). O painel abre um canal próprio
(`panel-endpoint-<pid>.json`), marcado como origem "Claude" em vez de
"Claude Code", e não depende dessa opção.

### 3. O painel usa o Claude Code instalado, sem chave de API

Opções: (a) a API da Anthropic com chave do usuário, (b) o Claude Code
instalado, com o login dele. O Tiago escolheu (b): ele já usa o Claude Code,
não há chave para guardar nem cobrança separada, e o painel ganha de graça o
que o Claude Code sabe fazer (contexto, retomada de sessão, modelos).

O painel roda `claude -p --input-format stream-json --output-format
stream-json --include-partial-messages` com `--tools ""` (sem shell nem
arquivos), `--strict-mcp-config` com só o `postpilot`, e
`--allowedTools "mcp__postpilot__*"`. Um processo por conversa, uma linha
JSON por mensagem; Parar mata o processo e a próxima mensagem continua a
sessão com `--resume`. O `claude` é procurado em `PGSTUDIO_CLAUDE`, no
`PATH`, em `~/.claude/local` e em `~/.local/bin` (o lançador do Hyprland nem
sempre tem `~/.local/bin` no `PATH`).

Cada mensagem leva uma linha `[PostPilot context]` com a janela ativa,
servidor, banco e a seleção. As ações rápidas mostram na conversa uma
pergunta curta e mandam ao Claude instruções a mais (que janela ler, que modo
de edição usar, o erro da última execução).

### 4. Segurança: ler livre, alterar com confirmação

- `run_query` classifica o SQL com o `sqlparser` (`safety.rs`). Só leitura
  roda na hora, dentro de `BEGIN TRANSACTION READ ONLY` com
  `statement_timeout` de 60 s e `ROLLBACK` no fim. A transação é a defesa de
  verdade: uma função plpgsql que faz `INSERT` é barrada pelo PostgreSQL
  mesmo que o classificador a deixe passar.
- DML, DDL, `SELECT ... FOR UPDATE`, `SELECT INTO`, `EXPLAIN ANALYZE` de
  escrita, funções com efeito colateral (lista em `SIDE_EFFECT_FUNCTIONS`) e
  tudo o que não dá para classificar abrem um diálogo modal no app com o SQL
  inteiro. Sem clique em **Run**, nada roda. `execute_query_window` passa
  pela mesma regra.
- Escrever no editor é livre: o texto novo fica selecionado e Ctrl+Z desfaz.
- Os resultados que vão para o Claude são cortados (200 linhas por padrão,
  1000 no máximo, 64 KB por chamada, 2000 caracteres por célula). Senhas não
  passam pelas ferramentas.

## Consequências

- Uma ferramenta nova entra uma vez em `tools.rs` e no `claude.rs` e aparece
  nas três portas.
- O painel depende de o usuário ter o Claude Code instalado e logado; sem
  ele, o painel explica como instalar.
- O formato stream-json do Claude Code pode mudar; o leitor
  (`agent.rs`) ignora eventos que não conhece.
- No Windows, o `postpilot.exe` é do subsistema GUI; o `postpilot mcp`
  depende dos handles de stdio que o Claude Code passa. Não foi testado num
  Windows de verdade, e o `.msi` não instala o plugin.

## Testes

- `pgstudio-automation`: classificador, coleta e corte de resultados,
  protocolo MCP, canal com token, leitor do stream-json com um `claude`
  falso.
- `ui-shell/tests/claude.rs`: as ferramentas contra um PostgreSQL de verdade,
  confirmação, edição com Ctrl+Z e o canal local.
- `ui-shell/tests/claude_panel.rs`: painel e ações rápidas com um `claude`
  falso.
- Ponta a ponta com o Claude Code real sob Xvfb (marcos 7.2 a 7.5): consulta,
  edição no editor, DELETE com confirmação recusada e aceita, plugin com
  `/postpilot:optimize`, correção de erro e pergunta sobre tabela.

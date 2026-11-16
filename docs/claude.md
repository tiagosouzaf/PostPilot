# Claude no PostPilot

O PostPilot trabalha com o Claude de três jeitos. Os três usam o
[Claude Code](https://claude.com/claude-code) instalado na sua máquina e o
login dele, sem chave de API:

| Jeito | Onde você conversa | Para quê |
| --- | --- | --- |
| **Painel do Claude** | dentro do PostPilot (Exibir > Claude) | perguntar sobre os bancos e pedir consultas sem sair do app |
| **Ações rápidas** | menu de contexto do editor e do Object Explorer | explicar, otimizar ou corrigir a consulta, ou perguntar sobre uma tabela, com um clique |
| **Claude Code no terminal** (plugin ou MCP) | `claude` no terminal | o Claude mexe no PostPilot aberto enquanto você trabalha no código |

Nos três, o Claude vê o que o PostPilot vê: servidores conectados, bancos,
tabelas, colunas, índices, funções, janelas de consulta e resultados. As
senhas nunca saem do PostPilot.

## O que o Claude pode fazer sozinho e o que pede

- **Ler é livre.** Consultas de leitura rodam na hora, dentro de uma
  transação `READ ONLY` com timeout de 60 s. Mesmo uma função que tenta
  gravar é barrada pelo próprio PostgreSQL.
- **Escrever no editor é livre.** O texto que o Claude escreve fica
  selecionado, e Ctrl+Z desfaz.
- **Alterar dados ou esquema pede confirmação.** `INSERT`, `UPDATE`,
  `DELETE`, DDL, `VACUUM`, funções com efeito colateral (`nextval`,
  `pg_terminate_backend`, `dblink`...) e qualquer SQL que o PostPilot não
  consiga classificar abrem o diálogo "Claude wants to run SQL" com o SQL
  inteiro. Só roda se você clicar em **Run**. Executar uma janela de consulta
  a pedido do Claude passa pela mesma regra.

- **O plano real segue a mesma regra de rodar.** O plano estimado não
  executa nada e é livre. O plano real executa o comando para medir: de um
  `SELECT` roda na hora, de um comando que altera pede confirmação. Nos dois
  casos tudo roda numa transação que é desfeita no fim.

## Redis, MongoDB e Elasticsearch

As mesmas ferramentas valem nos motores sem SQL, cada um na sua linguagem:
comandos do Redis, consultas no estilo do mongosh e requisições do console
do Elasticsearch. O `list_servers` diz o motor de cada servidor.

| Ferramenta | Redis | MongoDB | Elasticsearch |
|---|---|---|---|
| `list_databases` | bancos numerados | bancos | saúde do cluster |
| `list_objects` | chaves (`schema` é um padrão do `SCAN`) | coleções | índices e aliases (`schema` é um padrão de nome) |
| `describe_table` | validade, tamanho e tipo da chave | documentos de exemplo, índices e contagem | mapeamento e contagem |
| `run_query` | leitura na hora, escrita com confirmação | igual | igual |

`explain_query`, `get_activity` e `describe_function` não existem nesses
motores: a resposta diz ao Claude o que usar no lugar (`.explain()` no
MongoDB, `"profile": true` no Elasticsearch, `SLOWLOG GET` no Redis). Não
há transação só de leitura neles, então a proteção é o classificador: o
que o PostPilot não consegue ler como só leitura pede confirmação.

## Plano de execução e atividade

O Claude não precisa adivinhar por que uma consulta é lenta nem quem está
travando quem:

- **`explain_query`** devolve o plano de execução em texto, no PostgreSQL
  (`EXPLAIN`, e `EXPLAIN (ANALYZE, BUFFERS)` no real) e no SQL Server
  (`SHOWPLAN_XML`, e `STATISTICS XML` no real): operadores, linhas estimadas
  e reais, custo, predicados, avisos e, no SQL Server, os índices que o
  servidor sente falta. O **Otimizar com o Claude** e o `/postpilot:optimize`
  leem o plano antes de mexer na consulta e comparam o custo depois.
- **`get_activity`** lê o que o Monitor de Atividade mostra: sessões com o
  SQL que estão rodando, há quanto tempo e o que esperam; as cadeias de
  bloqueio, começando por quem segura todo mundo; os locks em espera; e, se
  pedido, as consultas mais caras. O Claude só lê: ele diz qual sessão
  cancelar ou encerrar, e quem faz isso é você, no Monitor de Atividade.

Pergunte, por exemplo, "por que esta consulta está lenta?" ou "tem alguém
bloqueando alguém no servidor do cliente X?".

## Ambientes: Produção pergunta antes

A IA respeita o ambiente do servidor (o mesmo da etiqueta colorida das
janelas de consulta). O diálogo de confirmação mostra a etiqueta do
ambiente, e cada ambiente tem uma regra para a IA, em **Servidores
Registrados > botão direito na raiz > Ambientes...** (ou na lista de
servidores do workspace):

| Regra | O que a IA pode fazer nos servidores do ambiente |
| --- | --- |
| **IA: lê livremente** | A regra de sempre: ler é livre, alterar pede confirmação. |
| **IA: pergunta antes** | Ler o esquema (tabelas, colunas, índices, funções) e pedir o plano estimado continua livre. Tudo o que roda SQL ou traz dados pede confirmação, mesmo um `SELECT`: consultas, executar a janela, plano real e atividade do servidor. |
| **IA: bloqueada** | Nenhuma ferramenta chega ao servidor, nem para ler o esquema ou os resultados de uma janela. A IA recebe o aviso de que você bloqueou e não tenta de novo. |

**Produção já vem em "pergunta antes"**; Desenvolvimento e Homologação, em
"lê livremente". Um ambiente seu que tenha **Confirmar alterações** marcado
também começa em "pergunta antes", até você escolher outra regra. Servidor
sem ambiente segue a regra de sempre. Ao executar uma janela de consulta a
pedido da IA em Produção, a confirmação é uma só.

A regra vale para o painel, para o Claude Code no terminal e para os outros
provedores de IA (OpenAI, OpenRouter, GLM): todos passam pelas mesmas
ferramentas.

O que o Claude lê (esquema, texto das consultas, resultados, planos e o SQL
das sessões na atividade) vai para a Anthropic como o resto da conversa. Os resultados chegam cortados: 200
linhas por resultado por padrão (até 1000) e 64 KB por chamada.

## No Omarchy (Arch + Hyprland)

### 1. Instalar o Claude Code e fazer login

```sh
curl -fsSL https://claude.ai/install.sh | bash
claude          # na primeira vez, faz o login; depois /exit
```

O instalador põe o `claude` em `~/.local/bin`. O PostPilot também procura
lá e em `~/.claude/local`, então funciona mesmo que o lançador do Hyprland
não tenha esse caminho no `PATH`. Num lugar diferente, aponte com
`PGSTUDIO_CLAUDE=/caminho/do/claude`.

### 2. Instalar o PostPilot

```sh
curl -fsSL https://raw.githubusercontent.com/tiagosouzaf/postpilot/main/install.sh | bash
# ou, do código: cd packaging/arch && makepkg -si
```

O pacote põe o `postpilot` em `/usr/bin` e o plugin do Claude Code em
`/usr/share/postpilot`.

### 3. Usar o painel

1. Abra o PostPilot e conecte num servidor.
2. **Exibir > Claude** abre o painel ao lado de Propriedades. Ele pode ser
   arrastado para outro lugar como qualquer painel.
3. Escreva e aperte Enter (Shift+Enter quebra a linha). Cada mensagem leva
   junto qual janela de consulta está ativa, em que servidor e banco, e o
   texto selecionado.

Cada ferramenta que o Claude usa aparece como um cartão (clique para ver o
que ele mandou e o que voltou). Blocos de SQL na resposta têm **Inserir**
(na janela ativa, ou numa nova) e **Copiar**. **Parar** interrompe;
**Nova conversa** começa do zero.

### 4. Ações rápidas

- No editor, botão direito (ou o menu Consulta > Claude):
  **Explicar com o Claude**, **Otimizar com o Claude** e
  **Corrigir o Erro com o Claude**. Com texto selecionado, vale só a
  seleção. Otimizar e Corrigir escrevem a versão nova no editor e não
  executam.
- No Object Explorer, botão direito numa tabela ou view:
  **Perguntar ao Claude**.

### 5. Claude Code no terminal (plugin)

1. No PostPilot, ligue **Ferramentas > Permitir o Claude Code (MCP)**. A
   escolha fica salva; a barra de status mostra "Claude Code: ligado".
2. No terminal, instale o plugin que veio no pacote:

   ```sh
   claude plugin marketplace add /usr/share/postpilot
   claude plugin install postpilot@postpilot
   ```

   Sem o pacote, direto do GitHub:
   `claude plugin marketplace add tiagosouzaf/postpilot`.
3. Abra o `claude` em qualquer pasta e peça, por exemplo, "liste as tabelas
   do banco loja no PostPilot". Os comandos `/postpilot:explain`,
   `/postpilot:optimize` e `/postpilot:fix` agem na janela ativa.

Se você só quer as ferramentas, sem o plugin:
`claude mcp add postpilot -- postpilot mcp`. Use um ou outro, não os dois.

Com o PostPilot fechado ou a opção desligada, as ferramentas respondem
"PostPilot is not open, or "Allow Claude Code (MCP)" is off" (a mensagem
vai para o Claude, que a repassa a você).

## Windows

O painel e as ações rápidas funcionam igual (o PostPilot procura
`claude.exe` e `claude.cmd`). Para o plugin, o `.zip` traz as pastas
`claude-plugin` e `.claude-plugin`: use `claude plugin marketplace add
<pasta do zip>`. O `.mcp.json` do plugin chama `postpilot`, então a pasta do
`postpilot.exe` precisa estar no `PATH`; senão, use
`claude mcp add postpilot -- "C:\Program Files\PostPilot\postpilot.exe" mcp`.
O `.msi` não instala o plugin. Nada disso foi testado num Windows de
verdade.

## ChatGPT pela sua conta (sem chave de API)

Do mesmo jeito que o Claude usa o login do Claude Code, o ChatGPT pode usar
a sua conta (o plano do ChatGPT) em vez de uma chave de API paga por uso.
O PostPilot faz isso pelo **Codex**, o programa oficial da OpenAI, que tem o
"Sign in with ChatGPT". O PostPilot não guarda o login: quem guarda é o
Codex.

1. Instale o Codex: `npm install -g @openai/codex` (precisa do Node.js).
2. Em **Ferramentas > Provedores de IA…**, escolha **ChatGPT (sua conta)**
   na lista de cima.
3. Clique em **Entrar com o ChatGPT**. O navegador abre a página da OpenAI;
   entre com a sua conta. Se o navegador não abrir, use **Copiar o link de
   login**. (Dá no mesmo rodar `codex login` num terminal.)
4. Quando aparecer "Conectado com a sua conta do ChatGPT", dê OK.

O painel, o **Gerar com IA** do Commit e o nome de arquivo sugerido passam a
usar o ChatGPT. Por baixo, o PostPilot roda `codex exec` com o sandbox só de
leitura e sem a ferramenta de shell, e liga as ferramentas do PostPilot por
MCP: alterar dados ou esquema continua pedindo a confirmação de sempre. O
modelo é o configurado no Codex (`~/.codex/config.toml`). Se o Codex está
fora do `PATH`, aponte `PGSTUDIO_CODEX` para ele.

## Outras IAs: OpenAI, OpenRouter, GLM

O Claude Code é o padrão, mas o painel, o botão **Gerar com IA** do Commit e
o nome de arquivo sugerido ao salvar funcionam com qualquer serviço que fale
a API da OpenAI. Em **Ferramentas > Provedores de IA…**:

1. **Adicionar** e escolha o provedor: OpenAI (ChatGPT), OpenRouter, GLM
   (Z.ai) ou "Outro (compatível com OpenAI)" para Ollama, LM Studio etc.
2. Confira a **URL base** e o **Modelo** e cole a **Chave de API**.
   O GLM já vem com o endereço da assinatura GLM Coding Plan
   (`https://api.z.ai/api/coding/paas/v4`); a chave paga por uso usa
   `https://api.z.ai/api/paas/v4`. A OpenAI pede uma chave de
   platform.openai.com: a assinatura do ChatGPT sozinha não inclui uma.
3. **Testar conexão** faz uma pergunta curta ao modelo e lista os modelos
   que a chave enxerga.
4. Em cima, escolha qual assistente o PostPilot usa e dê OK.

As chaves ficam no cofre do sistema (Secret Service, por exemplo o GNOME
Keyring); sem cofre, em `~/.config/PostPilot/ai-keys.json`, que só você lê
(0600). A lista de provedores fica em `~/.config/PostPilot/ai.json`, sem
chave nenhuma. Nada disso vai para a pasta do workspace nem para o Git.

Com um provedor escolhido, o painel usa as mesmas ferramentas e a mesma
regra: ler é livre, alterar pede confirmação, e a regra do ambiente
(Produção pergunta antes, ou bloqueada) vale igual. O que o assistente lê vai para
o servidor do provedor.

**Com uma IA conectada** (Claude Code instalado ou um provedor com chave):

- o diálogo de **Commit** do workspace ganha **Gerar com IA**, que escreve a
  mensagem a partir da lista de arquivos e do diff (o `secrets.json` entra
  só pelo nome) e deixa você editar antes de confirmar;
- ao **salvar uma consulta nova**, o nome sugerido deixa de ser
  `SQLQuery1.sql` e passa a descrever o SQL (`clientes_inativos.sql`). Um
  aviso com **Pular** aparece enquanto a IA pensa; se ela demorar mais de
  12 segundos ou falhar, o diálogo abre com o nome padrão.

Sem IA conectada, o botão fica desligado e o nome é o padrão.

## Quando algo não funciona

| Sintoma | O que fazer |
| --- | --- |
| "Claude Code was not found" no painel | instale o Claude Code (passo 1) ou defina `PGSTUDIO_CLAUDE` |
| "Claude Code stopped before answering" | rode `claude` num terminal e faça o login de novo |
| no terminal, "PostPilot is not open..." | abra o PostPilot e ligue Ferramentas > Permitir o Claude Code (MCP) |
| o diálogo de confirmação não aparece | ele abre na janela do PostPilot; no Hyprland, olhe o workspace dela |

## Como funciona por dentro

Detalhes e decisões em [ADR 0003](adr/0003-claude.md).

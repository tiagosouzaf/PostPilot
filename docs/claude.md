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

O que o Claude lê (esquema, texto das consultas, resultados) vai para a
Anthropic como o resto da conversa. Os resultados chegam cortados: 200
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
sudo pacman -U postpilot-<versão>-1-x86_64.pkg.tar.zst
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

## Quando algo não funciona

| Sintoma | O que fazer |
| --- | --- |
| "Claude Code was not found" no painel | instale o Claude Code (passo 1) ou defina `PGSTUDIO_CLAUDE` |
| "Claude Code stopped before answering" | rode `claude` num terminal e faça o login de novo |
| no terminal, "PostPilot is not open..." | abra o PostPilot e ligue Ferramentas > Permitir o Claude Code (MCP) |
| o diálogo de confirmação não aparece | ele abre na janela do PostPilot; no Hyprland, olhe o workspace dela |

## Como funciona por dentro

Detalhes e decisões em [ADR 0003](adr/0003-claude.md).

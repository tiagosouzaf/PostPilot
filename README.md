# PostPilot

Aplicativo desktop para administrar PostgreSQL, inspirado no Microsoft SQL Server
Management Studio (SSMS). Escrito 100% em Rust, para Windows e Linux, sem Electron
nem WebView.

Até a versão 0.1 o app se chamava **PGStudio**; o nome mudou porque outros
produtos de PostgreSQL já usam esse. Na primeira execução, o PostPilot copia a
configuração antiga (`~/.config/PGStudio` ou `%APPDATA%\PGStudio`: conexões
recentes, servidores registrados, layout e keymap) e as senhas lembradas no
cofre do sistema. Os pacotes novos substituem os antigos (`pgstudio`) no apt e
no pacman.

> As Fases 0 a 7 do roteiro estão prontas: janela no estilo do SSMS com
> painéis encaixáveis, Object Explorer, editor com IntelliSense, grade que
> edita e exporta, Table Designer, diagramas, Query Designer, planos de
> execução, Activity Monitor, segurança, backup e restore, Schema Compare,
> servidores registrados e o Claude (painel no app, ações rápidas e o
> Claude Code pelo MCP). O que ainda não foi testado está em
> [Limitações conhecidas](#limitações-conhecidas).

## Estrutura

```txt
apps/desktop/     binário `postpilot`
crates/core/      tipos de domínio, comandos e keymap, sem GUI e sem I/O
crates/ui-shell/  janela, menus, toolbar, status bar e tema (egui)
crates/docking/   painéis encaixáveis (invólucro do egui_dock)
crates/object-explorer/  árvore do Object Explorer e provedor de demonstração
crates/pg-client/ conexão com o PostgreSQL (tokio-postgres, TLS com rustls)
crates/pg-metadata/ catálogo do PostgreSQL para o Object Explorer
crates/sql-editor/ editor de SQL: realce, números de linha, busca, Go To Line
crates/result-grid/ buffer paginado, grade de resultados e resultados em texto
crates/schema-model/ modelo do schema, diff para DDL, comparação e snapshot
crates/diagram-engine/ desenho de tabelas e relacionamentos (diagramas e Query Designer)
crates/execution-plan/ leitura e desenho do EXPLAIN (FORMAT JSON)
crates/automation/ ferramentas do Claude, servidor MCP (`postpilot mcp`) e ponte com o Claude Code
claude-plugin/    plugin do Claude Code (MCP, skill e comandos); o marketplace fica em `.claude-plugin/`
xtask/            tarefas de desenvolvimento (`cargo xtask bench`, `dist`, `pgmatrix`)
packaging/        ícone, atalho .desktop e textos dos pacotes de instalação
```

Novas crates entram na fase do roteiro que as usa.

## Instalação

Os pacotes ficam na [página de Releases](https://github.com/tiagosouzaf/postpilot/releases/latest).

### Linux: um comando só

```sh
curl -fsSL https://raw.githubusercontent.com/tiagosouzaf/postpilot/main/install.sh | bash
```

O [`install.sh`](install.sh) baixa a versão mais recente, confere o SHA-256
contra o `SHA256SUMS` do Release e instala do jeito do seu sistema:

- **Arch Linux e derivados (Omarchy, Manjaro, EndeavourOS):** o
  `.pkg.tar.zst`, com `sudo pacman -U`. Ele substitui o antigo `pgstudio` (o
  pacman pergunta; responda `y`).
- **Ubuntu, Debian e derivados:** o `.deb`, com `sudo apt install`.
- **Outras distribuições (Fedora, openSUSE…):** o AppImage em
  `~/.local/bin/PostPilot.AppImage`, com atalho no menu de aplicativos, sem
  sudo. O AppImage precisa da `fuse2` (`libfuse2` no Debian e no Ubuntu).

Para **atualizar**, use o próprio app: quando sai uma versão nova ele mostra
uma faixa no alto da janela com o botão **Atualizar agora** (ou em Ajuda >
Verificar se Há Atualizações). Ele baixa o pacote, confere o SHA-256, pede a
senha de administrador pela janela do sistema (`pkexec`/polkit) para instalar
e oferece reiniciar. Sem `pkexec`, o app mostra o comando acima; rodar o
instalador de novo também atualiza (ele não faz nada se a versão já estiver
instalada). Opções vão depois de `bash -s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/tiagosouzaf/postpilot/main/install.sh | bash -s -- --appimage   # AppImage mesmo em Arch/Debian
curl -fsSL https://raw.githubusercontent.com/tiagosouzaf/postpilot/main/install.sh | bash -s -- --version 0.1.0
curl -fsSL https://raw.githubusercontent.com/tiagosouzaf/postpilot/main/install.sh | bash -s -- --uninstall
```

Prefere ver o script antes de rodar? Baixe com
`curl -fsSLO https://raw.githubusercontent.com/tiagosouzaf/postpilot/main/install.sh`,
leia e rode `bash install.sh`.

**AUR:** o pacote `tf-postpilot-bin` está pronto, mas ainda não foi publicado
(o AUR fechou temporariamente o cadastro de contas novas). Quando sair, vai
ser `yay -S tf-postpilot-bin`. Até lá, use o comando acima.

### Linux: à mão

Baixe o arquivo do Release e:

- **Arch e derivados:** `sudo pacman -U ./postpilot-<versão>-1-x86_64.pkg.tar.zst`
  (pelo arquivo baixado: `pacman -U <URL>` falha procurando uma assinatura
  `.sig` que o Release não tem), ou compile do código com
  `cd packaging/arch && makepkg -si`. Para remover, `sudo pacman -R postpilot`.
- **Ubuntu, Debian e derivados:** `sudo apt install ./postpilot_<versão>_amd64.deb`.
  Para remover, `sudo apt remove postpilot`.
- **Qualquer distribuição:** `chmod +x PostPilot-<versão>-x86_64.AppImage`
  e execute. Sem FUSE (alguns containers), use
  `./PostPilot-<versão>-x86_64.AppImage --appimage-extract-and-run`. Ou extraia
  o `postpilot-<versão>-linux-x86_64.tar.gz` em `/` (tem `usr/bin`, ícones e
  atalho).

### Windows 10 e 11 (64 bits)

Baixe o `PostPilot-<versão>-x86_64.msi` e dê dois cliques (ou
`msiexec /i PostPilot-<versão>-x86_64.msi`). Ele instala para todos os
usuários em `C:\Program Files\PostPilot`, pede elevação, põe o atalho no menu
Iniciar e o PostPilot no "Abrir com" dos arquivos `.sql` (sem virar o programa
padrão). Uma versão nova substitui a anterior, e o próprio app se atualiza
pelo menu Ajuda; para remover, Configurações > Aplicativos > PostPilot. Sem
instalar, extraia o `PostPilot-<versão>-windows-x86_64.zip` e abra
`postpilot.exe`. O exe e o msi não são assinados, então o SmartScreen pode
avisar ("Mais informações" > "Executar assim mesmo"). O instalador só foi
testado no Wine (`msiexec` do Wine 9), não num Windows de verdade.

### Requisitos

No Linux, o binário é ligado contra a glibc 2.31 (Ubuntu 20.04, Debian 11 e
posteriores) e precisa de Vulkan ou OpenGL ES (o Mesa de qualquer desktop
serve). Os pacotes foram testados em Ubuntu 22.04 e 24.04 limpos; no 20.04 o
binário carrega, mas o renderizador por software do Mesa daquela versão é
antigo demais para o wgpu (numa GPU de verdade deve abrir, mas não foi
testado).

### Renderizador e Wayland

O PostPilot desenha a janela com o **wgpu** (Vulkan no Linux, Direct3D 12 no
Windows) e traz o **glow** (OpenGL) como alternativa. Para escolher:

```sh
postpilot --renderer glow            # ou --renderer=glow
PGSTUDIO_RENDERER=glow postpilot     # o mesmo, por variável de ambiente
```

A opção da linha de comando vence a variável. Sem nenhuma das duas, o app
tenta o wgpu e, se ele não abrir (sem driver Vulkan nem GL que sirva), reabre
sozinho com o glow e avisa no terminal. Help > About mostra o renderizador em
uso (no wgpu, também o backend e a placa). Para deixar o glow fixo no
Hyprland, ponha `env = PGSTUDIO_RENDERER,glow` no `hyprland.conf`.

No Wayland o app abre nativo, sem XWayland, com o `app_id` `postpilot` (o mesmo
do `.desktop`, para o ícone e para as regras de janela do compositor, que
veem `postpilot` como class). A escala fracionária ou inteira do
monitor vem do compositor. O copiar e colar usa a área de transferência do
Wayland (a mesma do `wl-copy`/`wl-paste`). Os diálogos de abrir e salvar
arquivo passam pelo xdg-desktop-portal, que precisa de um backend com seletor
de arquivos: o portal do Hyprland e o do sway (`-wlr`) não têm, então instale
também o `xdg-desktop-portal-gtk` (`pacman -Q xdg-desktop-portal-gtk` diz se
já está instalado). Sem ele, File > Open não abre nada.

Testado num sway sem tela (headless, sem X), com escala 2, nos dois
renderizadores (Mesa por software): abrir, conectar, copiar do editor para o
`wl-paste`, colar do `wl-copy` no editor e abrir um arquivo pelo diálogo do
portal (GTK). Não foi testado no Hyprland nem com GPU de verdade.

## Uso

```sh
postpilot [--renderer wgpu|glow] [arquivo.sql ...]
```

Ao abrir, o PostPilot mostra o "Connect to Server", como o SSMS: Server name
(`host` ou `host,porta`), Login, Password, Database e Encryption (Disable,
Prefer, Require ou Verify full, os modos `sslmode` do libpq). Depois de conectar,
o Object Explorer mostra o servidor como no SSMS: Databases > banco > Schemas >
schema > Tables, Views, Functions e Sequences, com Columns, Constraints e
Indexes embaixo de cada tabela, e Security > Roles. Schemas do sistema
(`pg_*`, `information_schema`) ficam de fora. F5 com o Object Explorer em foco,
ou Refresh no menu de contexto, relê o nó selecionado. File > Connect Object
Explorer abre o diálogo de novo. As conexões usadas ficam em
`connections.json`, na pasta de configuração, sem a senha; a seta ao lado do
Server name lista as recentes.

O menu de contexto do Object Explorer abre janelas de consulta no banco do nó:
New Query (vazia), Select Top 1000 Rows em tabelas e views (já executa) e
Script as > CREATE To / DROP To New Query Window em bancos, schemas, tabelas, views, funções, índices,
sequences e constraints. O CREATE vem do próprio servidor (`pg_get_viewdef`,
`pg_get_functiondef`, `pg_get_indexdef`, `pg_get_constraintdef`), e o de
tabela inclui colunas, `serial`, identity, colunas geradas, defaults,
constraints, `PARTITION BY` e os índices; os scripts rodam sem erro num banco
limpo (há um teste que faz isso).

Edit Top 200 Rows numa tabela abre a grade editável, como no SSMS. Clique
numa célula e digite (ou F2, Enter ou duplo clique para editar o valor
atual); Ctrl+0 põe NULL. A célula alterada fica marcada e a linha vai ao
banco ao sair dela (ou com Enter): um UPDATE pela chave primária, ou um
INSERT pela linha `*` do fim, que deixa as colunas não preenchidas com o
padrão. O `RETURNING` traz os valores como o banco gravou. Se der erro, a
linha fica marcada com a mensagem e Esc desfaz as alterações. Delete (ou o
menu da linha) exclui depois de confirmar. Colunas geradas e de identidade
`ALWAYS` não são editáveis, e tabela sem chave primária (ou view) fica só
leitura, com o motivo.

Ctrl+N abre uma janela de consulta, Ctrl+O abre um arquivo, Ctrl+S e
Ctrl+Shift+S salvam e Ctrl+F4 fecha. A aba mostra `*` quando há alterações, e
fechar a aba ou o aplicativo pergunta antes de descartar.

O editor colore o SQL do PostgreSQL como o SSMS (palavras-chave, strings,
`$$` de funções, comentários, números, `$1`) e mostra os números de linha.
Ctrl+K, Ctrl+C comenta as linhas selecionadas com `--` e Ctrl+K, Ctrl+U tira o
comentário (também em Edit > Advanced). Ctrl+F abre a barra de busca, já com a
palavra selecionada, e Ctrl+G vai para uma linha. A status bar mostra a linha e
a coluna do cursor. Ctrl+Z e Ctrl+Y desfazem e refazem (uma palavra por vez),
Enter mantém a indentação e Tab com várias linhas selecionadas indenta todas
(Shift+Tab tira). O editor monta só as linhas visíveis e refaz o realce só do
trecho mudado, então uma tecla num script de 5 mil linhas leva cerca de 1 ms.

O editor tem vários cursores e seleção em coluna, com os gestos do VS Code.
Clique num lugar, segure Shift+Alt (ou Ctrl+Shift) e clique em outro: a seleção
em coluna vai do cursor até o clique, uma seleção por linha. Alt+arrastar faz o
mesmo a partir de onde o botão desceu. Pelo teclado, Shift+Alt+↑/↓ (ou
Ctrl+Alt+↑/↓) põe um cursor na linha de cima ou de baixo, e Ctrl+Shift+Alt+setas
(ou Shift+Alt+←/→ com o bloco já começado) mudam o bloco. Alt+clique ou
Ctrl+clique põe um cursor onde você clicar (e tira, se já houver um ali). Ctrl+D
seleciona a palavra do cursor e, de novo, a próxima ocorrência dela; Ctrl+F2
seleciona todas. Shift+Alt+→ expande a seleção (palavra, texto entre aspas,
parênteses, comando até o `;`, tudo) e Shift+Alt+← encolhe de volta. Depois de
um duplo ou triplo clique, arrastar continua por palavra ou por linha.

Digitar, apagar, Enter, Tab e Shift+Tab, recortar, colar e o IntelliSense valem
para todos os cursores, e Ctrl+Z desfaz em todos de uma vez. Copiar junta as
seleções, uma por linha, e colar de volta num bloco com o mesmo número de linhas
devolve uma linha a cada cursor. Esc (ou um clique simples) volta a um cursor
só. Executar, comentar e formatar usam a seleção principal, a do último cursor.
Com o editor em foco, Ctrl+D é a próxima ocorrência; Results to Grid continua no
menu Query (e Ctrl+K, Ctrl+D continua formatando).

Format Document (Shift+Alt+F ou Ctrl+Shift+I, como no VS Code, ou Ctrl+K, Ctrl+D,
como no Visual Studio; também em Edit > Advanced e no menu de contexto do editor)
formata o SQL: cada cláusula numa linha, colunas do `SELECT` e condições do
`WHERE` uma por linha, subconsultas e `CASE` indentados e palavras-chave em
maiúsculas. Com seleção, formata só as linhas selecionadas; sem, o script
inteiro. Ctrl+Z desfaz a formatação de uma vez. O formatador só mexe nos espaços
e na caixa das palavras-chave: comentários, strings, nomes entre aspas e corpos
`$$` ficam como estão, e o resultado é conferido token a token contra o
original. SQL com string, comentário ou parêntese sem fecho fica como está, com o
motivo na status bar.

O IntelliSense lê o catálogo do banco (schemas, tabelas, views, colunas e
funções) em segundo plano, na primeira vez que uma janela de consulta precisa
dele, e guarda em cache; Ctrl+Shift+R (Edit > IntelliSense > Refresh Local
Cache) lê de novo. A lista abre sozinha ao digitar: depois de `FROM` e `JOIN`
vêm tabelas e views, depois de `alias.` as colunas daquela tabela, depois de
`schema.` o que há no schema, e no resto as colunas das tabelas do comando,
palavras-chave e funções. Ctrl+Espaço completa a palavra (direto, se só houver
uma opção), Ctrl+J lista, Tab ou Enter escolhe e Esc fecha.

A lista acha o nome mesmo sem a palavra certa. Depois dos nomes que começam
com o que foi digitado vêm os que têm a palavra no meio (`cria` ou `datahora`
acham `data_hora_criacao`) e os que têm os começos das palavras (`dhc`). Se
nada combinou assim, entram os palpites: abreviações que pulam letras
(`dt_cria`, `qtd`) e erros de digitação, com letra trocada, faltando, sobrando
ou invertida (`statsu` acha `status`, `clinte` acha `cliente`). Maiúsculas,
acentos e underscores não contam, as letras que casaram ficam destacadas, e a
sugestão escolhida troca a palavra digitada inteira. Vale para nomes do banco,
palavras-chave e os comandos do Redis, MongoDB e Elasticsearch.

Strings, nomes
entre aspas, `$$` e comentários sem fim e parênteses sem par ficam sublinhados
em vermelho, assim como o trecho do erro que o servidor apontou na última
execução; o mouse em cima mostra a mensagem.

Parar o mouse no `*` de um `SELECT` mostra as colunas que ele representa, com
os tipos, tabela por tabela (as do `FROM` e dos `JOIN` daquele `SELECT`);
`alias.*` mostra só as daquela tabela. A lista se divide em blocos lado a lado
para caber na janela (e rola, se nem assim couber) e fica aberta enquanto o
mouse está nela: um clique escolhe uma coluna (no nome da tabela, todas as
dela), e os botões de baixo trocam o `*` pelas colunas escolhidas ou copiam os
nomes. Sem escolha, valem todas.

F5 ou Ctrl+E executa a seleção, ou o documento inteiro se nada estiver
selecionado, numa conexão própria da janela de consulta (aberta na primeira
execução, no banco de onde a janela veio ou no da conexão do Object Explorer). Vários comandos separados
por `;` rodam juntos, como no protocolo simples do PostgreSQL: se um falhar, os
anteriores do mesmo lote são desfeitos, a menos que haja `BEGIN`/`COMMIT`
explícitos. Embaixo do editor, a aba Results mostra cada resultado e a aba
Messages mostra as linhas afetadas, os avisos (`RAISE NOTICE`) e os erros com
linha e coluna; o cursor vai até o erro, e clicar na mensagem volta a ele. A
faixa embaixo mostra a situação, o servidor, o banco, o tempo e as linhas.
"Cancel Executing Query" (botão ■ da toolbar ou menu Query) cancela pelo
`pg_cancel_backend` do protocolo. O atalho Alt+Break do SSMS está no keymap, mas
o egui ainda não entrega a tecla Pause/Break, então por enquanto ele não chega.

A grade de resultados mostra só as linhas visíveis e rola com milhões de
linhas. As linhas ficam num buffer paginado: até 64 MB por resultado em
memória, e o resto em páginas num arquivo temporário (apagado ao fechar). Clicar
seleciona uma célula; Shift+clique ou arrastar estende; o número da linha
seleciona a linha, o cabeçalho seleciona a coluna e o canto seleciona tudo
(Ctrl+A também). As setas, PageUp/PageDown e Ctrl+Home/End movem o cursor.
Ctrl+C copia a seleção separada por tab e Ctrl+Shift+C copia com o cabeçalho
(também no menu de contexto). Arrastar a borda do cabeçalho muda a largura da
coluna. NULL aparece em amarelo claro. Ctrl+T ("Results to Text") e Ctrl+D
("Results to Grid") escolhem como as próximas execuções da janela mostram os
resultados; no modo texto, as larguras das colunas saem das primeiras mil
linhas, até 256 caracteres, como no SSMS.

O resultado de uma consulta aparece em páginas, como no DataGrip: 500 linhas
por vez, com uma faixa embaixo da grade para ir e voltar (Alt+PageDown e
Alt+PageUp também), ver quais linhas estão à mostra e trocar as linhas por
página (50 a 10.000, ou "Todas as linhas" para desligar a paginação). O número
da linha continua o do resultado inteiro, e ordenar e filtrar valem para o
resultado todo, não só para a página. O canto, o Ctrl+A e o cabeçalho
selecionam o que está na página à mostra.

Na mesma faixa, "Copiar como" escolhe o formato do que o Ctrl+C manda para a
área de transferência: separado por tab (o padrão, que cola direto numa
planilha), CSV com vírgula ou com ponto e vírgula (o do Excel em português),
JSON, SQL INSERT, lista para `IN (...)`, tabela Markdown ou tabela HTML, com ou
sem os nomes das colunas. O menu de contexto tem "Copy As" para copiar uma vez
em outro formato. As duas escolhas (páginas e formato) valem para todas as
grades, ficam em Tools > Options > Query Results > Results to Grid e voltam
iguais ao reabrir o app.

Para exportar, o menu de contexto da grade tem "Save Results As...",
"Export to Excel..." e "Copy as INSERT". Ctrl+Shift+F ("Results to File") faz as próximas execuções
perguntarem um arquivo e escreverem as linhas nele direto do servidor, sem
guardá-las. A extensão escolhe o formato:

- `.csv`: como o `COPY ... (FORMAT csv, HEADER)` lê. NULL fica como campo
  vazio e o texto vazio como `""`.
- `.tsv` ou `.txt`: o formato texto do `COPY` (tab, `\N` para NULL).
- `.json`: uma lista de objetos.
- `.sql`: um `INSERT INTO table_name` por linha.
- `.xlsx`: uma pasta de trabalho do Excel, com o cabeçalho em negrito,
  congelado e com filtro. Cada resultado é uma planilha, e o que passa de
  1.048.576 linhas continua na planilha seguinte. Números viram números
  (menos códigos com zero à esquerda e inteiros com mais de 11 dígitos, que
  ficam como texto para o Excel não mudar o valor); datas ficam como texto.

O CSV e o TSV voltam iguais com `COPY`; há um teste que faz isso com 1
milhão de linhas.

Para importar, o menu de contexto de uma tabela no Pesquisador de Objetos
tem "Import CSV..." (PostgreSQL e SQL Server). A janela descobre o separador
(vírgula, ponto e vírgula, tab ou barra vertical), casa as colunas do
arquivo com as da tabela pelo nome (ou pela ordem, sem linha de nomes),
mostra uma prévia e carrega tudo numa transação: um erro ou o cancelamento
desfaz a carga inteira, e a mensagem diz as linhas do arquivo em que o erro
aconteceu. Um campo vazio é NULL e `""` é texto vazio; arquivo que não é
UTF-8 é lido como Windows-1252 (o CSV do Excel). Num servidor de um ambiente
que pede confirmação, como Produção, a janela pergunta antes de carregar.

O layout dos painéis e o tamanho e a posição da janela são salvos ao sair em
`layout.json`, na mesma pasta de configuração do keymap (abaixo), e restaurados
ao abrir. Se o arquivo estiver estragado, o PostPilot abre com o layout padrão e
avisa na status bar. Window > Reset Window Layout volta ao padrão a qualquer
momento.

### Designers

"New Table..." na pasta Tables e "Design" numa tabela abrem o **Table
Designer**: a grade de colunas (nome, tipo, Allow Nulls), Column Properties, a
chave primária e as janelas Relationships, Indexes/Keys e Check Constraints.
Generate Change Script mostra o DDL exato que Save aplica, numa transação: se
der erro, nada muda e as edições continuam na tela. Antes de salvar, o designer
avisa quando a mudança pode perder dados.

"New Database Diagram" na pasta Database Diagrams de um banco abre o **diagrama**
das tabelas escolhidas, com zoom (Ctrl + roda), pan e layout automático. Dá para
mover e redimensionar tabelas, criar tabela e coluna e arrastar uma coluna até
outra para criar um relacionamento, com undo e redo. Ctrl+S mostra o DDL e
aplica, e o diagrama (tabelas, posições e zoom) fica num arquivo `.pgsdiagram`,
que File > Open abre de novo.

Query > Design Query in Editor (Ctrl+Shift+Q) abre o **Query Designer** no banco
da janela de consulta. Com um SELECT selecionado, ele abre já montado com ele, e
o OK troca a seleção pelo SQL novo. As FKs entre as tabelas viram JOINs, e
arrastar uma coluna até outra cria um JOIN; a linha Joins escolhe o tipo
(LEFT, RIGHT ou FULL, as "all rows from" do SSMS). A grade tem apelido, saída,
ordenação, filtro, colunas Or... (cada uma um grupo de condições ligado por
AND, e os grupos por OR) e, com Group By, a agregação de cada coluna (o filtro
de uma coluna agregada vai para o HAVING). Distinct e Limit ficam na barra, e
Add Expression põe uma coluna digitada à mão, como `lower(nome)` ou
`count(*)`. CTE, subconsulta e UNION ele não desenha: avisa, e o OK põe a
consulta nova depois da seleção, sem apagá-la.

### Planos de execução e monitoramento

Ctrl+L mostra o **plano estimado** da seleção (ou do documento) na aba
Execution plan, desenhado como no SSMS: raiz à esquerda, custo de cada
operação e setas que engrossam com as linhas. Ctrl+M liga o **plano real**: o
F5 roda `EXPLAIN (ANALYZE, BUFFERS)` e mostra linhas, execuções e tempos de
verdade, e um nó que errou a estimativa em 10 vezes ou mais ganha um aviso.
Um lote com INSERT, UPDATE ou DELETE pergunta antes e, por padrão, roda dentro
de `BEGIN ... ROLLBACK`.

O **Query History** (Ctrl+Alt+H, menu View) guarda cada execução: o texto, o
servidor, o banco, a hora, a duração, as linhas e se deu certo, errou ou foi
cancelada. A pesquisa olha dentro do texto, do servidor, do banco e do erro,
e um duplo clique reabre a consulta numa janela nova, no mesmo servidor e
banco. O histórico fica só neste computador
(`<config>/PostPilot/history.jsonl`, fora do workspace do Git), continua lá ao
reabrir o app, e o que parece senha (`PASSWORD 'x'`, `password=x` numa
connection string) é gravado como `***`.

O **Activity Monitor** (Ctrl+Alt+A, ou no menu do servidor) mostra gráficos,
as sessões (com filtro, Cancel Query e Kill Process, que pedem confirmação),
esperas, E/S por banco, as consultas mais caras (com `pg_stat_statements`) e
os locks, com as cadeias de bloqueio em árvore. Alt+F1 no nome de uma tabela
faz o que o `sp_help` faz no SQL Server: objeto, colunas, índices,
constraints e as FKs que apontam para ela.

### Segurança, backup e comparação

"New Role..." em Security > Roles e "Properties" numa role abrem a janela da
role (senha, validade, atributos e Membership); "Properties" num banco,
schema, tabela, view ou função abre Permissions, com Grant e With Grant por
privilégio. Script mostra o SQL das mudanças, e OK aplica numa transação. Nos
outros objetos, "Properties" (ou F4) mostra as propriedades lidas do catálogo.

Tasks > Back Up... num banco abre o "Back Up Database", que roda o `pg_dump`
(formato custom, plain ou directory) com progresso e Stop. Tasks >
Restore... abre o "Restore Database", que lista o conteúdo do arquivo com o
`pg_restore -l`, deixa escolher os itens e restaura num banco existente, num
banco novo ou no do arquivo. Os dois procuram o `pg_dump` e o `pg_restore`
no PATH e nas pastas de instalação do PostgreSQL (ou em `PGSTUDIO_PG_BIN`).

Tools > Schema Compare... (ou Tasks > Compare Schema... num banco) compara
origem e destino: cada lado é um banco do servidor conectado, um banco de
outro servidor ou um snapshot `.pgsschema` gravado antes com "Save
Snapshot...". Entram tabelas, colunas, chaves, índices, checks, views,
funções, procedures, triggers, sequências e tipos. A janela lista as
diferenças com o script de cada uma, deixa marcar o que entra, gera o script
ou aplica numa transação e compara de novo.

### Servidores

O **Registered Servers** (Ctrl+Alt+G) guarda servidores em grupos, que podem
ser arrastados de um grupo a outro; dois cliques conectam. "Register..." no
menu de um servidor do Object Explorer registra aquela conexão, e Tasks >
Import... / Export... num grupo troca a árvore com outra máquina (sem senhas).
Os grupos são pastas com cor, e "Move To" muda uma conexão de pasta. Pastas e
conexões podem ter um ambiente (Desenvolvimento, Homologação, Produção ou
outros criados em "Environments..."): a janela de consulta mostra a cor e a
etiqueta dele, para ficar claro quando se está em produção. Em Produção, o F5
pergunta antes de rodar SQL que altera o banco.
A árvore fica em `registered_servers.json`. Vários servidores aparecem juntos
no Object Explorer. "Remember password", no Connect to Server e no registro,
guarda a senha no cofre do sistema (Credential Manager no Windows, Secret
Service no Linux), nunca em arquivo; sem cofre, o PostPilot pede a senha.

### Consulta entre servidores

**Consulta > Consulta entre Servidores** transforma a janela num `SELECT` que
junta dados de conexões guardadas diferentes: PostgreSQL com SQL Server, e
também com MongoDB, Elasticsearch e Redis. Cada fonte é lida no servidor dela
(`servidor.banco.schema.tabela`, ou a consulta do próprio servidor em
`servidor($$ ... $$)`) e o join roda num SQLite local. De cada tabela o
servidor manda só o que a consulta usa (colunas, filtros, chaves do join,
ordem e limite), e uma tabela que viria com mais de 200.000 linhas para antes
de baixar e avisa. Só leitura. Sintaxe e limites em [docs/consulta-entre-servidores.md](docs/consulta-entre-servidores.md).

### Workspace num repositório

O **Workspace Explorer** (Ctrl+Alt+L) abre uma pasta, normalmente um
repositório git, com os servidores (`servers.json`), as consultas
(`queries/*.sql`) e as senhas do projeto. As senhas ficam criptografadas com
uma senha mestra (Argon2id + XChaCha20-Poly1305) em `secrets.json`. Vários
workspaces ficam abertos ao mesmo tempo (um por cliente, por exemplo), cada um
num cartão com o estado do repositório e os botões Salvar alterações e
Sincronizar; **Adicionar** clona um repositório ou usa uma pasta do computador. Colar uma connection string `postgresql://...` no registro
de um servidor preenche os campos. Detalhes, o que fica visível no repositório
e o que fazer se a senha mestra for esquecida estão em
[`docs/workspace.md`](docs/workspace.md).

### Túnel SSH

Para o banco que só um bastion alcança, o **Connect to Server** e o registro
de um servidor têm **Conectar por um túnel SSH**: servidor e usuário SSH, e
arquivo de chave, senha ou agente SSH. Vale para PostgreSQL e SQL Server, no
Linux e no Windows, sem precisar do `ssh` instalado. O túnel vai junto com a
conexão (inclusive no `servers.json` do workspace); a senha do SSH ou a frase
secreta da chave ficam no cofre, e a chave privada é só o caminho do arquivo.
Detalhes em [`docs/ssh-tunnel.md`](docs/ssh-tunnel.md).

### Claude

O PostPilot trabalha com o [Claude Code](https://claude.com/claude-code)
instalado na máquina, com o login dele (sem chave de API):

- **View > Claude** abre um painel de conversa. O Claude vê os servidores,
  o catálogo, as janelas de consulta e os resultados; escreve consultas no
  editor (Ctrl+Z desfaz) e roda leituras numa transação `READ ONLY`.
  Qualquer coisa que altere dados ou esquema abre uma confirmação com o SQL.
- **Ações rápidas:** Explicar, Otimizar e Corrigir o Erro com o Claude no
  menu de contexto do editor (e em Query > Claude); Perguntar ao Claude nas
  tabelas e views do Object Explorer.
- **Claude Code no terminal:** com Tools > Allow Claude Code (MCP) ligado,
  o `claude` mexe no PostPilot aberto pelo plugin (`claude plugin
  marketplace add /usr/share/postpilot`, depois `claude plugin install
  postpilot@postpilot`) ou só pelo MCP (`claude mcp add postpilot --
  postpilot mcp`).

O passo a passo, inclusive no Omarchy, está em
[`docs/claude.md`](docs/claude.md); as decisões, no
[ADR 0003](docs/adr/0003-claude.md).

### Layout

Como no SSMS, um painel pode ser desafixado (auto-hide): "Auto Hide" no menu de
contexto da aba ou no menu Window o esconde numa faixa na borda; passar o mouse
ou clicar na faixa faz o painel deslizar por cima dos documentos, arrastar a
borda dele muda o tamanho (que fica salvo no layout), e o alfinete o fixa de novo.

## Atalhos de teclado

Os atalhos seguem o SSMS (F5 ou Ctrl+E executa, Ctrl+K, Ctrl+C comenta etc.). Para
mudar, crie o arquivo `keymap.txt` na pasta de configuração:

- Linux: `~/.config/PostPilot/keymap.txt`
- Windows: `%APPDATA%\PostPilot\keymap.txt`

Uma linha por atalho, com o nome do comando à esquerda:

```txt
# A primeira linha de um comando substitui os atalhos padrão dele.
Query.Execute = Ctrl+Enter
# Linhas seguintes do mesmo comando acrescentam atalhos.
Query.Execute = F5
# Sem atalho, o comando fica sem atalho.
File.Save =
```

Os nomes dos comandos estão em `crates/core/src/command.rs`. Linhas com erro são
ignoradas e avisadas na status bar.

## Desenvolvimento

Requer Rust estável (o `rust-toolchain.toml` instala rustfmt e clippy).

```sh
cargo run -p pgstudio-desktop     # executa o app
cargo test --workspace            # testes
PGSTUDIO_CONNECT=localhost PGPASSWORD=... cargo run -p pgstudio-desktop  # conecta direto
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

O CI roda formatação, clippy e testes só em Linux, e o `cargo-deny` para
licenças e vulnerabilidades. Windows saiu do CI para não gastar minutos do
GitHub Actions; o app e os pacotes de Windows (`cargo xtask dist`) continuam.

O repositório é público. `cargo test --workspace` inclui uma checagem de segredos
(`xtask/tests/sem_segredos.rs`) que falha se algum arquivo versionado tiver chave
privada, token, senha real numa connection string, `.env` ou e-mail pessoal.
Senhas de teste do PostgreSQL local (`postgres`, `secret`) são aceitas.

Os testes de `pg-client` e `pg-metadata` que precisam de servidor usam `PGSTUDIO_TEST_HOST`,
`PGSTUDIO_TEST_PORT`, `PGSTUDIO_TEST_USER` e `PGSTUDIO_TEST_PASSWORD`; sem
`PGSTUDIO_TEST_HOST` eles só avisam. No CI eles rodam contra o PostgreSQL 13 e 17
em container, com TLS ligado.

`cargo xtask pgmatrix` roda a suíte inteira contra cada versão suportada
(PostgreSQL 13, 14, 15, 16 e 17): cria um cluster descartável com o `initdb` de
cada versão, sobe com TLS, roda `cargo test --workspace` apontando para ele e
imprime uma tabela com o resultado. Os binários vêm de `/usr/lib/postgresql/<V>/bin`,
`/usr/pgsql-<V>/bin`, `/opt/pgmatrix/<V>/bin` ou `--pg-bin <V>=<pasta>`; a versão
sem binários aparece como "não testada" e faz o comando falhar. No Arch, que só
empacota a versão atual, as outras se compilam do código-fonte (veja
`cargo xtask pgmatrix --help`). A extensão `citext` (contrib) precisa estar
instalada em cada versão.

`cargo xtask bench` mede o tempo até o primeiro frame e a RAM em repouso contra
as metas (inicialização < 1 s, RAM < 100 MB). Os números ficam em
[`docs/benchmarks.md`](docs/benchmarks.md).

### Pacotes

`cargo xtask dist` compila no perfil `dist` (o `release` sem símbolos) e
gera em `target/packages/`, com um `SHA256SUMS`:

| Pacote | Ferramentas |
| --- | --- |
| `postpilot_<versão>_amd64.deb` | `dpkg-deb` |
| `PostPilot-<versão>-x86_64.AppImage` | `mksquashfs` (squashfs-tools); o runtime do AppImage é baixado na primeira vez para `target/packages/tools/`, ou use `APPIMAGE_RUNTIME=caminho` |
| `postpilot-<versão>-1-x86_64.pkg.tar.zst` | `bsdtar` (libarchive-tools no Ubuntu; no Arch já vem); `packaging/arch/PKGBUILD` gera o mesmo pacote com o `makepkg` |
| `PostPilot-<versão>-windows-x86_64.zip` | no Linux, compilação cruzada: `rustup target add x86_64-pc-windows-gnu`, o mingw-w64 e o `zip` |
| `PostPilot-<versão>-x86_64.msi` | o mesmo exe do zip e o `wixl` (pacote `wixl` no Ubuntu, `msitools` no Arch), que lê `packaging/windows/postpilot.wxs` |

O binário Linux sai ligado contra a glibc 2.31 quando há o
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)
(`cargo install cargo-zigbuild` e `pip install ziglang`); sem ele, ou com
`--native`, sai um build comum, que só roda com a glibc da máquina que
compilou ou mais nova.

`--only deb,appimage,arch,zip,msi` escolhe os formatos e `--no-build` reaproveita os
binários. No Ubuntu, as ferramentas vêm de
`sudo apt install dpkg-dev squashfs-tools libarchive-tools zip mingw-w64 wixl`. O ícone fica em
`packaging/icons/` (gerado por `make_icon.py`); o exe do Windows o recebe pelo
`windres` do MinGW, e o msi o usa no menu Iniciar e em Aplicativos instalados.

O `wixl` não tem as telas do WiX: o msi abre direto na barra de progresso, sem
escolher pasta. O `UpgradeCode` do `.wxs` é fixo; é ele que faz a versão nova
substituir a velha, então não o troque.

## Limitações conhecidas

O que não foi feito, ou foi feito e não pôde ser testado aqui. O que depende de
uma máquina de verdade tem os passos em
[`docs/na-sua-maquina.md`](docs/na-sua-maquina.md).

- **Windows nunca rodou num Windows de verdade.** O `.exe` e o `.msi` foram
  testados só no Wine, sem assinatura (o SmartScreen avisa). Os números de
  medição no Windows existem só para a partida e a RAM (marco 1.8, quando o CI
  ainda rodava no Windows).
- **Sem medição com GPU.** Todos os números de `docs/benchmarks.md` depois do
  marco 1.8 vêm de renderização por software. A RAM em repouso (cerca de
  129 MB) e o frame do diagrama de 2.000 tabelas, do plano de 2.000 nós e da
  exportação ficam acima da meta assim, com a interface gastando menos de
  1 ms por frame.
- **Wayland:** testado num sway sem tela, não no Hyprland; sem escala
  fracionária e sem mouse de verdade. File > Open precisa de um portal com
  seletor de arquivos (`xdg-desktop-portal-gtk`).
- **Query Designer:** não desenha CTE, subconsulta, UNION, `USING`,
  `OFFSET`, `DISTINCT ON`, `NULLS FIRST` nem dois grupos de OR no mesmo
  WHERE (avisa e não apaga a consulta).
- **Edição na grade:** uma tabela por vez, pela chave primária; sem chave
  primária, ou numa view, fica só leitura.
- **Alt+Break** (cancelar) não funciona: o egui não entrega a tecla
  Pause/Break. Cancel fica no botão ■ e no menu Query.
- **PostgreSQL 13 a 17:** a suíte passa nas cinco versões (`cargo xtask
  pgmatrix`), mas o 13 e o 15 testados não eram as últimas correções.
- **Claude no Windows:** o painel, o `postpilot mcp` e o plugin não foram
  testados num Windows de verdade, e o `.msi` não instala o plugin (o `.zip`
  traz). Veja [`docs/claude.md`](docs/claude.md#windows).
- **macOS** não é alvo e nunca foi testado.

## Roteiro

1. **Fase 0:** escolha do toolkit de GUI (egui, Slint ou GPUI).
2. **Fase 1:** shell no estilo SSMS (menus, toolbar, docking, abas, Object Explorer
   mockado, atalhos, layouts persistentes).
3. **Fase 2:** conexão PostgreSQL, introspecção, editor SQL com IntelliSense e grade
   virtualizada.
4. **Fase 3:** Table Designer, diagramas e Query Designer.
5. **Fase 4:** planos de execução, monitoramento, permissões, backup/restore e
   comparação de schemas.
6. **Fase 5:** edição e exportação na grade, Object Explorer completo, servidores
   registrados, Query Designer de mão dupla e pacotes de instalação.
7. **Fase 6:** acabamento: Query Designer, Schema Compare e servidores
   registrados completos, auto-hide, PostgreSQL 13 a 17, Wayland e `.msi`.
8. **Fase 7:** Claude no PostPilot: ferramentas, servidor MCP, plugin do
   Claude Code, painel do Claude no app e ações rápidas.

Versão mínima do PostgreSQL suportada: 13.

## Licença

Licenciado sob [MIT](LICENSE-MIT) ou [Apache-2.0](LICENSE-APACHE), à sua escolha.

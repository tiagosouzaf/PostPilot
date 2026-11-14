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

> As Fases 0 a 6 do roteiro estão prontas: janela no estilo do SSMS com
> painéis encaixáveis, Object Explorer, editor com IntelliSense, grade que
> edita e exporta, Table Designer, diagramas, Query Designer, planos de
> execução, Activity Monitor, segurança, backup e restore, Schema Compare e
> servidores registrados. O que ainda não foi testado está em
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
xtask/            tarefas de desenvolvimento (`cargo xtask bench`, `dist`, `pgmatrix`)
packaging/        ícone, atalho .desktop e textos dos pacotes de instalação
```

Novas crates entram na fase do roteiro que as usa.

## Instalação

`cargo xtask dist` gera os pacotes em `target/packages/` (veja
[Pacotes](#pacotes)):

- **Ubuntu, Debian e derivados:** `sudo apt install ./postpilot_<versão>_amd64.deb`.
  O PostPilot aparece no menu de aplicativos; para remover,
  `sudo apt remove postpilot`.
- **Arch Linux e derivados (Omarchy, Manjaro, EndeavourOS):**
  `sudo pacman -U postpilot-<versão>-1-x86_64.pkg.tar.zst`, ou compile do código
  com `cd packaging/arch && makepkg -si`. O atalho aparece no lançador de
  aplicativos; para remover, `sudo pacman -R postpilot`.
- **Outras distribuições Linux:** `chmod +x PostPilot-<versão>-x86_64.AppImage`
  e execute. Sem FUSE (alguns containers), use
  `./PostPilot-<versão>-x86_64.AppImage --appimage-extract-and-run`.
- **Windows 10 e 11 (64 bits):** dê dois cliques em
  `PostPilot-<versão>-x86_64.msi` (ou `msiexec /i PostPilot-<versão>-x86_64.msi`).
  Ele instala para todos os usuários em `C:\Program Files\PostPilot`, pede
  elevação, põe o atalho no menu Iniciar e o PostPilot no "Abrir com" dos
  arquivos `.sql` (sem virar o programa padrão). Uma versão nova substitui a
  anterior; para remover, Configurações > Aplicativos > PostPilot. Sem
  instalar, extraia o `.zip` e abra `postpilot.exe`. O exe e o msi não são
  assinados, então o SmartScreen pode avisar ("Mais informações" > "Executar
  assim mesmo"). O instalador só foi testado no Wine (`msiexec` do Wine 9), não
  num Windows de verdade.

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
uma opção), Ctrl+J lista, Tab ou Enter escolhe e Esc fecha. Strings, nomes
entre aspas, `$$` e comentários sem fim e parênteses sem par ficam sublinhados
em vermelho, assim como o trecho do erro que o servidor apontou na última
execução; o mouse em cima mostra a mensagem.

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

Para exportar, o menu de contexto da grade tem "Save Results As..." e "Copy
as INSERT". Ctrl+Shift+F ("Results to File") faz as próximas execuções
perguntarem um arquivo e escreverem as linhas nele direto do servidor, sem
guardá-las. A extensão escolhe o formato:

- `.csv`: como o `COPY ... (FORMAT csv, HEADER)` lê. NULL fica como campo
  vazio e o texto vazio como `""`.
- `.tsv` ou `.txt`: o formato texto do `COPY` (tab, `\N` para NULL).
- `.json`: uma lista de objetos.
- `.sql`: um `INSERT INTO table_name` por linha.

O CSV e o TSV voltam iguais com `COPY`; há um teste que faz isso com 1
milhão de linhas.

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
A árvore fica em `registered_servers.json`. Vários servidores aparecem juntos
no Object Explorer. "Remember password", no Connect to Server e no registro,
guarda a senha no cofre do sistema (Credential Manager no Windows, Secret
Service no Linux), nunca em arquivo; sem cofre, o PostPilot pede a senha.

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

O CI roda formatação, clippy e testes em Linux e Windows, e o `cargo-deny` para
licenças e vulnerabilidades.

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
  medição no Windows existem só para a partida e a RAM (marco 1.8, no CI).
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

Versão mínima do PostgreSQL suportada: 13.

## Licença

Licenciado sob [MIT](LICENSE-MIT) ou [Apache-2.0](LICENSE-APACHE), à sua escolha.

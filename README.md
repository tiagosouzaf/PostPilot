# PGStudio

Aplicativo desktop para administrar PostgreSQL, inspirado no Microsoft SQL Server
Management Studio (SSMS). Escrito 100% em Rust, para Windows e Linux, sem Electron
nem WebView.

> Em desenvolvimento inicial. Já abre a janela com os menus, a toolbar, os
> atalhos, os painéis encaixáveis e janelas de consulta que abrem e salvam
> arquivos `.sql` com realce de SQL, conecta a um PostgreSQL mostrando o
> catálogo no Object Explorer e executa consultas (F5) com resultados numa
> grade que aguenta milhões de linhas, com IntelliSense e scripts gerados
> pelo menu de contexto do Object Explorer.

## Estrutura

```txt
apps/desktop/     binário `pgstudio`
crates/core/      tipos de domínio, comandos e keymap, sem GUI e sem I/O
crates/ui-shell/  janela, menus, toolbar, status bar e tema (egui)
crates/docking/   painéis encaixáveis (invólucro do egui_dock)
crates/object-explorer/  árvore do Object Explorer e provedor de demonstração
crates/pg-client/ conexão com o PostgreSQL (tokio-postgres, TLS com rustls)
crates/pg-metadata/ catálogo do PostgreSQL para o Object Explorer
crates/sql-editor/ editor de SQL: realce, números de linha, busca, Go To Line
crates/result-grid/ buffer paginado, grade de resultados e resultados em texto
xtask/            tarefas de desenvolvimento (`cargo xtask bench`, `dist`, `pgmatrix`)
packaging/        ícone, atalho .desktop e textos dos pacotes de instalação
```

Novas crates entram na fase do roteiro que as usa.

## Instalação

`cargo xtask dist` gera os pacotes em `target/packages/` (veja
[Pacotes](#pacotes)):

- **Ubuntu, Debian e derivados:** `sudo apt install ./pgstudio_<versão>_amd64.deb`.
  O PGStudio aparece no menu de aplicativos; para remover,
  `sudo apt remove pgstudio`.
- **Arch Linux e derivados (Omarchy, Manjaro, EndeavourOS):**
  `sudo pacman -U pgstudio-<versão>-1-x86_64.pkg.tar.zst`, ou compile do código
  com `cd packaging/arch && makepkg -si`. O atalho aparece no lançador de
  aplicativos; para remover, `sudo pacman -R pgstudio`.
- **Outras distribuições Linux:** `chmod +x PGStudio-<versão>-x86_64.AppImage`
  e execute. Sem FUSE (alguns containers), use
  `./PGStudio-<versão>-x86_64.AppImage --appimage-extract-and-run`.
- **Windows 10 e 11 (64 bits):** extraia o `.zip` e abra `pgstudio.exe`. Não
  há instalador; o SmartScreen pode avisar porque o exe não é assinado ("Mais
  informações" > "Executar assim mesmo").

No Linux, o binário é ligado contra a glibc 2.31 (Ubuntu 20.04, Debian 11 e
posteriores) e precisa de Vulkan ou OpenGL ES (o Mesa de qualquer desktop
serve). Os pacotes foram testados em Ubuntu 22.04 e 24.04 limpos; no 20.04 o
binário carrega, mas o renderizador por software do Mesa daquela versão é
antigo demais para o wgpu (numa GPU de verdade deve abrir, mas não foi
testado).

## Uso

```sh
pgstudio [arquivo.sql ...]
```

Ao abrir, o PGStudio mostra o "Connect to Server", como o SSMS: Server name
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
ao abrir. Se o arquivo estiver estragado, o PGStudio abre com o layout padrão e
avisa na status bar. Window > Reset Window Layout volta ao padrão a qualquer
momento.

Como no SSMS, um painel pode ser desafixado (auto-hide): "Auto Hide" no menu de
contexto da aba ou no menu Window o esconde numa faixa na borda; clicar na
faixa mostra o painel por cima dos documentos, e o alfinete o fixa de novo.

## Atalhos de teclado

Os atalhos seguem o SSMS (F5 ou Ctrl+E executa, Ctrl+K, Ctrl+C comenta etc.). Para
mudar, crie o arquivo `keymap.txt` na pasta de configuração:

- Linux: `~/.config/PGStudio/keymap.txt`
- Windows: `%APPDATA%\PGStudio\keymap.txt`

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
| `pgstudio_<versão>_amd64.deb` | `dpkg-deb` |
| `PGStudio-<versão>-x86_64.AppImage` | `mksquashfs` (squashfs-tools); o runtime do AppImage é baixado na primeira vez para `target/packages/tools/`, ou use `APPIMAGE_RUNTIME=caminho` |
| `pgstudio-<versão>-1-x86_64.pkg.tar.zst` | `bsdtar` (libarchive-tools no Ubuntu; no Arch já vem); `packaging/arch/PKGBUILD` gera o mesmo pacote com o `makepkg` |
| `PGStudio-<versão>-windows-x86_64.zip` | no Linux, compilação cruzada: `rustup target add x86_64-pc-windows-gnu`, o mingw-w64 e o `zip` |

O binário Linux sai ligado contra a glibc 2.31 quando há o
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)
(`cargo install cargo-zigbuild` e `pip install ziglang`); sem ele, ou com
`--native`, sai um build comum, que só roda com a glibc da máquina que
compilou ou mais nova.

`--only deb,appimage,arch,zip` escolhe os formatos e `--no-build` reaproveita os
binários. No Ubuntu, as ferramentas vêm de
`sudo apt install dpkg-dev squashfs-tools libarchive-tools zip mingw-w64`. O ícone fica em
`packaging/icons/` (gerado por `make_icon.py`); o exe do Windows o recebe pelo
`windres` do MinGW. O `.msi` fica para quando houver uma máquina ou CI com
Windows.

## Roteiro

1. **Fase 0:** escolha do toolkit de GUI (egui, Slint ou GPUI).
2. **Fase 1:** shell no estilo SSMS (menus, toolbar, docking, abas, Object Explorer
   mockado, atalhos, layouts persistentes).
3. **Fase 2:** conexão PostgreSQL, introspecção, editor SQL com IntelliSense e grade
   virtualizada.
4. **Fase 3:** Table Designer, diagramas e Query Designer.
5. **Fase 4:** planos de execução, monitoramento, permissões, backup/restore e
   comparação de schemas.

Versão mínima do PostgreSQL suportada: 13.

## Licença

Licenciado sob [MIT](LICENSE-MIT) ou [Apache-2.0](LICENSE-APACHE), à sua escolha.

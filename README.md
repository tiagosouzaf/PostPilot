# PGStudio

Aplicativo desktop para administrar PostgreSQL, inspirado no Microsoft SQL Server
Management Studio (SSMS). Escrito 100% em Rust, para Windows e Linux, sem Electron
nem WebView.

> Em desenvolvimento inicial. Já abre a janela com os menus, a toolbar, os
> atalhos, os painéis encaixáveis e janelas de consulta que abrem e salvam
> arquivos `.sql`, e conecta a um PostgreSQL mostrando os bancos no Object
> Explorer. Ainda não executa consultas.

## Estrutura

```txt
apps/desktop/     binário `pgstudio`
crates/core/      tipos de domínio, comandos e keymap, sem GUI e sem I/O
crates/ui-shell/  janela, menus, toolbar, status bar e tema (egui)
crates/docking/   painéis encaixáveis (invólucro do egui_dock)
crates/object-explorer/  árvore do Object Explorer e provedor de demonstração
crates/pg-client/ conexão com o PostgreSQL (tokio-postgres, TLS com rustls)
xtask/            tarefas de desenvolvimento (`cargo xtask bench`)
```

Novas crates (`pg-metadata`, `sql-editor`, …) entram na fase do roteiro que as usa.

## Uso

```sh
pgstudio [arquivo.sql ...]
```

Ao abrir, o PGStudio mostra o "Connect to Server", como o SSMS: Server name
(`host` ou `host,porta`), Login, Password, Database e Encryption (Disable,
Prefer, Require ou Verify full, os modos `sslmode` do libpq). Depois de conectar,
o servidor e os bancos aparecem no Object Explorer; File > Connect Object
Explorer abre o diálogo de novo. As conexões usadas ficam em
`connections.json`, na pasta de configuração, sem a senha; a seta ao lado do
Server name lista as recentes.

Ctrl+N abre uma janela de consulta, Ctrl+O abre um arquivo, Ctrl+S e
Ctrl+Shift+S salvam e Ctrl+F4 fecha. A aba mostra `*` quando há alterações, e
fechar a aba ou o aplicativo pergunta antes de descartar.

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

Os testes de `pg-client` que precisam de servidor usam `PGSTUDIO_TEST_HOST`,
`PGSTUDIO_TEST_PORT`, `PGSTUDIO_TEST_USER` e `PGSTUDIO_TEST_PASSWORD`; sem
`PGSTUDIO_TEST_HOST` eles só avisam. No CI eles rodam contra o PostgreSQL 13 e 17
em container, com TLS ligado.

`cargo xtask bench` mede o tempo até o primeiro frame e a RAM em repouso contra
as metas (inicialização < 1 s, RAM < 100 MB). Os números ficam em
[`docs/benchmarks.md`](docs/benchmarks.md).

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

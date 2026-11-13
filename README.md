# PGStudio

Aplicativo desktop para administrar PostgreSQL, inspirado no Microsoft SQL Server
Management Studio (SSMS). Escrito 100% em Rust, para Windows e Linux, sem Electron
nem WebView.

> Em desenvolvimento inicial. Ainda não há interface gráfica: o marco atual é a
> fundação do repositório (workspace, CI e licenças).

## Estrutura

```txt
apps/desktop/   binário `pgstudio`
crates/core/    tipos de domínio, sem GUI e sem I/O
```

Novas crates (`ui-shell`, `docking`, `object-explorer`, `pg-connection`, …) entram
na fase do roteiro que as usa.

## Desenvolvimento

Requer Rust estável (o `rust-toolchain.toml` instala rustfmt e clippy).

```sh
cargo run -p pgstudio-desktop     # executa o app
cargo test --workspace            # testes
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

O CI roda formatação, clippy e testes em Linux e Windows, e o `cargo-deny` para
licenças e vulnerabilidades.

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

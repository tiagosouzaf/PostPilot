//! Núcleo do PGStudio: tipos de domínio, comandos e configurações.
//!
//! Esta crate não depende de toolkit de GUI nem faz I/O. A camada de UI e o
//! backend se comunicam através dos tipos definidos aqui.

/// Nome do produto exibido na interface.
pub const APP_NAME: &str = "PGStudio";

/// Versão do PGStudio, igual à versão do workspace Cargo.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Título padrão da janela principal, por exemplo `PGStudio 0.1.0`.
pub fn window_title() -> String {
    format!("{APP_NAME} {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_title_has_name_and_version() {
        assert_eq!(
            window_title(),
            format!("PGStudio {}", env!("CARGO_PKG_VERSION"))
        );
    }
}

//! Núcleo do PGStudio: tipos de domínio, comandos e configurações.
//!
//! Esta crate não depende de toolkit de GUI nem faz I/O. A camada de UI e o
//! backend se comunicam através dos tipos definidos aqui.

pub mod catalog;
pub mod command;
pub mod connection;
pub mod keymap;
pub mod metadata;
pub mod query;

pub use command::CommandId;
/// O modelo do schema dos designers (Fase 3).
pub use pgstudio_schema_model as schema;

/// Nome do produto exibido na interface.
pub const APP_NAME: &str = "PGStudio";

/// Versão do PGStudio, igual à versão do workspace Cargo.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Título padrão da janela principal, por exemplo `PGStudio 0.1.0`.
pub fn window_title() -> String {
    format!("{APP_NAME} {VERSION}")
}

/// Tema de cores da interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Theme {
    /// Tema claro, o padrão do SSMS.
    #[default]
    Light,
    /// Tema escuro.
    Dark,
}

impl Theme {
    /// Todos os temas, na ordem em que aparecem nos menus.
    pub const ALL: [Theme; 2] = [Theme::Light, Theme::Dark];

    /// Nome exibido no menu, no idioma da interface.
    pub fn label(self) -> &'static str {
        pgstudio_i18n::tr(match self {
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        })
    }
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

    #[test]
    fn default_theme_is_light() {
        assert_eq!(Theme::default(), Theme::Light);
    }
}

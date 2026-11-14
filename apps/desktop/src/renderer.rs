//! Escolha do renderizador gráfico: wgpu (Vulkan, Metal, Direct3D ou
//! OpenGL ES, o padrão) ou glow (OpenGL direto).
//!
//! A escolha vem da linha de comando (`--renderer glow` ou
//! `--renderer=glow`) ou da variável `PGSTUDIO_RENDERER`; a linha de comando
//! vence. Sem nenhuma das duas, o app usa o wgpu e, se ele não conseguir
//! abrir (sem driver Vulkan nem GL que sirva), reabre sozinho com o glow.

use std::ffi::OsString;
use std::path::PathBuf;

use eframe::Renderer;

/// Variável de ambiente com o renderizador.
pub const ENV: &str = "PGSTUDIO_RENDERER";
/// Opção da linha de comando.
const FLAG: &str = "--renderer";

/// Renderizador pedido e os arquivos da linha de comando.
#[derive(Debug, PartialEq)]
pub struct Launch {
    /// `None` quando ninguém escolheu: usa o wgpu com o glow de reserva.
    pub renderer: Option<Renderer>,
    pub files: Vec<PathBuf>,
    /// Avisos para o stderr (valor desconhecido, opção sem valor).
    pub warnings: Vec<String>,
}

impl Launch {
    /// Lê os argumentos (sem o nome do programa) e o valor de
    /// `PGSTUDIO_RENDERER`. Um valor desconhecido vira aviso e o app abre
    /// com o padrão, em vez de não abrir: quem lança pelo menu não vê o
    /// stderr.
    pub fn parse(args: impl IntoIterator<Item = OsString>, env: Option<&str>) -> Self {
        let mut launch = Launch {
            renderer: None,
            files: Vec::new(),
            warnings: Vec::new(),
        };
        let mut from_args = None;
        let mut args = args.into_iter();
        let mut only_files = false;
        while let Some(arg) = args.next() {
            if only_files {
                launch.files.push(arg.into());
                continue;
            }
            let text = arg.to_str();
            if text == Some("--") {
                only_files = true;
            } else if text == Some(FLAG) {
                match args.next() {
                    Some(value) => from_args = Some(value.to_string_lossy().into_owned()),
                    None => launch
                        .warnings
                        .push(format!("{FLAG} needs a value: wgpu or glow")),
                }
            } else if let Some(value) = text.and_then(|t| t.strip_prefix("--renderer=")) {
                from_args = Some(value.to_string());
            } else {
                launch.files.push(arg.into());
            }
        }
        let (source, value) = match (&from_args, env.filter(|v| !v.trim().is_empty())) {
            (Some(v), _) => (FLAG, v.as_str()),
            (None, Some(v)) => (ENV, v),
            (None, None) => return launch,
        };
        match parse_name(value) {
            Some(r) => launch.renderer = Some(r),
            None => launch.warnings.push(format!(
                "{source}: unknown renderer {value:?} (use wgpu or glow); using the default"
            )),
        }
        launch
    }

    /// Opções do eframe para esta escolha.
    pub fn eframe_renderer(&self) -> Renderer {
        self.renderer.unwrap_or(Renderer::Wgpu)
    }

    /// Se o wgpu falhou sem escolha explícita, o app deve reabrir com o
    /// glow.
    pub fn should_fall_back(&self, error: &eframe::Error) -> bool {
        self.renderer.is_none() && matches!(error, eframe::Error::Wgpu(_))
    }
}

/// `wgpu` ou `glow`, sem diferenciar maiúsculas.
fn parse_name(name: &str) -> Option<Renderer> {
    match name.trim().to_ascii_lowercase().as_str() {
        "wgpu" => Some(Renderer::Wgpu),
        "glow" | "gl" | "opengl" => Some(Renderer::Glow),
        _ => None,
    }
}

/// Descrição do renderizador para o "About": o adaptador do wgpu (backend e
/// placa) ou só "glow (OpenGL)".
pub fn describe(cc: &eframe::CreationContext<'_>) -> String {
    if let Some(state) = &cc.wgpu_render_state {
        let info = state.adapter.get_info();
        return format!("wgpu, {:?}, {}", info.backend, info.name);
    }
    if cc.gl.is_some() {
        return "glow (OpenGL)".into();
    }
    "unknown".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(args: &[&str], env: Option<&str>) -> Launch {
        Launch::parse(args.iter().map(OsString::from), env)
    }

    #[test]
    fn default_is_wgpu_with_fallback() {
        let l = launch(&["a.sql"], None);
        assert_eq!(l.renderer, None);
        assert_eq!(l.eframe_renderer(), Renderer::Wgpu);
        assert_eq!(l.files, vec![PathBuf::from("a.sql")]);
        assert!(l.warnings.is_empty());
        let wgpu_error =
            eframe::Error::Wgpu(eframe::egui_wgpu::WgpuError::NoSurfaceFormatsAvailable);
        assert!(l.should_fall_back(&wgpu_error));
    }

    #[test]
    fn flag_in_both_forms_and_case() {
        assert_eq!(
            launch(&["--renderer", "glow", "a.sql"], None).renderer,
            Some(Renderer::Glow)
        );
        let l = launch(&["a.sql", "--renderer=GLOW", "b.sql"], None);
        assert_eq!(l.renderer, Some(Renderer::Glow));
        assert_eq!(
            l.files,
            vec![PathBuf::from("a.sql"), PathBuf::from("b.sql")]
        );
    }

    #[test]
    fn flag_wins_over_env() {
        assert_eq!(
            launch(&["--renderer=wgpu"], Some("glow")).renderer,
            Some(Renderer::Wgpu)
        );
        assert_eq!(launch(&[], Some("glow")).renderer, Some(Renderer::Glow));
        assert_eq!(launch(&[], Some("  ")).renderer, None);
    }

    #[test]
    fn explicit_choice_never_falls_back() {
        let l = launch(&[], Some("wgpu"));
        let wgpu_error =
            eframe::Error::Wgpu(eframe::egui_wgpu::WgpuError::NoSurfaceFormatsAvailable);
        assert!(!l.should_fall_back(&wgpu_error));
    }

    #[test]
    fn unknown_value_warns_and_uses_default() {
        let l = launch(&["--renderer", "vulkan"], None);
        assert_eq!(l.renderer, None);
        assert_eq!(l.warnings.len(), 1);
        assert!(l.warnings[0].contains("vulkan"), "{:?}", l.warnings);
        let l = launch(&["--renderer"], None);
        assert_eq!(l.warnings.len(), 1);
    }

    #[test]
    fn double_dash_ends_options() {
        let l = launch(&["--", "--renderer=glow"], None);
        assert_eq!(l.renderer, None);
        assert_eq!(l.files, vec![PathBuf::from("--renderer=glow")]);
    }
}

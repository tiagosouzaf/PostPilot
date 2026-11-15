//! Tema claro/escuro do sistema, para a opção "Same as system".
//!
//! No Linux a preferência vem do portal do freedesktop
//! (`org.freedesktop.portal.Settings`, chave `org.freedesktop.appearance
//! color-scheme`), que GNOME, KDE e o Omarchy mantêm atualizada: o Omarchy,
//! ao trocar de tema, grava `org.gnome.desktop.interface color-scheme` e o
//! xdg-desktop-portal-gtk avisa a troca com o sinal `SettingChanged`. Uma
//! thread escuta o sinal e acorda a interface.
//!
//! Nos outros sistemas vale o que o winit informa (`Context::system_theme`).
//!
//! No Omarchy o app vai além do claro/escuro e usa as cores do tema atual:
//! `~/.config/omarchy/current/theme/colors.toml` (fundo, texto, destaque e
//! as 16 cores do terminal). O `omarchy-theme-set` troca essa pasta inteira
//! a cada mudança de tema; outra thread relê o arquivo a cada segundo e
//! avisa quando o conteúdo muda.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use eframe::egui::{self, Color32};
use pgstudio_core::Theme;

use crate::theme::SystemColors;

/// De quanto em quanto tempo o arquivo de cores do Omarchy é relido.
const COLORS_POLL: Duration = Duration::from_secs(1);

/// Lê `#rrggbb` (ou `#rgb`, ou sem o `#`).
fn parse_hex(text: &str) -> Option<Color32> {
    let hex = text.trim().trim_start_matches('#');
    let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
    match hex.len() {
        3 => Some(Color32::from_rgb(
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
        )),
        6 | 8 => {
            let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
            Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?))
        }
        _ => None,
    }
}

/// Lê o `colors.toml` de um tema do Omarchy: linhas `chave = "#rrggbb"`.
/// Precisa ter `background` e `foreground`; as outras chaves são opcionais
/// e as desconhecidas são ignoradas.
pub fn parse_colors(text: &str) -> Option<SystemColors> {
    let mut background = None;
    let mut foreground = None;
    let mut accent = None;
    let mut ansi = [None; 16];
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        // O valor fica entre as primeiras aspas; o resto é comentário.
        let value = match value.strip_prefix(['"', '\'']) {
            Some(rest) => rest.split(['"', '\'']).next().unwrap_or(""),
            None => value.split('#').next().unwrap_or(""),
        };
        let Some(color) = parse_hex(value) else {
            continue;
        };
        match key.trim().trim_matches(['"', '\'']) {
            "background" => background = Some(color),
            "foreground" => foreground = Some(color),
            "accent" => accent = Some(color),
            key => {
                if let Some(n) = key.strip_prefix("color")
                    && let Ok(n) = n.parse::<usize>()
                    && n < 16
                {
                    ansi[n] = Some(color);
                }
            }
        }
    }
    Some(SystemColors {
        background: background?,
        foreground: foreground?,
        accent,
        ansi,
    })
}

/// Onde o Omarchy guarda as cores do tema atual: `<config>/omarchy/current`
/// (ou, em outras versões, `<state>/omarchy/current`).
pub fn omarchy_colors_files() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dir = |var: &str, fallback: &str| {
        std::env::var_os(var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(fallback)))
    };
    [
        dir("XDG_CONFIG_HOME", ".config"),
        dir("XDG_STATE_HOME", ".local/state"),
    ]
    .into_iter()
    .flatten()
    .map(|d| d.join("omarchy/current/theme/colors.toml"))
    .collect()
}

/// Cores do primeiro arquivo que existir e estiver bom.
fn read_colors(files: &[PathBuf]) -> Option<(String, SystemColors)> {
    files.iter().find_map(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        let colors = parse_colors(&text)?;
        Some((text, colors))
    })
}

/// Se algum dos arquivos fica numa instalação do Omarchy (a pasta
/// `omarchy` existe), mesmo que o arquivo de cores ainda não.
fn has_omarchy(files: &[PathBuf]) -> bool {
    files
        .iter()
        .filter_map(|f| f.ancestors().nth(3))
        .any(Path::is_dir)
}

/// Abre a thread que relê as cores e avisa a cada mudança. A troca de tema
/// apaga a pasta e põe outra no lugar: enquanto o arquivo não existe (ou
/// está pela metade), valem as últimas cores lidas.
fn watch_colors(
    ctx: egui::Context,
    files: Vec<PathBuf>,
    mut last: Option<String>,
) -> Receiver<SystemColors> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("omarchy-colors".into())
        .spawn(move || {
            loop {
                std::thread::sleep(COLORS_POLL);
                let Some((text, colors)) = read_colors(&files) else {
                    continue;
                };
                if last.as_deref() == Some(text.as_str()) {
                    continue;
                }
                last = Some(text);
                if tx.send(colors).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        eprintln!("system theme: {e}");
    }
    rx
}

/// Converte o valor de `color-scheme` do portal: 1 é escuro, 2 é claro e 0
/// (ou outro valor) é "sem preferência".
pub fn from_color_scheme(value: u32) -> Option<Theme> {
    match value {
        1 => Some(Theme::Dark),
        2 => Some(Theme::Light),
        _ => None,
    }
}

/// Acompanha o tema do sistema.
#[derive(Default)]
pub struct SystemTheme {
    /// Avisos da thread do portal (só no Linux).
    rx: Option<Receiver<Option<Theme>>>,
    /// Último tema conhecido do sistema (`None`: sem preferência).
    current: Option<Theme>,
    /// Avisos da thread que relê as cores do Omarchy.
    colors_rx: Option<Receiver<SystemColors>>,
    /// Cores do tema do Omarchy (`None`: não é um Omarchy).
    colors: Option<SystemColors>,
    /// Se já começou a acompanhar.
    started: bool,
}

impl SystemTheme {
    /// Começa a acompanhar o tema do sistema. Espera um pouco pela primeira
    /// resposta, para o app já abrir com o tema certo.
    pub fn start(ctx: &egui::Context) -> Self {
        let mut this = SystemTheme {
            started: true,
            ..Default::default()
        };
        #[cfg(target_os = "linux")]
        {
            let rx = portal::spawn(ctx.clone());
            if let Ok(theme) = rx.recv_timeout(Duration::from_millis(400)) {
                this.current = theme;
            }
            this.rx = Some(rx);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = Duration::ZERO;
            this.current = winit_theme(ctx);
        }
        this.watch_colors(ctx, omarchy_colors_files());
        this
    }

    /// Lê as cores do Omarchy nesses arquivos e passa a acompanhá-las. Sem
    /// Omarchy, não faz nada.
    pub fn watch_colors(&mut self, ctx: &egui::Context, files: Vec<PathBuf>) {
        if !has_omarchy(&files) {
            return;
        }
        self.started = true;
        let first = read_colors(&files);
        self.colors = first.as_ref().map(|(_, c)| *c);
        self.colors_rx = Some(watch_colors(ctx.clone(), files, first.map(|(t, _)| t)));
    }

    /// Cores do tema do sistema (hoje, só no Omarchy).
    pub fn colors(&self) -> Option<SystemColors> {
        self.colors
    }

    /// Tema do sistema agora (`None`: sem preferência ou desconhecido).
    pub fn current(&self) -> Option<Theme> {
        self.current
    }

    /// Lê as novidades. Devolve `true` quando o tema do sistema (claro ou
    /// escuro, ou as cores) mudou.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        if !self.started {
            return false;
        }
        let before = self.current;
        let colors_before = self.colors;
        if let Some(rx) = &self.colors_rx {
            while let Ok(colors) = rx.try_recv() {
                self.colors = Some(colors);
            }
        }
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(theme) => self.current = theme,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.rx = None;
                        break;
                    }
                }
            }
        } else if !cfg!(target_os = "linux") {
            self.current = winit_theme(ctx);
        }
        self.current != before || self.colors != colors_before
    }
}

fn winit_theme(ctx: &egui::Context) -> Option<Theme> {
    ctx.system_theme().map(|t| match t {
        egui::Theme::Dark => Theme::Dark,
        egui::Theme::Light => Theme::Light,
    })
}

#[cfg(target_os = "linux")]
mod portal {
    use std::sync::mpsc::{Sender, channel};

    use eframe::egui;
    use pgstudio_core::Theme;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedValue, Value};

    const NAMESPACE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";

    /// Abre a thread que lê e escuta o portal.
    pub fn spawn(ctx: egui::Context) -> std::sync::mpsc::Receiver<Option<Theme>> {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("system-theme".into())
            .spawn(move || {
                if let Err(e) = watch(&ctx, &tx) {
                    eprintln!("system theme: {e}");
                }
            });
        if let Err(e) = spawned {
            eprintln!("system theme: {e}");
        }
        rx
    }

    fn watch(ctx: &egui::Context, tx: &Sender<Option<Theme>>) -> zbus::Result<()> {
        let conn = Connection::session()?;
        let proxy = Proxy::new(
            &conn,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )?;
        // Assina antes de ler, para não perder uma troca entre os dois.
        let signals = proxy.receive_signal("SettingChanged")?;
        let first = read(&proxy).unwrap_or(None);
        if tx.send(first).is_err() {
            return Ok(());
        }
        for message in signals {
            let Ok((namespace, key, value)) =
                message.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            if namespace != NAMESPACE || key != KEY {
                continue;
            }
            if tx.send(scheme(&value)).is_err() {
                break;
            }
            ctx.request_repaint();
        }
        Ok(())
    }

    /// Lê o valor atual: `ReadOne` (portal novo) ou `Read` (antigo, que
    /// devolve o valor dentro de mais uma variante).
    fn read(proxy: &Proxy<'_>) -> zbus::Result<Option<Theme>> {
        let value: OwnedValue = match proxy.call("ReadOne", &(NAMESPACE, KEY)) {
            Ok(v) => v,
            Err(_) => proxy.call("Read", &(NAMESPACE, KEY))?,
        };
        Ok(scheme(&value))
    }

    fn scheme(value: &Value<'_>) -> Option<Theme> {
        match value {
            Value::U32(n) => super::from_color_scheme(*n),
            Value::Value(inner) => scheme(inner),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_plain_and_nested_values() {
            assert_eq!(scheme(&Value::U32(1)), Some(Theme::Dark));
            assert_eq!(scheme(&Value::U32(2)), Some(Theme::Light));
            assert_eq!(scheme(&Value::U32(0)), None);
            let nested = Value::Value(Box::new(Value::U32(2)));
            assert_eq!(scheme(&nested), Some(Theme::Light));
            assert_eq!(scheme(&Value::Str("dark".into())), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_scheme_values() {
        assert_eq!(from_color_scheme(0), None);
        assert_eq!(from_color_scheme(1), Some(Theme::Dark));
        assert_eq!(from_color_scheme(2), Some(Theme::Light));
        assert_eq!(from_color_scheme(7), None);
    }

    const TOKYO_NIGHT: &str = r##"accent = "#7aa2f7"
cursor = "#c0caf5"
foreground = "#a9b1d6"
background = "#1a1b26"
selection_background = "#7aa2f7"

color0 = "#32344a"
color4 = "#7aa2f7"
color15 = "#acb0d0"
color16 = "#ffffff"
"##;

    #[test]
    fn reads_an_omarchy_colors_file() {
        let c = parse_colors(TOKYO_NIGHT).unwrap();
        assert_eq!(c.background, Color32::from_rgb(0x1a, 0x1b, 0x26));
        assert_eq!(c.foreground, Color32::from_rgb(0xa9, 0xb1, 0xd6));
        assert_eq!(c.accent, Some(Color32::from_rgb(0x7a, 0xa2, 0xf7)));
        assert_eq!(c.ansi[0], Some(Color32::from_rgb(0x32, 0x34, 0x4a)));
        assert_eq!(c.ansi[4], Some(Color32::from_rgb(0x7a, 0xa2, 0xf7)));
        assert_eq!(c.ansi[1], None);
        assert_eq!(c.theme(), Theme::Dark);
    }

    #[test]
    fn tolerates_other_spellings_and_needs_the_two_main_colors() {
        let c = parse_colors(
            "# tema\nbackground='#FFF' # claro\nforeground = 222222\nmode = \"light\"\n",
        )
        .unwrap();
        assert_eq!(c.background, Color32::WHITE);
        assert_eq!(c.foreground, Color32::from_rgb(0x22, 0x22, 0x22));
        assert_eq!(c.theme(), Theme::Light);
        assert_eq!(parse_colors("background = \"#000000\"\n"), None);
        assert_eq!(
            parse_colors("background = \"#00\"\nforeground = \"#fff\""),
            None
        );
        assert_eq!(parse_colors(""), None);
    }

    #[test]
    fn follows_the_colors_file_while_it_changes() {
        let dir = std::env::temp_dir().join(format!("postpilot-omarchy-{}", std::process::id()));
        let theme = dir.join("omarchy/current/theme");
        std::fs::create_dir_all(&theme).unwrap();
        let file = theme.join("colors.toml");
        std::fs::write(&file, TOKYO_NIGHT).unwrap();
        let ctx = egui::Context::default();
        let mut s = SystemTheme::default();
        s.watch_colors(&ctx, vec![file.clone()]);
        assert_eq!(s.colors().map(|c| c.theme()), Some(Theme::Dark));
        assert!(!s.poll(&ctx));

        // A troca de tema apaga a pasta e põe outra: sem arquivo, nada muda.
        std::fs::remove_dir_all(&theme).unwrap();
        std::thread::sleep(COLORS_POLL + Duration::from_millis(300));
        assert!(!s.poll(&ctx));
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(
            &file,
            "background = \"#eff1f5\"\nforeground = \"#4c4f69\"\n",
        )
        .unwrap();
        let mut changed = false;
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(100));
            if s.poll(&ctx) {
                changed = true;
                break;
            }
        }
        assert!(changed);
        assert_eq!(s.colors().map(|c| c.theme()), Some(Theme::Light));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_omarchy_there_are_no_colors() {
        let ctx = egui::Context::default();
        let mut s = SystemTheme::default();
        s.watch_colors(
            &ctx,
            vec!["/nonexistent/omarchy/current/theme/colors.toml".into()],
        );
        assert_eq!(s.colors(), None);
        assert!(!s.poll(&ctx));
    }

    #[test]
    fn not_started_never_reports_a_change() {
        let ctx = egui::Context::default();
        let mut s = SystemTheme::default();
        assert!(!s.poll(&ctx));
        assert_eq!(s.current(), None);
    }
}

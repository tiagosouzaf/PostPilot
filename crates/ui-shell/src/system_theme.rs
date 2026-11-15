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

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui;
use pgstudio_core::Theme;

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
        this
    }

    /// Tema do sistema agora (`None`: sem preferência ou desconhecido).
    pub fn current(&self) -> Option<Theme> {
        self.current
    }

    /// Lê as novidades. Devolve `true` quando o tema do sistema mudou.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        if !self.started {
            return false;
        }
        let before = self.current;
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
        self.current != before
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

    #[test]
    fn not_started_never_reports_a_change() {
        let ctx = egui::Context::default();
        let mut s = SystemTheme::default();
        assert!(!s.poll(&ctx));
        assert_eq!(s.current(), None);
    }
}

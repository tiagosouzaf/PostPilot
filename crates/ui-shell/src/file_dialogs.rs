//! Diálogos de arquivo sem travar a janela.
//!
//! Os diálogos nativos (rfd) esperam a pessoa escolher. Rodando na thread
//! da interface, a janela para de desenhar e de responder ao compositor
//! enquanto o seletor está aberto; no Hyprland isso vira o aviso "O
//! aplicativo não está respondendo" depois de poucos segundos (o que
//! acontecia em File > New Workspace na 0.2.1). Aqui a pergunta vai para
//! outra thread e a resposta volta num frame seguinte, para quem pediu.
//!
//! Diálogos que respondem na hora (os dos testes, `blocks() == false`)
//! continuam síncronos: a resposta é tratada dentro da própria chamada.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::ShellApp;
use crate::documents::FileDialogs;

/// O que perguntar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Question {
    /// Onde salvar; nome sugerido e arquivo atual.
    Save {
        name: String,
        current: Option<PathBuf>,
    },
    /// Um arquivo para abrir.
    Open,
    /// Onde salvar resultados; nome sugerido.
    SaveResults { name: String },
    /// Uma pasta; título do diálogo.
    Folder { title: String },
}

impl Question {
    /// `Save` sem arquivo atual.
    pub fn save(name: impl Into<String>) -> Self {
        Question::Save {
            name: name.into(),
            current: None,
        }
    }

    fn ask(&self, dialogs: &mut dyn FileDialogs) -> Option<PathBuf> {
        match self {
            Question::Save { name, current } => dialogs.save_path(name, current.as_deref()),
            Question::Open => dialogs.open_path(),
            Question::SaveResults { name } => dialogs.save_results_path(name),
            Question::Folder { title } => dialogs.folder_path(title),
        }
    }
}

/// O que fazer com a resposta (`None`: cancelado).
pub(crate) type Then = Box<dyn FnOnce(&mut ShellApp, &egui::Context, Option<PathBuf>)>;

/// Diálogos compartilhados com a thread que pergunta.
pub(crate) type SharedDialogs = Arc<Mutex<Box<dyn FileDialogs + Send>>>;

/// Uma pergunta em andamento noutra thread.
pub(crate) struct Pending {
    rx: Receiver<Option<PathBuf>>,
    then: Then,
}

impl Pending {
    fn poll(&self) -> Option<Option<PathBuf>> {
        match self.rx.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => None,
            // A thread do diálogo caiu: conta como cancelado.
            Err(TryRecvError::Disconnected) => Some(None),
        }
    }
}

pub(crate) fn shared(dialogs: impl FileDialogs + Send + 'static) -> SharedDialogs {
    Arc::new(Mutex::new(Box::new(dialogs)))
}

fn lock(d: &SharedDialogs) -> std::sync::MutexGuard<'_, Box<dyn FileDialogs + Send>> {
    d.lock().unwrap_or_else(|e| e.into_inner())
}

impl ShellApp {
    /// Pergunta um caminho e chama `then` com a resposta. Com diálogos
    /// nativos, a pergunta roda noutra thread e a janela continua viva;
    /// `then` roda num frame seguinte. Só um diálogo por vez.
    pub(crate) fn ask_path(
        &mut self,
        question: Question,
        then: impl FnOnce(&mut ShellApp, &egui::Context, Option<PathBuf>) + 'static,
    ) {
        if self.pending_dialog.is_some() {
            self.status = pgstudio_i18n::tr("A file dialog is already open.").to_string();
            return;
        }
        let blocks = lock(&self.dialogs).blocks();
        if !blocks {
            let answer = question.ask(&mut **lock(&self.dialogs));
            let ctx = self.frame_ctx.clone().unwrap_or_default();
            then(self, &ctx, answer);
            return;
        }
        crate::log_line!("file dialog: asking {question:?}");
        let (tx, rx) = mpsc::channel();
        let dialogs = self.dialogs.clone();
        let ctx = self.frame_ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("postpilot-file-dialog".into())
            .spawn(move || {
                let answer = question.ask(&mut **lock(&dialogs));
                let _ = tx.send(answer);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.pending_dialog = Some(Pending {
                    rx,
                    then: Box::new(then),
                });
            }
            Err(e) => {
                crate::log_line!("file dialog: could not start the dialog thread: {e}");
            }
        }
    }

    /// Se há um diálogo de arquivo aberto esperando resposta.
    pub fn is_file_dialog_open(&self) -> bool {
        self.pending_dialog.is_some()
    }

    /// Uma vez por frame: entrega a resposta do diálogo, se chegou.
    pub(crate) fn file_dialog_step(&mut self, ctx: &egui::Context) {
        let Some(answer) = self.pending_dialog.as_ref().and_then(Pending::poll) else {
            return;
        };
        let pending = self.pending_dialog.take().expect("checked above");
        crate::log_line!(
            "file dialog: {}",
            if answer.is_some() {
                "answered"
            } else {
                "canceled"
            }
        );
        (pending.then)(self, ctx, answer);
    }
}

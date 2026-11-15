//! Nome de arquivo sugerido pela IA ao salvar uma consulta nova, no lugar
//! de `SQLQuery1.sql`.
//!
//! Só acontece com uma IA conectada. A pergunta roda numa thread e a
//! janela continua viva: um aviso pequeno mostra que a IA está pensando,
//! com "Skip" para salvar já com o nome padrão. Se a resposta demora ou
//! falha, o diálogo de salvar abre com o nome padrão, como sempre.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_automation::naming;
use pgstudio_i18n::tr;

use crate::ShellApp;

/// Quanto esperar pela IA antes de seguir com o nome padrão.
const TIMEOUT: Duration = Duration::from_secs(12);

type Then = Box<dyn FnOnce(&mut ShellApp, Option<String>)>;

struct Pending {
    doc: u32,
    hash: u64,
    rx: mpsc::Receiver<Option<String>>,
    started: Instant,
    then: Then,
}

/// Sugestões em andamento e já feitas.
#[derive(Default)]
pub struct NameSuggestions {
    pending: Option<Pending>,
    /// Por documento: o texto (hash) e o nome sugerido para ele.
    cache: HashMap<u32, (u64, String)>,
}

impl NameSuggestions {
    /// Se a IA está pensando num nome.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

fn hash_of(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// `SQLQuery12.sql`: o nome que o app deu, não a pessoa.
fn is_default_name(name: &str) -> bool {
    name.strip_prefix("SQLQuery")
        .and_then(|rest| rest.strip_suffix(".sql"))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

impl ShellApp {
    /// Se a IA está sugerindo um nome de arquivo (testes).
    pub fn is_suggesting_name(&self) -> bool {
        self.name_suggestions.is_pending()
    }

    /// Chama `then` com o nome que a IA sugere para o documento `id`, ou
    /// com `None` quando não há IA conectada, o documento já tem nome
    /// próprio, está vazio, ou a IA não respondeu a tempo.
    pub(crate) fn suggest_file_name(
        &mut self,
        id: u32,
        then: impl FnOnce(&mut ShellApp, Option<String>) + 'static,
    ) {
        let Some(doc) = self.documents.get(id) else {
            return then(self, None);
        };
        if !is_default_name(doc.name()) || doc.text.trim().is_empty() {
            return then(self, None);
        }
        let hash = hash_of(&doc.text);
        if let Some((h, name)) = self.name_suggestions.cache.get(&id)
            && *h == hash
        {
            let name = name.clone();
            return then(self, Some(name));
        }
        // Outra sugestão em andamento: esta segue com o nome padrão.
        if self.name_suggestions.pending.is_some() {
            return then(self, None);
        }
        let Some(backend) = self.ai.backend() else {
            return then(self, None);
        };
        let sql = naming::user_prompt(&doc.text);
        let language = match pgstudio_i18n::language() {
            pgstudio_i18n::Language::PortugueseBrazil => "Brazilian Portuguese",
            _ => "English",
        };
        let (tx, rx) = mpsc::channel();
        let ctx = self.frame_ctx.clone();
        std::thread::spawn(move || {
            let name = backend
                .ask(
                    &naming::system_prompt(language),
                    &sql,
                    &std::env::temp_dir().join("postpilot-ai"),
                )
                .ok()
                .and_then(|answer| naming::file_name(&answer));
            let _ = tx.send(name);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        self.name_suggestions.pending = Some(Pending {
            doc: id,
            hash,
            rx,
            started: Instant::now(),
            then: Box::new(then),
        });
    }

    /// A cada frame: recebe a sugestão (ou desiste dela) e mostra o aviso
    /// enquanto a IA pensa.
    pub(crate) fn name_suggestion_step(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.name_suggestions.pending else {
            return;
        };
        let mut skip = false;
        crate::theme::modal(egui::Id::new("ai_file_name")).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("The AI is suggesting a file name…"));
                if ui.button(tr("Skip")).clicked() {
                    skip = true;
                }
            });
        });
        let answer = match pending.rx.try_recv() {
            Ok(name) => Some(name),
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
            Err(mpsc::TryRecvError::Empty) if skip || pending.started.elapsed() > TIMEOUT => {
                Some(None)
            }
            Err(mpsc::TryRecvError::Empty) => None,
        };
        let Some(name) = answer else {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        };
        let pending = self.name_suggestions.pending.take().expect("checked above");
        if let Some(name) = &name {
            self.name_suggestions
                .cache
                .insert(pending.doc, (pending.hash, name.clone()));
        }
        (pending.then)(self, name);
    }
}

#[cfg(test)]
mod tests {
    use super::is_default_name;

    #[test]
    fn default_names() {
        assert!(is_default_name("SQLQuery1.sql"));
        assert!(is_default_name("SQLQuery42.sql"));
        assert!(!is_default_name("SQLQuery.sql"));
        assert!(!is_default_name("clientes.sql"));
        assert!(!is_default_name("SQLQuery1.txt"));
    }
}

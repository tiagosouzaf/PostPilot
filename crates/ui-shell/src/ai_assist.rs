//! As sugestões da IA no editor: com uma IA conectada, ela lê o comando
//! em que o cursor está enquanto a pessoa escreve e propõe o que dá para
//! fazer (um JOIN pela chave estrangeira, um filtro, um operador, um
//! índice, um conserto, o resto do comando). O editor mostra um aviso no
//! canto e a lista (veja `pgstudio_sql_editor::hints`); aqui fica quando
//! perguntar e o que mandar.
//!
//! Digitar nunca espera a IA: a pergunta sai numa thread, só depois de
//! uma pausa na digitação, uma de cada vez, e a resposta de cada comando
//! fica guardada (voltar a um comando já lido não pergunta de novo).
//!
//! Nada roda no banco. Vão para a IA o texto do comando e o que o
//! catálogo do IntelliSense sabe das tabelas citadas (nomes, tipos,
//! chaves e índices); os dados das tabelas não vão. Num ambiente com a IA
//! bloqueada, nada é mandado.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_automation::assist;
use pgstudio_core::CommandId;
use pgstudio_core::connection::Engine;
use pgstudio_core::schema::Model;
use pgstudio_i18n::{tr, trf};
use pgstudio_sql_editor::hints::{Hint, HintAction, HintKind, HintStatus, Hints, statement_at};

use crate::ShellApp;
use crate::ai::AssistMode;
use crate::registered::AiAccess;

/// Pausa na digitação antes de perguntar à IA.
const TYPING_PAUSE: Duration = Duration::from_millis(1200);
/// Pausa depois de só mover o cursor para outro comando.
const CURSOR_PAUSE: Duration = Duration::from_millis(500);
/// Intervalo mínimo entre duas perguntas automáticas.
const MIN_GAP: Duration = Duration::from_secs(2);
/// Quanto esperar pela resposta.
const TIMEOUT: Duration = Duration::from_secs(90);
/// Quanto esperar pelas chaves e índices do banco antes de perguntar sem
/// eles.
const MODEL_WAIT: Duration = Duration::from_secs(6);
/// Um comando menor que isto ainda não diz nada.
const MIN_CHARS: usize = 6;
/// Acima disto o texto não é lido a cada pausa (só a pedido).
const MAX_TEXT: usize = 400_000;
/// O menor intervalo pedido ao relógio do egui: abaixo de um frame ele
/// vira um repaint imediato, e a janela ficaria redesenhando à toa.
const TICK: Duration = Duration::from_millis(25);
/// Respostas guardadas.
const CACHE: usize = 64;

type Answer = Result<Vec<Hint>, String>;

struct Pending {
    doc: u32,
    key: u64,
    rx: mpsc::Receiver<Answer>,
    started: Instant,
    /// Abrir a lista quando a resposta chegar (a pessoa pediu).
    open: bool,
}

enum ModelSlot {
    Loading(mpsc::Receiver<Option<Model>>, Instant),
    Ready(Option<Arc<Model>>),
}

/// O estado das sugestões da IA.
#[derive(Default)]
pub struct Assist {
    /// A janela, a revisão do texto e o cursor vistos por último.
    seen: Option<(u32, u64, usize)>,
    /// Desde quando há algo novo para ler, e quanto esperar.
    changed: Option<(Instant, Duration)>,
    pending: Option<Pending>,
    last_request: Option<Instant>,
    /// Abrir a lista na próxima vez que houver sugestões.
    open_next: bool,
    cache: HashMap<u64, Vec<Hint>>,
    order: VecDeque<u64>,
    /// O comando que a IA não conseguiu responder, para não insistir.
    failed: Option<(u64, String)>,
    /// Chaves e índices de cada banco (relacional).
    models: HashMap<String, ModelSlot>,
    /// Perguntas feitas à IA (testes).
    requests: usize,
    /// Pausa no lugar das de verdade (testes).
    pause: Option<Duration>,
}

impl Assist {
    /// Esquece o que foi lido do banco e as respostas (ao desconectar, ao
    /// reler o catálogo ou trocar de IA).
    pub fn reset(&mut self) {
        self.models.clear();
        self.cache.clear();
        self.order.clear();
        self.failed = None;
        self.seen = None;
    }

    fn remember(&mut self, key: u64, items: Vec<Hint>) {
        if self.cache.insert(key, items).is_none() {
            self.order.push_back(key);
            while self.order.len() > CACHE {
                if let Some(old) = self.order.pop_front() {
                    self.cache.remove(&old);
                }
            }
        }
    }

    fn forget(&mut self, key: u64) {
        self.cache.remove(&key);
        self.order.retain(|k| *k != key);
        if self.failed.as_ref().is_some_and(|(k, _)| *k == key) {
            self.failed = None;
        }
    }

    fn poll_models(&mut self) {
        for slot in self.models.values_mut() {
            if let ModelSlot::Loading(rx, _) = slot {
                match rx.try_recv() {
                    Ok(model) => *slot = ModelSlot::Ready(model.map(Arc::new)),
                    Err(mpsc::TryRecvError::Disconnected) => *slot = ModelSlot::Ready(None),
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
        }
    }
}

fn key_of(engine: Engine, database: &str, statement: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{engine:?}").hash(&mut h);
    database.hash(&mut h);
    pgstudio_i18n::language().hash(&mut h);
    statement.hash(&mut h);
    h.finish()
}

fn hint_of(s: assist::Suggestion) -> Hint {
    Hint {
        kind: HintKind::parse(&s.kind),
        title: s.title,
        detail: s.detail,
        action: match s.action {
            assist::Action::Replace => HintAction::Replace(s.code),
            assist::Action::Insert => HintAction::Insert(s.code),
            assist::Action::None => HintAction::None,
        },
    }
}

impl ShellApp {
    /// As sugestões da IA mostradas na janela de consulta `id`.
    pub fn ai_hints(&self, id: u32) -> Option<&Hints> {
        self.documents.get(id)?.editor.hints()
    }

    /// Se a IA está lendo um comando para sugerir.
    pub fn is_assist_thinking(&self) -> bool {
        self.assist.pending.is_some()
    }

    /// Quantas perguntas as sugestões já fizeram à IA.
    pub fn assist_requests(&self) -> usize {
        self.assist.requests
    }

    /// Pausa na digitação antes de perguntar à IA (testes e capturas de
    /// tela).
    pub fn with_assist_pause(mut self, pause: Duration) -> Self {
        self.assist.pause = Some(pause);
        self
    }

    /// As chaves e os índices do banco, lidos numa thread na primeira vez.
    /// `Err` enquanto a leitura não chega (e ainda vale esperar).
    fn assist_model(&mut self, database: &str) -> Result<Option<Arc<Model>>, ()> {
        let Some(provider) = self.explorer.provider().filter(|p| p.relational()) else {
            return Ok(None);
        };
        if !self.assist.models.contains_key(database) {
            let (tx, rx) = mpsc::channel();
            let db = database.to_string();
            let ctx = self.frame_ctx.clone();
            let _ = std::thread::Builder::new()
                .name("pgstudio-ai-assist-model".into())
                .spawn(move || {
                    let _ = tx.send(provider.load_model(&db).ok());
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                });
            self.assist
                .models
                .insert(database.to_string(), ModelSlot::Loading(rx, Instant::now()));
        }
        match self.assist.models.get(database) {
            Some(ModelSlot::Ready(model)) => Ok(model.clone()),
            Some(ModelSlot::Loading(_, since)) if since.elapsed() < MODEL_WAIT => Err(()),
            _ => Ok(None),
        }
    }

    /// Recebe a resposta da IA, se chegou (ou desiste dela).
    fn assist_receive(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.assist.pending else {
            return;
        };
        let answer = match pending.rx.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(tr("The AI stopped before answering.").to_string())
            }
            Err(mpsc::TryRecvError::Empty) if pending.started.elapsed() > TIMEOUT => {
                Err(tr("The AI took too long to answer.").to_string())
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_secs(1));
                return;
            }
        };
        let pending = self.assist.pending.take().expect("checked above");
        match answer {
            Ok(items) => self.assist.remember(pending.key, items),
            Err(e) => {
                self.output.push(trf!("AI suggestions: {error}", error = e));
                self.assist.failed = Some((pending.key, e));
            }
        }
        self.assist.open_next |= pending.open;
        // O passo abaixo mostra a resposta se o comando ainda for o mesmo.
        if self.active_doc == Some(pending.doc) {
            self.assist.changed = Some((Instant::now(), Duration::ZERO));
        }
    }

    /// A cada frame: vê se o comando do cursor mudou e, depois de uma
    /// pausa, mostra as sugestões guardadas ou pergunta à IA.
    pub(crate) fn assist_step(&mut self, ctx: &egui::Context) {
        self.assist.poll_models();
        self.assist_receive(ctx);
        let Some(id) = self.active_doc else {
            return;
        };
        let mode = self.ai.settings().assist;
        let blocked = self
            .document_tags()
            .get(&id)
            .is_some_and(|tag| tag.ai == AiAccess::Blocked);
        let engine = self.document_engine(id).unwrap_or_default();
        let database = self.document_database(id).unwrap_or_default();
        let shortcut = self
            .keymap
            .shortcut_for(CommandId::AiSuggestions)
            .map(|s| s.to_string())
            .unwrap_or_default();
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        doc.editor.set_hint_shortcut(&shortcut);
        if let Some(hint) = doc.applied_hint.take() {
            self.status = trf!("AI suggestion applied: {title}", title = hint.title);
        }
        let asked = doc.editor.take_hint_request();
        if mode == AssistMode::Off || blocked {
            doc.editor.set_hints(None);
            doc.editor.set_hint_status(HintStatus::Off);
            if asked {
                self.status = if blocked {
                    tr("The AI is blocked in the environment of this server.").to_string()
                } else {
                    tr("AI suggestions are turned off in Tools > Options > AI Providers.")
                        .to_string()
                };
            }
            return;
        }
        let idle = match mode {
            AssistMode::OnDemand => HintStatus::Ask,
            _ => HintStatus::Idle,
        };

        let now = Instant::now();
        let stamp = (id, doc.editor.revision(), doc.cursor_byte);
        if self.assist.seen != Some(stamp) {
            let typed = self
                .assist
                .seen
                .is_none_or(|(d, revision, _)| d != id || revision != stamp.1);
            self.assist.seen = Some(stamp);
            // O comando que a IA leu mudou: as sugestões saem já.
            if doc
                .editor
                .hints()
                .is_some_and(|h| doc.text.get(h.range.clone()) != Some(h.base.as_str()))
            {
                doc.editor.set_hints(None);
            }
            let pause = match (self.assist.pause, typed) {
                (Some(pause), _) => pause,
                (None, true) => TYPING_PAUSE,
                (None, false) => CURSOR_PAUSE,
            };
            self.assist.changed = Some((now, pause));
            // Sem IA conectada não há o que esperar: nem o relógio corre.
            if !asked && self.ai.backend().is_none() {
                self.assist.changed = None;
                doc.editor.set_hints(None);
                doc.editor.set_hint_status(HintStatus::Off);
                return;
            }
        }
        if !asked {
            match self.assist.changed {
                Some((since, pause)) if since.elapsed() < pause => {
                    ctx.request_repaint_after((pause - since.elapsed()).max(TICK));
                    return;
                }
                Some(_) => {}
                None => return,
            }
        }
        self.assist.changed = None;
        let open = asked || std::mem::take(&mut self.assist.open_next);

        if doc.text.len() > MAX_TEXT && !asked {
            doc.editor.set_hint_status(HintStatus::Ask);
            return;
        }
        let range = statement_at(&doc.text, doc.editor.dialect(), doc.cursor_byte)
            .filter(|r| doc.text[r.clone()].chars().count() >= MIN_CHARS)
            .filter(|r| r.len() <= assist::MAX_STATEMENT);
        let Some(range) = range else {
            doc.editor.set_hints(None);
            doc.editor.set_hint_status(idle);
            if asked {
                self.status =
                    tr("Write a query first: the AI reads the one under the cursor.").to_string();
            }
            return;
        };
        let statement = doc.text[range.clone()].to_string();
        let key = key_of(engine, &database, &statement);
        // Pedir de novo onde a IA não achou nada (ou falhou) pergunta
        // outra vez.
        if asked
            && self
                .assist
                .cache
                .get(&key)
                .is_none_or(|items| items.is_empty())
        {
            self.assist.forget(key);
        }
        if let Some(items) = self.assist.cache.get(&key) {
            let source = self.ai.active_label();
            let some = !items.is_empty();
            doc.editor.set_hints(Some(Hints {
                items: items.clone(),
                range,
                base: statement,
                source,
            }));
            doc.editor.set_hint_status(idle);
            if open && some {
                doc.editor.open_hints();
            }
            return;
        }
        doc.editor.set_hints(None);
        if let Some((failed, error)) = &self.assist.failed
            && *failed == key
        {
            doc.editor
                .set_hint_status(HintStatus::Failed(error.clone()));
            return;
        }
        if mode == AssistMode::OnDemand && !asked {
            doc.editor.set_hint_status(HintStatus::Ask);
            return;
        }
        // Uma pergunta de cada vez: esta espera a que está no ar.
        if let Some(pending) = &mut self.assist.pending {
            if pending.key == key {
                pending.open |= asked;
                doc.editor.set_hint_status(HintStatus::Thinking);
            } else {
                self.assist.open_next |= asked;
                self.assist.changed = Some((now, Duration::from_millis(300)));
                ctx.request_repaint_after(Duration::from_millis(300));
            }
            return;
        }
        if !asked
            && self.assist.pause.is_none()
            && let Some(last) = self.assist.last_request
            && last.elapsed() < MIN_GAP
        {
            let wait = (MIN_GAP - last.elapsed()).max(TICK);
            self.assist.changed = Some((now, wait));
            ctx.request_repaint_after(wait);
            return;
        }
        let Some(backend) = self.ai.backend() else {
            doc.editor.set_hint_status(HintStatus::Off);
            if asked {
                self.status =
                    tr("Connect an AI in Tools > AI Providers to get suggestions.").to_string();
            }
            return;
        };
        let cursor = doc
            .cursor_byte
            .checked_sub(range.start)
            .filter(|c| *c <= statement.len());
        let model = match self.assist_model(&database) {
            Ok(model) => model,
            Err(()) => {
                // As chaves e os índices estão chegando: vale esperar.
                self.assist.open_next |= asked;
                self.assist.changed = Some((now, Duration::from_millis(300)));
                ctx.request_repaint_after(Duration::from_millis(300));
                if let Some(doc) = self.documents.get_mut(id) {
                    doc.editor.set_hint_status(HintStatus::Thinking);
                }
                return;
            }
        };
        let context = match self.catalogs.get(&database) {
            Some(catalog) => assist::context(catalog, model.as_deref(), &statement),
            None => assist::context(
                &pgstudio_core::catalog::Catalog {
                    engine,
                    ..Default::default()
                },
                None,
                &statement,
            ),
        };
        let language = match pgstudio_i18n::language() {
            pgstudio_i18n::Language::PortugueseBrazil => "Brazilian Portuguese",
            _ => "English",
        };
        let system = assist::system_prompt(engine, language);
        let user = assist::user_prompt(engine, &database, &context, &statement, cursor);
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("pgstudio-ai-assist".into())
            .spawn(move || {
                let answer = backend
                    .ask(&system, &user, &std::env::temp_dir().join("postpilot-ai"))
                    .and_then(|answer| assist::parse(&answer, &statement))
                    .map(|list| list.into_iter().map(hint_of).collect());
                let _ = tx.send(answer);
                repaint.request_repaint();
            });
        if spawned.is_err() {
            return;
        }
        self.assist.requests += 1;
        self.assist.last_request = Some(now);
        self.assist.pending = Some(Pending {
            doc: id,
            key,
            rx,
            started: now,
            open: asked,
        });
        if let Some(doc) = self.documents.get_mut(id) {
            doc.editor.set_hint_status(HintStatus::Thinking);
        }
    }
}

//! "Test connection": tenta conectar com o que está preenchido num diálogo
//! de conexão, sem salvar nem abrir nada, e diz se deu certo.
//!
//! O teste roda numa thread separada, pelo mesmo [`Connector`] da conexão
//! de verdade (todos os motores e o túnel SSH passam por ele). A conexão
//! aberta é fechada logo depois. Quem espera é só o diálogo: há um limite
//! de tempo ([`TEST_LIMIT`]) e um botão para desistir antes. A thread de
//! uma tentativa abandonada termina sozinha, no tempo do driver, e o que
//! ela trouxer é jogado fora.
//!
//! O resultado vale para os campos testados: mudou servidor, login, senha,
//! banco, criptografia ou túnel, ele some.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Connector};
use pgstudio_i18n::{tr, trf};

/// Quanto o diálogo espera por um teste antes de desistir.
pub const TEST_LIMIT: Duration = Duration::from_secs(15);

/// Verde do "deu certo", legível no tema claro e no escuro.
const OK_GREEN: egui::Color32 = egui::Color32::from_rgb(0x3f, 0xa8, 0x5f);

/// Como terminou o último teste.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Conectou: motor e versão do servidor, e quanto demorou.
    Connected {
        server: String,
        product: String,
        elapsed: Duration,
    },
    /// O servidor (ou o caminho até ele) recusou: a mensagem do driver.
    Failed(String),
    /// Faltou preencher algum campo.
    Invalid(String),
    /// Passou do limite de tempo sem resposta.
    TimedOut { server: String, limit: Duration },
    /// A pessoa desistiu.
    Canceled,
}

impl Outcome {
    /// Se o teste conectou.
    pub fn is_ok(&self) -> bool {
        matches!(self, Outcome::Connected { .. })
    }

    /// O texto mostrado no diálogo.
    pub fn message(&self) -> String {
        match self {
            Outcome::Connected {
                server,
                product,
                elapsed,
            } => trf!(
                "Connection succeeded: {server} answered as {product} in {time}.",
                server = server,
                product = product,
                time = elapsed_label(*elapsed)
            ),
            Outcome::Failed(error) => trf!("Connection test failed: {error}", error = error),
            Outcome::Invalid(error) => error.clone(),
            Outcome::TimedOut { server, limit } => trf!(
                "No answer from {server} in {seconds} seconds. Check the server name, the port and whether a firewall or VPN is in the way.",
                server = server,
                seconds = limit.as_secs()
            ),
            Outcome::Canceled => tr("Connection test canceled.").to_string(),
        }
    }
}

fn elapsed_label(elapsed: Duration) -> String {
    if elapsed < Duration::from_secs(1) {
        format!("{} ms", elapsed.as_millis().max(1))
    } else {
        format!("{:.1} s", elapsed.as_secs_f32())
    }
}

/// O que foi testado: os campos e a senha.
type Tested = (ConnectionSettings, String);

struct Running {
    rx: Receiver<Result<String, String>>,
    started: Instant,
    server: String,
}

/// Estado do teste de conexão de um diálogo.
pub struct ConnectionTest {
    /// O botão foi clicado: começa no próximo passo do aplicativo.
    requested: Option<Tested>,
    running: Option<Running>,
    outcome: Option<Outcome>,
    /// A que campos `running`/`outcome` se referem.
    tested: Option<Tested>,
    limit: Duration,
}

impl Default for ConnectionTest {
    fn default() -> Self {
        ConnectionTest {
            requested: None,
            running: None,
            outcome: None,
            tested: None,
            limit: TEST_LIMIT,
        }
    }
}

impl ConnectionTest {
    /// Com outro limite de tempo (testes).
    pub fn with_limit(mut self, limit: Duration) -> Self {
        self.limit = limit;
        self
    }

    /// Muda o limite de tempo (testes).
    pub fn set_limit(&mut self, limit: Duration) {
        self.limit = limit;
    }

    /// Pede um teste com estes campos; ele começa no próximo [`Self::step`].
    pub fn request(&mut self, settings: ConnectionSettings, password: String) {
        self.cancel_quietly();
        self.requested = Some((settings, password));
    }

    /// Os campos não formam uma conexão: mostra o que falta.
    pub fn invalid(&mut self, error: String) {
        self.cancel_quietly();
        self.outcome = Some(Outcome::Invalid(error));
    }

    /// Se há um teste pedido ou em andamento.
    pub fn is_running(&self) -> bool {
        self.requested.is_some() || self.running.is_some()
    }

    /// O resultado do último teste, se ainda vale.
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    /// Desiste do teste em andamento.
    pub fn cancel(&mut self) {
        if self.is_running() {
            self.cancel_quietly();
            self.outcome = Some(Outcome::Canceled);
        }
    }

    /// Para tudo e apaga o resultado (o diálogo vai conectar de verdade).
    pub fn clear(&mut self) {
        self.cancel_quietly();
    }

    fn cancel_quietly(&mut self) {
        self.requested = None;
        self.running = None;
        self.outcome = None;
        self.tested = None;
    }

    /// Os campos do diálogo agora: se não são mais os testados, o teste em
    /// andamento é abandonado e o resultado some.
    pub fn keep_for(&mut self, current: Option<(&ConnectionSettings, &str)>) {
        let Some((settings, password)) = &self.tested else {
            return;
        };
        if current != Some((settings, password.as_str())) {
            self.cancel_quietly();
        }
    }

    /// Uma vez por frame: começa o teste pedido e recebe o resultado do
    /// que está em andamento. Não espera pela rede.
    pub fn step(&mut self, connector: Option<&Arc<dyn Connector>>, ctx: &egui::Context) {
        if let Some((settings, password)) = self.requested.take() {
            match connector {
                Some(connector) => self.start(connector.clone(), settings, password, ctx),
                None => {
                    self.outcome = Some(Outcome::Failed(trf!(
                        "This build of PostPilot cannot connect to {engine}.",
                        engine = settings.engine.label()
                    )));
                }
            }
        }
        let Some(running) = &self.running else {
            return;
        };
        let outcome = match running.rx.try_recv() {
            Ok(Ok(product)) => Outcome::Connected {
                server: running.server.clone(),
                product,
                elapsed: running.started.elapsed(),
            },
            Ok(Err(error)) => Outcome::Failed(error),
            Err(TryRecvError::Disconnected) => {
                Outcome::Failed(tr("The connection attempt stopped unexpectedly.").to_string())
            }
            Err(TryRecvError::Empty) if running.started.elapsed() >= self.limit => {
                Outcome::TimedOut {
                    server: running.server.clone(),
                    limit: self.limit,
                }
            }
            Err(TryRecvError::Empty) => {
                // Acorda para o limite de tempo mesmo sem resposta.
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
        };
        crate::log_line!(
            "connection test: {} -> {}",
            running.server,
            if outcome.is_ok() { "ok" } else { "failed" }
        );
        self.running = None;
        self.outcome = Some(outcome);
    }

    fn start(
        &mut self,
        connector: Arc<dyn Connector>,
        settings: ConnectionSettings,
        password: String,
        ctx: &egui::Context,
    ) {
        let (tx, rx) = mpsc::channel();
        self.outcome = None;
        self.tested = Some((settings.clone(), password.clone()));
        self.running = Some(Running {
            rx,
            started: Instant::now(),
            server: settings.server_name(),
        });
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("pgstudio-connection-test".into())
            .spawn(move || {
                let result = connector
                    .connect(&settings, &password)
                    // A conexão do teste fecha aqui, ao soltar o `Connected`.
                    .map(|c| format!("{} {}", c.settings.engine.label(), c.server_version))
                    .map_err(|e| e.message().to_string());
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            self.running = None;
            self.outcome = Some(Outcome::Failed(e.to_string()));
        }
    }

    /// O botão: "Test connection" ou, com o teste em andamento, "Cancel
    /// test" (que já desiste). Devolve se a pessoa pediu um teste.
    pub fn button(&mut self, ui: &mut egui::Ui, enabled: bool) -> bool {
        if self.is_running() {
            if ui
                .button(tr("Cancel test"))
                .on_hover_text(tr("Stop waiting for this connection test"))
                .clicked()
            {
                self.cancel();
            }
            return false;
        }
        ui.add_enabled(enabled, egui::Button::new(tr("Test connection")))
            .on_hover_text(tr(
                "Try to connect with these fields, without saving or opening the connection",
            ))
            .clicked()
    }

    /// A linha de estado: o andamento ou o resultado, quebrando em várias
    /// linhas quando o diálogo é estreito.
    pub fn status(&self, ui: &mut egui::Ui) {
        if let Some(running) = &self.running {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.add(egui::Label::new(trf!("Testing {server}…", server = running.server)).wrap());
            });
        } else if self.requested.is_some() {
            ui.spinner();
        } else if let Some(outcome) = &self.outcome {
            let color = match outcome {
                Outcome::Connected { .. } => OK_GREEN,
                Outcome::Canceled => ui.visuals().weak_text_color(),
                _ => ui.visuals().error_fg_color,
            };
            ui.add(egui::Label::new(egui::RichText::new(outcome.message()).color(color)).wrap());
        }
    }

    /// Botão e, embaixo, a linha de estado. Devolve se a pessoa pediu um
    /// teste.
    pub fn show(&mut self, ui: &mut egui::Ui, enabled: bool) -> bool {
        let clicked = ui.horizontal_wrapped(|ui| self.button(ui, enabled)).inner;
        if self.is_running() || self.outcome.is_some() {
            ui.add_space(2.0);
            self.status(ui);
        }
        clicked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgstudio_core::connection::{ConnectError, Connected};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// `down*` recusa; `slow*` só responde depois de `release`.
    #[derive(Default)]
    struct Fake {
        attempts: AtomicUsize,
        release: AtomicBool,
    }

    impl Connector for Fake {
        fn connect(
            &self,
            s: &ConnectionSettings,
            password: &str,
        ) -> Result<Connected, ConnectError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            while s.host.starts_with("slow") && !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
            if s.host.starts_with("down") {
                return Err(ConnectError::Network("connection refused".into()));
            }
            if password != "ok" {
                return Err(ConnectError::Authentication("Login failed.".into()));
            }
            Ok(Connected {
                settings: s.clone(),
                server_version: "17.2".into(),
                metadata: Arc::new(pgstudio_object_explorer::fake::FakeProvider::new(
                    Duration::ZERO,
                )),
                queries: Arc::new(pgstudio_core::query::Offline("offline")),
            })
        }
    }

    fn settings(host: &str) -> ConnectionSettings {
        ConnectionSettings {
            host: host.into(),
            ..ConnectionSettings::default()
        }
    }

    fn finish(test: &mut ConnectionTest, connector: &Arc<dyn Connector>) -> Outcome {
        let ctx = egui::Context::default();
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            test.step(Some(connector), &ctx);
            if !test.is_running() {
                return test.outcome().cloned().expect("an outcome");
            }
            assert!(Instant::now() < until, "the test never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_good_connection_reports_the_server_and_a_bad_one_the_error() {
        let fake = Arc::new(Fake::default());
        let connector: Arc<dyn Connector> = fake.clone();
        let mut test = ConnectionTest::default();
        test.request(settings("db"), "ok".into());
        assert!(test.is_running());
        match finish(&mut test, &connector) {
            Outcome::Connected {
                server, product, ..
            } => {
                assert_eq!(server, "db");
                assert_eq!(product, "PostgreSQL 17.2");
            }
            other => panic!("{other:?}"),
        }
        test.request(settings("db"), "wrong".into());
        assert_eq!(
            finish(&mut test, &connector),
            Outcome::Failed("Login failed.".into())
        );
        test.request(settings("down"), "ok".into());
        assert_eq!(
            finish(&mut test, &connector),
            Outcome::Failed("connection refused".into())
        );
        assert_eq!(fake.attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_silent_server_times_out_and_a_late_answer_is_ignored() {
        let fake = Arc::new(Fake::default());
        let connector: Arc<dyn Connector> = fake.clone();
        let mut test = ConnectionTest::default().with_limit(Duration::from_millis(40));
        test.request(settings("slow"), "ok".into());
        let outcome = finish(&mut test, &connector);
        assert_eq!(
            outcome,
            Outcome::TimedOut {
                server: "slow".into(),
                limit: Duration::from_millis(40)
            }
        );
        // A thread abandonada termina depois e não muda o resultado.
        fake.release.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        test.step(Some(&connector), &egui::Context::default());
        assert_eq!(test.outcome(), Some(&outcome));
    }

    #[test]
    fn canceling_stops_waiting_at_once() {
        let fake = Arc::new(Fake::default());
        let connector: Arc<dyn Connector> = fake.clone();
        let ctx = egui::Context::default();
        let mut test = ConnectionTest::default();
        test.request(settings("slow"), "ok".into());
        test.step(Some(&connector), &ctx);
        assert!(test.is_running());
        test.cancel();
        assert!(!test.is_running());
        assert_eq!(test.outcome(), Some(&Outcome::Canceled));
        fake.release.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        test.step(Some(&connector), &ctx);
        assert_eq!(test.outcome(), Some(&Outcome::Canceled));
    }

    #[test]
    fn the_result_goes_away_when_the_fields_change() {
        let connector: Arc<dyn Connector> = Arc::new(Fake::default());
        let mut test = ConnectionTest::default();
        test.request(settings("db"), "ok".into());
        assert!(finish(&mut test, &connector).is_ok());
        test.keep_for(Some((&settings("db"), "ok")));
        assert!(test.outcome().is_some());
        test.keep_for(Some((&settings("db"), "other")));
        assert!(test.outcome().is_none());

        test.request(settings("db"), "ok".into());
        assert!(finish(&mut test, &connector).is_ok());
        test.keep_for(Some((&settings("other"), "ok")));
        assert!(test.outcome().is_none());

        // Um campo que falta é dito na hora, sem tentar conectar.
        test.invalid("Enter a login.".into());
        assert_eq!(
            test.outcome().map(Outcome::message).as_deref(),
            Some("Enter a login.")
        );
        // Sem conector, o teste diz que não dá, em vez de ficar esperando.
        test.request(settings("db"), "ok".into());
        test.step(None, &egui::Context::default());
        assert!(matches!(test.outcome(), Some(Outcome::Failed(_))));
    }
}

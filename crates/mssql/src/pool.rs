//! Conexões dos metadados de um servidor.
//!
//! Uma conexão do SQL Server roda um comando por vez. Com uma só, a lista
//! de bancos da árvore esperava atrás de qualquer leitura demorada (o
//! catálogo inteiro de um banco grande para o IntelliSense, por exemplo).
//! Cada [`Pool`] guarda poucas conexões, abertas só quando a anterior está
//! ocupada, e o Object Explorer usa um para a árvore e outro para as
//! leituras pesadas.

use std::ops::{Deref, DerefMut};
use std::sync::Mutex;
use std::time::Duration;

use pgstudio_core::connection::ConnectionSettings;
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::Session;

/// Até `limit` conexões com o mesmo servidor.
pub(crate) struct Pool {
    settings: ConnectionSettings,
    command_timeout: Option<Duration>,
    /// As abertas que ninguém está usando.
    idle: Mutex<Vec<Session>>,
    /// Uma licença por conexão em uso.
    permits: Semaphore,
}

impl Pool {
    /// Um conjunto vazio: a primeira conexão abre no primeiro uso.
    pub(crate) fn new(
        settings: ConnectionSettings,
        command_timeout: Option<Duration>,
        limit: usize,
    ) -> Self {
        Pool {
            settings,
            command_timeout,
            idle: Mutex::default(),
            permits: Semaphore::new(limit.max(1)),
        }
    }

    /// Como [`Pool::new`], já com uma conexão aberta.
    pub(crate) fn with(session: Session, limit: usize) -> Self {
        let pool = Pool::new(session.settings().clone(), session.command_timeout(), limit);
        pool.put(session);
        pool
    }

    fn put(&self, session: Session) {
        if let Ok(mut idle) = self.idle.lock() {
            idle.push(session);
        }
    }

    /// Uma conexão que serve: uma parada, ou uma nova se todas as abertas
    /// estão ocupadas. Com `limit` em uso, espera uma voltar.
    pub(crate) async fn get(&self, password: &str) -> Result<Lease<'_>, String> {
        let permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
        let parked = self.idle.lock().ok().and_then(|mut idle| idle.pop());
        let session = match parked {
            Some(session) => session,
            None => Session::connect_with(&self.settings, password, self.command_timeout)
                .await
                .map_err(|e| e.message().to_string())?,
        };
        Ok(Lease {
            session: Some(session),
            pool: self,
            _permit: permit,
        })
    }
}

/// Uma conexão emprestada do [`Pool`]: volta para ele ao sair de escopo, a
/// não ser que tenha quebrado (tempo esgotado, queda). Aí é descartada, e
/// o próximo pedido abre outra.
pub(crate) struct Lease<'a> {
    session: Option<Session>,
    pool: &'a Pool,
    _permit: SemaphorePermit<'a>,
}

impl Deref for Lease<'_> {
    type Target = Session;

    fn deref(&self) -> &Session {
        self.session.as_ref().expect("held until drop")
    }
}

impl DerefMut for Lease<'_> {
    fn deref_mut(&mut self) -> &mut Session {
        self.session.as_mut().expect("held until drop")
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(session) = self.session.take()
            && !session.is_broken()
        {
            self.pool.put(session);
        }
    }
}

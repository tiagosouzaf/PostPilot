//! Cache do catálogo para o IntelliSense: um por banco, lido numa thread
//! na primeira vez que uma janela de consulta precisa dele e de novo com
//! Ctrl+Shift+R ("Refresh Local Cache", como no SSMS).

use pgstudio_i18n::tr;
use std::collections::HashMap;
use std::sync::{Arc, mpsc};

use eframe::egui;
use pgstudio_core::catalog::Catalog;
use pgstudio_core::metadata::{MetadataError, MetadataProvider};

enum Slot {
    /// Lendo; o catálogo anterior continua valendo até chegar o novo.
    Loading(
        mpsc::Receiver<Result<Catalog, MetadataError>>,
        Option<Arc<Catalog>>,
    ),
    Ready(Arc<Catalog>),
    Failed,
}

/// Catálogos lidos, por banco.
#[derive(Default)]
pub struct Catalogs {
    slots: HashMap<String, Slot>,
}

/// Uma leitura terminada, para o painel Output.
pub struct Loaded {
    pub database: String,
    pub result: Result<usize, String>,
}

impl Catalogs {
    /// Começa a ler o catálogo de `database`, se ainda não leu.
    pub fn ensure(
        &mut self,
        database: &str,
        provider: &Arc<dyn MetadataProvider>,
        ctx: &egui::Context,
    ) {
        if !self.slots.contains_key(database) {
            self.load(database, provider, ctx);
        }
    }

    /// Lê de novo (Ctrl+Shift+R). As sugestões usam o catálogo antigo até o
    /// novo chegar.
    pub fn refresh(
        &mut self,
        database: &str,
        provider: &Arc<dyn MetadataProvider>,
        ctx: &egui::Context,
    ) {
        if !matches!(self.slots.get(database), Some(Slot::Loading(..))) {
            self.load(database, provider, ctx);
        }
    }

    fn load(&mut self, database: &str, provider: &Arc<dyn MetadataProvider>, ctx: &egui::Context) {
        let previous = match self.slots.remove(database) {
            Some(Slot::Ready(c)) => Some(c),
            _ => None,
        };
        let (tx, rx) = mpsc::channel();
        let provider = provider.clone();
        let db = database.to_string();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider.catalog(&db));
            ctx.request_repaint();
        });
        self.slots
            .insert(database.to_string(), Slot::Loading(rx, previous));
    }

    /// Recebe as leituras que terminaram.
    pub fn poll(&mut self) -> Vec<Loaded> {
        let mut done = Vec::new();
        for (db, slot) in &mut self.slots {
            let Slot::Loading(rx, previous) = slot else {
                continue;
            };
            let result = match rx.try_recv() {
                Ok(r) => r,
                Err(mpsc::TryRecvError::Empty) => continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(MetadataError(tr("The catalog read stopped.").into()))
                }
            };
            match result {
                Ok(catalog) => {
                    let objects = catalog
                        .schemas
                        .iter()
                        .map(|s| s.relations.len() + s.functions.len())
                        .sum();
                    *slot = Slot::Ready(Arc::new(catalog));
                    done.push(Loaded {
                        database: db.clone(),
                        result: Ok(objects),
                    });
                }
                Err(e) => {
                    *slot = match previous.take() {
                        Some(c) => Slot::Ready(c),
                        None => Slot::Failed,
                    };
                    done.push(Loaded {
                        database: db.clone(),
                        result: Err(e.0),
                    });
                }
            }
        }
        done
    }

    /// Catálogo de `database`, se já foi lido.
    pub fn get(&self, database: &str) -> Option<&Catalog> {
        match self.slots.get(database)? {
            Slot::Ready(c) | Slot::Loading(_, Some(c)) => Some(c),
            _ => None,
        }
    }

    /// Se há alguma leitura em andamento.
    pub fn is_loading(&self) -> bool {
        self.slots.values().any(|s| matches!(s, Slot::Loading(..)))
    }

    /// Esquece tudo (ao desconectar).
    pub fn clear(&mut self) {
        self.slots.clear();
    }
}

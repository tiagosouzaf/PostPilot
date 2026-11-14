//! Janelas de consulta abertas: texto, arquivo e se há alterações. O editor
//! de cada uma é o `pgstudio-sql-editor`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Uma janela de consulta.
#[derive(Debug)]
pub struct Document {
    name: String,
    path: Option<PathBuf>,
    /// Texto no editor.
    pub text: String,
    saved: String,
    /// Busca e "Go To Line" do editor.
    pub editor: pgstudio_sql_editor::EditorState,
    /// Linha e coluna do cursor (a partir de 1), para a status bar.
    pub cursor: Option<(usize, usize)>,
    /// Conexão da janela, aberta na primeira execução.
    pub session: Option<Session>,
    /// Banco em que a janela conecta (o do nó do Object Explorer de onde ela
    /// veio); None usa o da conexão.
    pub database: Option<String>,
    /// Última execução: resultados e mensagens.
    pub execution: Option<crate::execution::Execution>,
    /// Grade ou texto nas próximas execuções (Ctrl+D, Ctrl+T).
    pub results_mode: crate::execution::ResultsMode,
    /// "Include Actual Execution Plan" (Ctrl+M) ligado: o F5 roda os
    /// comandos com `EXPLAIN ANALYZE` e mostra os planos reais.
    pub actual_plan: bool,
}

/// A conexão de uma janela de consulta.
#[derive(Clone)]
pub struct Session(pub std::sync::Arc<dyn pgstudio_core::query::QuerySession>);

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Session").field(&self.0.database()).finish()
    }
}

impl Document {
    /// Nome mostrado na aba, como `SQLQuery1.sql`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Arquivo onde o documento foi salvo ou de onde foi aberto.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Se o texto mudou desde a última vez que foi salvo ou aberto.
    pub fn is_dirty(&self) -> bool {
        self.text != self.saved
    }

    /// Texto da aba: o nome, com `*` quando há alterações (como no SSMS).
    pub fn title(&self) -> String {
        if self.is_dirty() {
            format!("{}*", self.name)
        } else {
            self.name.clone()
        }
    }

    /// Grava o texto em `path` e passa a usar esse arquivo.
    pub fn save_to(&mut self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, &self.text)?;
        self.saved = self.text.clone();
        self.name = file_name(path);
        self.path = Some(path.to_path_buf());
        Ok(())
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Todas as janelas de consulta, por número.
#[derive(Debug, Default)]
pub struct Documents {
    docs: BTreeMap<u32, Document>,
    next_id: u32,
    next_query: u32,
}

impl Documents {
    /// Cria uma consulta vazia `SQLQueryN.sql` e devolve o número dela.
    pub fn new_query(&mut self) -> u32 {
        self.next_query += 1;
        let name = format!("SQLQuery{}.sql", self.next_query);
        self.insert(Document {
            name,
            path: None,
            text: String::new(),
            saved: String::new(),
            editor: Default::default(),
            cursor: None,
            session: None,
            database: None,
            execution: None,
            results_mode: Default::default(),
            actual_plan: false,
        })
    }

    /// Cria `SQLQueryN.sql` com um texto (um script gerado) num banco.
    pub fn new_script(&mut self, text: String, database: Option<String>) -> u32 {
        let id = self.new_query();
        let doc = self.docs.get_mut(&id).expect("acabou de ser criado");
        doc.text = text;
        doc.database = database;
        id
    }

    /// Abre um arquivo. Se ele já está aberto, devolve o documento existente.
    pub fn open(&mut self, path: &Path) -> std::io::Result<u32> {
        if let Some(id) = self.find_path(path) {
            return Ok(id);
        }
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8(bytes).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the file is not valid UTF-8 text",
            )
        })?;
        Ok(self.insert(Document {
            name: file_name(path),
            path: Some(path.to_path_buf()),
            saved: text.clone(),
            text,
            editor: Default::default(),
            cursor: None,
            session: None,
            database: None,
            execution: None,
            results_mode: Default::default(),
            actual_plan: false,
        }))
    }

    fn insert(&mut self, doc: Document) -> u32 {
        self.next_id += 1;
        self.docs.insert(self.next_id, doc);
        self.next_id
    }

    fn find_path(&self, path: &Path) -> Option<u32> {
        let wanted = std::fs::canonicalize(path).ok();
        self.docs.iter().find_map(|(id, d)| {
            let p = d.path.as_deref()?;
            let same = match (&wanted, std::fs::canonicalize(p).ok()) {
                (Some(a), Some(b)) => *a == b,
                _ => p == path,
            };
            same.then_some(*id)
        })
    }

    /// Documento pelo número.
    pub fn get(&self, id: u32) -> Option<&Document> {
        self.docs.get(&id)
    }

    /// Documento pelo número, mutável.
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Document> {
        self.docs.get_mut(&id)
    }

    /// Remove o documento.
    pub fn remove(&mut self, id: u32) -> Option<Document> {
        self.docs.remove(&id)
    }

    /// Números dos documentos abertos, em ordem de abertura.
    pub fn ids(&self) -> Vec<u32> {
        self.docs.keys().copied().collect()
    }

    /// Números dos documentos com alterações.
    pub fn dirty(&self) -> Vec<u32> {
        self.docs
            .iter()
            .filter(|(_, d)| d.is_dirty())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Mantém só os documentos para os quais `keep` devolve `true`.
    pub fn retain(&mut self, mut keep: impl FnMut(u32) -> bool) {
        self.docs.retain(|id, _| keep(*id));
    }
}

/// Diálogos de arquivo do sistema. A casca não conhece a implementação:
/// o aplicativo usa os nativos e os testes usam respostas prontas.
pub trait FileDialogs {
    /// Pede onde salvar; `name` é o nome sugerido e `current`, o arquivo atual.
    fn save_path(&mut self, name: &str, current: Option<&Path>) -> Option<PathBuf>;
    /// Pede um arquivo para abrir.
    fn open_path(&mut self) -> Option<PathBuf>;
}

/// Sem diálogos: toda pergunta é cancelada.
#[derive(Debug, Default)]
pub struct NoDialogs;

impl FileDialogs for NoDialogs {
    fn save_path(&mut self, _name: &str, _current: Option<&Path>) -> Option<PathBuf> {
        None
    }

    fn open_path(&mut self) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pgstudio-docs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn new_queries_are_numbered() {
        let mut docs = Documents::default();
        let a = docs.new_query();
        let b = docs.new_query();
        assert_eq!(docs.get(a).unwrap().name(), "SQLQuery1.sql");
        assert_eq!(docs.get(b).unwrap().name(), "SQLQuery2.sql");
    }

    #[test]
    fn editing_marks_dirty_and_saving_clears_it() {
        let mut docs = Documents::default();
        let id = docs.new_query();
        let doc = docs.get_mut(id).unwrap();
        assert!(!doc.is_dirty());
        doc.text.push_str("select 1;");
        assert!(doc.is_dirty());
        assert_eq!(doc.title(), "SQLQuery1.sql*");
        let path = temp_file("saved.sql");
        doc.save_to(&path).unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(doc.title(), "saved.sql");
    }

    #[test]
    fn saved_file_reopens_identical() {
        let text = "select 'olá'\r\n  from t;\n\t-- fim";
        let path = temp_file("roundtrip.sql");
        let mut docs = Documents::default();
        let id = docs.new_query();
        let doc = docs.get_mut(id).unwrap();
        doc.text = text.to_string();
        doc.save_to(&path).unwrap();
        docs.remove(id);
        let id = docs.open(&path).unwrap();
        let doc = docs.get(id).unwrap();
        assert_eq!(doc.text, text);
        assert!(!doc.is_dirty());
        assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
    }

    #[test]
    fn opening_the_same_file_twice_reuses_the_document() {
        let path = temp_file("twice.sql");
        std::fs::write(&path, "select 2;").unwrap();
        let mut docs = Documents::default();
        let a = docs.open(&path).unwrap();
        let b = docs.open(&path).unwrap();
        assert_eq!(a, b);
        assert_eq!(docs.ids().len(), 1);
    }

    #[test]
    fn binary_file_is_rejected() {
        let path = temp_file("binary.sql");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let mut docs = Documents::default();
        assert!(docs.open(&path).is_err());
    }
}

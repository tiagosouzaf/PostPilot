//! Execução de consultas, do jeito que a interface vê.
//!
//! Cada janela de consulta tem a sua [`QuerySession`] (uma conexão própria,
//! como no SSMS), aberta por um [`QueryService`]. A sessão roda o SQL fora
//! da thread da interface e conta o que acontece por [`QueryEvent`]s, na
//! ordem: colunas e linhas de cada resultado, fim de cada comando, avisos do
//! servidor, erro e, sempre por último, [`QueryEvent::Finished`].

use std::fmt;
use std::sync::Arc;

/// Recebe os eventos de uma execução. É chamado de outra thread.
pub type QuerySink = Box<dyn FnMut(QueryEvent) + Send>;

/// Abre sessões de consulta no servidor conectado.
pub trait QueryService: Send + Sync {
    /// Cria uma sessão no banco `database`. A conexão só abre na primeira
    /// execução, fora da thread da interface.
    fn open(&self, database: &str) -> Arc<dyn QuerySession>;

    /// Motor do servidor, para o SQL que a interface monta.
    fn engine(&self) -> crate::connection::Engine {
        crate::connection::Engine::PostgreSql
    }
}

/// A conexão de uma janela de consulta.
pub trait QuerySession: Send + Sync {
    /// Começa a rodar `sql` (um ou mais comandos) e volta logo. Os eventos
    /// chegam em `sink`, terminando com [`QueryEvent::Finished`].
    fn execute(&self, sql: String, sink: QuerySink);

    /// Pede ao servidor para cancelar o comando em andamento, como o
    /// `pg_cancel_backend`. Sem comando em andamento, não faz nada.
    fn cancel(&self);

    /// Banco da sessão.
    fn database(&self) -> String;

    /// Motor do servidor, para o SQL que a interface monta.
    fn engine(&self) -> crate::connection::Engine {
        crate::connection::Engine::PostgreSql
    }

    /// De onde vem cada coluna do resultado de `sql` (um comando só), sem
    /// rodar nada: o servidor prepara o comando e diz, para cada coluna,
    /// a tabela e o número da coluna nela (`None` para expressões). Serve
    /// para saber se a grade do resultado pode ser editada. Por padrão a
    /// sessão não sabe dizer.
    fn describe(&self, sql: String, done: DescribeDone) {
        let _ = sql;
        done(Err("The session cannot describe queries.".to_string()));
    }

    /// Para os motores de documentos: como editar, na grade, o resultado
    /// de `text` (o que foi executado). `None` quando o motor edita por
    /// SQL ([`QuerySession::describe`]); `Err` com o motivo quando este
    /// resultado é só leitura.
    fn document_edits(&self, text: &str) -> Option<Result<Arc<dyn DocumentEdits>, String>> {
        let _ = text;
        None
    }
}

/// Recebe o fim de uma gravação de [`DocumentEdits`]. É chamado de outra
/// thread.
pub type EditDone<T> = Box<dyn FnOnce(Result<T, String>) + Send>;

/// Grava na coleção o que o usuário muda na grade de um resultado de
/// documentos. Cada linha é um documento, achado pelo valor da coluna da
/// chave como a grade mostra (`None` é NULL).
pub trait DocumentEdits: Send + Sync {
    /// De onde são os documentos, para mostrar (`banco.coleção`).
    fn display(&self) -> String;

    /// A coluna com a chave dos documentos (`_id`). Não é editável.
    fn key(&self) -> &str;

    /// Grava `value` (o texto digitado; `None` é NULL) no campo `field`
    /// do documento de chave `key`. Devolve o valor gravado como a grade
    /// mostra.
    fn update(
        &self,
        key: Option<String>,
        field: String,
        value: Option<String>,
        done: EditDone<Option<String>>,
    );

    /// Exclui os documentos com essas chaves. Devolve as chaves dos que
    /// foram excluídos.
    fn delete(&self, keys: Vec<Option<String>>, done: EditDone<Vec<Option<String>>>);
}

/// Recebe a resposta de [`QuerySession::describe`]. É chamado de outra
/// thread.
pub type DescribeDone = Box<dyn FnOnce(Result<Vec<Option<ColumnSource>>, String>) + Send>;

/// Coluna de tabela de onde sai uma coluna de um resultado.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColumnSource {
    /// `oid` da tabela.
    pub table: u32,
    /// `attnum` da coluna na tabela.
    pub column: i16,
}

/// O que aconteceu durante uma execução.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryEvent {
    /// Começa um resultado com estas colunas.
    Columns(Vec<String>),
    /// Uma linha do resultado atual, como texto (`None` é NULL).
    Row(Vec<Option<String>>),
    /// Um comando terminou; `rows` é o número de linhas lidas ou alteradas.
    /// Fecha o resultado atual, se houver.
    CommandComplete { rows: u64 },
    /// Aviso do servidor (`RAISE NOTICE`, avisos de DDL...).
    Notice(ServerMessage),
    /// Erro que parou a execução.
    Error(ServerMessage),
    /// A execução acabou. `canceled` quando parou por [`QuerySession::cancel`].
    Finished { canceled: bool },
}

/// Mensagem do servidor (aviso ou erro), com os campos do PostgreSQL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerMessage {
    /// `ERROR`, `NOTICE`, `WARNING`...
    pub severity: String,
    /// Código SQLSTATE, como `42P01`.
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    /// Posição do erro no SQL enviado, em caracteres a partir de 1.
    pub position: Option<usize>,
}

impl ServerMessage {
    /// Mensagem de erro sem campos do servidor (falha de conexão, por
    /// exemplo).
    pub fn error(message: impl Into<String>) -> Self {
        ServerMessage {
            severity: "ERROR".into(),
            message: message.into(),
            ..Default::default()
        }
    }
}

impl fmt::Display for ServerMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Serviço que não roda consultas: cada execução termina com `message`
/// como erro. Serve para o servidor de demonstração e para testes.
pub struct Offline(pub &'static str);

impl QueryService for Offline {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(OfflineSession(self.0, database.to_string()))
    }
}

struct OfflineSession(&'static str, String);

impl QuerySession for OfflineSession {
    fn execute(&self, _sql: String, mut sink: QuerySink) {
        sink(QueryEvent::Error(ServerMessage::error(self.0)));
        sink(QueryEvent::Finished { canceled: false });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.1.clone()
    }
}

/// Linha e coluna (a partir de 1) do caractere `char_idx` (a partir de 0).
pub fn line_col(text: &str, char_idx: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for c in text.chars().take(char_idx) {
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_of_a_position() {
        let t = "select 1;\nselect x\nfrom t";
        assert_eq!(line_col(t, 0), (1, 1));
        assert_eq!(line_col(t, 9), (1, 10));
        assert_eq!(line_col(t, 10), (2, 1));
        assert_eq!(line_col(t, 17), (2, 8));
        assert_eq!(line_col(t, 999), (3, 7));
    }
}

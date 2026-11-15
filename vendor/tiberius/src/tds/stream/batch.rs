//! PostPilot: a batch stream that keeps everything the server says.
//!
//! [`QueryStream`](super::QueryStream) only yields result metadata and rows:
//! `INFO` tokens (`PRINT`, `RAISERROR ... WITH NOWAIT`, `SET STATISTICS`),
//! the row counts of `DONE` tokens and every error but the first are dropped.
//! A query window needs all of them, in order, like the Messages tab of SSMS.
//! [`BatchStream`] yields them as [`BatchItem`]s.

use crate::tds::codec::{TokenError, TokenInfo};
use crate::tds::stream::ReceivedToken;
use crate::{row::ColumnType, Column, Row};
use futures_util::{
    ready,
    stream::{BoxStream, Stream, StreamExt},
};
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{self, Poll, Waker},
};

/// Cancel state shared by a [`Client`](crate::Client) and its
/// [`CancelHandle`]s.
#[derive(Debug, Default)]
pub struct CancelState {
    requested: AtomicBool,
    sent: AtomicBool,
    acknowledged: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl CancelState {
    pub(crate) fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_sent(&self) {
        self.sent.store(true, Ordering::SeqCst);
    }

    pub(crate) fn mark_acknowledged(&self) {
        self.acknowledged.store(true, Ordering::SeqCst);
    }

    pub(crate) fn register(&self, waker: &Waker) {
        if let Ok(mut w) = self.waker.lock() {
            match w.as_ref() {
                Some(old) if old.will_wake(waker) => {}
                _ => *w = Some(waker.clone()),
            }
        }
    }
}

/// Cancels the request a [`Client`](crate::Client) is reading, from another
/// task or thread. The client sends the TDS Attention packet the next time
/// it reads from the server (it is woken up for that), and the server ends
/// the response with an acknowledging `DONE` ([`BatchItem::Done`] with
/// `attention`).
#[derive(Clone, Debug)]
pub struct CancelHandle(pub(crate) Arc<CancelState>);

impl CancelHandle {
    /// Asks for the cancel.
    pub fn cancel(&self) {
        self.0.requested.store(true, Ordering::SeqCst);
        let waker = self.0.waker.lock().ok().and_then(|mut w| w.take());
        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Clears the state before a new request.
    pub fn reset(&self) {
        self.0.requested.store(false, Ordering::SeqCst);
        self.0.sent.store(false, Ordering::SeqCst);
        self.0.acknowledged.store(false, Ordering::SeqCst);
    }

    /// A cancel was asked for since the last [`reset`](Self::reset).
    pub fn is_requested(&self) -> bool {
        self.0.requested()
    }

    /// The Attention packet went out and the server has not acknowledged it
    /// yet: call [`Client::finish_cancel`](crate::Client::finish_cancel)
    /// before the next request.
    pub fn is_pending(&self) -> bool {
        self.0.sent.load(Ordering::SeqCst) && !self.0.acknowledged.load(Ordering::SeqCst)
    }
}

/// A message from the server: an `INFO` or an `ERROR` token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchMessage {
    /// Message number (`50000` for `RAISERROR` with text, `0` for `PRINT`).
    pub number: u32,
    /// Error state.
    pub state: u8,
    /// Severity: up to 10 is information, 11 and up is an error.
    pub class: u8,
    /// The text.
    pub message: String,
    /// Server name.
    pub server: String,
    /// Procedure the message came from, empty for the batch itself.
    pub procedure: String,
    /// Line in the batch (or in the procedure), from 1.
    pub line: u32,
    /// `true` for an `ERROR` token.
    pub is_error: bool,
}

impl From<TokenInfo> for BatchMessage {
    fn from(t: TokenInfo) -> Self {
        BatchMessage {
            number: t.number,
            state: t.state,
            class: t.class,
            message: t.message,
            server: t.server,
            procedure: t.procedure,
            line: t.line,
            is_error: false,
        }
    }
}

impl From<TokenError> for BatchMessage {
    fn from(t: TokenError) -> Self {
        BatchMessage {
            number: t.code,
            state: t.state,
            class: t.class,
            message: t.message,
            server: t.server,
            procedure: t.procedure,
            line: t.line,
            is_error: true,
        }
    }
}

/// What a batch produced, in the order the server sent it.
#[derive(Debug)]
pub enum BatchItem {
    /// A result set starts with these columns.
    Metadata(Arc<Vec<Column>>),
    /// A row of the current result set.
    Row(Row),
    /// A statement finished. `rows` is the row count when the server sent
    /// one (`SET NOCOUNT OFF`); `in_proc` for statements inside procedures.
    Done {
        /// Rows read or changed, when counted.
        rows: Option<u64>,
        /// `DONEINPROC`/`DONEPROC` instead of `DONE`.
        in_proc: bool,
        /// The server acknowledged a cancel (attention).
        attention: bool,
    },
    /// `INFO` or `ERROR` token.
    Message(BatchMessage),
    /// `RETURN` value of a procedure.
    ReturnStatus(u32),
}

/// Stream of [`BatchItem`]s, from [`Client::batch`](crate::Client::batch).
/// It must be polled to the end (or dropped) before the next request.
pub struct BatchStream<'a> {
    token_stream: BoxStream<'a, crate::Result<ReceivedToken>>,
    cancel: Arc<CancelState>,
    columns: Option<Arc<Vec<Column>>>,
    result_index: usize,
    saw_error: bool,
}

impl<'a> std::fmt::Debug for BatchStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchStream")
            .field("result_index", &self.result_index)
            .finish_non_exhaustive()
    }
}

impl<'a> BatchStream<'a> {
    pub(crate) fn new(
        token_stream: BoxStream<'a, crate::Result<ReceivedToken>>,
        cancel: Arc<CancelState>,
    ) -> Self {
        Self {
            token_stream,
            cancel,
            columns: None,
            result_index: 0,
            saw_error: false,
        }
    }
}

impl<'a> Stream for BatchStream<'a> {
    type Item = crate::Result<BatchItem>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            let token = match ready!(this.token_stream.poll_next_unpin(cx)) {
                Some(Ok(token)) => token,
                // The token stream repeats the first server error at the end;
                // it was already yielded as a message.
                Some(Err(crate::Error::Server(_))) if this.saw_error => {
                    return Poll::Ready(None);
                }
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                None => return Poll::Ready(None),
            };
            let item = match token {
                ReceivedToken::NewResultset(meta) => {
                    let columns = meta
                        .columns
                        .iter()
                        .map(|x| Column {
                            name: x.col_name.to_string(),
                            column_type: ColumnType::from(&x.base.ty),
                        })
                        .collect::<Vec<_>>();
                    if this.columns.is_some() {
                        this.result_index += 1;
                    }
                    let columns = Arc::new(columns);
                    this.columns = Some(columns.clone());
                    BatchItem::Metadata(columns)
                }
                ReceivedToken::Row(data) => {
                    let Some(columns) = this.columns.clone() else {
                        return Poll::Ready(Some(Err(crate::Error::Protocol(
                            "ROW token arrived before any column metadata".into(),
                        ))));
                    };
                    BatchItem::Row(Row {
                        columns,
                        data,
                        result_index: this.result_index,
                    })
                }
                ReceivedToken::Done(d)
                | ReceivedToken::DoneInProc(d)
                | ReceivedToken::DoneProc(d)
                    if d.is_attention() =>
                {
                    this.cancel.mark_acknowledged();
                    BatchItem::Done {
                        rows: None,
                        in_proc: false,
                        attention: true,
                    }
                }
                ReceivedToken::Done(d) => BatchItem::Done {
                    rows: d.counted_rows(),
                    in_proc: false,
                    attention: false,
                },
                ReceivedToken::DoneInProc(d) | ReceivedToken::DoneProc(d) => BatchItem::Done {
                    rows: d.counted_rows(),
                    in_proc: true,
                    attention: false,
                },
                ReceivedToken::Info(info) => BatchItem::Message(info.into()),
                ReceivedToken::Error(err) => {
                    this.saw_error = true;
                    BatchItem::Message(err.into())
                }
                ReceivedToken::ReturnStatus(s) => BatchItem::ReturnStatus(s),
                _ => continue,
            };
            return Poll::Ready(Some(Ok(item)));
        }
    }
}

//! Buffer paginado de um resultado.
//!
//! As linhas chegam em stream e são guardadas em páginas compactas: o texto
//! de todas as células da página num `String` só, o fim de cada célula num
//! `Vec<u32>` e os NULLs num mapa de bits. Quando as páginas em memória
//! passam do orçamento ([`ResultStore::with_budget`]), as mais antigas vão
//! para um arquivo temporário e voltam sob demanda, guardando poucas em
//! cache. Assim a memória fica limitada pelo orçamento, não pelo tamanho do
//! resultado.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::view::{Filter, ViewProgress, ViewSpec, sort_rows};

/// Linhas por página.
pub const PAGE_ROWS: usize = 1024;

/// Orçamento padrão de memória das páginas de um resultado.
pub const DEFAULT_BUDGET: usize = 64 * 1024 * 1024;

/// Páginas lidas do disco mantidas em memória.
const CACHE_PAGES: usize = 8;

/// Até tantas linhas, a ordenação roda na hora; acima, numa thread.
const SORT_INLINE_ROWS: usize = 20_000;

/// Linhas de um resultado, como texto (`None` é NULL).
pub struct ResultStore {
    columns: Vec<String>,
    pages: Vec<Page>,
    /// Página sendo preenchida (sempre em memória).
    open: PageData,
    rows: u64,
    budget: usize,
    /// Bytes das páginas fechadas que estão em memória.
    resident: usize,
    spill: Option<Spill>,
    /// Páginas lidas do disco, da mais recente para a mais antiga.
    cache: Vec<(usize, PageData)>,
    /// Erro ao usar o arquivo temporário; daí em diante tudo fica em memória.
    spill_error: Option<String>,
    /// Valores trocados depois da leitura (edição na grade), pela linha
    /// guardada e a coluna. As páginas não mudam.
    edits: HashMap<(u64, usize), Option<String>>,
    /// Linhas guardadas que saíram do resultado (excluídas na grade), em
    /// ordem.
    removed: Vec<u64>,
    /// Ordem e filtro em uso: as linhas guardadas que aparecem, na ordem
    /// em que aparecem. `None`: todas, como chegaram.
    view: Option<View>,
    /// Visão sendo montada (a anterior continua valendo até terminar).
    job: Option<ViewJob>,
    /// Muda quando as linhas mostradas trocam de posição.
    view_generation: u64,
}

struct View {
    spec: ViewSpec,
    rows: Vec<u64>,
    /// Linhas guardadas já vistas (as que chegaram depois ainda não).
    covered: u64,
}

struct ViewJob {
    spec: ViewSpec,
    filter: Filter,
    /// Só as linhas que chegaram depois da visão atual (mesmo filtro, sem
    /// ordem): o que achar é acrescentado no fim.
    extend: bool,
    /// Próxima linha guardada a ler.
    next: u64,
    rows: Vec<u64>,
    /// Valor da coluna da ordenação em cada linha de `rows`.
    keys: Vec<Option<String>>,
    /// A ordenação, rodando numa thread.
    sorting: Option<Receiver<Vec<u64>>>,
}

enum Page {
    Resident(PageData),
    Spilled { offset: u64, len: u64, rows: usize },
}

#[derive(Clone, Default)]
struct PageData {
    text: String,
    /// Fim (byte em `text`) de cada célula, linha a linha.
    ends: Vec<u32>,
    /// Bit `i` ligado: a célula `i` é NULL.
    nulls: Vec<u64>,
    rows: usize,
}

impl PageData {
    fn bytes(&self) -> usize {
        self.text.len() + self.ends.len() * 4 + self.nulls.len() * 8
    }

    fn push(&mut self, row: &[Option<String>], columns: usize) {
        for c in 0..columns {
            let i = self.rows * columns + c;
            match row.get(c).and_then(|v| v.as_deref()) {
                Some(v) => self.text.push_str(v),
                None => {
                    if self.nulls.len() <= i / 64 {
                        self.nulls.resize(i / 64 + 1, 0);
                    }
                    self.nulls[i / 64] |= 1 << (i % 64);
                }
            }
            self.ends.push(self.text.len() as u32);
        }
        self.rows += 1;
    }

    fn cell(&self, row: usize, col: usize, columns: usize) -> Option<&str> {
        let i = row * columns + col;
        if self
            .nulls
            .get(i / 64)
            .is_some_and(|w| w & (1 << (i % 64)) != 0)
        {
            return None;
        }
        let start = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        Some(&self.text[start..self.ends[i] as usize])
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bytes() + 24);
        for n in [
            self.rows,
            self.text.len(),
            self.ends.len(),
            self.nulls.len(),
        ] {
            out.extend_from_slice(&(n as u64).to_le_bytes());
        }
        out.extend_from_slice(self.text.as_bytes());
        for e in &self.ends {
            out.extend_from_slice(&e.to_le_bytes());
        }
        for n in &self.nulls {
            out.extend_from_slice(&n.to_le_bytes());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Option<PageData> {
        let u64_at = |at: usize| -> Option<u64> {
            Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
        };
        let rows = u64_at(0)? as usize;
        let text_len = u64_at(8)? as usize;
        let ends_len = u64_at(16)? as usize;
        let nulls_len = u64_at(24)? as usize;
        let mut at = 32;
        let text = String::from_utf8(bytes.get(at..at + text_len)?.to_vec()).ok()?;
        at += text_len;
        let ends = bytes
            .get(at..at + ends_len * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        at += ends_len * 4;
        let nulls = bytes
            .get(at..at + nulls_len * 8)?
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect();
        Some(PageData {
            text,
            ends,
            nulls,
            rows,
        })
    }
}

/// Arquivo temporário das páginas que saíram da memória. Apagado ao soltar.
struct Spill {
    file: File,
    path: PathBuf,
    len: u64,
}

impl Spill {
    fn create() -> std::io::Result<Spill> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "pgstudio-results-{}-{}.bin",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Spill { file, path, len: 0 })
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl ResultStore {
    /// Resultado vazio com estas colunas e o orçamento padrão.
    pub fn new(columns: Vec<String>) -> Self {
        Self::with_budget(columns, DEFAULT_BUDGET)
    }

    /// Resultado com `budget` bytes de páginas em memória; o resto vai para
    /// o disco.
    pub fn with_budget(columns: Vec<String>, budget: usize) -> Self {
        ResultStore {
            columns,
            pages: Vec::new(),
            open: PageData::default(),
            rows: 0,
            budget,
            resident: 0,
            spill: None,
            cache: Vec::new(),
            spill_error: None,
            edits: HashMap::new(),
            removed: Vec::new(),
            view: None,
            job: None,
            view_generation: 0,
        }
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Linhas à mostra: as do resultado que passam no filtro em uso.
    pub fn len(&self) -> u64 {
        match &self.view {
            Some(v) => v.rows.len() as u64,
            None => self.total_len(),
        }
    }

    /// Linhas do resultado (as guardadas menos as removidas), com ou sem
    /// filtro.
    pub fn total_len(&self) -> u64 {
        self.rows - self.removed.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Linha guardada que aparece como a linha `row` do resultado.
    fn stored_row(&self, row: u64) -> u64 {
        if let Some(v) = &self.view {
            return v.rows.get(row as usize).copied().unwrap_or(u64::MAX);
        }
        let mut stored = row;
        for &r in &self.removed {
            if r <= stored {
                stored += 1;
            } else {
                break;
            }
        }
        stored
    }

    /// Troca o valor de uma célula (depois de gravado no banco).
    pub fn set_cell(&mut self, row: u64, col: usize, value: Option<String>) {
        if col >= self.columns.len() || row >= self.len() {
            return;
        }
        let stored = self.stored_row(row);
        self.edits.insert((stored, col), value);
    }

    /// Tira a linha `row` do resultado (excluída no banco); as de baixo
    /// sobem uma posição.
    pub fn remove_row(&mut self, row: u64) {
        if row >= self.len() {
            return;
        }
        let stored = self.stored_row(row);
        let at = self.removed.partition_point(|&r| r < stored);
        self.removed.insert(at, stored);
        if let Some(v) = &mut self.view {
            v.rows.remove(row as usize);
        }
    }

    /// Bytes de linhas em memória agora (páginas, página aberta e cache).
    pub fn resident_bytes(&self) -> usize {
        self.resident + self.open.bytes() + self.cache.iter().map(|(_, p)| p.bytes()).sum::<usize>()
    }

    /// Se alguma página foi para o disco.
    pub fn is_spilled(&self) -> bool {
        self.spill.is_some()
    }

    /// Erro do arquivo temporário, se houve (o resultado continua em
    /// memória).
    pub fn spill_error(&self) -> Option<&str> {
        self.spill_error.as_deref()
    }

    /// Acrescenta uma linha. Colunas a mais são ignoradas; a menos viram
    /// NULL.
    pub fn push(&mut self, row: &[Option<String>]) {
        self.open.push(row, self.columns.len());
        self.rows += 1;
        if self.open.rows == PAGE_ROWS {
            let page = std::mem::take(&mut self.open);
            self.resident += page.bytes();
            self.pages.push(Page::Resident(page));
            self.enforce_budget();
        }
    }

    /// Manda páginas antigas para o disco até caber no orçamento.
    fn enforce_budget(&mut self) {
        if self.spill_error.is_some() {
            return;
        }
        let mut i = 0;
        while self.resident > self.budget && i < self.pages.len() {
            if let Page::Resident(data) = &self.pages[i] {
                match write_page(&mut self.spill, data) {
                    Ok(page) => {
                        self.resident -= data.bytes();
                        self.pages[i] = page;
                    }
                    Err(e) => {
                        self.spill_error = Some(e.to_string());
                        return;
                    }
                }
            }
            i += 1;
        }
    }

    /// Valor da célula; `None` para NULL ou fora do resultado. Pode ler do
    /// disco, por isso pede `&mut`.
    pub fn cell(&mut self, row: u64, col: usize) -> Option<&str> {
        if col >= self.columns.len() || row >= self.len() {
            return None;
        }
        let row = if self.removed.is_empty() && self.view.is_none() {
            row
        } else {
            self.stored_row(row)
        };
        self.stored_cell(row, col)
    }

    /// Valor da célula pela linha guardada (sem filtro, ordem e remoções).
    fn stored_cell(&mut self, row: u64, col: usize) -> Option<&str> {
        if !self.edits.is_empty() && self.edits.contains_key(&(row, col)) {
            return self.edits.get(&(row, col)).and_then(|v| v.as_deref());
        }
        let page = (row / PAGE_ROWS as u64) as usize;
        let local = (row % PAGE_ROWS as u64) as usize;
        let columns = self.columns.len();
        self.page(page)?.cell(local, col, columns)
    }

    /// A linha inteira, como texto.
    pub fn row(&mut self, row: u64) -> Vec<Option<String>> {
        (0..self.columns.len())
            .map(|c| self.cell(row, c).map(str::to_string))
            .collect()
    }

    fn page(&mut self, index: usize) -> Option<&PageData> {
        if index == self.pages.len() {
            return Some(&self.open);
        }
        let (offset, len) = match self.pages.get(index)? {
            Page::Resident(_) => {
                return match &self.pages[index] {
                    Page::Resident(d) => Some(d),
                    Page::Spilled { .. } => None,
                };
            }
            Page::Spilled { offset, len, .. } => (*offset, *len),
        };
        if let Some(pos) = self.cache.iter().position(|(i, _)| *i == index) {
            let hit = self.cache.remove(pos);
            self.cache.insert(0, hit);
        } else {
            let data = self.read_page(offset, len)?;
            self.cache.insert(0, (index, data));
            self.cache.truncate(CACHE_PAGES);
        }
        self.cache.first().map(|(_, d)| d)
    }

    fn read_page(&mut self, offset: u64, len: u64) -> Option<PageData> {
        let spill = self.spill.as_mut()?;
        let mut bytes = vec![0; len as usize];
        spill.file.seek(SeekFrom::Start(offset)).ok()?;
        spill.file.read_exact(&mut bytes).ok()?;
        PageData::decode(&bytes)
    }

    /// A ordem e o filtro em uso (os da última visão que ficou pronta).
    pub fn view_spec(&self) -> Option<&ViewSpec> {
        self.view.as_ref().map(|v| &v.spec)
    }

    /// Muda sempre que as linhas à mostra trocam de posição (uma seleção
    /// feita antes deixa de valer).
    pub fn view_generation(&self) -> u64 {
        self.view_generation
    }

    /// Leva a visão para `spec`, trabalhando no máximo `budget` nesta
    /// chamada; chame a cada frame enquanto devolver `working`. Até
    /// terminar, o resultado continua à mostra como estava. As linhas que
    /// chegam depois entram na visão nas chamadas seguintes.
    pub fn sync_view(&mut self, spec: &ViewSpec, budget: Duration) -> ViewProgress {
        if spec.is_plain() {
            self.job = None;
            if self.view.take().is_some() {
                self.view_generation += 1;
            }
            return ViewProgress::IDLE;
        }
        if self.job.as_ref().is_some_and(|j| j.spec != *spec) {
            self.job = None;
        }
        let mut job = match self.job.take() {
            Some(job) => job,
            None => {
                let current = self.view.as_ref().filter(|v| v.spec == *spec);
                if current.is_some_and(|v| v.covered == self.rows) {
                    return ViewProgress::IDLE;
                }
                let extend = current.filter(|_| spec.sort.is_none());
                ViewJob {
                    spec: spec.clone(),
                    filter: Filter::parse(&spec.filter, &self.columns),
                    extend: extend.is_some(),
                    next: extend.map_or(0, |v| v.covered),
                    rows: Vec::new(),
                    keys: Vec::new(),
                    sorting: None,
                }
            }
        };
        let started = Instant::now();
        let columns = self.columns.len();
        let sort_col = job.spec.sort.map(|s| s.col).filter(|&c| c < columns);
        let mut buf = String::new();
        while job.sorting.is_none() && job.next < self.rows {
            let stored = job.next;
            job.next += 1;
            if self.removed.binary_search(&stored).is_err() {
                // Uma linha por vez: as células são lidas e comparadas na
                // hora, sem copiar.
                let hit = job.filter.is_empty() || self.row_matches(&job.filter, stored, &mut buf);
                if hit {
                    job.rows.push(stored);
                    if let Some(c) = sort_col {
                        let key = self.stored_cell(stored, c).map(str::to_string);
                        job.keys.push(key);
                    }
                }
            }
            if job.next.is_multiple_of(128) && started.elapsed() >= budget {
                break;
            }
        }
        if job.next < self.rows {
            let fraction = job.next as f32 / self.rows.max(1) as f32;
            self.job = Some(job);
            return ViewProgress {
                working: true,
                fraction,
            };
        }
        if let (Some(sort), None) = (job.spec.sort, &job.sorting)
            && sort_col.is_some()
        {
            let rows = std::mem::take(&mut job.rows);
            let keys = std::mem::take(&mut job.keys);
            let (tx, rx) = mpsc::channel();
            if rows.len() <= SORT_INLINE_ROWS {
                // Pouca coisa: ordena aqui mesmo, sem esperar uma thread.
                let _ = tx.send(sort_rows(rows, keys, sort.descending));
            } else {
                std::thread::spawn(move || {
                    let _ = tx.send(sort_rows(rows, keys, sort.descending));
                });
            }
            job.sorting = Some(rx);
        }
        if let Some(rx) = &job.sorting {
            match rx.try_recv() {
                Ok(rows) => job.rows = rows,
                Err(mpsc::TryRecvError::Empty) => {
                    self.job = Some(job);
                    return ViewProgress {
                        working: true,
                        fraction: 1.0,
                    };
                }
                // A thread morreu: recomeça na próxima chamada.
                Err(mpsc::TryRecvError::Disconnected) => {
                    return ViewProgress {
                        working: true,
                        fraction: 0.0,
                    };
                }
            }
        }
        // Linhas excluídas enquanto a visão era montada.
        let removed = &self.removed;
        if !removed.is_empty() {
            job.rows.retain(|r| removed.binary_search(r).is_err());
        }
        match &mut self.view {
            Some(v) if job.extend => {
                v.rows.append(&mut job.rows);
                v.covered = job.next;
            }
            _ => {
                self.view = Some(View {
                    spec: job.spec,
                    rows: job.rows,
                    covered: job.next,
                });
                self.view_generation += 1;
            }
        }
        ViewProgress {
            working: self.rows > job.next,
            fraction: 1.0,
        }
    }

    /// Se a linha guardada passa no filtro.
    fn row_matches(&mut self, filter: &Filter, stored: u64, buf: &mut String) -> bool {
        let columns = self.columns.len();
        if self.edits.is_empty() {
            // Sem edições, a página é lida uma vez só para a linha toda.
            let local = (stored % PAGE_ROWS as u64) as usize;
            let Some(page) = self.page((stored / PAGE_ROWS as u64) as usize) else {
                return false;
            };
            return filter.matches(columns, buf, |c| page.cell(local, c, columns));
        }
        let row: Vec<Option<String>> = (0..columns)
            .map(|c| self.stored_cell(stored, c).map(str::to_string))
            .collect();
        filter.matches(columns, buf, |c| row[c].as_deref())
    }

    /// Número de linhas de cada página fechada (para testes).
    #[doc(hidden)]
    pub fn page_rows(&self) -> Vec<usize> {
        self.pages
            .iter()
            .map(|p| match p {
                Page::Resident(d) => d.rows,
                Page::Spilled { rows, .. } => *rows,
            })
            .collect()
    }
}

fn write_page(spill: &mut Option<Spill>, data: &PageData) -> std::io::Result<Page> {
    if spill.is_none() {
        *spill = Some(Spill::create()?);
    }
    let spill = spill.as_mut().ok_or(std::io::ErrorKind::Other)?;
    let bytes = data.encode();
    spill.file.seek(SeekFrom::Start(spill.len))?;
    spill.file.write_all(&bytes)?;
    let page = Page::Spilled {
        offset: spill.len,
        len: bytes.len() as u64,
        rows: data.rows,
    };
    spill.len += bytes.len() as u64;
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("c{i}")).collect()
    }

    #[test]
    fn edits_and_removed_rows_change_what_is_read() {
        let mut s = ResultStore::new(cols(2));
        for i in 0..5 {
            s.push(&[Some(i.to_string()), None]);
        }
        s.set_cell(1, 1, Some("x".into()));
        s.remove_row(0);
        // A antiga linha 1 agora é a 0, com o valor trocado.
        assert_eq!(s.len(), 4);
        assert_eq!(s.row(0), vec![Some("1".into()), Some("x".into())]);
        s.remove_row(1);
        assert_eq!(s.cell(1, 0), Some("3"));
        s.set_cell(1, 1, Some("y".into()));
        s.set_cell(0, 0, None);
        assert_eq!(s.row(0), vec![None, Some("x".into())]);
        assert_eq!(s.row(1), vec![Some("3".into()), Some("y".into())]);
        assert_eq!(s.cell(3, 0), None);
        s.remove_row(0);
        s.remove_row(0);
        assert_eq!(s.row(0), vec![Some("4".into()), None]);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn keeps_values_and_nulls() {
        let mut s = ResultStore::new(cols(3));
        s.push(&[Some("a".into()), None, Some("".into())]);
        s.push(&[Some("ção".into()), Some("x".into())]);
        assert_eq!(s.len(), 2);
        assert_eq!(s.cell(0, 0), Some("a"));
        assert_eq!(s.cell(0, 1), None);
        assert_eq!(s.cell(0, 2), Some(""));
        assert_eq!(s.cell(1, 0), Some("ção"));
        assert_eq!(s.cell(1, 2), None, "faltando vira NULL");
        assert_eq!(s.cell(2, 0), None);
        assert_eq!(s.cell(0, 9), None);
    }

    #[test]
    fn pages_spill_to_disk_beyond_the_budget_and_come_back() {
        let mut s = ResultStore::with_budget(cols(2), 64 * 1024);
        let n = PAGE_ROWS as u64 * 40 + 7;
        for i in 0..n {
            let null = (i % 13 == 0).then_some(());
            s.push(&[
                Some(format!("linha {i}")),
                null.map_or_else(|| Some("é".repeat((i % 5) as usize)), |_| None),
            ]);
        }
        assert_eq!(s.len(), n);
        assert!(s.is_spilled());
        assert!(
            s.resident_bytes() < 64 * 1024 + 2 * 40 * 1024 * 8,
            "{}",
            s.resident_bytes()
        );
        for i in [0, 1, 1023, 1024, 20_000, n - 1, 5, n - 2] {
            assert_eq!(s.cell(i, 0).map(str::to_string), Some(format!("linha {i}")));
            let expected = (i % 13 != 0).then(|| "é".repeat((i % 5) as usize));
            assert_eq!(s.cell(i, 1).map(str::to_string), expected, "{i}");
        }
        assert!(s.page_rows().iter().all(|r| *r == PAGE_ROWS));
    }

    #[test]
    fn spill_file_is_removed_on_drop() {
        let mut s = ResultStore::with_budget(cols(1), 0);
        for i in 0..PAGE_ROWS * 2 {
            s.push(&[Some(i.to_string())]);
        }
        let path = s.spill.as_ref().unwrap().path.clone();
        assert!(path.exists());
        drop(s);
        assert!(!path.exists());
    }
}

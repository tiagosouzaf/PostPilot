//! As escolhas que valem para todas as grades de resultado: se o resultado
//! aparece em páginas, de quantas linhas, e o formato do que o Ctrl+C copia.
//!
//! Ficam no contexto do egui, e não em cada [`crate::GridState`], porque o
//! controle embaixo de qualquer grade muda a escolha de todas (e a casca
//! grava nas preferências, para voltar igual ao reabrir o app).

use eframe::egui;

use crate::copy::CopyFormat;

/// Tamanhos de página oferecidos embaixo da grade e nas opções.
pub const PAGE_SIZES: [u32; 6] = [50, 100, 500, 1000, 5000, 10_000];

/// Tamanho de página padrão, o mesmo do DataGrip.
pub const DEFAULT_PAGE_SIZE: u32 = 500;

/// As escolhas das grades.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridPrefs {
    /// Mostrar o resultado em páginas.
    pub paginate: bool,
    /// Linhas por página (vale com `paginate`; fica guardado sem ele, para
    /// voltar ao mesmo tamanho ao ligar de novo).
    pub page_size: u32,
    /// Formato do que o Ctrl+C copia.
    pub copy_format: CopyFormat,
    /// Nomes das colunas na primeira linha (TSV e CSV).
    pub copy_headers: bool,
}

impl Default for GridPrefs {
    fn default() -> Self {
        GridPrefs {
            paginate: true,
            page_size: DEFAULT_PAGE_SIZE,
            copy_format: CopyFormat::Tsv,
            copy_headers: false,
        }
    }
}

impl GridPrefs {
    /// Linhas por página em uso; `None` sem paginação.
    pub fn page_rows(&self) -> Option<u64> {
        self.paginate.then(|| u64::from(self.page_size.max(1)))
    }
}

fn id() -> egui::Id {
    egui::Id::new("pgstudio_result_grid_prefs")
}

/// As escolhas em uso (as padrão, se ninguém definiu).
pub fn get(ctx: &egui::Context) -> GridPrefs {
    ctx.data(|d| d.get_temp(id())).unwrap_or_default()
}

/// Troca as escolhas de todas as grades.
pub fn set(ctx: &egui::Context, prefs: GridPrefs) {
    ctx.data_mut(|d| d.insert_temp(id(), prefs));
}

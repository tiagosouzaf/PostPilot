//! Layout salvo entre sessões: painéis encaixados e geometria da janela.
//!
//! Fica num arquivo JSON versionado (`layout.json` na pasta de
//! configuração). Um arquivo ilegível, de outra versão ou com valores
//! absurdos nunca impede o aplicativo de abrir: cai no layout padrão.

use std::path::{Path, PathBuf};

use pgstudio_docking::Layout;
use serde::{Deserialize, Serialize};

use crate::panels::Panel;

/// Versão atual do formato. Mude ao alterar o formato de modo incompatível.
pub const VERSION: u32 = 1;

/// Menor tamanho aceito para a janela restaurada.
pub const MIN_SIZE: [f32; 2] = [640.0, 400.0];

/// Posição e tamanho da janela (sem maximizar).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    /// Canto superior esquerdo da janela, quando o sistema informa.
    pub position: Option<[f32; 2]>,
    /// Tamanho da área interna.
    pub size: [f32; 2],
    /// Se estava maximizada.
    #[serde(default)]
    pub maximized: bool,
}

impl WindowGeometry {
    /// Corrige valores fora do razoável (NaN, negativos, gigantes).
    pub fn sanitized(self) -> Option<Self> {
        let [w, h] = self.size;
        if !(w.is_finite() && h.is_finite()) {
            return None;
        }
        let size = [
            w.clamp(MIN_SIZE[0], 16_384.0),
            h.clamp(MIN_SIZE[1], 16_384.0),
        ];
        let position = self
            .position
            .filter(|[x, y]| x.is_finite() && y.is_finite())
            .filter(|[x, y]| x.abs() < 100_000.0 && y.abs() < 100_000.0);
        Some(WindowGeometry {
            position,
            size,
            maximized: self.maximized,
        })
    }
}

/// Conteúdo do arquivo de layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedLayout {
    /// Geometria da janela principal.
    pub window: Option<WindowGeometry>,
    /// Painéis encaixados (sem as janelas de consulta).
    pub docking: Layout<Panel>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(flatten)]
    layout: SavedLayout,
}

/// Lê o layout de um texto JSON.
pub fn parse(text: &str) -> Result<SavedLayout, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(v) if v == u64::from(VERSION) => {}
        Some(v) => return Err(format!("unsupported layout version {v}")),
        None => return Err("missing layout version".into()),
    }
    let file: File = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let mut layout = file.layout;
    layout.window = layout.window.and_then(WindowGeometry::sanitized);
    Ok(layout)
}

/// Escreve o layout como texto JSON.
pub fn to_json(layout: &SavedLayout) -> String {
    let file = File {
        version: VERSION,
        layout: layout.clone(),
    };
    serde_json::to_string_pretty(&file).expect("layout is always serializable")
}

/// Arquivo de layout e o que foi lido dele ao abrir.
#[derive(Clone, Debug)]
pub struct LayoutStore {
    path: PathBuf,
    /// Layout lido, se o arquivo existia e estava bom.
    pub saved: Option<SavedLayout>,
    /// Problema ao ler o arquivo, para avisar o usuário.
    pub error: Option<String>,
}

impl LayoutStore {
    /// Lê `path`. Arquivo inexistente não é erro; os demais problemas ficam
    /// em `error` e o layout fica vazio (o padrão é usado).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (saved, error) = match std::fs::read_to_string(&path) {
            Ok(text) => match parse(&text) {
                Ok(layout) => (Some(layout), None),
                Err(e) => (None, Some(e)),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(e) => (None, Some(e.to_string())),
        };
        LayoutStore { path, saved, error }
    }

    /// Caminho do arquivo.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Grava o layout, criando a pasta se preciso. Escreve num arquivo
    /// temporário e renomeia, para nunca deixar um arquivo pela metade.
    pub fn save(&self, layout: &SavedLayout) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, to_json(layout))?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels;

    fn sample() -> SavedLayout {
        SavedLayout {
            window: Some(WindowGeometry {
                position: Some([10.0, 20.0]),
                size: [1024.0, 700.0],
                maximized: true,
            }),
            docking: panels::default_layout().layout(|_| true),
        }
    }

    #[test]
    fn round_trips() {
        let layout = sample();
        assert_eq!(parse(&to_json(&layout)).unwrap(), layout);
    }

    #[test]
    fn rejects_other_versions_and_garbage() {
        let json = to_json(&sample()).replace("\"version\": 1", "\"version\": 99");
        assert!(parse(&json).unwrap_err().contains("99"));
        for text in [
            "",
            "{",
            "null",
            "[]",
            "{\"version\":1}",
            "{\"version\":\"1\"}",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn absurd_window_values_are_fixed() {
        let g = WindowGeometry {
            position: Some([f32::NAN, 0.0]),
            size: [10.0, 1e9],
            maximized: false,
        };
        let g = g.sanitized().unwrap();
        assert_eq!(g.position, None);
        assert_eq!(g.size, [MIN_SIZE[0], 16_384.0]);
        let nan = WindowGeometry {
            position: None,
            size: [f32::NAN, 1.0],
            maximized: false,
        };
        assert_eq!(nan.sanitized(), None);
    }

    #[test]
    fn store_saves_and_reopens() {
        let dir = std::env::temp_dir().join(format!("pgstudio-layout-{}", std::process::id()));
        let path = dir.join("sub").join("layout.json");
        let _ = std::fs::remove_file(&path);
        let store = LayoutStore::open(&path);
        assert!(store.saved.is_none() && store.error.is_none());
        store.save(&sample()).unwrap();
        let store = LayoutStore::open(&path);
        assert_eq!(store.saved, Some(sample()));

        std::fs::write(&path, "{ not json").unwrap();
        let store = LayoutStore::open(&path);
        assert!(store.saved.is_none());
        assert!(store.error.is_some());
    }
}

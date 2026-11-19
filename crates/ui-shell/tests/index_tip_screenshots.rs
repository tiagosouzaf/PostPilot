//! Capturas de tela da dica de um índice no Object Explorer: o ponteiro
//! parado no índice mostra o nome inteiro e os detalhes. Só com
//! `--features screenshots`; os PNGs vão para `PGSTUDIO_SHOTS` (ou a pasta
//! temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::metadata::{
    Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, Property,
};
use pgstudio_object_explorer::ObjectExplorer;

const TABLE: &str = "controle_migracao_pasta_110_secretaria";
const INDEX: &str = "ux_cm_110_chave_versao (unique)";
const PARTIAL: &str = "ix_cm_110_pendentes_por_data_de_criacao";

/// Um servidor de mentira com uma tabela e os índices dela.
struct Tree;

fn node(id: &str, name: &str, kind: NodeKind, children: bool) -> ObjectNode {
    ObjectNode {
        id: NodeId(id.into()),
        name: name.into(),
        kind,
        has_children: children,
    }
}

impl MetadataProvider for Tree {
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(vec![node("srv", "JUCESP", NodeKind::Server, true)])
    }

    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError> {
        let folder = |id: &str, f: Folder| node(id, f.label(), NodeKind::Folder(f), true);
        Ok(match parent.0.as_str() {
            "srv" => vec![node("s", "public", NodeKind::Schema, true)],
            "s" => vec![folder("t", Folder::Tables)],
            "t" => vec![
                node("t/a", "alteracoes", NodeKind::Table, true),
                node("t/b", "avisos_de_infraestrutura", NodeKind::Table, true),
                node("t/c", TABLE, NodeKind::Table, true),
                node(
                    "t/d",
                    "controle_migracao_pasta_164_registro",
                    NodeKind::Table,
                    true,
                ),
            ],
            "t/c" => vec![
                folder("t/c/col", Folder::Columns),
                folder("t/c/con", Folder::Constraints),
                folder("t/c/idx", Folder::Indexes),
                folder("t/c/trg", Folder::Triggers),
            ],
            "t/c/idx" => vec![
                node("t/c/idx/1", INDEX, NodeKind::Index, false),
                node("t/c/idx/2", PARTIAL, NodeKind::Index, false),
            ],
            _ => Vec::new(),
        })
    }

    fn properties(&self, node: &NodeId) -> Result<Vec<Property>, MetadataError> {
        let p = Property::new;
        Ok(match node.0.as_str() {
            "t/c/idx/1" => vec![
                p("Table", TABLE),
                p("Columns", "chave, versao DESC"),
                p("Method", "btree"),
                p("Primary key", "no"),
                p("Unique", "yes"),
                p("Size", "2208 kB"),
                p("Scans", "18342"),
                p("Definition", "CREATE UNIQUE INDEX ..."),
            ],
            "t/c/idx/2" => vec![
                p("Table", TABLE),
                p("Columns", "criado_em DESC, secretaria_id"),
                p("Include", "chave, situacao"),
                p(
                    "Condition",
                    "situacao = 'pendente'::text AND excluido_em IS NULL AND tentativas < 5",
                ),
                p("Method", "btree"),
                p("Primary key", "no"),
                p("Unique", "no"),
                p("Size", "664 kB"),
                p("Scans", "0"),
                p("Definition", "CREATE INDEX ..."),
            ],
            _ => Vec::new(),
        })
    }
}

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    dir.join(name)
}

/// Abre a árvore até os índices, para o ponteiro em `index` e salva.
fn shot(index: &str, size: [f32; 2], theme: pgstudio_core::Theme, name: &str) {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut oe = ObjectExplorer::default();
    oe.connect(Arc::new(Tree));
    let wait = Duration::from_secs(5);
    assert!(oe.wait_idle(wait));
    for id in ["srv", "s", "t", "t/c", "t/c/idx"] {
        oe.expand(&NodeId(id.into()));
        assert!(oe.wait_idle(wait));
    }
    let mut h = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(2.0)
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, oe: &mut ObjectExplorer| {
                egui::Frame::central_panel(ui.style()).show(ui, |ui| {
                    oe.show(ui);
                });
            },
            oe,
        );
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, theme);
    h.run();
    let row = h.get_by_label_contains(index).rect();
    let screen = h.ctx.content_rect();
    let x = (row.left() + 60.0).min(screen.right() - 8.0);
    h.hover_at(egui::pos2(x, row.center().y));
    h.run_steps(90);
    // A dica inteira fica dentro da janela.
    for label in ["Colunas", "Método", "Tamanho", "Leituras"] {
        let rect = h.get_by_label(label).rect();
        assert!(screen.contains_rect(rect), "{name}: {label} fora da janela");
    }
    assert!(h.query_by_label("Definição").is_none(), "{name}");
    h.render()
        .expect("render")
        .save(out(name))
        .expect("save png");
}

#[test]
fn index_tip() {
    use pgstudio_core::Theme;
    shot(INDEX, [420.0, 360.0], Theme::Dark, "1-indice-exclusivo.png");
    shot(PARTIAL, [420.0, 400.0], Theme::Dark, "2-indice-parcial.png");
    shot(
        PARTIAL,
        [240.0, 420.0],
        Theme::Dark,
        "3-janela-estreita.png",
    );
    shot(INDEX, [420.0, 360.0], Theme::Light, "4-tema-claro.png");
}

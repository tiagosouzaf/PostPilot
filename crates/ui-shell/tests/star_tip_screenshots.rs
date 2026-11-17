//! Capturas de tela das colunas do `*`: o ponteiro parado no asterisco de
//! um `SELECT`, com o editor sozinho. Só com `--features screenshots`; os
//! PNGs vão para `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;

use eframe::egui::{self, Id, Pos2, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_sql_editor::{Colors, EditorState, SqlEditor};

struct Doc {
    catalog: Catalog,
    text: String,
    state: EditorState,
    rect: Rect,
}

const LOTES: &[(&str, &str)] = &[
    ("id", "bigint"),
    ("numero", "character varying(30)"),
    ("fazenda_id", "bigint"),
    ("fornecedor_id", "bigint"),
    ("raca_id", "integer"),
    ("categoria_id", "integer"),
    ("data_de_entrada", "date"),
    ("data_prevista_de_saida", "date"),
    ("data_de_fechamento", "date"),
    ("quantidade_de_animais_na_entrada", "integer"),
    ("quantidade_de_animais_atual", "integer"),
    ("quantidade_de_mortes", "integer"),
    ("peso_medio_na_entrada_kg", "numeric(10,2)"),
    ("peso_medio_atual_kg", "numeric(10,2)"),
    ("peso_medio_previsto_na_saida_kg", "numeric(10,2)"),
    ("ganho_medio_diario_kg", "numeric(8,3)"),
    ("consumo_de_materia_seca_por_cabeca_kg", "numeric(8,3)"),
    ("conversao_alimentar", "numeric(8,3)"),
    ("custo_da_arroba_produzida", "numeric(12,2)"),
    ("custo_total_de_alimentacao", "numeric(14,2)"),
    ("custo_total_de_sanidade", "numeric(14,2)"),
    ("custo_operacional_por_cabeca_dia", "numeric(10,4)"),
    ("valor_de_compra_por_arroba", "numeric(12,2)"),
    ("valor_de_venda_previsto_por_arroba", "numeric(12,2)"),
    ("rendimento_de_carcaca_previsto", "numeric(5,2)"),
    ("curral_id", "integer"),
    ("dieta_atual_id", "integer"),
    ("protocolo_sanitario_id", "integer"),
    ("situacao", "character varying(20)"),
    ("rastreado_sisbov", "boolean"),
    ("habilitado_para_exportacao", "boolean"),
    ("observacoes", "text"),
    ("dados_extras", "jsonb"),
    ("criado_por", "uuid"),
    ("criado_em", "timestamp with time zone"),
    ("atualizado_por", "uuid"),
    ("atualizado_em", "timestamp with time zone"),
    ("excluido_em", "timestamp with time zone"),
    (
        "registro_do_controle_de_qualidade_concluido_pela_equipe_em",
        "timestamp with time zone",
    ),
];

fn catalog() -> Catalog {
    let rel = |name: &str, cols: &[(&str, &str)]| Relation {
        name: name.into(),
        kind: RelationKind::Table,
        columns: cols
            .iter()
            .map(|(name, ty)| Column {
                name: (*name).into(),
                ty: (*ty).into(),
            })
            .collect(),
    };
    Catalog {
        engine: Default::default(),
        database: "confinamento".into(),
        search_path: vec!["public".into()],
        schemas: vec![Schema {
            name: "public".into(),
            relations: vec![
                rel("lotes", LOTES),
                rel(
                    "fazendas",
                    &[
                        ("id", "bigint"),
                        ("nome", "text"),
                        ("municipio", "text"),
                        ("uf", "character(2)"),
                    ],
                ),
            ],
            functions: Vec::new(),
        }],
    }
}

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    dir.join(name)
}

/// Abre `text`, para o ponteiro no caractere `col` da primeira linha e
/// salva.
fn shot(text: &str, col: usize, size: [f32; 2], pick: &[&str], name: &str) {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let doc = Doc {
        catalog: catalog(),
        text: text.to_string(),
        state: EditorState::default(),
        rect: Rect::NOTHING,
    };
    let mut h = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(2.0)
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, doc: &mut Doc| {
                egui::Frame::central_panel(ui.style()).show(ui, |ui| {
                    let out = SqlEditor::new(&mut doc.text, &mut doc.state, Id::new("editor"))
                        .catalog(Some(&doc.catalog))
                        .colors(Colors::DARK)
                        .show(ui);
                    doc.rect = out.response.rect;
                });
            },
            doc,
        );
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    let font = egui::TextStyle::Monospace.resolve(&h.ctx.global_style());
    let (char_w, line_h) = h
        .ctx
        .fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let rect = h.state().rect;
    let pos = Pos2::new(
        rect.left() + char_w * 3.0 + 20.0 + (col as f32 + 0.5) * char_w,
        rect.top() + line_h * 0.5,
    );
    h.hover_at(pos);
    h.run_steps(40);
    // O ponteiro vai para dentro da lista e escolhe as colunas.
    for label in pick {
        let pos = h.get_by_label(label).rect().center();
        h.hover_at(pos);
        h.run_steps(3);
        h.get_by_label(label).click();
        h.run_steps(3);
    }
    h.run_steps(60);
    let (_, tip) = h
        .state()
        .state
        .star_tip()
        .unwrap_or_else(|| panic!("{name}: sem a lista"));
    assert!(
        h.ctx.content_rect().contains_rect(tip),
        "{name}: fora da janela"
    );
    h.render()
        .expect("render")
        .save(out(name))
        .expect("save png");
}

#[test]
fn star_tip() {
    shot(
        "select * from lotes l where l.",
        7,
        [1000.0, 560.0],
        &["numero", "data_de_entrada", "quantidade_de_animais_atual"],
        "1-colunas-escolhidas.png",
    );
    shot(
        "select f.*, l.numero from lotes l join fazendas f on f.id = l.fazenda_id",
        9,
        [760.0, 300.0],
        &[],
        "2-alias-asterisco.png",
    );
    shot(
        "select * from lotes l join fazendas f on f.id = l.fazenda_id",
        7,
        [1100.0, 560.0],
        &["fazendas"],
        "3-join.png",
    );
    shot(
        "select * from lotes l where l.",
        7,
        [520.0, 300.0],
        &["fazenda_id"],
        "4-janela-pequena.png",
    );
}

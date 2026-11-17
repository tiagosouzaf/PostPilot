//! Capturas de tela da explicação das sugestões do IntelliSense, com o
//! editor sozinho em cada dialeto. Só com `--features screenshots`; os
//! PNGs vão para `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;

use eframe::egui::{self, Event, Id, Key, Modifiers};
use egui_kittest::Harness;
use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_sql_editor::lexer::Dialect;
use pgstudio_sql_editor::{Colors, EditorState, SqlEditor};

struct Doc {
    catalog: Catalog,
    text: String,
    state: EditorState,
}

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
        database: "loja".into(),
        search_path: vec!["public".into()],
        schemas: vec![Schema {
            name: "public".into(),
            relations: vec![
                rel(
                    "clientes",
                    &[("id", "integer"), ("nome", "text"), ("email", "text")],
                ),
                rel(
                    "lotes",
                    &[
                        ("status_lote", "integer"),
                        ("tipo_documental_metadado", "text"),
                        (
                            "tipo_documento_identificador_classificacao_cliente",
                            "character varying(200)",
                        ),
                        ("titulo_metadado", "text"),
                        ("usuario_id_registro_confirmacao_download", "bigint"),
                        ("versao_software_captura_metadado", "character varying(200)"),
                        (
                            "observacao_do_responsavel_pela_conferencia_do_lote_antes_do_envio_ao_cliente_final",
                            "timestamp without time zone",
                        ),
                    ],
                ),
                rel(
                    "movimentacao_estoque",
                    &[
                        ("id", "bigint"),
                        ("data_hora_atualizacao", "timestamp with time zone"),
                        ("data_hora_criacao", "timestamp with time zone"),
                        ("quantidade_movimentada", "numeric(12,3)"),
                        ("status", "character varying(20)"),
                        ("status_conferencia", "character varying(20)"),
                        ("usuario_responsavel_id", "bigint"),
                        ("valor_total", "numeric(14,2)"),
                    ],
                ),
                rel(
                    "pedidos",
                    &[
                        ("id", "integer"),
                        ("cliente_id", "integer"),
                        ("total", "numeric"),
                        ("criado_em", "timestamptz"),
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

/// Digita `text` num editor do dialeto, desce `down` sugestões e salva.
fn shot(dialect: Dialect, text: &str, down: usize, name: &str) {
    shot_at([760.0, 380.0], dialect, text, down, name);
}

fn shot_at(size: [f32; 2], dialect: Dialect, text: &str, down: usize, name: &str) {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut state = EditorState::default();
    state.set_dialect(dialect);
    state.focus_editor();
    let mut catalog = catalog();
    if dialect == Dialect::TSql {
        catalog.engine = pgstudio_core::connection::Engine::SqlServer;
    }
    let doc = Doc {
        catalog,
        text: String::new(),
        state,
    };
    let mut h = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(2.0)
        .build_ui_state(
            |ui, doc: &mut Doc| {
                egui::Frame::central_panel(ui.style()).show(ui, |ui| {
                    SqlEditor::new(&mut doc.text, &mut doc.state, Id::new("editor"))
                        .catalog(Some(&doc.catalog))
                        .colors(Colors::DARK)
                        .show(ui);
                });
            },
            doc,
        );
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    for line in text.split('\n').collect::<Vec<_>>().split_last().unwrap().1 {
        h.state_mut().text.push_str(line);
        h.state_mut().text.push('\n');
    }
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::End);
    h.run();
    for c in text.rsplit('\n').next().unwrap().chars() {
        h.event(Event::Text(c.to_string()));
        h.run();
    }
    for _ in 0..down {
        h.key_press(Key::ArrowDown);
        h.run();
    }
    assert!(h.state().state.is_completing(), "{name}: lista fechada");
    h.run_steps(3);
    h.render()
        .expect("render")
        .save(out(name))
        .expect("save png");
}

#[test]
fn explanations() {
    shot(
        Dialect::Redis,
        "SET sessao:42 \"ana\" EX 3600\nEXP",
        0,
        "redis.png",
    );
    shot(
        Dialect::MongoDb,
        "db.pedidos.aggregate([{ $",
        4,
        "mongodb.png",
    );
    shot(
        Dialect::Elasticsearch,
        "GET /pedidos/_search\n{\n  \"query\": {\n    \"ma",
        0,
        "elasticsearch.png",
    );
    shot(
        Dialect::Postgres,
        "select c.nome, sum(p.total)\nfrom pedidos p\njoin clientes c on c.id = p.cliente_id\nhav",
        0,
        "postgresql.png",
    );
    shot(Dialect::Postgres, "select * from ped", 0, "tabela.png");
    shot(
        Dialect::TSql,
        "select top 10 * from pedidos with (nol",
        0,
        "sqlserver.png",
    );
}

/// Nomes compridos: a lista alarga e, sem espaço, encurta com reticências.
#[test]
fn long_names() {
    let sql = "select * from lotes l where l.";
    shot_at(
        [900.0, 380.0],
        Dialect::Postgres,
        sql,
        5,
        "nomes-longos.png",
    );
    shot_at(
        [900.0, 380.0],
        Dialect::Postgres,
        sql,
        0,
        "nomes-longos-cortado.png",
    );
    shot_at(
        [460.0, 380.0],
        Dialect::Postgres,
        sql,
        5,
        "nomes-longos-estreito.png",
    );
}

/// A busca aproximada: abreviação, palavra do meio e erro de digitação
/// acham o nome, com as letras que casaram destacadas.
#[test]
fn fuzzy() {
    let from = "select *\nfrom movimentacao_estoque m\nwhere m.";
    for (typed, name) in [
        ("dt_cria", "1-abreviacao.png"),
        ("hora", "2-palavra-do-meio.png"),
        ("statsu", "3-erro-de-digitacao.png"),
        ("quantidde", "4-letra-faltando.png"),
    ] {
        shot_at(
            [760.0, 300.0],
            Dialect::Postgres,
            &format!("{from}{typed}"),
            0,
            name,
        );
    }
    shot_at(
        [760.0, 300.0],
        Dialect::Postgres,
        "select * from movimentcao",
        0,
        "5-tabela.png",
    );
    shot_at(
        [420.0, 300.0],
        Dialect::Postgres,
        &format!("{from}datahora"),
        0,
        "6-janela-pequena.png",
    );
}

//! JSON Query Guide: como consultar `json` e `jsonb` no PostgreSQL, com
//! exemplos prontos para inserir na janela de consulta.
//!
//! Todos os exemplos usam a mesma tabela temporária (`orders_demo`), criada
//! pelo primeiro tópico, então cada um roda como está e mostra o resultado
//! descrito. Um teste roda todos num PostgreSQL de verdade.

use eframe::egui::{self, FontId, RichText};
use egui_phosphor::regular as icon;
use pgstudio_i18n::tr;

/// Um exemplo do guia.
#[derive(Clone, Copy, Debug)]
pub struct Example {
    /// O que o exemplo faz.
    pub title: &'static str,
    pub sql: &'static str,
    /// O que ele devolve com a tabela de exemplo.
    pub result: &'static str,
    /// Versão mínima do PostgreSQL, se for maior que 13.
    pub since: Option<u32>,
}

/// Um tópico do guia.
#[derive(Clone, Copy, Debug)]
pub struct Topic {
    pub title: &'static str,
    pub intro: &'static str,
    pub examples: &'static [Example],
}

const fn ex(title: &'static str, sql: &'static str, result: &'static str) -> Example {
    Example {
        title,
        sql,
        result,
        since: None,
    }
}

/// Script da tabela de exemplo.
pub const SETUP: &str = r#"CREATE TEMP TABLE orders_demo (
    id   int PRIMARY KEY,
    data jsonb NOT NULL
);
INSERT INTO orders_demo VALUES
(1, '{"customer": {"name": "Ana", "city": "São Paulo"}, "status": "paid",
      "total": 150.00, "tags": ["vip", "promo"],
      "items": [{"sku": "A-1", "qty": 2, "price": 50}, {"sku": "B-7", "qty": 1, "price": 50}]}'),
(2, '{"customer": {"name": "Bruno", "city": "Recife"}, "status": "pending",
      "total": 35.90, "tags": [],
      "items": [{"sku": "C-3", "qty": 1, "price": 35.90}]}'),
(3, '{"customer": {"name": "Carla", "city": "São Paulo", "phone": null}, "status": "paid",
      "total": 420, "tags": ["vip"],
      "items": [{"sku": "A-1", "qty": 4, "price": 50}, {"sku": "D-9", "qty": 2, "price": 110}]}');"#;

/// Os tópicos, na ordem em que aparecem.
pub const TOPICS: &[Topic] = &[
    Topic {
        title: "Start here",
        intro: "Every example in this guide uses the same small table, orders_demo, with one jsonb \
                column called data. Insert the script below in a query window and run it once (F5): \
                it creates a temporary table that lives while the window stays connected. Then \
                insert any example after it and run only the example (select it and press F5).\n\n\
                Tip: double-click a JSON cell in the results (or click its {…} button) to open the \
                JSON Viewer. Click any node in its tree and it writes the query for that node.",
        examples: &[ex(
            "Create the sample table",
            SETUP,
            "CREATE TABLE, then INSERT 0 3.",
        )],
    },
    Topic {
        title: "Read a value",
        intro: "-> returns JSON (to keep walking into it), ->> returns text (to show or compare). \
                Use a key name for objects and a position (from 0) for arrays. #> and #>> take the \
                whole path at once, as a text array.",
        examples: &[
            ex(
                "Customer name as text",
                "SELECT id, data -> 'customer' ->> 'name' AS customer\nFROM orders_demo;",
                "1 Ana, 2 Bruno, 3 Carla",
            ),
            ex(
                "First item of each order (still JSON)",
                "SELECT id, data -> 'items' -> 0 AS first_item\nFROM orders_demo;",
                "{\"qty\": 2, \"sku\": \"A-1\", \"price\": 50} ...",
            ),
            ex(
                "The same path in one go",
                "SELECT id, data #>> '{items,0,sku}' AS first_sku\nFROM orders_demo;",
                "A-1, C-3, A-1",
            ),
            ex(
                "Numbers come back as text: cast them",
                "SELECT id, (data ->> 'total')::numeric * 1.1 AS total_with_tax\nFROM orders_demo;",
                "165.000, 39.490, 462.0",
            ),
        ],
    },
    Topic {
        title: "Filter rows",
        intro: "@> (contains) is the fastest filter: it uses a GIN index and checks types for you. \
                ? asks if a key (or array string) exists. For ranges and comparisons, extract the \
                value with ->> and cast it.",
        examples: &[
            ex(
                "Orders that are paid",
                "SELECT id\nFROM orders_demo\nWHERE data @> '{\"status\": \"paid\"}';",
                "1, 3",
            ),
            ex(
                "Nested: customers from São Paulo",
                "SELECT id\nFROM orders_demo\nWHERE data @> '{\"customer\": {\"city\": \"São Paulo\"}}';",
                "1, 3",
            ),
            ex(
                "Any item with sku A-1 (arrays match any element)",
                "SELECT id\nFROM orders_demo\nWHERE data @> '{\"items\": [{\"sku\": \"A-1\"}]}';",
                "1, 3",
            ),
            ex(
                "Tagged vip",
                "SELECT id\nFROM orders_demo\nWHERE data -> 'tags' ? 'vip';",
                "1, 3",
            ),
            ex(
                "Has a phone key (even if it is null)",
                "SELECT id\nFROM orders_demo\nWHERE data -> 'customer' ? 'phone';",
                "3",
            ),
            ex(
                "Total above 100",
                "SELECT id, data ->> 'total' AS total\nFROM orders_demo\nWHERE (data ->> 'total')::numeric > 100;",
                "1 150.00, 3 420",
            ),
            ex(
                "Any of these tags (?|) or all of them (?&)",
                "SELECT id,\n       data -> 'tags' ?| array['promo', 'new'] AS any_tag,\n       data -> 'tags' ?& array['vip', 'promo'] AS all_tags\nFROM orders_demo;",
                "1 true true, 2 false false, 3 false false",
            ),
        ],
    },
    Topic {
        title: "Arrays to rows",
        intro: "jsonb_array_elements turns an array into one row per element. Put it in a CROSS \
                JOIN LATERAL to keep the columns of the row it came from. Use LEFT JOIN LATERAL ... \
                ON true to keep rows whose array is empty.",
        examples: &[
            ex(
                "One row per item",
                "SELECT o.id, i ->> 'sku' AS sku, (i ->> 'qty')::int AS qty\nFROM orders_demo AS o\nCROSS JOIN LATERAL jsonb_array_elements(o.data -> 'items') AS i;",
                "5 rows: 1 A-1 2, 1 B-7 1, 2 C-3 1, 3 A-1 4, 3 D-9 2",
            ),
            ex(
                "Quantity sold per sku",
                "SELECT i ->> 'sku' AS sku, sum((i ->> 'qty')::int) AS qty\nFROM orders_demo AS o\nCROSS JOIN LATERAL jsonb_array_elements(o.data -> 'items') AS i\nGROUP BY 1\nORDER BY 2 DESC;",
                "A-1 6, D-9 2, B-7 1, C-3 1",
            ),
            ex(
                "Tags as text, keeping orders without tags",
                "SELECT o.id, t.tag\nFROM orders_demo AS o\nLEFT JOIN LATERAL jsonb_array_elements_text(o.data -> 'tags') AS t(tag) ON true;",
                "1 vip, 1 promo, 2 NULL, 3 vip",
            ),
            ex(
                "Element position with WITH ORDINALITY",
                "SELECT o.id, e.n, e.item ->> 'sku' AS sku\nFROM orders_demo AS o\nCROSS JOIN LATERAL jsonb_array_elements(o.data -> 'items') WITH ORDINALITY AS e(item, n);",
                "n counts from 1 inside each order",
            ),
            ex(
                "How many items",
                "SELECT id, jsonb_array_length(data -> 'items') AS items\nFROM orders_demo;",
                "1 2, 2 1, 3 2",
            ),
        ],
    },
    Topic {
        title: "Objects to rows and columns",
        intro: "jsonb_each gives one row per key, jsonb_object_keys only the keys. \
                jsonb_to_record and jsonb_to_recordset turn JSON into typed columns, which is the \
                easiest way to read many fields at once.",
        examples: &[
            ex(
                "Every key of the customer",
                "SELECT o.id, kv.key, kv.value\nFROM orders_demo AS o\nCROSS JOIN LATERAL jsonb_each(o.data -> 'customer') AS kv;",
                "name, city (and phone for order 3)",
            ),
            ex(
                "Which keys exist",
                "SELECT DISTINCT jsonb_object_keys(data) AS key\nFROM orders_demo\nORDER BY 1;",
                "customer, items, status, tags, total",
            ),
            ex(
                "Typed columns from an object",
                "SELECT o.id, c.*\nFROM orders_demo AS o\nCROSS JOIN LATERAL jsonb_to_record(o.data -> 'customer') AS c(name text, city text);",
                "1 Ana São Paulo, 2 Bruno Recife, 3 Carla São Paulo",
            ),
            ex(
                "Typed columns from an array of objects",
                "SELECT o.id, i.*\nFROM orders_demo AS o\nCROSS JOIN LATERAL jsonb_to_recordset(o.data -> 'items') AS i(sku text, qty int, price numeric);",
                "5 rows with sku, qty and price as real columns",
            ),
        ],
    },
    Topic {
        title: "SQL/JSON path",
        intro: "A JSON path ($.items[*].sku) walks the document in one string, and ? (...) filters \
                inside it, with @ as the current element. @? tests if the path finds anything, @@ \
                tests a condition; both use a GIN index. Strings in a path use double quotes.",
        examples: &[
            ex(
                "Every sku of every order",
                "SELECT id, jsonb_path_query(data, '$.items[*].sku') AS sku\nFROM orders_demo;",
                "one row per sku",
            ),
            ex(
                "Items that cost more than 60",
                "SELECT id, jsonb_path_query(data, '$.items[*] ? (@.price > 60)') AS item\nFROM orders_demo;",
                "3 {\"qty\": 2, \"sku\": \"D-9\", \"price\": 110}",
            ),
            ex(
                "Orders with an item of qty 4 or more",
                "SELECT id\nFROM orders_demo\nWHERE data @? '$.items[*] ? (@.qty >= 4)';",
                "3",
            ),
            ex(
                "The same with @@ (a condition)",
                "SELECT id\nFROM orders_demo\nWHERE data @@ '$.items[*].qty >= 4';",
                "3",
            ),
            ex(
                "All skus of an order as one array",
                "SELECT id, jsonb_path_query_array(data, '$.items[*].sku') AS skus\nFROM orders_demo;",
                "1 [\"A-1\", \"B-7\"], 2 [\"C-3\"], 3 [\"A-1\", \"D-9\"]",
            ),
            ex(
                "Variables instead of literals",
                "SELECT id\nFROM orders_demo\nWHERE jsonb_path_exists(data, '$.items[*] ? (@.sku == $sku)', '{\"sku\": \"B-7\"}');",
                "1",
            ),
            Example {
                title: "PostgreSQL 17+: JSON_VALUE and JSON_TABLE (standard SQL)",
                sql: "SELECT o.id, JSON_VALUE(o.data, '$.customer.name') AS customer, jt.*\nFROM orders_demo AS o,\n     JSON_TABLE(o.data, '$.items[*]' COLUMNS (sku text PATH '$.sku', qty int PATH '$.qty')) AS jt;",
                result: "one row per item, with the customer name",
                since: Some(17),
            },
        ],
    },
    Topic {
        title: "Build and change JSON",
        intro: "jsonb_build_object and jsonb_agg build JSON from rows. To change a document, \
                jsonb_set replaces a value at a path, || merges objects, - removes a key and #- \
                removes a path. They return the new value: use them in UPDATE ... SET data = ....",
        examples: &[
            ex(
                "Rows as JSON",
                "SELECT jsonb_agg(jsonb_build_object('id', id, 'customer', data #>> '{customer,name}')) AS orders\nFROM orders_demo;",
                "[{\"id\": 1, \"customer\": \"Ana\"}, ...]",
            ),
            ex(
                "Change one value",
                "UPDATE orders_demo\nSET data = jsonb_set(data, '{status}', '\"shipped\"')\nWHERE id = 2\nRETURNING data ->> 'status';",
                "shipped",
            ),
            ex(
                "Add or overwrite keys",
                "UPDATE orders_demo\nSET data = data || '{\"priority\": true, \"status\": \"paid\"}'\nWHERE id = 2\nRETURNING data;",
                "order 2 now has priority and is paid",
            ),
            ex(
                "Remove a key and a nested path",
                "SELECT data - 'tags' #- '{customer,phone}' AS cleaned\nFROM orders_demo\nWHERE id = 3;",
                "order 3 without tags and without customer.phone",
            ),
            ex(
                "Append to an array",
                "UPDATE orders_demo\nSET data = jsonb_set(data, '{tags}', (data -> 'tags') || '\"new\"')\nWHERE id = 1\nRETURNING data -> 'tags';",
                "[\"vip\", \"promo\", \"new\"]",
            ),
        ],
    },
    Topic {
        title: "Indexes",
        intro: "A GIN index on the column speeds up @>, ?, ?|, ?&, @? and @@ on that column. \
                jsonb_path_ops is smaller and faster but only supports @>, @? and @@. For ->> \
                comparisons, ORDER BY or ranges, index the expression itself with a normal \
                (btree) index. The query has to use the same expression as the index.",
        examples: &[
            ex(
                "GIN for containment and path filters",
                "CREATE INDEX orders_demo_data_gin ON orders_demo USING gin (data jsonb_path_ops);",
                "used by WHERE data @> '...' and WHERE data @? '...'",
            ),
            ex(
                "GIN with the default operator class (also ?, ?|, ?&)",
                "CREATE INDEX orders_demo_data_keys ON orders_demo USING gin (data);",
                "used by WHERE data ? 'status' too",
            ),
            ex(
                "Btree on one field, for = and ranges",
                "CREATE INDEX orders_demo_total ON orders_demo (((data ->> 'total')::numeric));",
                "used by WHERE (data ->> 'total')::numeric > 100",
            ),
            ex(
                "Check that the index is used",
                "SET enable_seqscan = off;\nEXPLAIN SELECT id FROM orders_demo WHERE data @> '{\"status\": \"paid\"}';\nRESET enable_seqscan;",
                "Bitmap Index Scan on orders_demo_data_gin (on a tiny table the planner prefers a Seq Scan, hence the SET)",
            ),
        ],
    },
    Topic {
        title: "json or jsonb, and pitfalls",
        intro: "Prefer jsonb: it is stored parsed, has indexes and the operators @>, ?, @? and @@. \
                json keeps the text exactly (spaces, key order, duplicate keys) and only has ->, ->>, \
                #>, #>> and json_* functions; cast it with ::jsonb to use the rest.\n\n\
                Pitfalls:\n\
                • ->> returns text: '10' > '9' is false. Cast before comparing numbers or dates.\n\
                • A JSON null is not SQL NULL: data -> 'customer' -> 'phone' is the JSON null, \
                but ->> turns it into SQL NULL.\n\
                • A missing key returns NULL, not an error: a typo looks like empty data.\n\
                • = compares whole jsonb documents; json has no = at all.\n\
                • jsonb sorts keys (shorter first) and keeps only the last of duplicate keys.",
        examples: &[
            ex(
                "JSON null versus SQL NULL",
                "SELECT id,\n       data -> 'customer' -> 'phone' AS as_json,\n       data -> 'customer' ->> 'phone' IS NULL AS text_is_null,\n       jsonb_typeof(data -> 'customer' -> 'phone') AS json_type\nFROM orders_demo;",
                "order 3: null, true, null; others: NULL, true, NULL",
            ),
            ex(
                "Text comparison trap",
                "SELECT '10' > '9' AS as_text, 10 > 9 AS as_number;",
                "false, true",
            ),
            ex(
                "json to jsonb",
                "SELECT '{\"b\": 1, \"a\": 2, \"a\": 3}'::json AS kept_as_text,\n       '{\"b\": 1, \"a\": 2, \"a\": 3}'::jsonb AS normalized;",
                "{\"b\": 1, \"a\": 2, \"a\": 3} versus {\"a\": 3, \"b\": 1}",
            ),
        ],
    },
];

/// A janela do guia.
#[derive(Debug, Default)]
pub struct JsonGuide {
    pub open: bool,
    pub topic: usize,
}

impl JsonGuide {
    /// Abre (ou traz para a frente) num tópico.
    pub fn open_at(&mut self, topic: usize) {
        self.open = true;
        self.topic = topic.min(TOPICS.len() - 1);
    }

    /// Desenha a janela. Devolve o SQL que o usuário pediu para inserir.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        colors: &pgstudio_sql_editor::Colors,
        can_insert: bool,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut insert = None;
        let mut open = true;
        egui::Window::new(tr("JSON Query Guide"))
            .id(egui::Id::new("json_guide"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .frame(crate::theme::dialog_frame(&ctx.global_style()))
            .default_size([900.0, 660.0])
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .min_size([520.0, 320.0])
            .show(ctx, |ui| {
                egui::Panel::left(egui::Id::new("json_guide_topics"))
                    .resizable(false)
                    .exact_size(190.0)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            ui.spacing_mut().button_padding = egui::vec2(8.0, 5.0);
                            for (i, t) in TOPICS.iter().enumerate() {
                                if ui.selectable_label(self.topic == i, tr(t.title)).clicked() {
                                    self.topic = i;
                                }
                            }
                        });
                    });
                let topic = &TOPICS[self.topic.min(TOPICS.len() - 1)];
                egui::ScrollArea::vertical()
                    .id_salt(("json_guide_body", self.topic))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        ui.heading(tr(topic.title));
                        ui.add_space(4.0);
                        ui.label(tr(topic.intro));
                        ui.add_space(10.0);
                        for (i, e) in topic.examples.iter().enumerate() {
                            ui.push_id(i, |ui| {
                                if let Some(sql) = example(ui, e, colors, can_insert) {
                                    insert = Some(sql);
                                }
                            });
                            ui.add_space(10.0);
                        }
                    });
            });
        self.open = open;
        insert
    }
}

/// Um exemplo: título, SQL colorido, Copy/Insert e o resultado esperado.
fn example(
    ui: &mut egui::Ui,
    e: &Example,
    colors: &pgstudio_sql_editor::Colors,
    can_insert: bool,
) -> Option<String> {
    let mut insert = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr(e.title)).strong());
        if let Some(v) = e.since {
            // i18n-ignore
            ui.label(RichText::new(format!("PostgreSQL {v}+")).small().weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add_enabled(
                can_insert,
                egui::Button::new(format!("{} {}", icon::ARROW_SQUARE_IN, tr("Insert"))).small(),
            );
            if r.on_hover_text(tr("Put this SQL in the query window."))
                .on_disabled_hover_text(tr("Open a query window first."))
                .clicked()
            {
                insert = Some(e.sql.to_string());
            }
            if ui
                .small_button(format!("{} {}", icon::COPY, tr("Copy")))
                .clicked()
            {
                ui.ctx().copy_text(e.sql.to_string());
            }
        });
    });
    let job = pgstudio_sql_editor::highlight::layout_job(e.sql, colors, FontId::monospace(12.5));
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(2)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(job).selectable(true));
        });
    ui.label(RichText::new(format!("{} {}", icon::ARROW_RIGHT, tr(e.result))).weak());
    insert
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_and_titles_have_portuguese_translations() {
        use pgstudio_i18n::{Language, has_translation};
        for t in TOPICS {
            let mut texts = vec![t.title, t.intro];
            texts.extend(t.examples.iter().map(|e| e.title));
            for text in texts {
                assert!(
                    has_translation(Language::PortugueseBrazil, None, text),
                    "sem tradução em locales/pt-BR.po: {text:?}"
                );
            }
        }
    }

    #[test]
    fn every_topic_has_examples_with_sql() {
        for t in TOPICS {
            assert!(!t.examples.is_empty(), "{}", t.title);
            for e in t.examples {
                assert!(
                    !e.sql.trim().is_empty() && !e.result.is_empty(),
                    "{}",
                    e.title
                );
            }
        }
        assert_eq!(TOPICS[0].examples[0].sql, SETUP);
    }
}

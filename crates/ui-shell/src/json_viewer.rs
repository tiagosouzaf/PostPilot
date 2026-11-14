//! JSON Viewer: o valor de uma célula `json`/`jsonb` formatado, numa
//! árvore ou em texto colorido, e o SQL pronto para consultar o nó que o
//! usuário escolhe na árvore.
//!
//! Escrever a consulta é a parte difícil de usar JSON no PostgreSQL, então
//! o painel "Query this value" monta as expressões do caminho escolhido
//! (`->`/`->>`, `#>>`, `@>`, `@?`, SQL/JSON path e as funções que viram
//! linhas), cada uma com Copy e Insert.
//!
//! No Edit Top Rows o viewer também edita: o texto é validado a cada
//! tecla e o OK devolve o JSON para a célula.

use eframe::egui::{
    self, Color32, FontId, RichText, TextFormat, collapsing_header::CollapsingState,
    text::LayoutJob,
};
use egui_phosphor::regular as icon;
use pgstudio_core::catalog::quote_ident;
use pgstudio_result_grid::json::{self, Node, ParseError, Step};

/// Tabela das consultas montadas (o resultado não diz de que tabela veio).
pub const TABLE_PLACEHOLDER: &str = "table_name";

/// Texto acima deste tamanho aparece sem cores, para não pesar o frame.
const HIGHLIGHT_LIMIT: usize = 512 * 1024;

/// Filhos mostrados por nó na árvore; o resto vira uma linha "… N more".
const TREE_CHILDREN_LIMIT: usize = 1000;

/// Tipo da coluna: muda as funções (`jsonb_*` ou `json_*`) e os
/// operadores que só existem no `jsonb`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JsonKind {
    #[default]
    Jsonb,
    Json,
}

impl JsonKind {
    /// Pelo nome do tipo (`format_type`), se for JSON.
    pub fn from_type_name(name: &str) -> Option<JsonKind> {
        match name.trim().to_ascii_lowercase().as_str() {
            "jsonb" => Some(JsonKind::Jsonb),
            "json" => Some(JsonKind::Json),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            JsonKind::Jsonb => "jsonb",
            JsonKind::Json => "json",
        }
    }

    /// Prefixo das funções de array e objeto.
    fn prefix(self) -> &'static str {
        match self {
            JsonKind::Jsonb => "jsonb",
            JsonKind::Json => "json",
        }
    }
}

/// Uma expressão do painel "Query this value".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expression {
    pub label: &'static str,
    pub sql: String,
}

/// Literal de SQL (`'...'`).
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `col -> 'a' -> 0 ->> 'b'`: `text` troca a última seta por `->>`.
pub fn arrows(base: &str, path: &[Step], text: bool) -> String {
    let mut out = base.to_string();
    for (i, step) in path.iter().enumerate() {
        let last = i + 1 == path.len();
        out.push_str(if last && text { " ->> " } else { " -> " });
        match step {
            Step::Key(k) => out.push_str(&literal(k)),
            Step::Index(n) => out.push_str(&n.to_string()),
        }
    }
    out
}

/// O caminho como array de texto do PostgreSQL (`'{a,0,b}'`), para `#>`
/// e `#>>`.
pub fn text_array(path: &[Step]) -> String {
    let items: Vec<String> = path
        .iter()
        .map(|s| match s {
            Step::Index(n) => n.to_string(),
            Step::Key(k) => {
                let plain = !k.is_empty()
                    && !k.eq_ignore_ascii_case("null")
                    && !k
                        .chars()
                        .any(|c| matches!(c, '{' | '}' | ',' | '"' | '\\') || c.is_whitespace());
                if plain {
                    k.clone()
                } else {
                    format!("\"{}\"", k.replace('\\', "\\\\").replace('"', "\\\""))
                }
            }
        })
        .collect();
    literal(&format!("{{{}}}", items.join(",")))
}

/// O JSON que o `@>` procura: o valor dentro dos objetos e arrays do
/// caminho. Um array no caminho vira `[...]`, que o `@>` casa com
/// qualquer elemento.
pub fn containment(path: &[Step], value: &Node) -> Node {
    path.iter().rev().fold(value.clone(), |v, step| match step {
        Step::Key(k) => Node::Object(vec![(k.clone(), v)]),
        Step::Index(_) => Node::Array(vec![v]),
    })
}

/// O caminho do SQL/JSON path com `[*]` no lugar das posições: todos os
/// elementos dos arrays.
fn wildcard_path(path: &[Step]) -> String {
    json::json_path(path)
        .split('[')
        .enumerate()
        .map(|(i, part)| {
            if i == 0 {
                part.to_string()
            } else {
                // `0].b` vira `*].b`.
                let rest = part.split_once(']').map_or("", |(_, r)| r);
                format!("*]{rest}")
            }
        })
        .collect::<Vec<_>>()
        .join("[")
}

/// As expressões para consultar `node`, que está em `path` dentro da
/// coluna `column`.
pub fn expressions(column: &str, path: &[Step], node: &Node, kind: JsonKind) -> Vec<Expression> {
    let col = quote_ident(column);
    // Operadores e funções de SQL/JSON path só existem no jsonb.
    let as_jsonb = match kind {
        JsonKind::Jsonb => col.clone(),
        JsonKind::Json => format!("{col}::jsonb"),
    };
    let f = kind.prefix();
    let mut out = Vec::new();
    let mut push = |label, sql: String| out.push(Expression { label, sql });
    let jpath = json::json_path(path);
    if !path.is_empty() {
        if node.is_container() {
            push("Get this value", arrows(&col, path, false));
            push("Same, by path", format!("{col} #> {}", text_array(path)));
        } else {
            push("Get this value as text", arrows(&col, path, true));
            push("Same, by path", format!("{col} #>> {}", text_array(path)));
        }
        let filter = containment(path, node).compact();
        let any = path.iter().any(|s| matches!(s, Step::Index(_)));
        push(
            if any {
                "Rows that contain this value (any array element; GIN index)"
            } else {
                "Rows that contain this value (uses a GIN index)"
            },
            format!(
                "SELECT *\nFROM {TABLE_PLACEHOLDER}\nWHERE {as_jsonb} @> {}",
                literal(&filter)
            ),
        );
        let exact = arrows(&col, path, true);
        let compare = match node {
            Node::String(s) => Some(format!("{exact} = {}", literal(s))),
            Node::Number(n) => Some(format!("({exact})::numeric = {n}")),
            Node::Bool(b) => Some(format!("({exact})::boolean = {b}")),
            _ => None,
        };
        if let Some(compare) = compare {
            push(
                "Rows where this exact position has this value",
                format!("SELECT *\nFROM {TABLE_PLACEHOLDER}\nWHERE {compare}"),
            );
        }
        push(
            "Rows where this path exists",
            format!(
                "SELECT *\nFROM {TABLE_PLACEHOLDER}\nWHERE {as_jsonb} @? {}",
                literal(&jpath)
            ),
        );
        push(
            "SQL/JSON path",
            format!("jsonb_path_query_first({as_jsonb}, {})", literal(&jpath)),
        );
        if any {
            push(
                "SQL/JSON path, every array element",
                format!(
                    "jsonb_path_query({as_jsonb}, {})",
                    literal(&wildcard_path(path))
                ),
            );
        }
    }
    let target = arrows(&format!("t.{col}"), path, false);
    match node {
        Node::Array(_) => {
            push(
                "One row per element",
                format!(
                    "SELECT e.value\nFROM {TABLE_PLACEHOLDER} AS t\nCROSS JOIN LATERAL {f}_array_elements({target}) AS e(value)"
                ),
            );
            push("Number of elements", format!("{f}_array_length({target})"));
        }
        Node::Object(_) => {
            push(
                "One row per key",
                format!(
                    "SELECT kv.key, kv.value\nFROM {TABLE_PLACEHOLDER} AS t\nCROSS JOIN LATERAL {f}_each({target}) AS kv"
                ),
            );
            push("Keys", format!("{f}_object_keys({target})"));
        }
        _ => {}
    }
    if path.is_empty() {
        push("Formatted text", format!("jsonb_pretty({as_jsonb})"));
    }
    out
}

/// Cores do JSON.
#[derive(Clone, Copy, Debug)]
struct JsonColors {
    key: Color32,
    string: Color32,
    number: Color32,
    literal: Color32,
    punct: Color32,
}

impl JsonColors {
    fn of(ui: &egui::Ui) -> Self {
        if ui.visuals().dark_mode {
            JsonColors {
                key: Color32::from_rgb(0x9C, 0xDC, 0xFE),
                string: Color32::from_rgb(0xCE, 0x91, 0x78),
                number: Color32::from_rgb(0xB5, 0xCE, 0xA8),
                literal: Color32::from_rgb(0x56, 0x9C, 0xD6),
                punct: Color32::from_rgb(0xB4, 0xB4, 0xB4),
            }
        } else {
            JsonColors {
                key: Color32::from_rgb(0x04, 0x51, 0xA5),
                string: Color32::from_rgb(0xA3, 0x15, 0x15),
                number: Color32::from_rgb(0x09, 0x86, 0x58),
                literal: Color32::from_rgb(0x00, 0x00, 0xFF),
                punct: Color32::from_rgb(0x50, 0x50, 0x50),
            }
        }
    }

    fn of_node(&self, node: &Node) -> Color32 {
        match node {
            Node::String(_) => self.string,
            Node::Number(_) => self.number,
            Node::Null | Node::Bool(_) => self.literal,
            _ => self.punct,
        }
    }
}

/// Texto JSON colorido (também com o texto inválido de uma edição).
fn highlight(text: &str, c: &JsonColors, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let fmt = |color| TextFormat::simple(font.clone(), color);
    if text.len() > HIGHLIGHT_LIMIT {
        job.append(text, 0.0, fmt(c.punct));
        return job;
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let color = match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(bytes.len());
                let after = text[i..].trim_start();
                if after.starts_with(':') {
                    c.key
                } else {
                    c.string
                }
            }
            b'-' | b'0'..=b'9' => {
                while i < bytes.len()
                    && matches!(bytes[i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    i += 1;
                }
                c.number
            }
            b if b.is_ascii_alphabetic() => {
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                c.literal
            }
            _ => {
                // Pontuação, espaços e o resto, até o próximo token.
                i += 1;
                while i < bytes.len()
                    && !matches!(bytes[i], b'"' | b'-' | b'0'..=b'9')
                    && !bytes[i].is_ascii_alphabetic()
                {
                    i += 1;
                }
                c.punct
            }
        };
        // O fim do token pode cair no meio de um caractere de vários bytes
        // (string sem fechar): anda até a fronteira.
        let mut end = i.min(bytes.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        i = end;
        job.append(&text[start..end], 0.0, fmt(color));
    }
    job
}

/// O que o usuário pediu no viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewerAction {
    /// Pôr este SQL na janela de consulta.
    Insert(String),
    /// Abrir o JSON Query Guide.
    Guide,
    /// OK na edição: o novo valor da célula.
    Save(String),
    /// Fechou (ou Cancel).
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Tree,
    Text,
}

/// Uma janela do JSON Viewer.
pub struct JsonViewer {
    id: egui::Id,
    title: String,
    column: String,
    root: Node,
    pretty: String,
    tab: Tab,
    selected: Vec<Step>,
    pub kind: JsonKind,
    /// Mostra o botão Insert (há uma janela de consulta para receber).
    can_insert: bool,
    /// Texto da edição; `None` é só leitura.
    edit: Option<String>,
    error: Option<ParseError>,
    /// Muda a cada Expand all/Collapse all, para a árvore esquecer o
    /// estado aberto de cada nó.
    generation: u64,
    expand_all: Option<bool>,
}

impl JsonViewer {
    /// Viewer só de leitura de `value` (já reconhecido como JSON).
    pub fn new(id: egui::Id, column: &str, row: u64, value: Node) -> Self {
        let pretty = value.pretty();
        JsonViewer {
            id,
            title: format!("JSON Viewer - {column} (row {})", row + 1),
            column: column.to_string(),
            root: value,
            pretty,
            tab: Tab::Tree,
            selected: Vec::new(),
            kind: JsonKind::default(),
            can_insert: true,
            edit: None,
            error: None,
            generation: 0,
            expand_all: None,
        }
    }

    /// Viewer que edita o texto `text` (que pode estar vazio ou inválido).
    pub fn editor(id: egui::Id, column: &str, row: usize, text: &str, kind: JsonKind) -> Self {
        let parsed = json::parse(text);
        let root = parsed.clone().unwrap_or(Node::Null);
        let edit = match &parsed {
            Ok(n) => n.pretty(),
            Err(_) => text.to_string(),
        };
        let mut v = Self::new(id, column, row as u64, root);
        v.title = format!("Edit JSON - {column} (row {})", row + 1);
        v.kind = kind;
        v.can_insert = false;
        v.tab = Tab::Text;
        v.error = if text.trim().is_empty() {
            None
        } else {
            parsed.err()
        };
        v.edit = Some(edit);
        v
    }

    /// Esconde o Insert (não há janela de consulta).
    pub fn set_can_insert(&mut self, can: bool) {
        self.can_insert = can;
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Caminho do nó escolhido na árvore.
    pub fn selected(&self) -> &[Step] {
        &self.selected
    }

    pub fn select(&mut self, path: Vec<Step>) {
        if self.root.get(&path).is_some() {
            self.selected = path;
            self.tab = Tab::Tree;
        }
    }

    /// Mostra a aba Text (ou Tree, com `false`).
    pub fn show_text(&mut self, text: bool) {
        self.tab = if text { Tab::Text } else { Tab::Tree };
    }

    /// As expressões do nó escolhido.
    pub fn expressions(&self) -> Vec<Expression> {
        let node = self.root.get(&self.selected).unwrap_or(&self.root);
        expressions(&self.column, &self.selected, node, self.kind)
    }

    /// Desenha a janela. Devolve o que o usuário pediu neste frame.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        sql_colors: &pgstudio_sql_editor::Colors,
    ) -> Option<ViewerAction> {
        let mut open = true;
        let mut action = None;
        egui::Window::new(self.title.clone())
            .id(self.id)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([760.0, 600.0])
            .min_size([420.0, 300.0])
            .show(ctx, |ui| action = self.ui(ui, sql_colors));
        if !open {
            action = Some(ViewerAction::Close);
        }
        action
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        sql_colors: &pgstudio_sql_editor::Colors,
    ) -> Option<ViewerAction> {
        let mut action = None;
        let colors = JsonColors::of(ui);
        let font = FontId::monospace(13.0);
        self.toolbar(ui, &mut action);
        ui.separator();
        if self.edit.is_some() {
            egui::Panel::bottom(self.id.with("edit_footer"))
                .resizable(false)
                .show(ui, |ui| self.edit_footer(ui, &mut action));
        }
        if self.tab == Tab::Tree && self.edit.as_ref().is_none_or(|_| self.error.is_none()) {
            egui::Panel::bottom(self.id.with("query"))
                .resizable(true)
                .default_size(230.0)
                .min_size(120.0)
                .show(ui, |ui| self.query_panel(ui, sql_colors, &mut action));
        }
        match self.tab {
            Tab::Tree => self.tree(ui, &colors, &font),
            Tab::Text => self.text(ui, &colors, &font),
        }
        action
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, action: &mut Option<ViewerAction>) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Tree, format!("{} Tree", icon::TREE_VIEW));
            ui.selectable_value(&mut self.tab, Tab::Text, format!("{} Text", icon::BRACKETS_CURLY));
            ui.separator();
            if ui
                .button(format!("{} Copy", icon::COPY))
                .on_hover_text("Copy the formatted JSON.")
                .clicked()
            {
                ui.ctx().copy_text(self.current_pretty());
            }
            if ui
                .button("Copy compact")
                .on_hover_text("Copy the JSON in one line.")
                .clicked()
            {
                ui.ctx().copy_text(self.root.compact());
            }
            if self.tab == Tab::Tree {
                ui.separator();
                if ui.button("Expand all").clicked() {
                    self.generation += 1;
                    self.expand_all = Some(true);
                }
                if ui.button("Collapse all").clicked() {
                    self.generation += 1;
                    self.expand_all = Some(false);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(format!("{} JSON Query Guide", icon::QUESTION))
                    .on_hover_text("How to query json and jsonb, with examples.")
                    .clicked()
                {
                    *action = Some(ViewerAction::Guide);
                }
                egui::ComboBox::from_id_salt(self.id.with("kind"))
                    .selected_text(self.kind.label())
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.kind, JsonKind::Jsonb, "jsonb");
                        ui.selectable_value(&mut self.kind, JsonKind::Json, "json");
                    })
                    .response
                    .on_hover_text(
                        "Column type. json has no @> or SQL/JSON path, so the queries cast it to jsonb.",
                    );
                ui.label("Column type:");
            });
        });
    }

    fn current_pretty(&self) -> String {
        match (&self.edit, &self.error) {
            (Some(text), Some(_)) => text.clone(),
            _ => self.pretty.clone(),
        }
    }

    fn text(&mut self, ui: &mut egui::Ui, colors: &JsonColors, font: &FontId) {
        let colors = *colors;
        let mut changed = false;
        egui::ScrollArea::both()
            .id_salt(self.id.with("text"))
            .auto_shrink(false)
            .show(ui, |ui| match &mut self.edit {
                Some(text) => {
                    let font = font.clone();
                    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                        let mut job = highlight(buf.as_str(), &colors, &font);
                        job.wrap.max_width = wrap;
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    let r = ui.add(
                        egui::TextEdit::multiline(text)
                            .id(self.id.with("edit"))
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(20)
                            .layouter(&mut layouter),
                    );
                    changed = r.changed();
                }
                None => {
                    let job = highlight(&self.pretty, &colors, font);
                    ui.add(egui::Label::new(job).selectable(true).extend());
                }
            });
        if changed {
            self.reparse();
        }
    }

    /// Lê de novo o texto da edição (vazio vale: é NULL).
    fn reparse(&mut self) {
        let Some(text) = &self.edit else {
            return;
        };
        if text.trim().is_empty() {
            self.error = None;
            self.root = Node::Null;
            self.pretty.clear();
            self.selected.clear();
            return;
        }
        match json::parse(text) {
            Ok(n) => {
                self.pretty = n.pretty();
                self.root = n;
                self.error = None;
                if self.root.get(&self.selected).is_none() {
                    self.selected.clear();
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Troca o texto da edição, como se o usuário tivesse digitado.
    pub fn set_edit_text(&mut self, text: &str) {
        if let Some(edit) = &mut self.edit {
            *edit = text.to_string();
            self.reparse();
        }
    }

    /// Erro do JSON em edição.
    pub fn error(&self) -> Option<&ParseError> {
        self.error.as_ref()
    }

    /// O que o OK grava: o JSON compacto, `""` para NULL, ou `None` se
    /// não está editando ou o JSON tem erro.
    pub fn ok_value(&self) -> Option<String> {
        let text = self.edit.as_ref()?;
        if self.error.is_some() {
            return None;
        }
        Some(if text.trim().is_empty() {
            String::new()
        } else {
            self.root.compact()
        })
    }

    fn edit_footer(&mut self, ui: &mut egui::Ui, action: &mut Option<ViewerAction>) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            match &self.error {
                Some(e) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("{} {e}", icon::WARNING_CIRCLE),
                    );
                }
                None => {
                    ui.label(RichText::new(format!("{} Valid JSON", icon::CHECK_CIRCLE)).weak());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    *action = Some(ViewerAction::Close);
                }
                let valid = self.error.is_none();
                if ui
                    .add_enabled(valid, egui::Button::new("OK"))
                    .on_disabled_hover_text("Fix the JSON first.")
                    .clicked()
                    && let Some(value) = self.ok_value()
                {
                    // Vazio é NULL; o resto vai compacto, como o jsonb guarda.
                    *action = Some(ViewerAction::Save(value));
                }
                if ui.add_enabled(valid, egui::Button::new("Format")).clicked()
                    && let Some(text) = &mut self.edit
                {
                    *text = self.pretty.clone();
                }
            });
        });
        ui.add_space(2.0);
    }

    fn tree(&mut self, ui: &mut egui::Ui, colors: &JsonColors, font: &FontId) {
        if self.edit.is_some() && self.error.is_some() {
            ui.label("The JSON has errors. Fix it in the Text tab to see the tree.");
            return;
        }
        let mut clicked = None;
        let root = std::mem::replace(&mut self.root, Node::Null);
        egui::ScrollArea::both()
            .id_salt(self.id.with("tree"))
            .auto_shrink(false)
            .show(ui, |ui| {
                let mut path = Vec::new();
                self.node(ui, &root, None, &mut path, colors, font, &mut clicked);
            });
        self.root = root;
        self.expand_all = None;
        if let Some(path) = clicked {
            self.selected = path;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        &self,
        ui: &mut egui::Ui,
        node: &Node,
        key: Option<&Step>,
        path: &mut Vec<Step>,
        colors: &JsonColors,
        font: &FontId,
        clicked: &mut Option<Vec<Step>>,
    ) {
        let mut job = LayoutJob::default();
        let fmt = |color| TextFormat::simple(font.clone(), color);
        match key {
            Some(Step::Key(k)) => {
                let mut quoted = String::new();
                json::write_string(&mut quoted, k);
                job.append(&quoted, 0.0, fmt(colors.key));
                job.append(": ", 0.0, fmt(colors.punct));
            }
            Some(Step::Index(i)) => job.append(&format!("[{i}]: "), 0.0, fmt(colors.punct)),
            None => job.append("$ ", 0.0, fmt(colors.punct)),
        }
        let mut preview = node.preview();
        if preview.chars().count() > 120 {
            preview = preview.chars().take(119).collect::<String>() + "…";
        }
        job.append(&preview, 0.0, fmt(colors.of_node(node)));
        let selected = self.selected == *path;
        let row = |ui: &mut egui::Ui, clicked: &mut Option<Vec<Step>>, path: &Vec<Step>| {
            let r = ui.selectable_label(selected, job.clone());
            if r.clicked() {
                *clicked = Some(path.clone());
            }
        };
        let children: Vec<(Step, &Node)> = match node {
            Node::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, n)| (Step::Index(i), n))
                .collect(),
            Node::Object(pairs) => pairs
                .iter()
                .map(|(k, n)| (Step::Key(k.clone()), n))
                .collect(),
            _ => {
                ui.horizontal(|ui| {
                    // Alinha com o texto dos nós que têm a seta.
                    ui.add_space(ui.spacing().indent);
                    row(ui, clicked, path);
                });
                return;
            }
        };
        let id = self.id.with(("node", self.generation, &*path));
        let default_open = self.expand_all.unwrap_or(path.len() < 2);
        CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
            .show_header(ui, |ui| row(ui, clicked, path))
            .body(|ui| {
                for (step, child) in children.iter().take(TREE_CHILDREN_LIMIT) {
                    path.push(step.clone());
                    self.node(ui, child, Some(step), path, colors, font, clicked);
                    path.pop();
                }
                if children.len() > TREE_CHILDREN_LIMIT {
                    ui.label(
                        RichText::new(format!(
                            "… {} more (see the Text tab)",
                            children.len() - TREE_CHILDREN_LIMIT
                        ))
                        .weak(),
                    );
                }
            });
    }

    fn query_panel(
        &mut self,
        ui: &mut egui::Ui,
        sql_colors: &pgstudio_sql_editor::Colors,
        action: &mut Option<ViewerAction>,
    ) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Query this value").strong());
            let node = self.root.get(&self.selected).unwrap_or(&self.root);
            ui.label(
                RichText::new(format!(
                    "{}  ({})",
                    json::json_path(&self.selected),
                    node.type_name()
                ))
                .monospace(),
            );
        });
        ui.label(
            RichText::new(format!(
                "Click a node in the tree to build its query. Replace {TABLE_PLACEHOLDER} with your table."
            ))
            .weak()
            .small(),
        );
        ui.add_space(4.0);
        let font = FontId::monospace(12.0);
        egui::ScrollArea::vertical()
            .id_salt(self.id.with("queries"))
            .auto_shrink(false)
            .show(ui, |ui| {
                for (i, e) in self.expressions().into_iter().enumerate() {
                    ui.push_id(i, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(e.label).small().weak());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if self.can_insert
                                        && ui
                                            .small_button("Insert")
                                            .on_hover_text("Put this SQL in the query window.")
                                            .clicked()
                                    {
                                        *action = Some(ViewerAction::Insert(e.sql.clone()));
                                    }
                                    if ui.small_button(format!("{} Copy", icon::COPY)).clicked() {
                                        ui.ctx().copy_text(e.sql.clone());
                                    }
                                },
                            );
                        });
                        let job = pgstudio_sql_editor::highlight::layout_job(
                            &e.sql,
                            sql_colors,
                            font.clone(),
                        );
                        egui::Frame::new()
                            .fill(ui.visuals().extreme_bg_color)
                            .inner_margin(egui::Margin::symmetric(6, 4))
                            .corner_radius(2)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(egui::Label::new(job).selectable(true));
                            });
                        ui.add_space(4.0);
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Step {
        Step::Key(s.into())
    }

    #[test]
    fn arrows_and_text_arrays() {
        let path = [k("address"), Step::Index(0), k("it's")];
        assert_eq!(
            arrows("data", &path, true),
            "data -> 'address' -> 0 ->> 'it''s'"
        );
        assert_eq!(
            arrows("data", &path, false),
            "data -> 'address' -> 0 -> 'it''s'"
        );
        assert_eq!(text_array(&path), "'{address,0,it''s}'");
        assert_eq!(
            text_array(&[k("a b"), k("x,\"y\""), k("null"), k("")]),
            r#"'{"a b","x,\"y\"","null",""}'"#
        );
    }

    #[test]
    fn containment_wraps_the_value_in_its_path() {
        let path = [k("items"), Step::Index(3), k("sku")];
        let node = containment(&path, &Node::String("A-1".into()));
        assert_eq!(node.compact(), r#"{"items":[{"sku":"A-1"}]}"#);
    }

    #[test]
    fn wildcard_path_replaces_every_index() {
        let path = [k("a"), Step::Index(2), k("b"), Step::Index(0)];
        assert_eq!(wildcard_path(&path), "$.a[*].b[*]");
    }

    #[test]
    fn expressions_for_a_scalar_in_jsonb() {
        let root = json::parse(r#"{"customer":{"name":"Ana"},"items":[{"qty":2}]}"#).unwrap();
        let path = vec![k("items"), Step::Index(0), k("qty")];
        let node = root.get(&path).unwrap();
        let sql: Vec<String> = expressions("Order Data", &path, node, JsonKind::Jsonb)
            .into_iter()
            .map(|e| e.sql)
            .collect();
        assert_eq!(sql[0], r#""Order Data" -> 'items' -> 0 ->> 'qty'"#);
        assert_eq!(sql[1], r#""Order Data" #>> '{items,0,qty}'"#);
        assert!(sql[2].ends_with(r#"WHERE "Order Data" @> '{"items":[{"qty":2}]}'"#));
        assert!(sql[3].ends_with(r#"WHERE ("Order Data" -> 'items' -> 0 ->> 'qty')::numeric = 2"#));
        assert!(sql[4].ends_with(r#"WHERE "Order Data" @? '$.items[0].qty'"#));
        assert_eq!(
            sql[5],
            r#"jsonb_path_query_first("Order Data", '$.items[0].qty')"#
        );
        assert_eq!(
            sql[6],
            r#"jsonb_path_query("Order Data", '$.items[*].qty')"#
        );
    }

    #[test]
    fn expressions_for_json_cast_to_jsonb_and_use_json_functions() {
        let root = json::parse(r#"{"tags":["a","b"]}"#).unwrap();
        let path = vec![k("tags")];
        let e = expressions("doc", &path, root.get(&path).unwrap(), JsonKind::Json);
        let find = |label: &str| {
            e.iter()
                .find(|e| e.label.starts_with(label))
                .unwrap()
                .sql
                .clone()
        };
        assert!(find("Rows that contain").ends_with(r#"WHERE doc::jsonb @> '{"tags":["a","b"]}'"#));
        assert_eq!(
            find("One row per element"),
            "SELECT e.value\nFROM table_name AS t\nCROSS JOIN LATERAL json_array_elements(t.doc -> 'tags') AS e(value)"
        );
        assert_eq!(
            find("Number of elements"),
            "json_array_length(t.doc -> 'tags')"
        );
    }

    #[test]
    fn root_object_offers_keys_and_pretty() {
        let root = json::parse(r#"{"a":1}"#).unwrap();
        let e = expressions("data", &[], &root, JsonKind::Jsonb);
        let labels: Vec<_> = e.iter().map(|e| e.label).collect();
        assert_eq!(labels, ["One row per key", "Keys", "Formatted text"]);
        assert_eq!(e[1].sql, "jsonb_object_keys(t.data)");
    }

    #[test]
    fn highlight_keeps_the_text() {
        let font = FontId::monospace(12.0);
        let c = JsonColors {
            key: Color32::RED,
            string: Color32::GREEN,
            number: Color32::BLUE,
            literal: Color32::YELLOW,
            punct: Color32::GRAY,
        };
        for text in [
            r#"{"a": [1, -2.5e3, true, null, "é\"x"]}"#,
            "{\"unterminated é",
            "",
        ] {
            assert_eq!(highlight(text, &c, &font).text, text);
        }
        let job = highlight(r#"{"k": "v"}"#, &c, &font);
        let colors: Vec<_> = job.sections.iter().map(|s| s.format.color).collect();
        assert_eq!(
            colors,
            [
                Color32::GRAY,
                Color32::RED,
                Color32::GRAY,
                Color32::GREEN,
                Color32::GRAY
            ]
        );
    }
}

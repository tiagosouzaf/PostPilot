//! Editores por tipo de coluna, como no DataGrip.
//!
//! Uma coluna de data ou hora abre, embaixo da célula, um calendário com
//! os campos da hora (e do fuso, quando o tipo guarda fuso); o valor
//! continua podendo ser digitado na célula, e os dois andam juntos. Uma
//! coluna booleana (ou de lista fixa, como um `enum`) abre a lista dos
//! valores, com NULL quando a coluna aceita.
//!
//! O valor é sempre o texto que o servidor mostra: a fração de segundo e
//! o fuso são guardados como vieram, então mexer no dia não perde os
//! microssegundos nem troca `+00` por outra escrita.

use chrono::{Datelike, Local, NaiveDate, Timelike};
use eframe::egui::{self, Align2, Key, Rect, RichText, Sense, vec2};
use egui_phosphor::regular as icon;
use pgstudio_i18n::{tr, tr_noop};

/// Como uma coluna é editada.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CellKind {
    /// Caixa de texto.
    #[default]
    Text,
    /// Lista fixa de valores (booleano, `enum`).
    Choice(Vec<Choice>),
    Date,
    Time {
        tz: bool,
    },
    DateTime {
        tz: bool,
    },
}

/// Um valor de uma lista fixa.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// O que a lista mostra.
    pub label: String,
    /// O texto gravado (o mesmo que o servidor mostra na grade).
    pub value: String,
}

/// O editor de uma coluna.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnKind {
    pub kind: CellKind,
    /// A coluna aceita NULL.
    pub nullable: bool,
    /// Casas da fração de segundo que o tipo guarda.
    pub digits: u8,
}

impl ColumnKind {
    /// O editor para o tipo `type_name` (como o `format_type` do
    /// PostgreSQL escreve, ou o nome do tipo no SQL Server, com a escala
    /// entre parênteses). `labels` são os valores de um `enum`.
    pub fn from_type(sql_server: bool, type_name: &str, nullable: bool, labels: &[String]) -> Self {
        let name = type_name.trim().to_lowercase();
        let base = name.split('(').next().unwrap_or("").trim();
        let scale = name
            .split_once('(')
            .and_then(|(_, rest)| rest.split(')').next())
            .and_then(|n| n.trim().parse::<u8>().ok());
        let boolean = |yes: &str, no: &str| {
            CellKind::Choice(vec![
                Choice {
                    label: "true".into(),
                    value: yes.into(),
                },
                Choice {
                    label: "false".into(),
                    value: no.into(),
                },
            ])
        };
        let (kind, digits) = if name.ends_with("[]") {
            (CellKind::Text, 0)
        } else if !labels.is_empty() {
            let choices = labels
                .iter()
                .map(|l| Choice {
                    label: l.clone(),
                    value: l.clone(),
                })
                .collect();
            (CellKind::Choice(choices), 0)
        } else if sql_server {
            match base {
                "bit" => (boolean("1", "0"), 0),
                "date" => (CellKind::Date, 0),
                "time" => (CellKind::Time { tz: false }, scale.unwrap_or(7)),
                "datetime" => (CellKind::DateTime { tz: false }, 3),
                "smalldatetime" => (CellKind::DateTime { tz: false }, 0),
                "datetime2" => (CellKind::DateTime { tz: false }, scale.unwrap_or(7)),
                "datetimeoffset" => (CellKind::DateTime { tz: true }, scale.unwrap_or(7)),
                _ => (CellKind::Text, 0),
            }
        } else {
            let tz = name.contains("with time zone") || base.ends_with("tz");
            match base {
                "boolean" | "bool" => (boolean("t", "f"), 0),
                "date" => (CellKind::Date, 0),
                _ if base.starts_with("timestamp") => {
                    (CellKind::DateTime { tz }, scale.unwrap_or(6))
                }
                // `interval` e `time zone` não começam com "time ".
                "time" | "timetz" => (CellKind::Time { tz }, scale.unwrap_or(6)),
                _ if base.starts_with("time ") => (CellKind::Time { tz }, scale.unwrap_or(6)),
                _ => (CellKind::Text, 0),
            }
        };
        ColumnKind {
            kind,
            nullable,
            digits: digits.min(9),
        }
    }

    /// Lista fixa (booleano, `enum`).
    pub fn is_choice(&self) -> bool {
        matches!(self.kind, CellKind::Choice(_))
    }

    /// Data, hora ou os dois.
    pub fn is_temporal(&self) -> bool {
        matches!(
            self.kind,
            CellKind::Date | CellKind::Time { .. } | CellKind::DateTime { .. }
        )
    }

    fn has_date(&self) -> bool {
        matches!(self.kind, CellKind::Date | CellKind::DateTime { .. })
    }

    fn has_time(&self) -> bool {
        matches!(self.kind, CellKind::Time { .. } | CellKind::DateTime { .. })
    }

    fn has_zone(&self) -> bool {
        matches!(
            self.kind,
            CellKind::Time { tz: true } | CellKind::DateTime { tz: true }
        )
    }

    /// O rótulo de `value` numa lista fixa (`t` aparece como `true`).
    pub fn label_of<'a>(&'a self, value: &'a str) -> &'a str {
        match &self.kind {
            CellKind::Choice(choices) => choices
                .iter()
                .find(|c| c.value == value)
                .map_or(value, |c| c.label.as_str()),
            _ => value,
        }
    }
}

/// Uma data, uma hora ou os dois, lidos do texto da célula.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Os dígitos da fração de segundo, como estavam.
    pub frac: String,
    /// O fuso, como estava (`+00`, `-03:00`, `Z`); vazio se não tinha.
    pub offset: String,
    /// Havia um espaço antes do fuso (o SQL Server escreve assim).
    pub gap: bool,
}

impl Stamp {
    /// Meia-noite do dia `date`.
    fn at(date: NaiveDate) -> Self {
        Stamp {
            year: date.year(),
            month: date.month(),
            day: date.day(),
            hour: 0,
            minute: 0,
            second: 0,
            frac: String::new(),
            offset: String::new(),
            gap: false,
        }
    }

    /// Lê `text` como um valor de `kind` no formato ISO que os servidores
    /// mostram. `None` se não for (`infinity`, datas antes de Cristo,
    /// outro `DateStyle`): aí o valor só é editado como texto.
    pub fn parse(kind: &CellKind, text: &str) -> Option<Stamp> {
        let s = text.trim();
        let mut stamp = Stamp::at(NaiveDate::from_ymd_opt(2000, 1, 1)?);
        match kind {
            CellKind::Date => {
                let rest = stamp.take_date(s)?;
                rest.is_empty().then_some(stamp)
            }
            CellKind::DateTime { .. } => {
                let rest = stamp.take_date(s)?;
                if rest.is_empty() {
                    return Some(stamp);
                }
                let rest = rest.strip_prefix([' ', 'T', 't'])?;
                stamp.take_time(rest.trim_start())?;
                Some(stamp)
            }
            CellKind::Time { .. } => {
                stamp.take_time(s)?;
                Some(stamp)
            }
            _ => None,
        }
    }

    /// `aaaa-mm-dd` no começo de `s`; devolve o resto.
    fn take_date<'a>(&mut self, s: &'a str) -> Option<&'a str> {
        let (year, rest) = digits(s, 4, 6)?;
        let rest = rest.strip_prefix('-')?;
        let (month, rest) = digits(rest, 1, 2)?;
        let rest = rest.strip_prefix('-')?;
        let (day, rest) = digits(rest, 1, 2)?;
        NaiveDate::from_ymd_opt(year as i32, month, day)?;
        self.year = year as i32;
        self.month = month;
        self.day = day;
        Some(rest)
    }

    /// `hh:mm[:ss[.fração]][fuso]` em `s` inteiro.
    fn take_time(&mut self, s: &str) -> Option<()> {
        let (hour, rest) = digits(s, 1, 2)?;
        let rest = rest.strip_prefix(':')?;
        let (minute, mut rest) = digits(rest, 1, 2)?;
        let mut second = 0;
        let mut frac = String::new();
        if let Some(r) = rest.strip_prefix(':') {
            let (sec, r) = digits(r, 1, 2)?;
            second = sec;
            rest = r;
            if let Some(r) = rest.strip_prefix('.') {
                let n = r.bytes().take_while(u8::is_ascii_digit).count();
                frac = r[..n].to_string();
                rest = &r[n..];
            }
        }
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        let offset = rest.trim();
        if !valid_offset(offset) {
            return None;
        }
        self.hour = hour;
        self.minute = minute;
        self.second = second;
        self.frac = frac;
        self.gap = !offset.is_empty() && rest.starts_with(' ');
        self.offset = offset.to_string();
        Some(())
    }

    /// O texto do valor, do jeito que foi lido.
    pub fn format(&self, kind: &CellKind) -> String {
        let date = format!("{:04}-{:02}-{:02}", self.year, self.month, self.day);
        let mut time = format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second);
        if !self.frac.is_empty() {
            time.push('.');
            time.push_str(&self.frac);
        }
        if !self.offset.is_empty() {
            if self.gap {
                time.push(' ');
            }
            time.push_str(&self.offset);
        }
        match kind {
            CellKind::Date => date,
            CellKind::Time { .. } => time,
            _ => format!("{date} {time}"),
        }
    }

    fn date(&self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, self.day)
    }
}

/// Um número de `min` a `max` dígitos no começo de `s`, e o resto.
fn digits(s: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    if n < min || n > max {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

/// Vazio, `Z` ou `±hh[:mm[:ss]]` (também `±hhmm`).
fn valid_offset(s: &str) -> bool {
    if s.is_empty() || s.eq_ignore_ascii_case("z") {
        return true;
    }
    let Some(rest) = s.strip_prefix(['+', '-']) else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 8
        && rest.starts_with(|c: char| c.is_ascii_digit())
        && rest.ends_with(|c: char| c.is_ascii_digit())
        && rest.chars().all(|c| c.is_ascii_digit() || c == ':')
}

/// O fuso local agora, como `+hh:mm`.
fn local_offset() -> String {
    let secs = Local::now().offset().local_minus_utc();
    let sign = if secs < 0 { '-' } else { '+' };
    let minutes = secs.abs() / 60;
    format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// O que o editor guarda entre frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picker {
    /// Mês à mostra no calendário (ano, mês).
    view: Option<(i32, u32)>,
    /// A data do valor no último frame: quando muda, o calendário vai
    /// para o mês dela.
    seen: Option<(i32, u32, u32)>,
    /// Item realçado na lista (o último é o NULL, quando há).
    pick: Option<usize>,
}

impl Picker {
    /// O mês à mostra no calendário, se já foi escolhido.
    pub fn view(&self) -> Option<(i32, u32)> {
        self.view
    }
}

/// O que o usuário decidiu no editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Gravar este valor (`None` é NULL).
    Commit(Option<String>),
    /// Gravar o texto que está na célula.
    Accept,
    Cancel,
}

/// O que o editor fez neste frame.
#[derive(Clone, Debug)]
pub struct PopupOut {
    pub action: Option<Action>,
    /// Onde o editor foi desenhado (um clique fora dele e da célula
    /// termina a edição).
    pub rect: Rect,
    /// Um botão do editor mudou o texto: o foco volta para a célula.
    pub refocus: bool,
}

/// O botão do mouse desceu fora de todos os `rects` neste frame.
pub fn pressed_outside(ctx: &egui::Context, rects: &[Rect]) -> bool {
    ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !rects.iter().any(|r| r.contains(p)))
    })
}

const MONTHS: [&str; 12] = [
    tr_noop("January"),
    tr_noop("February"),
    tr_noop("March"),
    tr_noop("April"),
    tr_noop("May"),
    tr_noop("June"),
    tr_noop("July"),
    tr_noop("August"),
    tr_noop("September"),
    tr_noop("October"),
    tr_noop("November"),
    tr_noop("December"),
];

/// Os dias da semana, a partir de domingo, em duas letras.
const WEEKDAYS: [&str; 7] = [
    tr_noop("Su"),
    tr_noop("Mo"),
    tr_noop("Tu"),
    tr_noop("We"),
    tr_noop("Th"),
    tr_noop("Fr"),
    tr_noop("Sa"),
];

const DAY_W: f32 = 32.0;
const DAY_H: f32 = 24.0;

/// Desenha `add` num quadro flutuante embaixo de `anchor` (em cima, se
/// embaixo não couber), sem sair da janela.
fn popup<R>(
    ctx: &egui::Context,
    id: egui::Id,
    anchor: Rect,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (Rect, R) {
    let screen = ctx.content_rect();
    // O tamanho do frame anterior diz se cabe embaixo.
    let size = ctx
        .memory(|m| m.area_rect(id))
        .map_or(vec2(220.0, 240.0), |r| r.size());
    let below = anchor.bottom() + 2.0;
    let y = if below + size.y > screen.bottom() && anchor.top() - size.y - 2.0 >= screen.top() {
        anchor.top() - size.y - 2.0
    } else {
        below
    };
    // A altura do conteúdo no frame anterior: só numa janela baixa demais
    // para ele o editor rola (a rolagem prenderia a altura do quadro).
    let content_id = id.with("content_height");
    let content: Option<f32> = ctx.data(|d| d.get_temp(content_id));
    let room = (screen.height() - 28.0).max(60.0);
    let out = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fade_in(false)
        .constrain_to(screen)
        .fixed_pos(egui::pos2(anchor.left(), y))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(6.0)
                .show(ui, |ui| {
                    let measured = |ui: &mut egui::Ui| {
                        let scope = ui.scope(add);
                        let height = scope.response.rect.height();
                        ui.ctx().data_mut(|d| d.insert_temp(content_id, height));
                        scope.inner
                    };
                    if content.is_some_and(|h| h > room) {
                        egui::ScrollArea::vertical()
                            .max_height(room)
                            .min_scrolled_height(room)
                            .show(ui, measured)
                            .inner
                    } else {
                        measured(ui)
                    }
                })
                .inner
        });
    (out.response.rect, out.inner)
}

/// A lista de valores de uma coluna de lista fixa, embaixo de `anchor`.
/// As setas mudam o item realçado, Enter grava, Esc desiste, uma letra
/// vai para o valor que começa com ela, e um clique grava na hora.
pub fn choice_popup(
    ctx: &egui::Context,
    id: egui::Id,
    anchor: Rect,
    column: &ColumnKind,
    current: Option<&str>,
    picker: &mut Picker,
) -> PopupOut {
    let CellKind::Choice(choices) = &column.kind else {
        return PopupOut {
            action: Some(Action::Cancel),
            rect: anchor,
            refocus: false,
        };
    };
    let count = choices.len() + usize::from(column.nullable);
    let null_at = column.nullable.then_some(choices.len());
    let mut pick = picker.pick.unwrap_or_else(|| match current {
        Some(v) => choices
            .iter()
            .position(|c| c.value == v || c.label.eq_ignore_ascii_case(v))
            .unwrap_or(0),
        None => null_at.unwrap_or(0),
    });
    let mut action = None;
    let value_at = |i: usize| choices.get(i).map(|c| c.value.clone());
    ctx.input_mut(|i| {
        let none = egui::Modifiers::NONE;
        if i.consume_key(none, Key::ArrowDown) {
            pick = (pick + 1).min(count.saturating_sub(1));
        }
        if i.consume_key(none, Key::ArrowUp) {
            pick = pick.saturating_sub(1);
        }
        if i.consume_key(none, Key::Home) {
            pick = 0;
        }
        if i.consume_key(none, Key::End) {
            pick = count.saturating_sub(1);
        }
        if i.consume_key(none, Key::Escape) {
            action = Some(Action::Cancel);
        } else if i.consume_key(none, Key::Enter) {
            action = Some(Action::Commit(value_at(pick)));
        }
        // Uma letra vai para o valor que começa com ela.
        let typed: String = i
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Text(t) => Some(t.to_lowercase()),
                _ => None,
            })
            .collect();
        if let Some(c) = typed.chars().next() {
            let starts = |label: &str| label.to_lowercase().starts_with(c);
            // A partir do item seguinte, para a mesma letra andar pela lista.
            let found = (1..=count).map(|k| (pick + k) % count).find(|&k| {
                choices
                    .get(k)
                    .map_or_else(|| starts("null"), |c| starts(&c.label))
            });
            if let Some(k) = found {
                pick = k;
            }
        }
    });
    let width = anchor.width().clamp(110.0, 320.0);
    let (rect, clicked) = popup(ctx, id, anchor, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
        ui.set_min_width(width);
        let mut clicked = None;
        for k in 0..count {
            let text = match choices.get(k) {
                Some(c) => RichText::new(&c.label),
                None => RichText::new(tr("NULL")).italics().weak(),
            };
            let resp = ui.add_sized(
                vec2(width, DAY_H),
                egui::Button::selectable(k == pick, text).truncate(),
            );
            if resp.clicked() {
                clicked = Some(k);
            }
        }
        clicked
    });
    if let Some(k) = clicked {
        pick = k;
        action = Some(Action::Commit(value_at(k)));
    }
    picker.pick = Some(pick);
    PopupOut {
        action,
        rect,
        refocus: false,
    }
}

/// O calendário e os campos da hora de uma coluna de data ou hora,
/// embaixo de `anchor`. `text` é o texto da célula em edição: o que se
/// digita nela aparece aqui, e o que se escolhe aqui é escrito nela.
pub fn temporal_popup(
    ctx: &egui::Context,
    id: egui::Id,
    anchor: Rect,
    column: &ColumnKind,
    text: &mut String,
    picker: &mut Picker,
) -> PopupOut {
    let kind = &column.kind;
    let parsed = Stamp::parse(kind, text);
    let now = Local::now();
    let today = now.date_naive();
    let date = parsed.as_ref().map(|s| (s.year, s.month, s.day));
    if date != picker.seen || picker.view.is_none() {
        picker.seen = date;
        picker.view = Some(date.map_or((today.year(), today.month()), |(y, m, _)| (y, m)));
    }
    let mut view = picker.view.unwrap_or((today.year(), today.month()));
    let before = parsed.clone().unwrap_or_else(|| Stamp::at(today));
    let mut stamp = before.clone();
    // Um botão escreveu o valor (mesmo que igual ao de antes).
    let mut write = false;
    let mut refocus = false;
    let mut action = None;
    let (rect, ()) = popup(ctx, id, anchor, |ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 3.0);
        ui.set_width(7.0 * DAY_W + 12.0);
        if column.has_date() {
            let selected = parsed.as_ref().and_then(Stamp::date);
            if let Some((picked, twice)) = calendar(ui, &mut view, selected, today) {
                stamp.year = picked.year();
                stamp.month = picked.month();
                stamp.day = picked.day();
                write = true;
                refocus = true;
                if twice {
                    action = Some(Action::Accept);
                }
            }
        }
        if column.has_time() {
            if column.has_date() {
                ui.separator();
            }
            time_fields(ui, column, &mut stamp);
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let label = if column.has_time() {
                tr("Now")
            } else {
                tr("Today")
            };
            if ui.button(label).clicked() {
                stamp = Stamp::at(today);
                if column.has_time() {
                    stamp.hour = now.hour();
                    stamp.minute = now.minute();
                    stamp.second = now.second();
                }
                // Com o fuso deste computador, para a hora ser a daqui.
                if column.has_zone() {
                    stamp.offset = local_offset();
                }
                view = (stamp.year, stamp.month);
                write = true;
                refocus = true;
            }
            if column.nullable && ui.button(tr("NULL")).clicked() {
                action = Some(Action::Commit(None));
            }
            if ui.button(tr("Cancel")).clicked() {
                action = Some(Action::Cancel);
            }
            if ui.button(tr("OK")).clicked() {
                action = Some(Action::Accept);
            }
        });
    });
    // Trocar de mês não tira o teclado da célula.
    refocus |= picker.view != Some(view);
    picker.view = Some(view);
    if write || stamp != before {
        *text = stamp.format(kind);
        picker.seen = column
            .has_date()
            .then_some((stamp.year, stamp.month, stamp.day));
    }
    PopupOut {
        action,
        rect,
        refocus,
    }
}

/// O mês `view` com as setas de mês e de ano. Devolve o dia clicado e se
/// foram dois cliques.
fn calendar(
    ui: &mut egui::Ui,
    view: &mut (i32, u32),
    selected: Option<NaiveDate>,
    today: NaiveDate,
) -> Option<(NaiveDate, bool)> {
    let shift = |view: &mut (i32, u32), months: i32| {
        let total = view.0 * 12 + view.1 as i32 - 1 + months;
        let year = total.div_euclid(12).clamp(1, 9999);
        *view = (year, total.rem_euclid(12) as u32 + 1);
    };
    ui.horizontal(|ui| {
        let small = |ui: &mut egui::Ui, glyph: &str, tip: &str| {
            ui.add(egui::Button::new(glyph).small().frame_when_inactive(false))
                .on_hover_text(tip)
                .clicked()
        };
        if small(ui, icon::CARET_DOUBLE_LEFT, tr("Previous year")) {
            shift(view, -12);
        }
        if small(ui, icon::CARET_LEFT, tr("Previous month")) {
            shift(view, -1);
        }
        // As setas da direita primeiro, para o nome do mês ficar com o
        // meio.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if small(ui, icon::CARET_DOUBLE_RIGHT, tr("Next year")) {
                shift(view, 12);
            }
            if small(ui, icon::CARET_RIGHT, tr("Next month")) {
                shift(view, 1);
            }
            // O nome do mês no meio do que sobrou, numa linha só.
            let month = tr(MONTHS[(view.1 as usize - 1).min(11)]);
            let title = format!("{month} {}", view.0);
            let rect = ui.available_rect_before_wrap();
            let resp = ui.allocate_rect(rect, Sense::hover());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &title));
            ui.painter().with_clip_rect(rect).text(
                rect.center(),
                Align2::CENTER_CENTER,
                &title,
                egui::TextStyle::Button.resolve(ui.style()),
                ui.visuals().strong_text_color(),
            );
        });
    });
    let first = NaiveDate::from_ymd_opt(view.0, view.1, 1)?;
    let lead = i64::from(first.weekday().num_days_from_sunday());
    let visuals = ui.visuals().clone();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut picked = None;
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    ui.horizontal(|ui| {
        for day in WEEKDAYS {
            let (rect, _) = ui.allocate_exact_size(vec2(DAY_W, DAY_H - 4.0), Sense::hover());
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                tr(day),
                font.clone(),
                visuals.weak_text_color(),
            );
        }
    });
    for week in 0..6 {
        ui.horizontal(|ui| {
            for weekday in 0..7 {
                let (rect, resp) = ui.allocate_exact_size(vec2(DAY_W, DAY_H), Sense::click());
                let offset = week * 7 + weekday - lead;
                let Some(date) = first.checked_add_signed(chrono::Duration::days(offset)) else {
                    continue;
                };
                let cell = rect.shrink(1.0);
                let chosen = selected == Some(date);
                if chosen {
                    ui.painter()
                        .rect_filled(cell, 4.0, visuals.selection.bg_fill);
                } else if resp.hovered() {
                    ui.painter()
                        .rect_filled(cell, 4.0, visuals.widgets.hovered.weak_bg_fill);
                }
                if date == today {
                    ui.painter().rect_stroke(
                        cell,
                        4.0,
                        egui::Stroke::new(1.0, visuals.selection.bg_fill),
                        egui::StrokeKind::Inside,
                    );
                }
                let color = if chosen {
                    visuals.selection.stroke.color
                } else if date.month() == view.1 {
                    visuals.text_color()
                } else {
                    visuals.weak_text_color()
                };
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    date.day().to_string(),
                    font.clone(),
                    color,
                );
                // O dia inteiro como nome, para leitores de tela (e
                // para os testes acharem o dia certo).
                let name = date.format("%Y-%m-%d").to_string();
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::Button, true, chosen, &name)
                });
                if resp.double_clicked() {
                    picked = Some((date, true));
                } else if resp.clicked() {
                    picked = Some((date, false));
                }
            }
        });
    }
    ui.add_space(2.0);
    picked
}

/// Hora, minuto, segundo, fração e fuso.
fn time_fields(ui: &mut egui::Ui, column: &ColumnKind, stamp: &mut Stamp) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 3.0);
        ui.label(icon::CLOCK);
        let two = |ui: &mut egui::Ui, value: &mut u32, max: u32, tip: &str| {
            ui.add(
                egui::DragValue::new(value)
                    .range(0..=max)
                    .speed(0.1)
                    .custom_formatter(|v, _| format!("{:02}", v as u32)),
            )
            .on_hover_text(tip);
        };
        two(ui, &mut stamp.hour, 23, tr("Hour"));
        ui.label(":");
        two(ui, &mut stamp.minute, 59, tr("Minute"));
        ui.label(":");
        two(ui, &mut stamp.second, 59, tr("Second"));
        if column.digits > 0 {
            ui.label(".");
            let digits = usize::from(column.digits);
            ui.add(
                egui::TextEdit::singleline(&mut stamp.frac)
                    .char_limit(digits)
                    .hint_text("0".repeat(digits))
                    .desired_width(8.0 * digits as f32 + 8.0),
            )
            .on_hover_text(tr("Fraction of a second"));
            stamp.frac.retain(|c| c.is_ascii_digit());
        }
    });
    if column.has_zone() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(tr("Time zone"));
            let mut offset = stamp.offset.clone();
            ui.add(
                egui::TextEdit::singleline(&mut offset)
                    .char_limit(9)
                    .hint_text("+00:00")
                    .desired_width(64.0),
            )
            .on_hover_text(tr(
                "Offset from UTC, like +00 or -03:00. Empty uses the time zone of the session.",
            ));
            offset.retain(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | ':' | 'Z' | 'z'));
            stamp.offset = offset;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg(name: &str) -> ColumnKind {
        ColumnKind::from_type(false, name, true, &[])
    }

    fn mssql(name: &str) -> ColumnKind {
        ColumnKind::from_type(true, name, false, &[])
    }

    #[test]
    fn postgres_types_pick_their_editor() {
        assert!(pg("boolean").is_choice());
        assert_eq!(pg("date").kind, CellKind::Date);
        assert_eq!(
            pg("timestamp with time zone").kind,
            CellKind::DateTime { tz: true }
        );
        assert_eq!(pg("timestamp with time zone").digits, 6);
        let t = pg("timestamp(3) without time zone");
        assert_eq!((t.kind, t.digits), (CellKind::DateTime { tz: false }, 3));
        assert_eq!(
            pg("time without time zone").kind,
            CellKind::Time { tz: false }
        );
        assert_eq!(
            pg("time(0) with time zone").kind,
            CellKind::Time { tz: true }
        );
        for other in [
            "interval",
            "text",
            "integer",
            "boolean[]",
            "date[]",
            "tstzrange",
        ] {
            assert_eq!(pg(other).kind, CellKind::Text, "{other}");
        }
        let mood = ColumnKind::from_type(false, "mood", true, &["sad".into(), "happy".into()]);
        assert!(mood.is_choice());
        assert_eq!(mood.label_of("happy"), "happy");
        assert_eq!(pg("boolean").label_of("t"), "true");
    }

    #[test]
    fn sql_server_types_pick_their_editor() {
        assert_eq!(mssql("bit").label_of("0"), "false");
        assert_eq!(mssql("date").kind, CellKind::Date);
        let t = mssql("datetime2(7)");
        assert_eq!((t.kind, t.digits), (CellKind::DateTime { tz: false }, 7));
        assert_eq!(mssql("datetime").digits, 3);
        assert_eq!(mssql("smalldatetime").digits, 0);
        assert_eq!(
            mssql("datetimeoffset(3)").kind,
            CellKind::DateTime { tz: true }
        );
        assert_eq!(mssql("time(2)").kind, CellKind::Time { tz: false });
        assert_eq!(mssql("nvarchar(60)").kind, CellKind::Text);
    }

    #[test]
    fn values_come_back_exactly_as_they_were_read() {
        let tz = CellKind::DateTime { tz: true };
        for text in [
            "2026-11-12 10:46:35.187386+00",
            "2026-11-12 10:46:35-03",
            "2026-11-12 10:46:35.5+05:30",
            "2026-11-12 10:46:35.1234567 -03:00",
            "2026-11-12 00:00:00",
            "12026-01-02 03:04:05",
        ] {
            let stamp = Stamp::parse(&tz, text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(stamp.format(&tz), text);
        }
        let time = CellKind::Time { tz: true };
        for text in ["10:46:35.187386", "23:59:59+00", "00:00:00.1234567"] {
            assert_eq!(Stamp::parse(&time, text).unwrap().format(&time), text);
        }
        let date = Stamp::parse(&CellKind::Date, "2024-02-29").unwrap();
        assert_eq!(date.format(&CellKind::Date), "2024-02-29");
    }

    #[test]
    fn changing_the_day_keeps_the_microseconds_and_the_zone() {
        let kind = CellKind::DateTime { tz: true };
        let mut stamp = Stamp::parse(&kind, "2026-11-12 10:46:35.187386+00").unwrap();
        assert_eq!(
            (stamp.frac.as_str(), stamp.offset.as_str()),
            ("187386", "+00")
        );
        stamp.day = 30;
        stamp.hour = 7;
        assert_eq!(stamp.format(&kind), "2026-11-30 07:46:35.187386+00");
    }

    #[test]
    fn typed_shortcuts_are_understood() {
        let kind = CellKind::DateTime { tz: false };
        // Só a data: meia-noite.
        assert_eq!(
            Stamp::parse(&kind, "2026-1-5").unwrap().format(&kind),
            "2026-01-05 00:00:00"
        );
        assert_eq!(
            Stamp::parse(&kind, "2026-01-05T08:30")
                .unwrap()
                .format(&kind),
            "2026-01-05 08:30:00"
        );
        assert_eq!(
            Stamp::parse(&kind, " 2026-01-05 08:30:15Z ")
                .unwrap()
                .format(&kind),
            "2026-01-05 08:30:15Z"
        );
    }

    #[test]
    fn what_is_not_an_iso_value_stays_text() {
        let kind = CellKind::DateTime { tz: true };
        for text in [
            "",
            "infinity",
            "-infinity",
            "0044-03-15 BC",
            "12/11/2026 10:00:00",
            "2026-02-30 10:00:00",
            "2026-11-12 25:00:00",
            "2026-11-12 10:00:00 America/Sao_Paulo",
            "2026-11-12 10:00:00+",
        ] {
            assert_eq!(Stamp::parse(&kind, text), None, "{text}");
        }
        assert_eq!(Stamp::parse(&CellKind::Date, "2026-11-12 10:00:00"), None);
        assert_eq!(
            Stamp::parse(&CellKind::Time { tz: false }, "2026-11-12"),
            None
        );
    }
}

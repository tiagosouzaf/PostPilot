//! Cores do SQL, como o editor do SSMS (palavras-chave azuis, strings
//! vermelhas, comentários verdes).

use eframe::egui::{self, Color32, FontId, TextFormat, text::LayoutJob};

use crate::lexer::{TokenKind, tokenize};

/// Cores de um tema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Colors {
    pub text: Color32,
    pub keyword: Color32,
    pub string: Color32,
    pub comment: Color32,
    pub number: Color32,
    pub operator: Color32,
    pub identifier_quoted: Color32,
}

impl Colors {
    /// Cores do SSMS no tema claro.
    pub const LIGHT: Colors = Colors {
        text: Color32::from_rgb(0x00, 0x00, 0x00),
        keyword: Color32::from_rgb(0x00, 0x00, 0xFF),
        string: Color32::from_rgb(0xA3, 0x15, 0x15),
        comment: Color32::from_rgb(0x00, 0x80, 0x00),
        number: Color32::from_rgb(0x09, 0x86, 0x58),
        operator: Color32::from_rgb(0x80, 0x80, 0x80),
        identifier_quoted: Color32::from_rgb(0x26, 0x7F, 0x99),
    };

    /// Cores do tema escuro do Visual Studio.
    pub const DARK: Colors = Colors {
        text: Color32::from_rgb(0xDC, 0xDC, 0xDC),
        keyword: Color32::from_rgb(0x56, 0x9C, 0xD6),
        string: Color32::from_rgb(0xD6, 0x9D, 0x85),
        comment: Color32::from_rgb(0x57, 0xA6, 0x4A),
        number: Color32::from_rgb(0xB5, 0xCE, 0xA8),
        operator: Color32::from_rgb(0xB4, 0xB4, 0xB4),
        identifier_quoted: Color32::from_rgb(0x4E, 0xC9, 0xB0),
    };

    /// Cor de cada tipo de token.
    pub fn of(&self, kind: TokenKind) -> Color32 {
        match kind {
            TokenKind::Keyword => self.keyword,
            TokenKind::String => self.string,
            TokenKind::Comment => self.comment,
            TokenKind::Number | TokenKind::Parameter => self.number,
            TokenKind::Operator => self.operator,
            TokenKind::QuotedIdentifier => self.identifier_quoted,
            _ => self.text,
        }
    }
}

/// Monta o texto colorido. Tokens vizinhos da mesma cor viram um pedaço só,
/// para o layout ficar leve em arquivos grandes.
pub fn layout_job(text: &str, colors: &Colors, font: FontId) -> LayoutJob {
    let mut job = LayoutJob {
        text: text.to_string(),
        ..Default::default()
    };
    let mut current: Option<(Color32, usize, usize)> = None;
    for token in tokenize(text) {
        let color = if token.kind == TokenKind::Whitespace {
            // Espaço herda a cor de quem estiver aberto, para juntar pedaços.
            current.map_or(colors.text, |(c, _, _)| c)
        } else {
            colors.of(token.kind)
        };
        match &mut current {
            Some((c, _, end)) if *c == color => *end = token.range.end,
            _ => {
                if let Some((c, start, end)) = current.take() {
                    push(&mut job, start..end, c, &font);
                }
                current = Some((color, token.range.start, token.range.end));
            }
        }
    }
    if let Some((c, start, end)) = current {
        push(&mut job, start..end, c, &font);
    }
    job
}

fn push(job: &mut LayoutJob, range: std::ops::Range<usize>, color: Color32, font: &FontId) {
    job.sections.push(egui::text::LayoutSection {
        leading_space: 0.0,
        byte_range: egui::text::ByteIndex(range.start)..egui::text::ByteIndex(range.end),
        format: TextFormat::simple(font.clone(), color),
    });
}

#[derive(Default)]
pub(crate) struct Highlighter;

/// A chave do cache leva o tamanho da fonte em bits (`f32::to_bits`), porque
/// precisa ser `Copy`; a fonte é sempre a monoespaçada.
impl egui::cache::ComputerMut<(&str, Colors, u32), LayoutJob> for Highlighter {
    fn compute(&mut self, (text, colors, size): (&str, Colors, u32)) -> LayoutJob {
        layout_job(text, &colors, FontId::monospace(f32::from_bits(size)))
    }
}

/// Cache do texto colorido: só recalcula quando o texto muda.
pub(crate) type HighlightCache = egui::cache::FrameCache<LayoutJob, Highlighter>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_cover_the_text_and_merge_neighbours() {
        let text = "SELECT a, b FROM t -- fim";
        let job = layout_job(text, &Colors::LIGHT, FontId::monospace(12.0));
        let mut pos = 0;
        for s in &job.sections {
            assert_eq!(usize::from(s.byte_range.start), pos);
            pos = s.byte_range.end.into();
        }
        assert_eq!(pos, text.len());
        let color_at = |b: usize| {
            job.sections
                .iter()
                .find(|s| {
                    (usize::from(s.byte_range.start)..usize::from(s.byte_range.end)).contains(&b)
                })
                .unwrap()
                .format
                .color
        };
        assert_eq!(color_at(0), Colors::LIGHT.keyword);
        assert_eq!(color_at(7), Colors::LIGHT.text);
        assert_eq!(color_at(text.find("--").unwrap()), Colors::LIGHT.comment);
        // "a, b" (identificador, vírgula, espaço, identificador) vira um
        // pedaço só.
        assert!(job.sections.len() <= 6, "{}", job.sections.len());
    }
}

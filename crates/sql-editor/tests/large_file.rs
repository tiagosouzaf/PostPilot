//! Desempenho com um arquivo de 5 mil linhas: o realce é recalculado a cada
//! tecla digitada, então precisa caber folgado num frame.

use std::time::{Duration, Instant};

use eframe::egui::FontId;
use pgstudio_sql_editor::{Colors, highlight, lexer};

fn big_script() -> String {
    let block = "-- pedidos do dia\n\
SELECT o.id, o.total, c.\"Nome\" FROM loja.orders o\n\
  JOIN loja.customers c ON c.id = o.customer_id\n\
 WHERE o.created_at >= now() - interval '1 day' AND o.total > 100.50;\n\
/* bloco */ UPDATE t SET v = $1 WHERE k = E'a\\'b';\n";
    block.repeat(1000)
}

#[test]
fn five_thousand_lines_highlight_quickly() {
    let text = big_script();
    assert_eq!(text.lines().count(), 5000);

    // Aquece e mede o melhor de três, para não medir ruído da máquina.
    let mut best = Duration::MAX;
    let mut sections = 0;
    for _ in 0..3 {
        let start = Instant::now();
        let job = highlight::layout_job(&text, &Colors::LIGHT, FontId::monospace(13.0));
        best = best.min(start.elapsed());
        sections = job.sections.len();
    }
    let tokens = lexer::tokenize(&text).len();
    eprintln!("5000 linhas: {tokens} tokens, {sections} trechos, realce em {best:?}");

    // Em build de teste (sem otimização) fica bem abaixo disto; o limite só
    // pega uma regressão grosseira (algo quadrático, por exemplo).
    assert!(best < Duration::from_millis(500), "{best:?}");
}

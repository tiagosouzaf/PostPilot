//! Busca aproximada do IntelliSense: acha o nome que a pessoa quis mesmo
//! quando ela digita só um pedaço, abrevia, junta as palavras ou erra uma
//! letra.
//!
//! Do melhor para o pior, um nome combina com o que foi digitado quando:
//!
//! 1. é igual (`status`);
//! 2. começa igual (`sta` em `status`);
//! 3. começa igual sem os separadores (`datahora` em `data_hora_criacao`);
//! 4. uma das palavras começa igual (`cria` em `data_hora_criacao`);
//! 5. tem o trecho no meio de uma palavra (`riac` em `data_hora_criacao`);
//! 6. tem os começos das palavras, na ordem (`dhc`, `dat_cri`);
//! 7. tem as letras na ordem, pulando algumas (`dt_cria`, `qtd`);
//! 8. tem o trecho com um erro de digitação (`statsu`, `clinte`).
//!
//! Maiúsculas, acentos e separadores (`_`, `-`, espaço, ponto) não contam.
//! A lista de sugestões mostra os grupos nessa ordem, então quem digita
//! certo continua vendo primeiro o que via antes. Os dois últimos grupos
//! são palpites: só entram na lista quando nenhum nome combinou de um dos
//! outros jeitos, para não encher de ruído a lista de quem digitou certo.

use crate::complete::Suggestion;

/// Como o nome combinou; a ordem é a da lista (o melhor primeiro).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Exact,
    Prefix,
    Joined,
    Word,
    Inside,
    Chain,
    Loose,
    Typo,
}

/// Um nome que combinou com o que foi digitado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub tier: Tier,
    /// Desempate dentro do grupo: quanto menor, melhor.
    pub penalty: u32,
    /// As letras do nome que casaram (índices em caracteres, em ordem),
    /// para destacar na lista.
    pub chars: Vec<usize>,
}

fn is_separator(c: char) -> bool {
    matches!(c, '_' | '-' | ' ' | '.')
}

/// A letra sem maiúscula e sem acento.
fn fold(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        c => c,
    }
}

/// Um nome sem os separadores, com o que é preciso para voltar a ele.
struct Name {
    chars: Vec<char>,
    /// Posição de cada letra no nome original (em caracteres).
    orig: Vec<usize>,
    /// Se a letra começa uma palavra (`data_hora`, `dataHora`, `$set`).
    start: Vec<bool>,
}

impl Name {
    fn new(name: &str) -> Self {
        let mut out = Name {
            chars: Vec::with_capacity(name.len()),
            orig: Vec::with_capacity(name.len()),
            start: Vec::with_capacity(name.len()),
        };
        let mut prev: Option<char> = None;
        for (i, c) in name.chars().enumerate() {
            if !is_separator(c) {
                let start = match prev {
                    None => true,
                    Some(p) => {
                        (!p.is_alphanumeric() && c.is_alphanumeric())
                            || (p.is_lowercase() && c.is_uppercase())
                    }
                };
                out.chars.push(fold(c));
                out.orig.push(i);
                out.start.push(start);
            }
            prev = Some(c);
        }
        if let Some(first) = out.start.first_mut() {
            *first = true;
        }
        out
    }

    fn span(&self, from: usize, len: usize) -> Vec<usize> {
        self.orig[from..from + len].to_vec()
    }
}

/// Se (e como) `name` combina com o que foi digitado. Nada digitado
/// combina com tudo.
pub fn find(name: &str, typed: &str) -> Option<Match> {
    if typed.is_empty() {
        return Some(Match {
            tier: Tier::Prefix,
            penalty: 0,
            chars: Vec::new(),
        });
    }
    // O caso de sempre, sem alocar: o nome começa com o que foi digitado.
    let mut rest = name.chars();
    let mut len = 0;
    if typed.chars().all(|t| {
        len += 1;
        rest.next().is_some_and(|n| fold(n) == fold(t))
    }) {
        return Some(Match {
            tier: if rest.next().is_none() {
                Tier::Exact
            } else {
                Tier::Prefix
            },
            penalty: 0,
            chars: (0..len).collect(),
        });
    }
    let q: Vec<char> = typed
        .chars()
        .filter(|c| !is_separator(*c))
        .map(fold)
        .collect();
    if q.is_empty() || q.len() > name.len() + 2 {
        return None;
    }
    let n = Name::new(name);
    if n.chars.is_empty() {
        return None;
    }
    if n.chars.starts_with(&q) {
        return Some(Match {
            tier: Tier::Joined,
            penalty: 0,
            chars: n.span(0, q.len()),
        });
    }
    // Uma letra só combina com o começo do nome: o resto seria ruído.
    if q.len() < 2 {
        return None;
    }
    // O trecho inteiro: no começo de uma palavra ou no meio.
    let mut inside = None;
    if q.len() <= n.chars.len() {
        for at in 0..=n.chars.len() - q.len() {
            if n.chars[at..at + q.len()] == q[..] {
                if n.start[at] {
                    return Some(Match {
                        tier: Tier::Word,
                        penalty: at as u32,
                        chars: n.span(at, q.len()),
                    });
                }
                inside.get_or_insert(at);
            }
        }
    }
    if let Some(at) = inside
        && q.len() >= 3
    {
        return Some(Match {
            tier: Tier::Inside,
            penalty: at as u32,
            chars: n.span(at, q.len()),
        });
    }
    scattered(&n, &q).or_else(|| typo(&n, &q))
}

const START_BONUS: i32 = 8;
const RUN_BONUS: i32 = 6;
/// Quanto se perde, no máximo, por pular letras entre duas que casaram.
const GAP_MAX: usize = 3;

/// Pontos que um nome precisa para entrar como abreviação com letras
/// puladas. Com duas letras só se pula uma (`dt` em `data`): corta os
/// nomes que só têm as letras por acaso.
fn scattered_min(len: usize) -> i32 {
    if len <= 2 { 7 } else { len as i32 }
}

/// As letras de `q` na ordem dentro do nome, a primeira no começo de uma
/// palavra. Vale mais quem casa começos de palavra e letras seguidas, e
/// menos quem pula muitas letras.
fn scattered(n: &Name, q: &[char]) -> Option<Match> {
    let (m, len) = (q.len(), n.chars.len());
    if m > len {
        return None;
    }
    // Antes da tabela: as letras estão lá, na ordem?
    let mut it = n.chars.iter();
    if !q.iter().all(|c| it.any(|x| x == c)) {
        return None;
    }
    const NONE: i32 = i32::MIN / 2;
    let mut score = vec![NONE; m * len];
    let mut back = vec![usize::MAX; m * len];
    for (j, first) in score.iter_mut().enumerate().take(len) {
        if n.chars[j] == q[0] && n.start[j] {
            *first = START_BONUS;
        }
    }
    for i in 1..m {
        for j in i..len {
            if n.chars[j] != q[i] {
                continue;
            }
            let mut best = NONE;
            let mut from = usize::MAX;
            for k in i - 1..j {
                let prev = score[(i - 1) * len + k];
                if prev == NONE {
                    continue;
                }
                let s = if k + 1 == j {
                    prev + if n.start[j] { START_BONUS } else { RUN_BONUS }
                } else {
                    prev - (j - 1 - k).min(GAP_MAX) as i32
                        + if n.start[j] { START_BONUS } else { 0 }
                };
                if s > best {
                    best = s;
                    from = k;
                }
            }
            score[i * len + j] = best;
            back[i * len + j] = from;
        }
    }
    let (mut j, best) = (0..len)
        .map(|j| (j, score[(m - 1) * len + j]))
        .filter(|(_, s)| *s != NONE)
        // O primeiro entre os empatados.
        .min_by_key(|(j, s)| (-*s, *j))?;
    let mut chars = vec![0; m];
    // Letras que não começam uma palavra nem seguem a letra anterior.
    let mut loose = 0;
    for i in (0..m).rev() {
        chars[i] = n.orig[j];
        let prev = if i > 0 { back[i * len + j] } else { usize::MAX };
        if !n.start[j] && prev.wrapping_add(1) != j {
            loose += 1;
        }
        j = prev;
    }
    if loose > 0 && best < scattered_min(m) {
        return None;
    }
    Some(Match {
        tier: if loose == 0 { Tier::Chain } else { Tier::Loose },
        penalty: (START_BONUS * m as i32 - best).max(0) as u32,
        chars,
    })
}

/// Daqui para cima não se procura erro de digitação.
const TYPO_MAX_LEN: usize = 48;

/// Quantos erros de digitação se aceita numa palavra desse tamanho.
fn typos_allowed(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=7 => 1,
        8..=TYPO_MAX_LEN => 2,
        _ => 0,
    }
}

/// O começo de uma das palavras do nome com poucos erros de digitação:
/// letra trocada, faltando, sobrando ou duas invertidas.
fn typo(n: &Name, q: &[char]) -> Option<Match> {
    let allowed = typos_allowed(q.len());
    if allowed == 0 {
        return None;
    }
    let mut best: Option<(usize, usize, usize)> = None;
    for s in (0..n.chars.len()).filter(|s| n.start[*s]) {
        let seg = &n.chars[s..];
        let seg = &seg[..seg.len().min(q.len() + allowed)];
        // Quem erra quase nunca erra a primeira letra; no máximo inverte
        // as duas primeiras.
        let first = seg[0] == q[0] || (seg.len() > 1 && seg[1] == q[0] && seg[0] == q[1]);
        if !first || seg.len() + allowed < q.len() {
            continue;
        }
        let (dist, len) = prefix_distance(q, seg);
        if dist <= allowed && best.is_none_or(|(d, _, _)| dist < d) {
            best = Some((dist, s, len));
        }
    }
    let (dist, s, len) = best?;
    Some(Match {
        tier: Tier::Typo,
        penalty: (dist * 1000 + s) as u32,
        chars: n.span(s, len),
    })
}

/// A menor distância de edição (com inversão de letras vizinhas) entre
/// `q` e um começo de `seg`, e o tamanho desse começo.
fn prefix_distance(q: &[char], seg: &[char]) -> (usize, usize) {
    // Três linhas da tabela, sem alocar: a palavra digitada é curta.
    const MAX: usize = TYPO_MAX_LEN + 3;
    let l = seg.len().min(MAX - 1);
    let mut before = [0usize; MAX];
    let mut prev = [0usize; MAX];
    let mut row = [0usize; MAX];
    for (j, cell) in prev.iter_mut().enumerate().take(l + 1) {
        *cell = j;
    }
    for i in 1..=q.len() {
        row[0] = i;
        for j in 1..=l {
            let cost = usize::from(q[i - 1] != seg[j - 1]);
            let mut v = (prev[j] + 1).min(row[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && q[i - 1] == seg[j - 2] && q[i - 2] == seg[j - 1] {
                v = v.min(before[j - 2] + 1);
            }
            row[j] = v;
        }
        before = prev;
        prev = row;
    }
    (0..=l)
        .map(|j| (prev[j], j))
        // No empate, o começo de tamanho mais parecido com o digitado.
        .min_by_key(|(dist, j)| (*dist, j.abs_diff(q.len())))
        .unwrap_or((q.len(), 0))
}

/// Se `name` combina com o que foi digitado.
pub fn matches(name: &str, typed: &str) -> bool {
    find(name, typed).is_some()
}

/// O grupo da sugestão para o que foi digitado.
pub fn tier(label: &str, typed: &str) -> Tier {
    find(label, typed).map_or(Tier::Typo, |m| m.tier)
}

/// Põe as sugestões na ordem dos grupos: primeiro as que começam com o
/// que foi digitado, por último as aproximadas. Dentro de um grupo fica a
/// ordem em que vieram (nos aproximados, os mais parecidos antes). Os
/// palpites (letras puladas, erro de digitação) saem da lista quando há
/// algum nome que combinou melhor.
pub fn sort(items: &mut Vec<Suggestion>, typed: &str) {
    if typed.is_empty() || items.is_empty() {
        return;
    }
    let mut keyed: Vec<((Tier, u32), Suggestion)> = std::mem::take(items)
        .into_iter()
        .map(|s| {
            let key = find(&s.label, typed).map_or((Tier::Typo, u32::MAX), |m| {
                (
                    m.tier,
                    // A posição da palavra não muda a ordem: quem digita
                    // `id` quer ver `cliente_id` e `pedido_id` em ordem.
                    if m.tier >= Tier::Chain { m.penalty } else { 0 },
                )
            });
            (key, s)
        })
        .collect();
    if keyed.iter().any(|(k, _)| k.0 <= Tier::Chain) {
        keyed.retain(|(k, _)| k.0 <= Tier::Chain);
    }
    keyed.sort_by_key(|(k, _)| *k);
    *items = keyed.into_iter().map(|(_, s)| s).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier_of(name: &str, typed: &str) -> Option<Tier> {
        find(name, typed).map(|m| m.tier)
    }

    /// As letras do nome que casaram.
    fn marked(name: &str, typed: &str) -> String {
        let m = find(name, typed).unwrap();
        let chars: Vec<char> = name.chars().collect();
        m.chars.iter().map(|i| chars[*i]).collect()
    }

    #[test]
    fn exact_and_prefix_ignore_case_and_accents() {
        assert_eq!(tier_of("status", "STATUS"), Some(Tier::Exact));
        assert_eq!(tier_of("status", "sta"), Some(Tier::Prefix));
        assert_eq!(tier_of("Descrição", "descricao"), Some(Tier::Exact));
        assert_eq!(tier_of("data_hora_criacao", "data_h"), Some(Tier::Prefix));
        assert_eq!(tier_of("qualquer", ""), Some(Tier::Prefix));
    }

    #[test]
    fn words_can_be_joined_or_picked_from_the_middle() {
        assert_eq!(tier_of("data_hora_criacao", "datahora"), Some(Tier::Joined));
        assert_eq!(tier_of("data_hora_criacao", "cria"), Some(Tier::Word));
        assert_eq!(marked("data_hora_criacao", "cria"), "cria");
        assert_eq!(tier_of("data_hora_criacao", "hora_cri"), Some(Tier::Word));
        assert_eq!(tier_of("dataHoraCriacao", "hora"), Some(Tier::Word));
        assert_eq!(tier_of("data_hora_criacao", "riac"), Some(Tier::Inside));
        // Duas letras no meio de uma palavra é ruído.
        assert_eq!(tier_of("pedido", "di"), None);
        assert_eq!(tier_of("$set", "set"), Some(Tier::Word));
        assert_eq!(tier_of("cliente_id", "i"), None);
    }

    #[test]
    fn abbreviations_match_the_letters_in_order() {
        assert_eq!(tier_of("data_hora_criacao", "dt_cria"), Some(Tier::Loose));
        assert_eq!(marked("data_hora_criacao", "dt_cria"), "dtcria");
        assert_eq!(tier_of("data_hora_criacao", "dhc"), Some(Tier::Chain));
        assert_eq!(tier_of("data_hora_criacao", "dat_cri"), Some(Tier::Chain));
        assert_eq!(tier_of("dataHoraCriacao", "dahocri"), Some(Tier::Chain));
        assert_eq!(marked("data_hora_criacao", "dhc"), "dhc");
        assert_eq!(tier_of("data_hora", "dthr"), Some(Tier::Loose));
        assert_eq!(tier_of("nome", "nm"), Some(Tier::Loose));
        assert_eq!(tier_of("usuario", "usr"), Some(Tier::Loose));
        assert_eq!(tier_of("quantidade", "qtd"), Some(Tier::Loose));
        assert_eq!(tier_of("valor_total", "vlr_tot"), Some(Tier::Loose));
        // Letras espalhadas por acaso não entram.
        assert_eq!(tier_of("sistema", "se"), None);
        assert_eq!(tier_of("cliente", "ct"), None);
        assert_eq!(tier_of("pedido", "id"), None);
    }

    #[test]
    fn typos_still_find_the_name() {
        for (name, typed) in [
            ("status", "statsu"),
            ("status", "sttaus"),
            ("cliente", "clinte"),
            ("cliente", "cleinte"),
            ("cliente_id", "cliemte"),
            ("data_hora_criacao", "craicao"),
            ("data_hora_criacao", "datahroa"),
            ("quantidade", "quamtidede"),
            ("select", "slect"),
            ("select", "selct"),
            ("from", "form"),
            ("pedido", "peddio"),
        ] {
            assert!(
                matches!(tier_of(name, typed), Some(Tier::Typo | Tier::Loose)),
                "{typed} deveria achar {name}: {:?}",
                tier_of(name, typed)
            );
        }
        assert_eq!(marked("data_hora_criacao", "craicao"), "criacao");
        assert_eq!(tier_of("data_hora_criacao", "criaçao"), Some(Tier::Word));
        // Palavra curta ou diferente demais não é erro de digitação.
        assert_eq!(tier_of("now", "nom"), None);
        assert_eq!(tier_of("status", "estoque"), None);
        assert_eq!(tier_of("cliente", "produto"), None);
        assert_eq!(tier_of("total", "xpto"), None);
    }

    #[test]
    fn the_list_keeps_what_starts_alike_first() {
        let item = |label: &str| Suggestion {
            label: label.into(),
            insert: label.into(),
            kind: crate::complete::Kind::Column,
            detail: String::new(),
        };
        let list = |names: &[&str], typed: &str| -> Vec<String> {
            let mut items: Vec<Suggestion> = names
                .iter()
                .filter(|n| matches(n, typed))
                .map(|n| item(n))
                .collect();
            sort(&mut items, typed);
            items.into_iter().map(|s| s.label).collect()
        };
        let names = [
            "atualizado_status",
            "estatistica",
            "stats_diario",
            "status",
            "status_pedido",
        ];
        // Errou: vêm os parecidos, o mais parecido primeiro.
        assert_eq!(
            list(&names, "statsu"),
            [
                "stats_diario",
                "status",
                "status_pedido",
                "atualizado_status"
            ]
        );
        // Acertou: o começo, depois a palavra do meio; nenhum palpite.
        assert_eq!(
            list(&names, "stat"),
            [
                "stats_diario",
                "status",
                "status_pedido",
                "atualizado_status",
                "estatistica"
            ]
        );
        assert_eq!(list(&["cliente", "conflict"], "cli"), ["cliente"]);
        assert_eq!(list(&["conflict"], "cli"), ["conflict"]);
    }
}

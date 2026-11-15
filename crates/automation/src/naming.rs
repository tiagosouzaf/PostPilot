//! O nome de arquivo que a IA sugere ao salvar uma consulta nova, no
//! lugar de `SQLQuery1.sql`: a pergunta e a limpeza da resposta.

/// Quanto do SQL vai para o modelo.
pub const MAX_SQL: usize = 4000;
/// Tamanho máximo do nome, sem a extensão.
const MAX_NAME: usize = 60;

/// Instruções para o modelo. `language` é o idioma da interface, em
/// inglês.
pub fn system_prompt(language: &str) -> String {
    format!(
        "You name SQL files. Given the SQL of a query window, answer with one short file name \
         that says what the SQL does, and nothing else: two to five words in lowercase joined \
         by underscores, only letters a-z, digits and underscores (no accents), no extension, \
         no quotes, no explanation. Use {language} words, keeping table and column names as \
         they are. Example: active_customers_by_month"
    )
}

/// A pergunta: o SQL, cortado em [`MAX_SQL`].
pub fn user_prompt(sql: &str) -> String {
    sql.chars().take(MAX_SQL).collect()
}

/// Transforma a resposta num nome de arquivo `.sql` seguro. `None` quando
/// não sobra nada que sirva.
pub fn file_name(answer: &str) -> Option<String> {
    let mut text = answer.trim();
    if let Some(end) = text.find("</think>") {
        text = text[end + "</think>".len()..].trim();
    }
    let line = text
        .lines()
        .map(|l| {
            l.trim()
                .trim_matches(|c| matches!(c, '`' | '"' | '\'' | '*'))
        })
        .find(|l| !l.is_empty())?;
    let line = line.strip_suffix(".sql").unwrap_or(line);
    let mut name = String::new();
    for c in line.chars() {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' => 'a',
            'é' | 'ê' | 'è' | 'É' | 'Ê' => 'e',
            'í' | 'Í' => 'i',
            'ó' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ô' | 'Õ' => 'o',
            'ú' | 'ü' | 'Ú' => 'u',
            'ç' | 'Ç' => 'c',
            c => c.to_ascii_lowercase(),
        };
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('_') {
            name.push('_');
        }
        if name.len() >= MAX_NAME {
            break;
        }
    }
    let name = name.trim_matches('_');
    // Uma frase inteira não é um nome: o modelo não seguiu o pedido.
    if name.is_empty() || name.split('_').count() > 8 {
        return None;
    }
    Some(format!("{name}.sql"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_safe_names() {
        assert_eq!(
            file_name("clientes_ativos_por_mes").as_deref(),
            Some("clientes_ativos_por_mes.sql")
        );
        assert_eq!(
            file_name("`Pedidos em Atraso.sql`\n").as_deref(),
            Some("pedidos_em_atraso.sql")
        );
        assert_eq!(
            file_name("<think>x</think>\n\"relatório-de-ações\"").as_deref(),
            Some("relatorio_de_acoes.sql")
        );
        assert_eq!(
            file_name("../../etc/passwd").as_deref(),
            Some("etc_passwd.sql")
        );
        assert_eq!(file_name("  \n"), None);
        assert_eq!(
            file_name("Sure! Here is a good name for this file you asked about"),
            None
        );
        assert!(file_name(&"a".repeat(200)).unwrap().len() <= MAX_NAME + 4);
    }
}

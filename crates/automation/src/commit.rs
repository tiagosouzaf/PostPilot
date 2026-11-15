//! A mensagem de commit escrita pela IA (o botão do diálogo de Commit do
//! Workspace): a pergunta que vai para o modelo e a limpeza da resposta.

/// Quanto do diff vai para o modelo.
pub const MAX_DIFF: usize = 24_000;

/// Instruções para o modelo. `language` é o idioma da interface, em
/// inglês ("Brazilian Portuguese").
pub fn system_prompt(language: &str) -> String {
    format!(
        "You write git commit messages for a PostPilot workspace: a folder with saved SQL \
         queries (queries/), registered database servers (servers.json) and workspace settings, \
         versioned with git. Given the list of changed files and the diff, answer with the \
         commit message and nothing else: no quotes, no code fences, no explanation. First a \
         summary line of at most 72 characters, in the imperative, saying what changed. If the \
         change needs it, add a blank line and up to four short lines with the details. Do not \
         invent anything that is not in the diff. Never repeat passwords, keys or tokens. \
         Write in {language}."
    )
}

/// A pergunta: os arquivos e o diff (cortado em [`MAX_DIFF`]).
pub fn user_prompt(changes: &str, diff: &str) -> String {
    let mut cut: String = diff.chars().take(MAX_DIFF).collect();
    if cut.len() < diff.len() {
        cut.push_str("\n[diff truncated]");
    }
    format!("Changed files:\n{changes}\n\nDiff:\n{cut}\n")
}

/// Tira o que alguns modelos põem em volta da mensagem: cercas de código,
/// aspas, "Commit message:" e o raciocínio em `<think>`.
pub fn clean(answer: &str) -> String {
    let mut text = answer.trim();
    if let Some(end) = text.find("</think>") {
        text = text[end + "</think>".len()..].trim();
    }
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.first().is_some_and(|l| l.trim_start().starts_with("```")) {
        lines.remove(0);
        if let Some(end) = lines.iter().rposition(|l| l.trim() == "```") {
            lines.truncate(end);
        }
    }
    let mut text = lines.join("\n").trim().to_string();
    for prefix in ["Commit message:", "commit message:", "Mensagem de commit:"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_string();
        }
    }
    for quote in ['"', '\'', '`'] {
        if text.len() >= 2
            && text.starts_with(quote)
            && text.ends_with(quote)
            && !text[1..text.len() - 1].contains(quote)
        {
            text = text[1..text.len() - 1].trim().to_string();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_what_models_add() {
        assert_eq!(clean("  Adiciona consulta\n"), "Adiciona consulta");
        assert_eq!(clean("```\nAdiciona consulta\n\n- x\n```"), "Adiciona consulta\n\n- x");
        assert_eq!(clean("```text\nA\n```\n"), "A");
        assert_eq!(clean("\"Adiciona consulta\""), "Adiciona consulta");
        assert_eq!(clean("Commit message: Add query"), "Add query");
        assert_eq!(clean("<think>hmm</think>\n\nAdd query"), "Add query");
        assert_eq!(clean("Renomeia \"a\" para \"b\""), "Renomeia \"a\" para \"b\"");
    }

    #[test]
    fn prompt_cuts_long_diffs() {
        let p = user_prompt("Added a.sql", &"x".repeat(MAX_DIFF + 10));
        assert!(p.contains("[diff truncated]"));
        assert!(p.starts_with("Changed files:\nAdded a.sql"));
        assert!(system_prompt("Brazilian Portuguese").ends_with("Write in Brazilian Portuguese."));
    }
}

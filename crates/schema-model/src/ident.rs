//! Nomes no SQL: quando um identificador precisa de aspas.

/// Palavras que o PostgreSQL não aceita como nome sem aspas: as reservadas e
/// as que podem ser nome de função ou de tipo (`RESERVED_KEYWORD` e
/// `TYPE_FUNC_NAME_KEYWORD` do `kwlist.h`), em ordem alfabética.
pub(crate) const RESERVED: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "authorization",
    "binary",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "collation",
    "column",
    "concurrently",
    "constraint",
    "create",
    "cross",
    "current_catalog",
    "current_date",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "freeze",
    "from",
    "full",
    "grant",
    "group",
    "having",
    "ilike",
    "in",
    "initially",
    "inner",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "lateral",
    "leading",
    "left",
    "like",
    "limit",
    "localtime",
    "localtimestamp",
    "natural",
    "not",
    "notnull",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "outer",
    "overlaps",
    "placing",
    "primary",
    "references",
    "returning",
    "right",
    "select",
    "session_user",
    "similar",
    "some",
    "symmetric",
    "system_user",
    "table",
    "tablesample",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "variadic",
    "verbose",
    "when",
    "where",
    "window",
    "with",
];

/// Como escrever `name` no SQL: entre aspas se precisar (maiúsculas,
/// espaços, símbolos ou palavra reservada, como `"user"`).
pub fn quote_ident(name: &str) -> String {
    let plain = RESERVED.binary_search(&name).is_err()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_lowercase() || c.is_ascii_digit() || c == '_' || c == '$');
    if plain {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quoted_when_needed() {
        assert_eq!(quote_ident("cliente"), "cliente");
        assert_eq!(quote_ident("Nome"), "\"Nome\"");
        assert_eq!(quote_ident("dois nomes"), "\"dois nomes\"");
        assert_eq!(quote_ident("user"), "\"user\"");
        assert_eq!(quote_ident("order"), "\"order\"");
        assert_eq!(quote_ident("time"), "time");
        assert_eq!(quote_ident("1a"), "\"1a\"");
        assert!(
            RESERVED.windows(2).all(|w| w[0] < w[1]),
            "RESERVED fora de ordem"
        );
    }
}

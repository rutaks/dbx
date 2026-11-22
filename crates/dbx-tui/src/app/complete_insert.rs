//! What goes into the text when a suggestion is accepted: a name quoted when
//! the dialect needs it, a keyword in the case being typed. dbx's own quoting
//! functions are private to its crates and per feature, so the rule is ours,
//! tested here.
//!
//! Names are otherwise inserted exactly as stored.

use crate::core::Dialect;

/// Words Postgres will not take as a name without quotes: its reserved words
/// and those that are reserved as column or table names (`order`, `user`,
/// `group`, `table`, `limit`, `end`, `column`...), lower-case, sorted.
const POSTGRES_RESERVED: &[&str] = &[
    "all", "analyse", "analyze", "and", "any", "array", "as", "asc", "asymmetric", "authorization", "binary", "both", "case", "cast",
    "check", "collate", "collation", "column", "concurrently", "constraint", "create", "cross", "current_catalog", "current_date",
    "current_role", "current_schema", "current_time", "current_timestamp", "current_user", "default", "deferrable", "desc",
    "distinct", "do", "else", "end", "except", "false", "fetch", "for", "foreign", "freeze", "from", "full", "grant", "group",
    "having", "ilike", "in", "initially", "inner", "intersect", "into", "is", "isnull", "join", "lateral", "leading", "left",
    "like", "limit", "localtime", "localtimestamp", "natural", "not", "notnull", "null", "offset", "on", "only", "or", "order",
    "outer", "overlaps", "placing", "primary", "references", "returning", "right", "select", "session_user", "similar", "some",
    "symmetric", "system_user", "table", "tablesample", "then", "to", "trailing", "true", "union", "unique", "user", "using",
    "variadic", "verbose", "when", "where", "window", "with",
];

/// A word MySQL needs backticks around when used as a name.
const MYSQL_RESERVED: &[&str] = &[
    "add", "all", "alter", "and", "as", "asc", "between", "by", "case", "column", "create", "cross", "database", "default", "delete",
    "desc", "distinct", "drop", "else", "exists", "false", "for", "foreign", "from", "group", "having", "if", "in", "index",
    "inner", "insert", "into", "is", "join", "key", "keys", "left", "like", "limit", "not", "null", "on", "or", "order", "outer",
    "primary", "references", "right", "select", "set", "table", "then", "to", "true", "union", "unique", "update", "use", "using",
    "values", "when", "where", "with",
];

/// `name` as it must be written in SQL for `dialect`: bare when it is a plain
/// lower-case name that is not a reserved word, otherwise quoted with the
/// dialect's quote (`"` for Postgres and most others, a backtick for MySQL),
/// the quote doubled inside.
pub fn quote_identifier(name: &str, dialect: Option<Dialect>) -> String {
    let mysql = dialect == Some(Dialect::MySql);
    let mut chars = name.chars();
    let shaped = chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$');
    let reserved = match dialect {
        Some(Dialect::PostgreSql) => POSTGRES_RESERVED.binary_search(&name).is_ok(),
        _ if mysql => MYSQL_RESERVED.contains(&name),
        _ => false,
    };
    if shaped && !reserved {
        return name.to_string();
    }
    let quote = if mysql { '`' } else { '"' };
    let doubled: String = name.chars().flat_map(|c| if c == quote { vec![c, c] } else { vec![c] }).collect();
    format!("{quote}{doubled}{quote}")
}

/// A keyword in the case the user is typing in: upper-case when what they
/// typed is all capitals, lower-case otherwise.
pub fn keyword_text(typed: &str, keyword: &str) -> String {
    let letters: Vec<char> = typed.chars().filter(|c| c.is_alphabetic()).collect();
    if !letters.is_empty() && letters.iter().all(|c| c.is_uppercase()) {
        keyword.to_uppercase()
    } else {
        keyword.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg(name: &str) -> String {
        quote_identifier(name, Some(Dialect::PostgreSql))
    }

    #[test]
    fn the_reserved_word_list_is_sorted_for_the_binary_search() {
        assert!(POSTGRES_RESERVED.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn plain_lower_case_names_are_inserted_as_they_are() {
        for name in ["film", "film_actor", "_x", "a1", "col$1"] {
            assert_eq!(pg(name), name, "{name}");
        }
        assert_eq!(pg("é"), "\"é\"", "anything beyond ASCII is quoted, which is always valid");
    }

    #[test]
    fn mixed_case_names_and_names_with_spaces_or_symbols_are_quoted() {
        assert_eq!(pg("Order Items"), "\"Order Items\"");
        assert_eq!(pg("Film"), "\"Film\"");
        assert_eq!(pg("film-actor"), "\"film-actor\"");
        assert_eq!(pg("1st"), "\"1st\"", "a name that starts with a digit");
        assert_eq!(pg(""), "\"\"");
    }

    #[test]
    fn reserved_words_are_quoted_for_postgres() {
        for word in ["order", "user", "group", "table", "select", "limit", "end", "column"] {
            assert_eq!(pg(word), format!("\"{word}\""), "{word}");
        }
        // Words that are only keywords are fine as names.
        for word in ["name", "type", "key", "date", "count", "first", "status"] {
            assert_eq!(pg(word), word, "{word}");
        }
    }

    #[test]
    fn a_quote_inside_a_name_is_doubled() {
        assert_eq!(pg("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn mysql_uses_backticks_and_other_kinds_the_standard_quote() {
        assert_eq!(quote_identifier("Order Items", Some(Dialect::MySql)), "`Order Items`");
        assert_eq!(quote_identifier("order", Some(Dialect::MySql)), "`order`");
        assert_eq!(quote_identifier("film", Some(Dialect::MySql)), "film");
        assert_eq!(quote_identifier("a`b", Some(Dialect::MySql)), "`a``b`");
        assert_eq!(quote_identifier("Order Items", Some(Dialect::Sqlite)), "\"Order Items\"");
        assert_eq!(quote_identifier("Order Items", None), "\"Order Items\"");
    }

    #[test]
    fn a_keyword_takes_the_case_the_user_is_typing_in() {
        assert_eq!(keyword_text("sel", "select"), "select");
        assert_eq!(keyword_text("SEL", "select"), "SELECT");
        assert_eq!(keyword_text("S", "select"), "SELECT");
        assert_eq!(keyword_text("Sel", "select"), "select");
        assert_eq!(keyword_text("", "select"), "select", "nothing typed: lower-case");
        assert_eq!(keyword_text("_", "select"), "select", "no letters: lower-case");
    }
}

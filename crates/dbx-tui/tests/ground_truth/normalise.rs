//! How a cell as the Core gives it is compared with the cell as `psql -At`
//! prints it. Both are text; the Core's text may be spelled differently from
//! psql's, and this module decides whether it is the *same value* spelled
//! differently (FORMAT) or a different or missing value (LOSS).
//!
//! The rules, by the Postgres type name psql's catalog gives the column
//! (`udt_name`; the Core's own `Column::db_type` must equal it, or the harness
//! lists a `TYPE` difference), applied only when the two texts are not already
//! identical:
//!
//! | type | the Core's text and psql's are the same value when |
//! |---|---|
//! | `float4`, `float8` | both read back, as `f32` or `f64`, to the same bits (`NaN` equals `NaN`); spelling (`1.0`, `1e-7`, `1e+15`) is ignored |
//! | `bool` | `true`/`t` or `false`/`f` |
//! | `bytea` | `0x` + hex and `\x` + hex have the same hex |
//! | `timestamptz`, `timestamp`, `date`, `time`, `timetz` | after `T` becomes a space, a trailing `Z` becomes `+00`, and an offset `+HH:00` becomes `+HH` |
//! | `interval` | after a trailing ` 00:00:00` is dropped and `years`, `days`, `mons` become singular |
//! | `money` | after `$` and `,` are dropped |
//! | `pg_lsn` | the Core's `0x` + 16 hex digits equals psql's `hi/lo` |
//! | `point` | both read as two numbers with the same values |
//! | `jsonb` | after whitespace outside strings is dropped and object keys are sorted (Postgres stores jsonb normalised, so the order and spacing are not the value's) |
//! | `json` | never: Postgres keeps `json` as the text it was given, so only identical text is the same |
//! | arrays (`_int4`, ...) | both parse as the same nested list, element by element under these same rules, and psql's text has no lower bound (`[0:2]=`), which the Core never shows |
//!
//! Anything else, including every integer, `numeric` and text value, must be
//! identical. NULL against a value is always a LOSS.

use dbx_tui::core::CellKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Same,
    Format,
    Loss,
}

/// `Some(why)` when the Core's type for a column is not the type psql's
/// catalog names for it. The rules above are chosen by psql's type, so a Core
/// that misreports a type cannot change how its values are judged.
pub fn type_mismatch(psql_type: &str, core_type: &str) -> Option<String> {
    (psql_type != core_type).then(|| format!("the Core reports `{core_type}` for a column psql's catalog types as `{psql_type}`"))
}

/// `rule_type` is the type psql's catalog names for the column (`udt_name`),
/// never the Core's.
pub fn classify(_kind: CellKind, rule_type: &str, ours: &Option<String>, theirs: &Option<String>) -> Verdict {
    let (o, t) = match (ours, theirs) {
        (None, None) => return Verdict::Same,
        (Some(o), Some(t)) => (o, t),
        _ => return Verdict::Loss,
    };
    if o == t {
        return Verdict::Same;
    }
    if equivalent(rule_type, o, t) {
        Verdict::Format
    } else {
        Verdict::Loss
    }
}

fn equivalent(db_type: &str, o: &str, t: &str) -> bool {
    if let Some(element) = db_type.strip_prefix('_') {
        return arrays_equivalent(element, o, t);
    }
    match db_type {
        "float4" => matches!((o.parse::<f32>(), t.parse::<f32>()), (Ok(a), Ok(b)) if a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())),
        "float8" => matches!((o.parse::<f64>(), t.parse::<f64>()), (Ok(a), Ok(b)) if a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())),
        "bool" => matches!((o, t), ("true", "t") | ("false", "f")),
        "bytea" => matches!((o.strip_prefix("0x"), t.strip_prefix("\\x")), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b)),
        "timestamptz" | "timestamp" | "date" | "time" | "timetz" => temporal(o) == temporal(t),
        "interval" => interval(o) == interval(t),
        "money" => strip(o, &['$', ',']) == strip(t, &['$', ',']),
        "pg_lsn" => lsn(o).is_some_and(|l| l == t),
        "point" => match (point(o), point(t)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
        "jsonb" => compact_json(o) == compact_json(t),
        _ => false,
    }
}

fn strip(s: &str, remove: &[char]) -> String {
    s.chars().filter(|c| !remove.contains(c)).collect()
}

/// ISO 8601 (dbx) and Postgres's own spelling of a date or time, as one text.
pub fn temporal(s: &str) -> String {
    let mut s = s.replacen('T', " ", 1);
    if let Some(stripped) = s.strip_suffix('Z') {
        s = format!("{stripped}+00");
    }
    // An offset after the time: `+HH:00` or `-HH:00` becomes `+HH`.
    if s.len() > 6 && s.ends_with(":00") {
        let bytes = s.as_bytes();
        let at = s.len() - 6;
        if (bytes[at] == b'+' || bytes[at] == b'-') && bytes[at + 1..at + 3].iter().all(u8::is_ascii_digit) && at > 8 {
            s.truncate(s.len() - 3);
        }
    }
    s
}

fn interval(s: &str) -> String {
    let s = s.strip_suffix(" 00:00:00").unwrap_or(s);
    s.replace("years", "year").replace("days", "day").replace("mons", "mon")
}

fn lsn(s: &str) -> Option<String> {
    let hex = s.strip_prefix("0x")?;
    if hex.len() != 16 {
        return None;
    }
    let (hi, lo) = (u32::from_str_radix(&hex[..8], 16).ok()?, u32::from_str_radix(&hex[8..], 16).ok()?);
    Some(format!("{hi:X}/{lo:X}"))
}

fn point(s: &str) -> Option<(f64, f64)> {
    let (x, y) = s.strip_prefix('(')?.strip_suffix(')')?.split_once(',')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

// ---- JSON ----------------------------------------------------------------------

/// `s` without whitespace outside strings and with object keys in byte order
/// (what two spellings of the same JSON value have in common). Not a
/// validator: text that is not JSON comes back unchanged apart from spaces.
fn compact_json(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut at = 0;
    match json_value(&chars, &mut at) {
        Some(v) if skip_ws(&chars, &mut at) == chars.len() => v,
        _ => s.to_string(),
    }
}

fn skip_ws(c: &[char], at: &mut usize) -> usize {
    while *at < c.len() && c[*at].is_ascii_whitespace() {
        *at += 1;
    }
    *at
}

fn json_value(c: &[char], at: &mut usize) -> Option<String> {
    skip_ws(c, at);
    match *c.get(*at)? {
        '"' => {
            let start = *at;
            *at += 1;
            while *c.get(*at)? != '"' {
                *at += if c[*at] == '\\' { 2 } else { 1 };
            }
            *at += 1;
            Some(c[start..*at].iter().collect())
        }
        '[' => {
            *at += 1;
            let mut items = vec![];
            skip_ws(c, at);
            if *c.get(*at)? == ']' {
                *at += 1;
                return Some("[]".into());
            }
            loop {
                items.push(json_value(c, at)?);
                skip_ws(c, at);
                match *c.get(*at)? {
                    ',' => *at += 1,
                    ']' => {
                        *at += 1;
                        return Some(format!("[{}]", items.join(",")));
                    }
                    _ => return None,
                }
            }
        }
        '{' => {
            *at += 1;
            let mut items: Vec<(String, String)> = vec![];
            skip_ws(c, at);
            if *c.get(*at)? == '}' {
                *at += 1;
                return Some("{}".into());
            }
            loop {
                let key = json_value(c, at)?;
                skip_ws(c, at);
                if *c.get(*at)? != ':' {
                    return None;
                }
                *at += 1;
                items.push((key, json_value(c, at)?));
                skip_ws(c, at);
                match *c.get(*at)? {
                    ',' => *at += 1,
                    '}' => {
                        *at += 1;
                        items.sort();
                        return Some(format!("{{{}}}", items.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(",")));
                    }
                    _ => return None,
                }
            }
        }
        _ => {
            let start = *at;
            while *at < c.len() && !matches!(c[*at], ',' | ']' | '}') && !c[*at].is_ascii_whitespace() {
                *at += 1;
            }
            (*at > start).then(|| c[start..*at].iter().collect())
        }
    }
}

// ---- arrays --------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Tree {
    Null,
    Leaf(String),
    List(Vec<Tree>),
}

/// The Core's array text: `[..]` or `{..}`, elements `null`, a number, a
/// JSON string or a nested list, separated by commas.
fn parse_ours(c: &[char], at: &mut usize) -> Option<Tree> {
    match *c.get(*at)? {
        open @ ('[' | '{') => {
            let close = if open == '[' { ']' } else { '}' };
            *at += 1;
            let mut items = vec![];
            if *c.get(*at)? == close {
                *at += 1;
                return Some(Tree::List(items));
            }
            loop {
                items.push(parse_ours(c, at)?);
                match *c.get(*at)? {
                    ',' => *at += 1,
                    x if x == close => {
                        *at += 1;
                        return Some(Tree::List(items));
                    }
                    _ => return None,
                }
            }
        }
        '"' => {
            *at += 1;
            let mut s = String::new();
            loop {
                match *c.get(*at)? {
                    '"' => {
                        *at += 1;
                        return Some(Tree::Leaf(s));
                    }
                    '\\' => {
                        *at += 1;
                        match *c.get(*at)? {
                            'n' => s.push('\n'),
                            'r' => s.push('\r'),
                            't' => s.push('\t'),
                            'u' => {
                                let hex: String = c.get(*at + 1..*at + 5)?.iter().collect();
                                s.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?).unwrap_or('\u{fffd}'));
                                *at += 4;
                            }
                            other => s.push(other),
                        }
                        *at += 1;
                    }
                    other => {
                        s.push(other);
                        *at += 1;
                    }
                }
            }
        }
        _ => {
            let start = *at;
            while *at < c.len() && !matches!(c[*at], ',' | ']' | '}') {
                *at += 1;
            }
            let token: String = c[start..*at].iter().collect();
            Some(if token == "null" { Tree::Null } else { Tree::Leaf(token) })
        }
    }
}

/// psql's array text: `{..}`, elements `NULL`, a bare word, a quoted string
/// with backslash escapes, or a nested list.
fn parse_theirs(c: &[char], at: &mut usize) -> Option<Tree> {
    match *c.get(*at)? {
        '{' => {
            *at += 1;
            let mut items = vec![];
            if *c.get(*at)? == '}' {
                *at += 1;
                return Some(Tree::List(items));
            }
            loop {
                items.push(parse_theirs(c, at)?);
                match *c.get(*at)? {
                    ',' => *at += 1,
                    '}' => {
                        *at += 1;
                        return Some(Tree::List(items));
                    }
                    _ => return None,
                }
            }
        }
        '"' => {
            *at += 1;
            let mut s = String::new();
            loop {
                match *c.get(*at)? {
                    '"' => {
                        *at += 1;
                        return Some(Tree::Leaf(s));
                    }
                    '\\' => {
                        *at += 1;
                        s.push(*c.get(*at)?);
                        *at += 1;
                    }
                    other => {
                        s.push(other);
                        *at += 1;
                    }
                }
            }
        }
        _ => {
            let start = *at;
            while *at < c.len() && !matches!(c[*at], ',' | '}') {
                *at += 1;
            }
            let token: String = c[start..*at].iter().collect();
            Some(if token == "NULL" { Tree::Null } else { Tree::Leaf(token) })
        }
    }
}

fn arrays_equivalent(element: &str, o: &str, t: &str) -> bool {
    let (oc, tc): (Vec<char>, Vec<char>) = (o.chars().collect(), t.chars().collect());
    let (mut a, mut b) = (0, 0);
    let (Some(ours), Some(theirs)) = (parse_ours(&oc, &mut a), parse_theirs(&tc, &mut b)) else {
        return false;
    };
    a == oc.len() && b == tc.len() && trees_equivalent(element, &ours, &theirs)
}

fn trees_equivalent(element: &str, ours: &Tree, theirs: &Tree) -> bool {
    match (ours, theirs) {
        (Tree::Null, Tree::Null) => true,
        (Tree::Leaf(o), Tree::Leaf(t)) => o == t || equivalent(element, o, t),
        (Tree::List(o), Tree::List(t)) => o.len() == t.len() && o.iter().zip(t).all(|(o, t)| trees_equivalent(element, o, t)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn a_type_the_core_reports_other_than_psqls_is_a_mismatch_and_the_rule_follows_psql() {
        assert_eq!(type_mismatch("numeric", "numeric"), None);
        assert!(type_mismatch("numeric", "float8").unwrap().contains("float8"));
        // Judged by psql's type, lost digits of a numeric are a LOSS even when the Core calls it float8.
        assert_eq!(classify(CellKind::Float, "numeric", &v("0.1"), &v("0.10000000000000000001")), Verdict::Loss);
        assert_eq!(classify(CellKind::Decimal, "float8", &v("0.1"), &v("0.10000000000000000001")), Verdict::Format);
    }

    #[test]
    fn the_rules_tell_a_spelling_from_a_different_value() {
        use Verdict::*;
        let k = CellKind::Other;
        assert_eq!(classify(k, "float8", &v("100000000.0"), &v("100000000")), Format);
        assert_eq!(classify(k, "float4", &v("0.333333"), &v("0.33333334")), Loss, "rounded");
        assert_eq!(classify(k, "float4", &v("999999986991104.0"), &v("1e+15")), Format, "the same float4");
        assert_eq!(classify(k, "float8", &None, &v("NaN")), Loss);
        assert_eq!(classify(k, "bool", &v("true"), &v("t")), Format);
        assert_eq!(classify(k, "bytea", &v("0xdead"), &v("\\xdead")), Format);
        assert_eq!(classify(k, "timestamptz", &v("2024-03-05T11:45:12.123456+00:00"), &v("2024-03-05 11:45:12.123456+00")), Format);
        assert_eq!(classify(k, "timestamptz", &v("2024-03-05T11:45:12+00:00"), &v("2024-03-05 11:45:13+00")), Loss);
        assert_eq!(classify(k, "interval", &v("-1 year 00:00:00"), &v("-1 years")), Format);
        assert_eq!(classify(k, "interval", &v("2 days"), &v("3 days")), Loss);
        assert_eq!(classify(k, "money", &v("-0.01"), &v("-$0.01")), Format);
        assert_eq!(classify(k, "money", &v("-0.02"), &v("-$0.01")), Loss);
        assert_eq!(classify(k, "pg_lsn", &v("0x00000016b374d848"), &v("16/B374D848")), Format);
        assert_eq!(classify(k, "jsonb", &v("{\"a\":1,\"b\":[1,2]}"), &v("{\"b\": [1, 2], \"a\": 1}")), Format);
        assert_eq!(classify(k, "jsonb", &v("{\"a\":1.1}"), &v("{\"a\": 1.10}")), Loss);
        assert_eq!(classify(k, "json", &v("{\"a\":1,\"b\":2}"), &v("{\"b\": 2, \"a\": 1}")), Loss, "json keeps its text: a reordered object is not the same text");
        assert_eq!(classify(k, "json", &v("{\"a\":1}"), &v("{\"a\": 1}")), Loss, "json keeps its whitespace");
        assert_eq!(classify(k, "_json", &v("[\"{\\\"a\\\":1}\"]"), &v("{\"{\\\"a\\\": 1}\"}")), Loss, "the same inside a json[]");
        assert_eq!(classify(k, "_json", &v("[\"{\\\"a\\\": 1}\"]"), &v("{\"{\\\"a\\\": 1}\"}")), Format);
        assert_eq!(classify(k, "_int4", &v("[1,null,3]"), &v("{1,NULL,3}")), Format);
        assert_eq!(classify(k, "_int4", &v("[1,2,3]"), &v("[0:2]={1,2,3}")), Loss, "the lower bound is gone");
        assert_eq!(classify(k, "_text", &v("[\"a\",\"b c\",\"\",null,\"NULL\"]"), &v("{a,\"b c\",\"\",NULL,\"NULL\"}")), Format);
        assert_eq!(classify(k, "_text", &v("[\"a\",null]"), &v("{a,\"NULL\"}")), Loss);
        assert_eq!(classify(k, "_float8", &v("{{1.0,2.0},{3.0,4.0}}"), &v("{{1,2},{3,4}}")), Format);
        assert_eq!(classify(k, "_float8", &v("[null,null]"), &v("{NaN,Infinity}")), Loss);
        assert_eq!(classify(k, "int8", &v("9007199254740992"), &v("9007199254740993")), Loss);
        assert_eq!(classify(k, "text", &v("a"), &v("a ")), Loss);
    }
}

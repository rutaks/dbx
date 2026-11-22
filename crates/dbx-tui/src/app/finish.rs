//! Complete current statement: closes the brackets the statement under the
//! caret leaves open and adds the semicolon it lacks. Pure: lines and a caret
//! in, one insertion out. Brackets inside strings, comments, quoted names and
//! dollar-quoted bodies are not brackets (the highlighter's tokenizer makes those
//! one token); a statement with one of them unterminated is left alone, with a
//! notice.

use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer, Whitespace};

use super::highlight::dialect_of;
use crate::core::{Cursor, Dialect};

/// Text to insert at a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: Cursor,
    pub text: String,
}

fn before(a: Cursor, b: Cursor) -> bool {
    (a.row, a.col) < (b.row, b.col)
}

fn at(l: Location) -> Cursor {
    Cursor { row: (l.line as usize).saturating_sub(1), col: (l.column as usize).saturating_sub(1) }
}

/// What the statement under `caret` lacks, `Ok(None)` when it lacks nothing
/// (or holds nothing), or why the client cannot tell: a string, comment or
/// dollar-quoted body that is not closed. The statement is read with the
/// highlighter's tokens (the dialect's own tokenizer), so escape strings,
/// nested comments and dollar quotes are what the highlighter shows. The
/// insertion goes right after the statement's last token that is not a
/// comment, so a trailing comment stays after it.
pub fn complete_statement(lines: &[String], caret: Cursor, dialect: Option<Dialect>) -> Result<Option<Edit>, &'static str> {
    let text = lines.join("\n");
    let sql_dialect = dialect_of(dialect);
    let mut tokens: Vec<TokenWithSpan> = Vec::new();
    let result = Tokenizer::new(sql_dialect.as_ref(), &text).tokenize_with_location_into_buf(&mut tokens);
    // Where the token that failed starts: the end of the last good one.
    let failed_at = result.is_err().then(|| tokens.last().map_or(Cursor::default(), |t| at(t.span.end)));
    let sig: Vec<&TokenWithSpan> = tokens
        .iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(Whitespace::Space | Whitespace::Newline | Whitespace::Tab | Whitespace::SingleLineComment { .. } | Whitespace::MultiLineComment(_))))
        .collect();
    // The statements: runs of tokens up to and including a `;`.
    let mut statements: Vec<(&[&TokenWithSpan], Option<Cursor>)> = Vec::new();
    let mut from = 0;
    for (i, t) in sig.iter().enumerate() {
        if t.token == Token::SemiColon {
            statements.push((&sig[from..i], Some(at(t.span.end))));
            from = i + 1;
        }
    }
    statements.push((&sig[from..], None));
    // The first statement the caret is not past the end of.
    let chosen = statements.iter().position(|(_, end)| end.is_none_or(|e| !before(e, caret))).unwrap_or(statements.len() - 1);
    // What did not tokenize is the end of the buffer: it is in the last
    // statement, and then nothing is known about how it ends.
    if let (Some(start), true) = (failed_at, chosen == statements.len() - 1) {
        let rest: String = lines.get(start.row).map(|l| l.chars().skip(start.col).collect()).unwrap_or_default();
        return Err(if rest.starts_with("/*") {
            "the statement is not finished: it has a comment that is not closed"
        } else if rest.starts_with('$') {
            "the statement is not finished: it has a dollar-quoted body that is not closed"
        } else if rest.starts_with('"') || rest.starts_with('`') {
            "the statement is not finished: it has a quoted name that is not closed"
        } else if rest.contains('\'') {
            "the statement is not finished: it has a string that is not closed"
        } else {
            "the statement is not finished: it has text the tokenizer refuses"
        });
    }
    let (stmt, semi) = &statements[chosen];
    let Some(last) = stmt.last() else { return Ok(None) };
    let mut open = 0usize;
    for t in stmt.iter() {
        match t.token {
            Token::LParen => open += 1,
            Token::RParen => open = open.saturating_sub(1),
            _ => {}
        }
    }
    let mut text = ")".repeat(open);
    if semi.is_none() {
        text.push(';');
    }
    Ok((!text.is_empty()).then_some(Edit { at: at(last.span.end), text }))
}

impl super::App {
    /// Closes the statement under the caret: one undo step, nothing when
    /// there is nothing to add.
    pub(super) fn finish_statement(&mut self, fx: &mut super::Effects) {
        let dialect = self.active_dialect();
        let edit = match complete_statement(self.ui.editor.lines(), self.ui.editor.cursor(), dialect) {
            Ok(Some(edit)) => edit,
            Ok(None) => return,
            Err(why) => return self.notify(format!("Nothing was added: {why}."), true, fx),
        };
        match self.ui.editor.replace(edit.at, edit.at, &edit.text) {
            Ok(_) => fx.repaint = true,
            Err(why) => self.notify(format!("Could not complete the statement: {why}."), true, fx),
        }
    }
}

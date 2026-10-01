//! The text of a script, as the parser reads it.

use wast::lexer::Lexer;
use wast::parser::{ParseBuffer, Result};
use wast::token::Span;

/// The parse buffer of the script `source`.
///
/// The lexer refuses a character that can make source code read other than
/// it runs, such as a change of the direction of the text, which is right
/// for code a person writes. `names.wast` puts such characters in the names
/// of exports on purpose, so the runner takes them.
pub fn buffer(source: &str) -> Result<ParseBuffer<'_>> {
    let mut lexer = Lexer::new(source);
    lexer.allow_confusing_unicode(true);
    ParseBuffer::new_with_lexer(lexer)
}

/// The line of `span` in `source`, counted from one.
pub fn line_of(source: &str, span: Span) -> usize {
    span.linecol_in(source).0 + 1
}

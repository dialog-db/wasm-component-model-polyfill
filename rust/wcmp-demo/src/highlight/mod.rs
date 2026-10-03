// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The highlighting of Zena source in the shelf's editor.
//!
//! It is a port of the Zena playground's highlighter, the CodeMirror
//! stream language in `packages/codemirror/src/cm-lang-zena.ts` of Zena's
//! repository: the same keyword sets, the same order of rules, and the
//! same categories. It runs over the whole source at once, carrying the
//! state the playground carries from line to line: an open block
//! comment, an open string, and how deep the template substitutions go.
//! One rule differs: inside a string, `//` and `/*` are the string's, as
//! they are to Zena, where the playground starts a comment.
//!
//! It runs on every keystroke, on the page, so it is a scan and not a
//! parse: it knows no more of the program than its characters.

mod kind;
mod token;

pub use kind::Kind;
pub use token::Token;

/// Keywords of control flow.
const CONTROL: &[&str] = &[
    "if", "else", "while", "for", "in", "return", "match", "case", "throw", "try", "catch",
    "finally", "break", "continue", "yield", "await",
];

/// Keywords that declare.
const DEFINITION: &[&str] = &[
    "let",
    "var",
    "function",
    "class",
    "sealed",
    "interface",
    "mixin",
    "enum",
    "type",
    "distinct",
    "opaque",
    "extension",
    "symbol",
    "using",
];

/// Keywords of modules.
const MODULE: &[&str] = &["export", "import", "from", "declare"];

/// Modifiers.
const MODIFIER: &[&str] = &["async", "gen", "static", "final", "abstract", "inline"];

/// Keywords that are operators.
const OPERATOR_KEYWORD: &[&str] = &["new", "as", "is", "extends", "implements", "with", "on"];

/// The built-in types whose names start in lower case. Every other name
/// with a leading capital is a type too.
const PRIMITIVE: &[&str] = &[
    "i32", "i64", "u32", "u64", "i8", "i16", "u8", "u16", "f32", "f64", "v128", "boolean", "void",
    "never", "anyref",
];

/// Operators of more than one character, longest first within each of
/// the playground's groups, in its order.
const OPERATORS: &[&str] = &[
    "|>", "??", "?.", "?(", "?[", "=>", "!==", "==", "!=", "<=", ">=", "**=", "<<=", ">>=", "+=",
    "-=", "*=", "/=", "%=", "&=", "|=", "^=", "**", "&&", "||", "...",
];

/// Operators of one character.
const SINGLE_OPERATORS: &[u8] = b"<>+-*/%!=|&^~";

/// Punctuation.
const PUNCTUATION: &[u8] = b"()[]{}:;,.";

/// The tokens of `source`, in order. A run of characters no rule names,
/// such as white space, is in no token.
pub fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    let mut push = |start: usize, end: usize, kind: Kind| {
        // A run of one kind is one token: a comment or a string is
        // scanned a character at a time.
        if let Some(last) = tokens.last_mut()
            && last.kind == kind
            && last.end == start
            && matches!(kind, Kind::Comment | Kind::String)
        {
            last.end = end;
            return;
        }
        tokens.push(Token { start, end, kind });
    };
    let mut at = 0;
    let mut block_comment = false;
    let mut string: Option<u8> = None;
    let mut template_depth = 0usize;
    while at < bytes.len() {
        let rest = &bytes[at..];
        let next_char = char_length(source, at);

        if block_comment {
            if rest.starts_with(b"*/") {
                block_comment = false;
                push(at, at + 2, Kind::Comment);
                at += 2;
            } else {
                push(at, at + next_char, Kind::Comment);
                at += next_char;
            }
            continue;
        }

        if let Some(quote) = string {
            if quote == b'`' && rest.starts_with(b"${") {
                template_depth += 1;
                string = None;
                push(at, at + 2, Kind::Punctuation);
                at += 2;
            } else if rest[0] == quote {
                string = None;
                push(at, at + 1, Kind::String);
                at += 1;
            } else if rest[0] == b'\\' && rest.len() > 1 {
                let escaped = 1 + char_length(source, at + 1);
                push(at, at + escaped, Kind::Escape);
                at += escaped;
            } else {
                push(at, at + next_char, Kind::String);
                at += next_char;
            }
            continue;
        }

        if rest.starts_with(b"/*") {
            block_comment = true;
            push(at, at + 2, Kind::Comment);
            at += 2;
            continue;
        }
        if rest.starts_with(b"//") {
            let end = rest
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |line| at + line);
            push(at, end, Kind::Comment);
            at = end;
            continue;
        }

        if matches!(rest[0], b'\'' | b'"' | b'`') {
            string = Some(rest[0]);
            push(at, at + 1, Kind::String);
            at += 1;
            continue;
        }

        if template_depth > 0 && rest[0] == b'}' {
            template_depth -= 1;
            string = Some(b'`');
            push(at, at + 1, Kind::Punctuation);
            at += 1;
            continue;
        }

        if rest[0] == b'@' && rest.len() > 1 && is_identifier_start(rest[1]) {
            let end = at + 1 + identifier_length(&rest[1..]);
            push(at, end, Kind::Meta);
            at = end;
            continue;
        }

        if let Some(length) = number_length(rest) {
            push(at, at + length, Kind::Number);
            at += length;
            continue;
        }

        if let Some(length) = operator_length(rest) {
            push(at, at + length, Kind::Operator);
            at += length;
            continue;
        }

        if rest[0] == b'#' && rest.len() > 1 && is_identifier_start(rest[1]) {
            let end = at + 1 + identifier_length(&rest[1..]);
            push(at, end, Kind::Property);
            at = end;
            continue;
        }

        if is_identifier_start(rest[0]) {
            let length = identifier_length(rest);
            let word = &source[at..at + length];
            push(at, at + length, word_kind(word, &rest[length..]));
            at += length;
            continue;
        }

        if PUNCTUATION.contains(&rest[0]) {
            push(at, at + 1, Kind::Punctuation);
            at += 1;
            continue;
        }

        at += next_char;
    }
    tokens
}

/// `source` as HTML: each token in a `span` whose class names its kind,
/// and every other character as it is, escaped.
pub fn html(source: &str) -> String {
    let mut out = String::with_capacity(source.len() * 2);
    let mut at = 0;
    for token in tokenize(source) {
        escape_into(&mut out, &source[at..token.start]);
        out.push_str("<span class=\"");
        out.push_str(token.kind.class());
        out.push_str("\">");
        escape_into(&mut out, &source[token.start..token.end]);
        out.push_str("</span>");
        at = token.end;
    }
    escape_into(&mut out, &source[at..]);
    out
}

/// Append `text` to `out`, with the characters HTML gives a meaning
/// escaped.
fn escape_into(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

/// The kind of the word `word`, which `after` follows.
fn word_kind(word: &str, after: &[u8]) -> Kind {
    // `tail` is a keyword only right before `return`.
    if word == "tail" && is_return_next(after) {
        return Kind::Control;
    }
    if CONTROL.contains(&word) {
        Kind::Control
    } else if DEFINITION.contains(&word) {
        Kind::Definition
    } else if MODULE.contains(&word) {
        Kind::Module
    } else if MODIFIER.contains(&word) {
        Kind::Modifier
    } else if OPERATOR_KEYWORD.contains(&word) {
        Kind::OperatorKeyword
    } else if word == "this" || word == "super" {
        Kind::SelfKeyword
    } else if word == "null" {
        Kind::Null
    } else if word == "true" || word == "false" {
        Kind::Bool
    } else if PRIMITIVE.contains(&word) || word.starts_with(|c: char| c.is_ascii_uppercase()) {
        Kind::Type
    } else {
        Kind::Variable
    }
}

/// Whether `after` is white space and then the word `return`.
fn is_return_next(after: &[u8]) -> bool {
    let spaces = after
        .iter()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    let rest = &after[spaces..];
    spaces > 0
        && rest.starts_with(b"return")
        && rest.get(6).is_none_or(|byte| !is_identifier_part(*byte))
}

/// The length of the number `rest` starts with: hexadecimal, a decimal
/// with a fraction, or an integer.
fn number_length(rest: &[u8]) -> Option<usize> {
    if rest.len() > 2
        && rest[0] == b'0'
        && matches!(rest[1], b'x' | b'X')
        && rest[2].is_ascii_hexdigit()
    {
        return Some(
            2 + rest[2..]
                .iter()
                .take_while(|byte| byte.is_ascii_hexdigit())
                .count(),
        );
    }
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    if rest.get(digits) == Some(&b'.') {
        let fraction = rest[digits + 1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if fraction > 0 {
            return Some(digits + 1 + fraction);
        }
    }
    Some(digits)
}

/// The length of the operator `rest` starts with.
fn operator_length(rest: &[u8]) -> Option<usize> {
    OPERATORS
        .iter()
        .find(|operator| rest.starts_with(operator.as_bytes()))
        .map(|operator| operator.len())
        .or_else(|| SINGLE_OPERATORS.contains(&rest[0]).then_some(1))
}

/// Whether `byte` can start a name.
fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

/// Whether `byte` can continue a name.
fn is_identifier_part(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

/// The length of the name `rest` starts with.
fn identifier_length(rest: &[u8]) -> usize {
    rest.iter()
        .take_while(|byte| is_identifier_part(**byte))
        .count()
}

/// The length in bytes of the character at byte `at` of `source`.
fn char_length(source: &str, at: usize) -> usize {
    source[at..].chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text and kind of each token of `source`.
    fn kinds(source: &str) -> Vec<(&str, Kind)> {
        tokenize(source)
            .into_iter()
            .map(|token| (&source[token.start..token.end], token.kind))
            .collect()
    }

    #[wcmp_macros::test]
    fn it_names_keywords_types_names_and_numbers() {
        assert_eq!(
            kinds("export let count: i32 = new Item(0x1F, 2.5);"),
            [
                ("export", Kind::Module),
                ("let", Kind::Definition),
                ("count", Kind::Variable),
                (":", Kind::Punctuation),
                ("i32", Kind::Type),
                ("=", Kind::Operator),
                ("new", Kind::OperatorKeyword),
                ("Item", Kind::Type),
                ("(", Kind::Punctuation),
                ("0x1F", Kind::Number),
                (",", Kind::Punctuation),
                ("2.5", Kind::Number),
                (")", Kind::Punctuation),
                (";", Kind::Punctuation),
            ]
        );
    }

    #[wcmp_macros::test]
    fn it_keeps_comments_and_strings_across_lines() {
        assert_eq!(
            kinds("/* one\ntwo */ x // end\n'a // b\\n'"),
            [
                ("/* one\ntwo */", Kind::Comment),
                ("x", Kind::Variable),
                ("// end", Kind::Comment),
                ("'a // b", Kind::String),
                ("\\n", Kind::Escape),
                ("'", Kind::String),
            ]
        );
    }

    #[wcmp_macros::test]
    fn it_highlights_template_substitutions_as_code() {
        assert_eq!(
            kinds("`a ${this.#b} c`"),
            [
                ("`a ", Kind::String),
                ("${", Kind::Punctuation),
                ("this", Kind::SelfKeyword),
                (".", Kind::Punctuation),
                ("#b", Kind::Property),
                ("}", Kind::Punctuation),
                (" c`", Kind::String),
            ]
        );
    }

    #[wcmp_macros::test]
    fn it_takes_tail_for_a_keyword_only_before_return() {
        assert_eq!(kinds("tail return")[0], ("tail", Kind::Control));
        assert_eq!(kinds("tail + 1")[0], ("tail", Kind::Variable));
    }

    #[wcmp_macros::test]
    fn it_escapes_html_and_wraps_each_token() {
        assert_eq!(
            html("a < b // é"),
            "<span class=\"tok-variable\">a</span> <span class=\"tok-operator\">&lt;</span> \
             <span class=\"tok-variable\">b</span> <span class=\"tok-comment\">// é</span>"
        );
    }
}

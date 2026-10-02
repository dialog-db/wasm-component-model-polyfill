// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The line-oriented text format that expectations and observations
//! share. The crate documentation describes it.

use crate::call::Call;
use crate::entry::Entry;
use crate::error::{Error, Result};
use crate::outcome::Outcome;
use crate::stage::Stage;
use crate::subject::Subject;
use crate::value::Value;
use crate::verdict::Verdict;

/// Every line of a file, read but not yet checked against what an
/// expectations file or an observations file allows.
pub struct Document {
    /// The `stage` line, with the number of the line it is on.
    pub stage: Option<(usize, Verdict)>,
    /// The `call` lines, each with the number of the line it is on.
    pub entries: Vec<(usize, Entry)>,
    /// The `output` lines, in order.
    pub output: Vec<String>,
}

impl Document {
    /// Read every line of `text`.
    pub fn parse(text: &str) -> Result<Self> {
        let mut document = Document {
            stage: None,
            entries: Vec::new(),
            output: Vec::new(),
        };
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fail = |reason: String| Error::Syntax {
                line: number,
                reason,
            };
            let mut cursor = Cursor { rest: line };
            match cursor.word().as_str() {
                "call" => {
                    let entry = cursor.entry().map_err(fail)?;
                    document.entries.push((number, entry));
                }
                "output" => {
                    let text = cursor.string().map_err(fail)?;
                    cursor.end().map_err(fail)?;
                    document.output.push(text);
                }
                "stage" => {
                    if document.stage.is_some() {
                        return Err(fail("a second `stage` line".to_string()));
                    }
                    let verdict = cursor.verdict().map_err(fail)?;
                    document.stage = Some((number, verdict));
                }
                word => {
                    return Err(fail(format!(
                        "`{word}` is not a keyword (`call`, `output`, or `stage`)"
                    )));
                }
            }
        }
        Ok(document)
    }
}

/// `text` in `delimiter` quotes, with the escapes the format reads back.
pub fn quote(text: &str, delimiter: char) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push(delimiter);
    for character in text.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\0' => quoted.push_str("\\0"),
            character if character == delimiter => {
                quoted.push('\\');
                quoted.push(character);
            }
            character if character.is_control() => {
                quoted.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
            }
            character => quoted.push(character),
        }
    }
    quoted.push(delimiter);
    quoted
}

/// A report line: `<scenario> <subject> <stage> ["<reason>"]`.
pub fn report(line: &str) -> core::result::Result<(String, Subject, Verdict), String> {
    let mut cursor = Cursor { rest: line };
    let scenario = cursor.word();
    if scenario.is_empty() {
        return Err("expected a scenario, found the end of the line".to_string());
    }
    let subject = cursor
        .word()
        .parse()
        .map_err(|error: Error| error.to_string())?;
    let verdict = cursor.verdict()?;
    Ok((scenario, subject, verdict))
}

/// The unread part of one line. Each method reads one piece of the
/// line and says what it expected when the piece is not there.
struct Cursor<'a> {
    rest: &'a str,
}

impl Cursor<'_> {
    fn skip_blanks(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn peek(&self) -> Option<char> {
        self.rest.chars().next()
    }

    /// Consume `token` if the line continues with it, after blanks.
    fn eat(&mut self, token: &str) -> bool {
        self.skip_blanks();
        match self.rest.strip_prefix(token) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn expect(&mut self, token: &str, what: &str) -> core::result::Result<(), String> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(format!("expected {what}, found {}", self.found()))
        }
    }

    fn found(&self) -> String {
        match self.rest.trim_start() {
            "" => "the end of the line".to_string(),
            rest => format!("`{rest}`"),
        }
    }

    /// The characters up to the next blank, or up to `stop`.
    fn take_until(&mut self, stop: impl Fn(char) -> bool) -> &str {
        self.skip_blanks();
        let end = self
            .rest
            .find(|character: char| character.is_whitespace() || stop(character))
            .unwrap_or(self.rest.len());
        let (taken, rest) = self.rest.split_at(end);
        self.rest = rest;
        taken
    }

    fn word(&mut self) -> String {
        self.take_until(|_| false).to_string()
    }

    fn end(&mut self) -> core::result::Result<(), String> {
        self.skip_blanks();
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "expected the end of the line, found {}",
                self.found()
            ))
        }
    }

    /// `[typed] <component> <export>(<arguments>) [-> <outcome>]`
    fn entry(&mut self) -> core::result::Result<Entry, String> {
        let mut component = self.word();
        let typed = component == "typed";
        if typed {
            component = self.word();
        }
        if component.is_empty() {
            return Err("expected a component name after `call`".to_string());
        }
        let export = self.take_until(|character| character == '(').to_string();
        if export.is_empty() {
            return Err(format!(
                "expected an export name after `{component}`, found {}",
                self.found()
            ));
        }
        self.expect("(", "`(` after the export name")?;
        let arguments = self.values(")")?;
        let outcome = if self.eat("->") {
            Some(self.outcome()?)
        } else {
            None
        };
        self.end()?;
        Ok(Entry {
            call: Call {
                component,
                export,
                arguments,
                typed,
            },
            outcome,
        })
    }

    /// `fail ["<message>"]`, `(<values>)`, or one value.
    fn outcome(&mut self) -> core::result::Result<Outcome, String> {
        let before = self.rest;
        if self.word() == "fail" {
            self.skip_blanks();
            let message = if self.peek() == Some('"') {
                self.string()?
            } else {
                String::new()
            };
            return Ok(Outcome::Failure(message));
        }
        self.rest = before;
        if self.eat("(") {
            return Ok(Outcome::Results(self.values(")")?));
        }
        Ok(Outcome::Results(vec![self.value()?]))
    }

    /// `<stage> ["<reason>"]`
    fn verdict(&mut self) -> core::result::Result<Verdict, String> {
        let name = self.word();
        let stage: Stage = name.parse().map_err(|error: Error| error.to_string())?;
        self.skip_blanks();
        let reason = if self.peek() == Some('"') {
            self.string()?
        } else {
            String::new()
        };
        self.end()?;
        Ok(Verdict::new(stage, reason))
    }

    /// Values separated by commas, up to and including `close`.
    fn values(&mut self, close: &str) -> core::result::Result<Vec<Value>, String> {
        let mut values = Vec::new();
        if self.eat(close) {
            return Ok(values);
        }
        loop {
            values.push(self.value()?);
            if self.eat(close) {
                return Ok(values);
            }
            self.expect(",", &format!("`,` or `{close}`"))?;
        }
    }

    fn value(&mut self) -> core::result::Result<Value, String> {
        self.skip_blanks();
        match self.peek() {
            Some('"') => return Ok(Value::String(self.string()?)),
            Some('\'') => {
                let text = self.quoted('\'')?;
                let mut characters = text.chars();
                return match (characters.next(), characters.next()) {
                    (Some(character), None) => Ok(Value::Char(character)),
                    _ => Err(format!("{} is not one character", quote(&text, '\''))),
                };
            }
            _ => {}
        }
        let token = self.take_until(|character| character == ',' || character == ')');
        match token {
            "" => return Err(format!("expected a value, found {}", self.found())),
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            _ => {}
        }
        number(token)
    }

    fn string(&mut self) -> core::result::Result<String, String> {
        self.skip_blanks();
        if self.peek() != Some('"') {
            return Err(format!("expected a quoted string, found {}", self.found()));
        }
        self.quoted('"')
    }

    /// The text between two `delimiter`s, with its escapes resolved.
    fn quoted(&mut self, delimiter: char) -> core::result::Result<String, String> {
        let mut characters = self.rest.char_indices();
        characters.next();
        let mut text = String::new();
        while let Some((index, character)) = characters.next() {
            match character {
                '\\' => {
                    let escaped = match characters.next().map(|(_, escaped)| escaped) {
                        Some('\\') => '\\',
                        Some('"') => '"',
                        Some('\'') => '\'',
                        Some('n') => '\n',
                        Some('r') => '\r',
                        Some('t') => '\t',
                        Some('0') => '\0',
                        Some('u') => {
                            let rest = characters.as_str();
                            let digits = rest
                                .strip_prefix('{')
                                .and_then(|rest| rest.split_once('}'))
                                .map(|(digits, _)| digits)
                                .ok_or("expected `{` and `}` around the digits of `\\u`")?;
                            // `{`, the digits, and `}`.
                            for _ in 0..digits.len() + 2 {
                                characters.next();
                            }
                            u32::from_str_radix(digits, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or_else(|| format!("`\\u{{{digits}}}` is not a character"))?
                        }
                        Some(other) => return Err(format!("`\\{other}` is not an escape")),
                        None => break,
                    };
                    text.push(escaped);
                }
                character if character == delimiter => {
                    self.rest = &self.rest[index + character.len_utf8()..];
                    return Ok(text);
                }
                character => text.push(character),
            }
        }
        Err(format!("expected a closing `{delimiter}`"))
    }
}

/// A number with its type as a suffix, such as `-3s32` or `1.5f64`.
fn number(token: &str) -> core::result::Result<Value, String> {
    fn parse<T: core::str::FromStr>(
        digits: &str,
        token: &str,
        wrap: fn(T) -> Value,
    ) -> core::result::Result<Value, String> {
        digits
            .parse()
            .map(wrap)
            .map_err(|_| format!("`{token}` is not a value of its type"))
    }
    const SUFFIXES: [&str; 10] = [
        "s8", "u8", "s16", "u16", "s32", "u32", "s64", "u64", "f32", "f64",
    ];
    let (digits, suffix) = SUFFIXES
        .into_iter()
        .find_map(|suffix| Some((token.strip_suffix(suffix)?, suffix)))
        .unwrap_or((token, ""));
    match suffix {
        "s8" => parse(digits, token, Value::S8),
        "u8" => parse(digits, token, Value::U8),
        "s16" => parse(digits, token, Value::S16),
        "u16" => parse(digits, token, Value::U16),
        "s32" => parse(digits, token, Value::S32),
        "u32" => parse(digits, token, Value::U32),
        "s64" => parse(digits, token, Value::S64),
        "u64" => parse(digits, token, Value::U64),
        "f32" => parse(digits, token, Value::F32),
        "f64" => parse(digits, token, Value::F64),
        _ => Err(format!(
            "`{token}` is not a value: a number needs its type as a suffix, such as `1u32`"
        )),
    }
}

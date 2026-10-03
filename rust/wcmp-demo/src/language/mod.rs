// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What Zena's language service tells the shelf's editor, as the
//! compiler component's `language` interface answers it: diagnostics,
//! hover, completions, and the location of a definition. The service is
//! Zena's own, compiled into the compiler component; this module reads
//! its records out of the component's values.

mod completion;
mod diagnostic;
mod hover;
mod location;
mod severity;

pub use completion::Completion;
pub use diagnostic::Diagnostic;
pub use hover::Hover;
pub use location::Location;
pub use severity::Severity;

use wcmp::{Error, Val};

/// The diagnostics of a `list<diagnostic>`.
///
/// # Errors
///
/// [`Error::Internal`] when the value is not of that type.
pub fn diagnostics_of(value: &Val) -> Result<Vec<Diagnostic>, Error> {
    list_of(value)?
        .iter()
        .map(|item| {
            let field = Fields::of(item)?;
            Ok(Diagnostic {
                file: field.string("file")?,
                start: field.u32("start")?,
                length: field.u32("length")?,
                line: field.u32("line")?,
                column: field.u32("column")?,
                severity: match field.get("severity")? {
                    Val::Enum(case) if case == "error" => Severity::Error,
                    Val::Enum(case) if case == "warning" => Severity::Warning,
                    Val::Enum(_) => Severity::Information,
                    other => return Err(unexpected(other)),
                },
                message: field.string("message")?,
            })
        })
        .collect()
}

/// The hover of an `option<hover-info>`.
///
/// # Errors
///
/// [`Error::Internal`] when the value is not of that type.
pub fn hover_of(value: &Val) -> Result<Option<Hover>, Error> {
    let Val::Option(found) = value else {
        return Err(unexpected(value));
    };
    found
        .as_deref()
        .map(|found| {
            let field = Fields::of(found)?;
            Ok(Hover {
                label: field.string("label")?,
                detail: field.string("detail")?,
                doc: field.string("doc")?,
            })
        })
        .transpose()
}

/// The completions of a `list<completion>`.
///
/// # Errors
///
/// [`Error::Internal`] when the value is not of that type.
pub fn completions_of(value: &Val) -> Result<Vec<Completion>, Error> {
    list_of(value)?
        .iter()
        .map(|item| {
            let field = Fields::of(item)?;
            Ok(Completion {
                label: field.string("label")?,
                kind: field.u32("kind")?,
                detail: field.string("detail")?,
                doc: field.string("doc")?,
            })
        })
        .collect()
}

/// The location of an `option<location>`.
///
/// # Errors
///
/// [`Error::Internal`] when the value is not of that type.
pub fn location_of(value: &Val) -> Result<Option<Location>, Error> {
    let Val::Option(found) = value else {
        return Err(unexpected(value));
    };
    found
        .as_deref()
        .map(|found| {
            let field = Fields::of(found)?;
            Ok(Location {
                file: field.string("file")?,
                start: field.u32("start")?,
                length: field.u32("length")?,
                line: field.u32("line")?,
                column: field.u32("column")?,
            })
        })
        .transpose()
}

/// The text of a `result<string, string>`: the formatted source, or the
/// formatter's error.
///
/// # Errors
///
/// [`Error::Internal`] when the value is not of that type.
pub fn formatted_of(value: &Val) -> Result<Result<String, String>, Error> {
    let text = |value: Option<&Val>| match value {
        Some(Val::String(text)) => Ok(text.clone()),
        Some(other) => Err(unexpected(other)),
        None => Err(Error::Internal {
            message: "the formatter answered no text".to_string(),
        }),
    };
    match value {
        Val::Result(Ok(found)) => Ok(Ok(text(found.as_deref())?)),
        Val::Result(Err(found)) => Ok(Err(text(found.as_deref())?)),
        other => Err(unexpected(other)),
    }
}

/// The fields of a record, by name.
struct Fields<'a>(&'a [wcmp::ValField]);

impl<'a> Fields<'a> {
    /// The fields of the record `value`.
    fn of(value: &'a Val) -> Result<Self, Error> {
        match value {
            Val::Record(fields) => Ok(Fields(fields)),
            other => Err(unexpected(other)),
        }
    }

    /// The value of the field `name`.
    fn get(&self, name: &str) -> Result<&'a Val, Error> {
        self.0
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
            .ok_or_else(|| Error::Internal {
                message: format!("the language service answered a record with no `{name}`"),
            })
    }

    /// The string field `name`.
    fn string(&self, name: &str) -> Result<String, Error> {
        match self.get(name)? {
            Val::String(text) => Ok(text.clone()),
            other => Err(unexpected(other)),
        }
    }

    /// The `u32` field `name`.
    fn u32(&self, name: &str) -> Result<u32, Error> {
        match self.get(name)? {
            Val::U32(number) => Ok(*number),
            other => Err(unexpected(other)),
        }
    }
}

/// The items of a list.
fn list_of(value: &Val) -> Result<&[Val], Error> {
    match value {
        Val::List(items) => Ok(items),
        other => Err(unexpected(other)),
    }
}

/// The error for a value of the wrong shape in an answer of the language
/// service.
fn unexpected(value: &Val) -> Error {
    Error::Internal {
        message: format!("the language service answered an unexpected value {value:?}"),
    }
}

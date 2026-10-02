// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How a call ended.

use core::fmt;

use crate::syntax::quote;
use crate::value::Value;

/// How a call ended, or must end: with results, with a component, with
/// an error the call returned, or with a failure.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The call returned these results, in order.
    Results(Vec<Value>),
    /// The call returned `ok` with the bytes of a component. When the
    /// outcome names the component, the runner parses, links, and
    /// instantiates it under that name, and later calls name it like any
    /// other. A component with no name is held to its bytes only.
    ///
    /// The expectations name the component only. A run also holds the
    /// digest of the bytes, so that a polyfill subject is held to the
    /// bytes the Wasmtime run's call returned without the observations
    /// holding a whole component.
    Component {
        /// The name later calls give the component, or `None` for a
        /// component the runner does not load.
        name: Option<String>,
        /// The SHA-256 digest of the bytes, as `sha256:` and hex digits,
        /// when a run observed them.
        digest: Option<String>,
    },
    /// The call returned `err` with this text, such as a compiler's
    /// diagnostics.
    Error {
        /// The text, or a part of it.
        text: String,
        /// Whether `text` is only a part of the text: an expectation
        /// can ask that the text contain it, where a run holds the whole
        /// text.
        partial: bool,
    },
    /// The call failed, for example with a trap or an error the runtime
    /// returned. The message is for a person only, and may be empty;
    /// two failures are equal whatever their messages say.
    Failure(String),
}

impl Outcome {
    /// The outcome of a call that returned the bytes `bytes` of the
    /// component `name`, or of a component it does not load.
    pub fn component(name: Option<&str>, bytes: &[u8]) -> Self {
        Outcome::Component {
            name: name.map(str::to_string),
            digest: Some(digest(bytes)),
        }
    }

    /// Whether this is a failure.
    pub fn is_failure(&self) -> bool {
        matches!(self, Outcome::Failure(_))
    }
}

/// The SHA-256 digest of `bytes`, as `sha256:` and hex digits.
pub fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(bytes);
    let mut text = String::with_capacity(7 + 2 * hash.len());
    text.push_str("sha256:");
    for byte in hash {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// The equality of outcomes is a match between an expectation and an
/// observation, not an identity: an expectation with no digest, or with
/// part of an error's text, matches more than one observation. So it is
/// not transitive, and an outcome is no key for a map or a set.
impl PartialEq for Outcome {
    /// Results are equal when every value is. Components are equal when
    /// their names are, and their digests too when both have one. Errors
    /// are equal when their texts are, or when one is a part of the
    /// other's whole text. Two failures are always equal.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Outcome::Results(a), Outcome::Results(b)) => a == b,
            (
                Outcome::Component {
                    name: a,
                    digest: digest_a,
                },
                Outcome::Component {
                    name: b,
                    digest: digest_b,
                },
            ) => {
                a == b
                    && match (digest_a, digest_b) {
                        (Some(digest_a), Some(digest_b)) => digest_a == digest_b,
                        _ => true,
                    }
            }
            (
                Outcome::Error {
                    text: a,
                    partial: partial_a,
                },
                Outcome::Error {
                    text: b,
                    partial: partial_b,
                },
            ) => match (partial_a, partial_b) {
                (false, false) => a == b,
                (true, false) => b.contains(a.as_str()),
                (false, true) => a.contains(b.as_str()),
                (true, true) => a == b,
            },
            (Outcome::Failure(_), Outcome::Failure(_)) => true,
            _ => false,
        }
    }
}

impl Eq for Outcome {}

impl fmt::Display for Outcome {
    /// The outcome as it follows the `->` of a `call` line.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Results(results) if results.len() == 1 => write!(formatter, "{}", results[0]),
            Outcome::Results(results) => {
                formatter.write_str("(")?;
                for (index, result) in results.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{result}")?;
                }
                formatter.write_str(")")
            }
            Outcome::Component { name, digest } => {
                formatter.write_str("component")?;
                if let Some(name) = name {
                    write!(formatter, " {name}")?;
                }
                match digest {
                    Some(digest) => write!(formatter, " {digest}"),
                    None => Ok(()),
                }
            }
            Outcome::Error { text, partial } => {
                formatter.write_str("err ")?;
                if *partial {
                    formatter.write_str("containing ")?;
                }
                formatter.write_str(&quote(text, '"'))
            }
            Outcome::Failure(message) if message.is_empty() => formatter.write_str("fail"),
            Outcome::Failure(message) => write!(formatter, "fail {}", quote(message, '"')),
        }
    }
}

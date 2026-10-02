// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How a call ended.

use core::fmt;

use crate::syntax::quote;
use crate::value::Value;

/// How a call ended, or must end: with results, or with a failure.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The call returned these results, in order.
    Results(Vec<Value>),
    /// The call failed, for example with a trap or an error the runtime
    /// returned. The message is for a person only, and may be empty;
    /// two failures are equal whatever their messages say.
    Failure(String),
}

impl PartialEq for Outcome {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Outcome::Results(a), Outcome::Results(b)) => a == b,
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
            Outcome::Failure(message) if message.is_empty() => formatter.write_str("fail"),
            Outcome::Failure(message) => write!(formatter, "fail {}", quote(message, '"')),
        }
    }
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One call a scenario makes.

use core::fmt;

use crate::value::Value;

/// One call a scenario makes: which export of which component, with
/// which arguments, and whether the call is typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The component the export belongs to, as the scenario names it.
    pub component: String,
    /// The export's name, such as `add` or `local:demo/api#greet`.
    pub export: String,
    /// The arguments, in order.
    pub arguments: Vec<Value>,
    /// Whether the call goes through a typed function, whose Rust types
    /// the runner fixes at compile time, rather than an untyped one.
    pub typed: bool,
}

impl fmt::Display for Call {
    /// The call as a `call` line spells it, without the keyword.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.typed {
            formatter.write_str("typed ")?;
        }
        write!(formatter, "{} {}(", self.component, self.export)?;
        for (index, argument) in self.arguments.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{argument}")?;
        }
        formatter.write_str(")")
    }
}

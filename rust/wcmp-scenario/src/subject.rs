// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Which run of a scenario a stage belongs to.

use core::fmt;
use core::str::FromStr;

use crate::error::Error;

/// One run of a scenario: the Wasmtime run, or the polyfill in the
/// browser or natively, over the Wasmtime or the Wasmi backend of its
/// runtime layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    /// The Wasmtime run, which goes first and sets the behavior the
    /// polyfill must match.
    Wasmtime,
    /// The polyfill in the browser.
    Web,
    /// The polyfill natively, over the Wasmtime backend.
    Native,
    /// The polyfill natively, over the Wasmi backend.
    Wasmi,
}

impl Subject {
    /// Every subject, in the order a scenario meets them.
    pub const ALL: [Subject; 4] = [
        Subject::Wasmtime,
        Subject::Web,
        Subject::Native,
        Subject::Wasmi,
    ];

    /// The polyfill subject of the target this code was built for, over
    /// the target's default backend: [`Subject::Web`] on `wasm32`, and
    /// [`Subject::Native`] elsewhere.
    pub const fn polyfill() -> Self {
        if cfg!(target_arch = "wasm32") {
            Subject::Web
        } else {
            Subject::Native
        }
    }

    /// The subject's name, as the files spell it.
    pub fn name(self) -> &'static str {
        match self {
            Subject::Wasmtime => "wasmtime",
            Subject::Web => "web",
            Subject::Native => "native",
            Subject::Wasmi => "wasmi",
        }
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for Subject {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Subject::ALL
            .into_iter()
            .find(|subject| subject.name() == text)
            .ok_or_else(|| Error::UnknownSubject(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_back_every_subject_it_names() {
        for subject in Subject::ALL {
            assert_eq!(subject.to_string().parse::<Subject>(), Ok(subject));
        }
        assert_eq!(
            "browser".parse::<Subject>(),
            Err(Error::UnknownSubject("browser".to_string()))
        );
    }

    #[wcmp_macros::test]
    fn it_names_the_polyfill_subject_of_the_target() {
        let expected = if cfg!(target_arch = "wasm32") {
            "web"
        } else {
            "native"
        };
        assert_eq!(Subject::polyfill().name(), expected);
    }
}

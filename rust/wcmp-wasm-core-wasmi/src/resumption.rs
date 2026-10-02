// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A resumable call of the Wasmi backend, which stopped before its wait.

use core::any::Any;

use wcmp_wasm_core::backend::BackendResumption;
use wcmp_wasm_core::{Error, Result, ResumableCall, Val};

/// A resumable call that Wasmi ran to its stop when it started or resumed
/// it: how the call ended, and its results where it finished, until a wait
/// takes them. A call that failed failed at its start or its resumption.
///
/// Wasmi runs a resumable call synchronously, so the call never runs
/// without its store, and a wait whose future drops loses nothing.
pub struct WasmiResumption {
    stop: Option<(ResumableCall, Vec<Val>)>,
}

impl WasmiResumption {
    /// The call that stopped at `outcome`, with its results in `results`
    /// where it finished.
    pub fn new(outcome: ResumableCall, results: Vec<Val>) -> Self {
        Self {
            stop: Some((outcome, results)),
        }
    }

    /// How the call ended, with its results written to `results` where it
    /// finished. A number of slots other than the call's results is
    /// [`Error::TypeMismatch`].
    pub fn take(&mut self, results: &mut [Val]) -> Result<ResumableCall> {
        let Some((_, given)) = &self.stop else {
            return Err(Error::Backend {
                message: "the resumable call already stopped".to_owned(),
            });
        };
        if given.len() != results.len() {
            return Err(Error::TypeMismatch {
                message: format!(
                    "the call gives {} results, and the wait gave {} result slots",
                    given.len(),
                    results.len(),
                ),
            });
        }
        let (outcome, given) = self.stop.take().ok_or_else(|| Error::Backend {
            message: "the resumable call already stopped".to_owned(),
        })?;
        results.copy_from_slice(&given);
        Ok(outcome)
    }
}

impl BackendResumption for WasmiResumption {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

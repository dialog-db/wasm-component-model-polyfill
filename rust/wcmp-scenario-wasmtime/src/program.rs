// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One compiled program of a scenario.

use std::path::Path;

use wcmp_scenario::Verdict;

use crate::error::{Error, Result};

/// One program of a scenario, as the build compiled it, and composed it
/// when its scenario's wiring plugs other components into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The program's name: its file name without the extension. The
    /// calls of the expectations file name a component by it.
    pub name: String,
    /// The compiler's exit status. `0` means the program compiled.
    pub status: i32,
    /// Everything the compiler printed.
    pub log: String,
    /// The component, present when the program compiled. When the build
    /// composed other components into it, this is the composition.
    pub component: Option<Vec<u8>>,
    /// The composition tool's exit status, present when the build
    /// composed other components into the program. `0` means the
    /// component is the composition.
    pub compose_status: Option<i32>,
    /// Everything the composition tool printed.
    pub compose_log: String,
}

impl Program {
    /// Read the program `name` from a compiled scenario's directory:
    /// `<name>.status`, `<name>.log`, and `<name>.wasm` when the status
    /// is `0`, and `<name>.compose-status` and `<name>.compose-log` when
    /// the build composed other components into it.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when a file cannot be read, and [`Error::Layout`]
    /// when a status is not a number, or when the status says the
    /// program compiled and there is no component.
    pub fn read(directory: &Path, name: &str) -> Result<Self> {
        let status = read_status(&directory.join(format!("{name}.status")))?;
        let log = read_to_string(&directory.join(format!("{name}.log")))?;
        let component_path = directory.join(format!("{name}.wasm"));
        let component = if status == 0 {
            Some(std::fs::read(&component_path).map_err(|source| Error::Io {
                path: component_path,
                source,
            })?)
        } else {
            None
        };
        let compose_status_path = directory.join(format!("{name}.compose-status"));
        let (compose_status, compose_log) = if compose_status_path.exists() {
            (
                Some(read_status(&compose_status_path)?),
                read_to_string(&directory.join(format!("{name}.compose-log")))?,
            )
        } else {
            (None, String::new())
        };
        Ok(Program {
            name: name.to_string(),
            status,
            log,
            component,
            compose_status,
            compose_log,
        })
    }

    /// The component, or the verdict of a scenario the program stops at
    /// `compile`, which names the compiler's exit status and output.
    pub fn compiled(&self) -> core::result::Result<&[u8], Verdict> {
        match &self.component {
            Some(component) => Ok(component),
            None => Err(Verdict::not_compiled(&self.name, self.status, &self.log)),
        }
    }

    /// Nothing when the program's composition succeeded or the build
    /// composed nothing into it, and otherwise the verdict of a scenario
    /// the program stops at `compose`, which names the composition
    /// tool's exit status and output.
    pub fn composed(&self) -> core::result::Result<(), Verdict> {
        match self.compose_status {
            Some(status) if status != 0 => {
                Err(Verdict::not_composed(&self.name, status, &self.compose_log))
            }
            _ => Ok(()),
        }
    }
}

/// Read an exit status that a build step wrote to `path`.
fn read_status(path: &Path) -> Result<i32> {
    let text = read_to_string(path)?;
    text.trim().parse().map_err(|_| Error::Layout {
        path: path.to_path_buf(),
        reason: format!("`{}` is not an exit status", text.trim()),
    })
}

fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

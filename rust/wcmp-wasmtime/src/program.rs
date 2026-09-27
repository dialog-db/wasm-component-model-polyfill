//! One compiled program of a scenario.

use std::path::Path;

use wcmp_scenario::Verdict;

use crate::error::{Error, Result};

/// One program of a scenario, as the build compiled it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The program's name: its file name without the extension. The
    /// calls of the expectations file name a component by it.
    pub name: String,
    /// The compiler's exit status. `0` means the program compiled.
    pub status: i32,
    /// Everything the compiler printed.
    pub log: String,
    /// The component, present when the program compiled.
    pub component: Option<Vec<u8>>,
}

impl Program {
    /// Read the program `name` from a compiled scenario's directory:
    /// `<name>.status`, `<name>.log`, and `<name>.wasm` when the status
    /// is `0`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when a file cannot be read, and [`Error::Layout`]
    /// when the status is not a number, or when it says the program
    /// compiled and there is no component.
    pub fn read(directory: &Path, name: &str) -> Result<Self> {
        let status_path = directory.join(format!("{name}.status"));
        let status_text = read_to_string(&status_path)?;
        let status = status_text.trim().parse().map_err(|_| Error::Layout {
            path: status_path.clone(),
            reason: format!("`{}` is not an exit status", status_text.trim()),
        })?;
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
        Ok(Program {
            name: name.to_string(),
            status,
            log,
            component,
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
}

fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

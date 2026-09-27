//! One compiled scenario and its expectations.

use std::path::Path;

use wcmp_scenario::Expectations;

use crate::error::{Error, Result};
use crate::program::Program;

/// The name of the expectations file in a scenario's sources.
const EXPECTATIONS: &str = "expectations.txt";

/// One scenario as the build left it: its expectations and its compiled
/// programs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    /// The scenario's name: the name of its directory.
    pub name: String,
    /// The calls to make and the lines to expect.
    pub expectations: Expectations,
    /// The scenario's programs, ordered by name.
    pub programs: Vec<Program>,
}

impl Scenario {
    /// Read every scenario the build compiled, ordered by name.
    ///
    /// `sources` holds one directory per scenario with its expectations
    /// file, and `compiled` one directory per scenario with its compiled
    /// programs. A file directly under `compiled`, such as the record of
    /// the toolchain's revision, is not a scenario.
    ///
    /// # Errors
    ///
    /// [`Error::Layout`] when the two directories do not hold the same
    /// scenarios, when `compiled` holds no scenario, or when a scenario
    /// holds no program. Otherwise any error of [`Scenario::read`].
    pub fn find(sources: &Path, compiled: &Path) -> Result<Vec<Self>> {
        let names = subdirectories(compiled)?;
        if names.is_empty() {
            return Err(Error::Layout {
                path: compiled.to_path_buf(),
                reason: "holds no compiled scenario".to_string(),
            });
        }
        if let Some(name) = subdirectories(sources)?
            .into_iter()
            .find(|name| !names.contains(name))
        {
            return Err(Error::Layout {
                path: sources.join(name),
                reason: "is a scenario the build did not compile".to_string(),
            });
        }
        names
            .iter()
            .map(|name| Scenario::read(&sources.join(name), &compiled.join(name)))
            .collect()
    }

    /// Read one scenario from its sources and its compiled programs.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when a file cannot be read, [`Error::Expectations`]
    /// when the expectations file is malformed, and [`Error::Layout`]
    /// when the compiled directory holds no program or a program's files
    /// are out of place.
    pub fn read(sources: &Path, compiled: &Path) -> Result<Self> {
        let name = compiled
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let expectations_path = sources.join(EXPECTATIONS);
        let expectations = std::fs::read_to_string(&expectations_path)
            .map_err(|source| Error::Io {
                path: expectations_path.clone(),
                source,
            })?
            .parse()
            .map_err(|source| Error::Expectations {
                path: expectations_path,
                source,
            })?;
        let mut program_names: Vec<String> = entries(compiled)?
            .into_iter()
            .filter_map(|file| file.strip_suffix(".status").map(str::to_string))
            .collect();
        program_names.sort();
        if program_names.is_empty() {
            return Err(Error::Layout {
                path: compiled.to_path_buf(),
                reason: "holds no compiled program".to_string(),
            });
        }
        let programs = program_names
            .iter()
            .map(|program| Program::read(compiled, program))
            .collect::<Result<_>>()?;
        Ok(Scenario {
            name,
            expectations,
            programs,
        })
    }
}

/// The names of the directories directly under `directory`, sorted.
fn subdirectories(directory: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = entries(directory)?
        .into_iter()
        .filter(|name| directory.join(name).is_dir())
        .collect();
    names.sort();
    Ok(names)
}

/// The names of everything directly under `directory`.
fn entries(directory: &Path) -> Result<Vec<String>> {
    let io = |source| Error::Io {
        path: directory.to_path_buf(),
        source,
    };
    std::fs::read_dir(directory)
        .map_err(io)?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(io)
        })
        .collect()
}

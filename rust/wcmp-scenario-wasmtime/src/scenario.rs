// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One compiled scenario, its expectations, and its wiring.

use std::path::Path;

use wcmp_scenario::{Expectations, Link, Wiring};

use crate::error::{Error, Result};
use crate::program::Program;

/// The name of the expectations file in a scenario's sources.
const EXPECTATIONS: &str = "expectations.txt";

/// The name of the wiring file in a scenario's sources. A scenario
/// whose components do not link has none.
const WIRING: &str = "wiring.txt";

/// One scenario as the build left it: its expectations, its wiring, and
/// its compiled programs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    /// The scenario's name: the name of its directory.
    pub name: String,
    /// The calls to make and the lines to expect.
    pub expectations: Expectations,
    /// Which component's exports satisfy which component's imports.
    /// Empty when the components do not link.
    pub wiring: Wiring,
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
    /// when the expectations file is malformed, [`Error::Wiring`] when
    /// the wiring file is malformed or its links name a program the
    /// scenario does not have or form a cycle, and [`Error::Layout`]
    /// when the compiled directory holds no program, a program's files
    /// are out of place, or every program compiled and a composition
    /// link of the wiring was not made: its importer carries no outcome
    /// of a composition, or its exporter is still a program of its own.
    pub fn read(sources: &Path, compiled: &Path) -> Result<Self> {
        let name = compiled
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let expectations_path = sources.join(EXPECTATIONS);
        let expectations = read_to_string(&expectations_path)?
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
        let wiring_path = sources.join(WIRING);
        let wiring: Wiring = if wiring_path.exists() {
            read_to_string(&wiring_path)?
                .parse()
                .map_err(|source| Error::Wiring {
                    path: wiring_path.clone(),
                    source,
                })?
        } else {
            Wiring::default()
        };
        let names: Vec<&str> = program_names.iter().map(String::as_str).collect();
        wiring.order(&names).map_err(|source| Error::Wiring {
            path: wiring_path.clone(),
            source,
        })?;
        let programs: Vec<Program> = program_names
            .iter()
            .map(|program| Program::read(compiled, program))
            .collect::<Result<_>>()?;
        if let Some(link) = uncomposed(&wiring, &programs) {
            return Err(Error::Layout {
                path: wiring_path,
                reason: format!("`{link}` asks for a composition, which the build did not make"),
            });
        }
        Ok(Scenario {
            name,
            expectations,
            wiring,
            programs,
        })
    }
}

/// The first composition link of `wiring` that the build should have
/// made and did not: every program compiled, and yet the link's
/// importer carries no outcome of a composition or its exporter is
/// still a program of its own. When a program did not compile, the
/// build attempts no composition, and the scenario stops at `compile`.
fn uncomposed<'a>(wiring: &'a Wiring, programs: &[Program]) -> Option<&'a Link> {
    if programs.iter().any(|program| program.status != 0) {
        return None;
    }
    wiring.composition().find(|link| {
        let composed = programs
            .iter()
            .any(|program| program.name == link.importer && program.compose_status.is_some());
        let consumed = programs.iter().all(|program| program.name != link.exporter);
        !(composed && consumed)
    })
}

fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use wcmp_scenario::Linking;

    use super::*;

    /// A scenario's two directories under the system's temporary
    /// directory, removed when the value is dropped. The compiled
    /// directory holds the programs `exporter` and `importer`, which
    /// Zena refused, so no component is needed.
    struct Layout {
        root: PathBuf,
    }

    impl Layout {
        fn new(test: &str, wiring: Option<&str>) -> Self {
            let root = std::env::temp_dir().join(format!(
                "wcmp-scenario-wasmtime-{}-{test}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let layout = Layout { root };
            std::fs::create_dir_all(layout.sources()).unwrap();
            std::fs::create_dir_all(layout.compiled()).unwrap();
            std::fs::write(layout.sources().join(EXPECTATIONS), "").unwrap();
            if let Some(wiring) = wiring {
                std::fs::write(layout.sources().join(WIRING), wiring).unwrap();
            }
            for program in ["exporter", "importer"] {
                std::fs::write(layout.compiled().join(format!("{program}.status")), "1").unwrap();
                std::fs::write(layout.compiled().join(format!("{program}.log")), "").unwrap();
            }
            layout
        }

        fn sources(&self) -> PathBuf {
            self.root.join("sources")
        }

        fn compiled(&self) -> PathBuf {
            self.root.join("linked")
        }

        fn read(&self) -> Result<Scenario> {
            Scenario::read(&self.sources(), &self.compiled())
        }

        /// Make `program` one that compiled.
        fn compile(&self, program: &str) {
            std::fs::write(self.compiled().join(format!("{program}.status")), "0\n").unwrap();
            std::fs::write(self.compiled().join(format!("{program}.wasm")), "\0asm").unwrap();
        }

        /// Give `program` the outcome of a composition into it.
        fn compose(&self, program: &str, status: i32, log: &str) {
            let file = |extension: &str| self.compiled().join(format!("{program}.{extension}"));
            std::fs::write(file("compose-status"), format!("{status}\n")).unwrap();
            std::fs::write(file("compose-log"), log).unwrap();
        }

        /// Remove `program`'s files, as a composition does with each
        /// program it plugs into another.
        fn remove(&self, program: &str) {
            for extension in ["status", "log", "wasm"] {
                let _ =
                    std::fs::remove_file(self.compiled().join(format!("{program}.{extension}")));
            }
        }
    }

    impl Drop for Layout {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// The wiring error `layout` fails to read with.
    fn wiring_error(layout: &Layout) -> wcmp_scenario::Error {
        match layout.read() {
            Err(Error::Wiring { path, source }) => {
                assert_eq!(path, layout.sources().join(WIRING));
                source
            }
            other => panic!("{other:?}"),
        }
    }

    #[wcmp_macros::test]
    fn it_reads_an_empty_wiring_when_the_scenario_has_no_wiring_file() {
        let layout = Layout::new("no-wiring", None);
        let scenario = layout.read().unwrap();
        assert_eq!(scenario.name, "linked");
        assert_eq!(scenario.wiring, Wiring::default());
        let programs: Vec<&str> = scenario
            .programs
            .iter()
            .map(|program| program.name.as_str())
            .collect();
        assert_eq!(programs, ["exporter", "importer"]);
    }

    #[wcmp_macros::test]
    fn it_reads_the_links_of_the_wiring_file() {
        let layout = Layout::new(
            "run-time",
            Some("# A comment.\nrun-time importer local:demo/greeter exporter\n"),
        );
        assert_eq!(
            layout.read().unwrap().wiring.links,
            [Link {
                linking: Linking::RunTime,
                importer: "importer".to_string(),
                import: "local:demo/greeter".to_string(),
                exporter: "exporter".to_string(),
            }]
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_malformed_wiring_or_one_that_names_a_missing_program_or_a_cycle() {
        let malformed = Layout::new("malformed", Some("run-time importer exporter"));
        assert_eq!(
            wiring_error(&malformed),
            wcmp_scenario::Error::Syntax {
                line: 1,
                reason: "expected `<linking> <importer> <import> <exporter>`, found 3 words"
                    .to_string(),
            }
        );
        let missing = Layout::new(
            "missing",
            Some("run-time importer local:demo/greeter exprter"),
        );
        assert_eq!(
            wiring_error(&missing),
            wcmp_scenario::Error::UnknownComponent {
                link: "run-time importer local:demo/greeter exprter".to_string(),
                component: "exprter".to_string(),
            }
        );
        let cycle = Layout::new(
            "cycle",
            Some(
                "run-time importer local:demo/greeter exporter\n\
                 run-time exporter local:demo/welcome importer",
            ),
        );
        assert_eq!(
            wiring_error(&cycle),
            wcmp_scenario::Error::LinkCycle(vec!["exporter".to_string(), "importer".to_string()])
        );
    }

    /// The wiring of a scenario whose build plugs `exporter` into
    /// `importer`.
    const COMPOSITION: &str = "composition importer local:demo/greeter exporter";

    #[wcmp_macros::test]
    fn it_reads_a_composition_under_its_importer_name_and_its_outcome() {
        let layout = Layout::new("composed", Some(COMPOSITION));
        layout.compile("importer");
        layout.compose("importer", 0, "");
        layout.remove("exporter");
        let scenario = layout.read().unwrap();
        let [program] = &scenario.programs[..] else {
            panic!("{:?}", scenario.programs);
        };
        assert_eq!(program.name, "importer");
        assert_eq!(program.compose_status, Some(0));
        assert_eq!(program.compiled(), Ok(&b"\0asm"[..]));
        assert_eq!(program.composed(), Ok(()));

        let layout = Layout::new("refused-composition", Some(COMPOSITION));
        layout.compile("importer");
        layout.compose("importer", 1, "error: no matching imports\n");
        layout.remove("exporter");
        let scenario = layout.read().unwrap();
        assert_eq!(
            scenario.programs[0].composed(),
            Err(wcmp_scenario::Verdict::not_composed(
                "importer",
                1,
                "error: no matching imports"
            ))
        );
    }

    #[wcmp_macros::test]
    fn it_reads_a_composition_whose_programs_did_not_compile_as_they_are() {
        // The build attempts no composition then, and the scenario
        // stops at `compile`.
        let layout = Layout::new("uncompiled", Some(COMPOSITION));
        layout.compile("importer");
        let scenario = layout.read().unwrap();
        let programs: Vec<_> = scenario
            .programs
            .iter()
            .map(|program| (program.name.as_str(), program.compose_status))
            .collect();
        assert_eq!(programs, [("exporter", None), ("importer", None)]);
    }

    #[wcmp_macros::test]
    fn it_refuses_a_composition_the_build_did_not_make() {
        let uncomposed = Layout::new("uncomposed", Some(COMPOSITION));
        uncomposed.compile("importer");
        uncomposed.compile("exporter");
        let unconsumed = Layout::new("unconsumed", Some(COMPOSITION));
        unconsumed.compile("importer");
        unconsumed.compile("exporter");
        unconsumed.compose("importer", 0, "");
        for layout in [uncomposed, unconsumed] {
            match layout.read() {
                Err(Error::Layout { path, reason }) => {
                    assert_eq!(path, layout.sources().join(WIRING));
                    assert_eq!(
                        reason,
                        format!(
                            "`{COMPOSITION}` asks for a composition, which the build did not make"
                        )
                    );
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

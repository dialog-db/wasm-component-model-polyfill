// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Zena scenarios as the build left them, read from one bundle.
//!
//! The build packs every file the polyfill subjects read into one
//! bundle with `tests/zena/bundle.sh`, whose header states the format:
//! each file is a line `file <path> <length>`, its bytes, and a
//! newline. The test embeds the bundle at compile time, so the same
//! bytes reach the browser and a native test.

use std::collections::BTreeMap;

use wcmp_scenario::{Expectations, Link, Observations, Verdict, Wiring};

/// One program of a scenario, as the build compiled it, and composed it
/// when the scenario's wiring plugs other components into it.
#[derive(Debug, Clone)]
pub struct Program {
    /// The program's name: its file name without the extension. The
    /// calls of the expectations name a component by it.
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
    /// A program that compiled to `component`.
    pub fn compiled(name: &str, component: &[u8]) -> Self {
        Program {
            name: name.to_string(),
            status: 0,
            log: String::new(),
            component: Some(component.to_vec()),
            compose_status: None,
            compose_log: String::new(),
        }
    }

    /// The component, or the verdict of a scenario the program stops at
    /// `compile`, which is the same for every subject.
    pub fn component(&self) -> Result<&[u8], Verdict> {
        match &self.component {
            Some(component) => Ok(component),
            None => Err(Verdict::not_compiled(&self.name, self.status, &self.log)),
        }
    }

    /// Nothing when the program's composition succeeded or the build
    /// composed nothing into it, and otherwise the verdict of a scenario
    /// the program stops at `compose`, which is the same for every
    /// subject.
    pub fn composed(&self) -> Result<(), Verdict> {
        match self.compose_status {
            Some(status) if status != 0 => {
                Err(Verdict::not_composed(&self.name, status, &self.compose_log))
            }
            _ => Ok(()),
        }
    }
}

/// One scenario: its expectations, the Wasmtime run's observations, its
/// wiring, and its compiled programs.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// The scenario's name: the name of its directory.
    pub name: String,
    /// The calls to make and the lines to expect.
    pub expectations: Expectations,
    /// What the Wasmtime run saw, with its verdict.
    pub observations: Observations,
    /// Which component's exports satisfy which component's imports:
    /// its `wiring.txt`, or empty when it has none.
    pub wiring: Wiring,
    /// The scenario's programs, ordered by name.
    pub programs: Vec<Program>,
}

/// Every scenario in `bundle`, ordered by name. A file directly at the
/// root of the bundle, such as the toolchain's revision, is not a
/// scenario.
///
/// A bundle that is not in the format, or a scenario whose files are
/// missing or malformed, is a fault of the build, and the answer says
/// what is wrong.
pub fn scenarios(bundle: &[u8]) -> Result<Vec<Scenario>, String> {
    let mut directories: BTreeMap<&str, BTreeMap<&str, &[u8]>> = BTreeMap::new();
    for (path, bytes) in files(bundle)? {
        if let Some((scenario, file)) = path.split_once('/') {
            directories.entry(scenario).or_default().insert(file, bytes);
        }
    }
    if directories.is_empty() {
        return Err("the bundle holds no scenario".to_string());
    }
    directories
        .into_iter()
        .map(|(name, files)| scenario(name, &files))
        .collect()
}

/// The revision of the toolchain that compiled the scenarios in
/// `bundle`: its `zena-revision` file, which the build writes from the
/// flake input the lock pins.
pub fn revision(bundle: &[u8]) -> Result<String, String> {
    let (_, bytes) = files(bundle)?
        .into_iter()
        .find(|(path, _)| *path == "zena-revision")
        .ok_or("the bundle has no zena-revision")?;
    let revision = core::str::from_utf8(bytes)
        .map_err(|error| format!("zena-revision: {error}"))?
        .trim();
    if revision.is_empty() {
        return Err("zena-revision is empty".to_string());
    }
    Ok(revision.to_string())
}

/// Read one scenario from its files.
fn scenario(name: &str, files: &BTreeMap<&str, &[u8]>) -> Result<Scenario, String> {
    let text = |file: &str| -> Result<&str, String> {
        let bytes = files
            .get(file)
            .ok_or_else(|| format!("scenario {name} has no {file}"))?;
        core::str::from_utf8(bytes).map_err(|error| format!("{name}/{file}: {error}"))
    };
    let expectations = text("expectations.txt")?
        .parse()
        .map_err(|error| format!("{name}/expectations.txt: {error}"))?;
    let observations = text("observations.txt")?
        .parse()
        .map_err(|error| format!("{name}/observations.txt: {error}"))?;
    let exit_status = |file: &str| -> Result<i32, String> {
        let status = text(file)?.trim();
        status
            .parse()
            .map_err(|_| format!("{name}/{file}: `{status}` is not an exit status"))
    };
    let programs = files
        .keys()
        .filter_map(|file| file.strip_suffix(".status"))
        .map(|program| {
            let status = exit_status(&format!("{program}.status"))?;
            let component = match status {
                0 => Some(
                    files
                        .get(format!("{program}.wasm").as_str())
                        .ok_or_else(|| {
                            format!("program {program} of {name} compiled but has no component")
                        })?
                        .to_vec(),
                ),
                _ => None,
            };
            let compose_status_file = format!("{program}.compose-status");
            let (compose_status, compose_log) = match files.get(compose_status_file.as_str()) {
                Some(_) => (
                    Some(exit_status(&compose_status_file)?),
                    text(&format!("{program}.compose-log"))?.to_string(),
                ),
                None => (None, String::new()),
            };
            Ok(Program {
                name: program.to_string(),
                status,
                log: text(&format!("{program}.log"))?.to_string(),
                component,
                compose_status,
                compose_log,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let wiring: Wiring = match files.get("wiring.txt") {
        Some(_) => text("wiring.txt")?
            .parse()
            .map_err(|error| format!("{name}/wiring.txt: {error}"))?,
        None => Wiring::default(),
    };
    if let Some(link) = uncomposed(&wiring, &programs) {
        return Err(format!(
            "{name}/wiring.txt: `{link}` asks for a composition, which the build did not make"
        ));
    }
    if programs.is_empty() {
        return Err(format!("scenario {name} has no compiled program"));
    }
    Ok(Scenario {
        name: name.to_string(),
        expectations,
        observations,
        wiring,
        programs,
    })
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

/// Every file in `bundle`, in order, with its path.
fn files(bundle: &[u8]) -> Result<Vec<(&str, &[u8])>, String> {
    let mut files = Vec::new();
    let mut rest = bundle;
    while !rest.is_empty() {
        let end = rest
            .iter()
            .position(|&byte| byte == b'\n')
            .ok_or("the bundle ends inside a header")?;
        let header = core::str::from_utf8(&rest[..end])
            .map_err(|error| format!("a header of the bundle: {error}"))?;
        let (path, length) = header
            .strip_prefix("file ")
            .and_then(|header| header.rsplit_once(' '))
            .ok_or_else(|| format!("`{header}` is not a header of the bundle"))?;
        let length: usize = length
            .parse()
            .map_err(|_| format!("`{header}` has no length"))?;
        let body = &rest[end + 1..];
        if body.len() <= length || body[length] != b'\n' {
            return Err(format!("{path} is shorter than its header says"));
        }
        files.push((path, &body[..length]));
        rest = &body[length + 1..];
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One file of a bundle, as `bundle.sh` writes it.
    fn file(path: &str, bytes: &[u8]) -> Vec<u8> {
        let mut file = format!("file {path} {}\n", bytes.len()).into_bytes();
        file.extend_from_slice(bytes);
        file.push(b'\n');
        file
    }

    #[wcmp_macros::test]
    fn it_reads_each_scenario_of_a_bundle_and_skips_the_files_at_its_root() {
        let bundle = [
            file("zena-revision", b"abc\n"),
            file("demo/expectations.txt", b"call main add(1s32) -> 1s32\n"),
            file(
                "demo/observations.txt",
                b"stage pass\ncall main add(1s32) -> 1s32\n",
            ),
            file("demo/main.log", b""),
            file("demo/main.status", b"0\n"),
            file("demo/main.wasm", b"\0asm"),
            file("demo/refused.log", b"no!\n\n"),
            file("demo/refused.status", b"1\n"),
        ]
        .concat();
        assert_eq!(revision(&bundle).as_deref(), Ok("abc"));
        let scenarios = scenarios(&bundle).unwrap();
        assert_eq!(scenarios.len(), 1);
        let demo = &scenarios[0];
        assert_eq!(demo.name, "demo");
        assert_eq!(demo.expectations.entries.len(), 1);
        assert!(demo.observations.verdict.passed());
        let programs: Vec<_> = demo
            .programs
            .iter()
            .map(|program| (program.name.as_str(), program.status))
            .collect();
        assert_eq!(programs, [("main", 0), ("refused", 1)]);
        assert_eq!(demo.programs[0].component(), Ok(&b"\0asm"[..]));
        assert_eq!(
            demo.programs[1].component().unwrap_err().reason,
            "program refused did not compile (exit 1): no!"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_bundle_that_is_cut_short_or_misses_a_file() {
        let status = file("demo/main.status", b"0\n");
        assert_eq!(
            scenarios(&status[..status.len() - 1]).unwrap_err(),
            "demo/main.status is shorter than its header says"
        );
        assert_eq!(
            scenarios(&status).unwrap_err(),
            "scenario demo has no expectations.txt"
        );
        assert_eq!(
            revision(&status).unwrap_err(),
            "the bundle has no zena-revision"
        );
        assert_eq!(
            scenarios(&file("zena-revision", b"abc\n")).unwrap_err(),
            "the bundle holds no scenario"
        );
    }

    #[wcmp_macros::test]
    fn it_reads_the_wiring_of_a_scenario() {
        let scenario = |wiring: &[u8]| {
            let mut bundle = [
                file("demo/expectations.txt", b""),
                file("demo/observations.txt", b"stage pass\n"),
                file("demo/main.log", b""),
                file("demo/main.status", b"0\n"),
                file("demo/main.wasm", b"\0asm"),
            ]
            .concat();
            if !wiring.is_empty() {
                bundle.extend(file("demo/wiring.txt", wiring));
            }
            scenarios(&bundle).map(|mut scenarios| scenarios.remove(0))
        };
        assert!(scenario(b"").unwrap().wiring.links.is_empty());
        let wired = scenario(b"run-time main local:demo/api partner\n").unwrap();
        assert_eq!(
            wired.wiring.to_string(),
            "run-time main local:demo/api partner\n"
        );
        assert!(
            scenario(b"run-time main\n")
                .unwrap_err()
                .starts_with("demo/wiring.txt: line 1: ")
        );
    }

    /// A scenario whose wiring plugs `partner` into `main`, with the
    /// files `extra` beside `main`'s.
    fn composition(extra: &[Vec<u8>]) -> Result<Scenario, String> {
        let mut bundle = [
            file("demo/expectations.txt", b""),
            file("demo/observations.txt", b"stage pass\n"),
            file(
                "demo/wiring.txt",
                b"composition main local:demo/api partner\n",
            ),
            file("demo/main.log", b""),
            file("demo/main.status", b"0\n"),
            file("demo/main.wasm", b"\0asm"),
        ]
        .concat();
        for extra in extra {
            bundle.extend(extra);
        }
        scenarios(&bundle).map(|mut scenarios| scenarios.remove(0))
    }

    #[wcmp_macros::test]
    fn it_reads_a_composition_under_its_importer_name_and_its_outcome() {
        let composed = composition(&[
            file("demo/main.compose-log", b""),
            file("demo/main.compose-status", b"0\n"),
        ])
        .unwrap();
        let [main] = &composed.programs[..] else {
            panic!("{:?}", composed.programs);
        };
        assert_eq!(main.compose_status, Some(0));
        assert_eq!(main.component(), Ok(&b"\0asm"[..]));
        assert_eq!(main.composed(), Ok(()));

        let refused = composition(&[
            file("demo/main.compose-log", b"error: no matching imports\n"),
            file("demo/main.compose-status", b"1\n"),
        ])
        .unwrap();
        assert_eq!(
            refused.programs[0].composed().unwrap_err().reason,
            "the composition into main failed (exit 1): error: no matching imports"
        );

        // A program that did not compile stops the scenario at
        // `compile`, and the build attempts no composition.
        let uncompiled = composition(&[
            file("demo/partner.log", b"no!\n"),
            file("demo/partner.status", b"1\n"),
        ])
        .unwrap();
        assert_eq!(uncompiled.programs.len(), 2);
    }

    #[wcmp_macros::test]
    fn it_refuses_a_composition_the_build_did_not_make() {
        let refusal = "demo/wiring.txt: `composition main local:demo/api partner` asks for a composition, which the build did not make";
        assert_eq!(composition(&[]).unwrap_err(), refusal);
        assert_eq!(
            composition(&[
                file("demo/main.compose-log", b""),
                file("demo/main.compose-status", b"0\n"),
                file("demo/partner.log", b""),
                file("demo/partner.status", b"0\n"),
                file("demo/partner.wasm", b"\0asm"),
            ])
            .unwrap_err(),
            refusal
        );
    }
}

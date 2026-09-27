//! The Zena scenarios as the build left them, read from one bundle.
//!
//! The build packs every file the polyfill subjects read into one
//! bundle with `tests/zena/bundle.sh`, whose header states the format:
//! each file is a line `file <path> <length>`, its bytes, and a
//! newline. The test embeds the bundle at compile time, so the same
//! bytes reach the browser and a native test.

use std::collections::BTreeMap;

use wcmp_scenario::{Expectations, Observations, Verdict};

/// One program of a scenario, as the build compiled it.
#[derive(Debug, Clone)]
pub struct Program {
    /// The program's name: its file name without the extension. The
    /// calls of the expectations name a component by it.
    pub name: String,
    /// The compiler's exit status. `0` means the program compiled.
    pub status: i32,
    /// Everything the compiler printed.
    pub log: String,
    /// The component, present when the program compiled.
    pub component: Option<Vec<u8>>,
}

impl Program {
    /// A program that compiled to `component`.
    pub fn compiled(name: &str, component: &[u8]) -> Self {
        Program {
            name: name.to_string(),
            status: 0,
            log: String::new(),
            component: Some(component.to_vec()),
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
}

/// One scenario: its expectations, the Wasmtime run's observations, and
/// its compiled programs.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// The scenario's name: the name of its directory.
    pub name: String,
    /// The calls to make and the lines to expect.
    pub expectations: Expectations,
    /// What the Wasmtime run saw, with its verdict.
    pub observations: Observations,
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
    let programs = files
        .keys()
        .filter_map(|file| file.strip_suffix(".status"))
        .map(|program| {
            let status_file = format!("{program}.status");
            let status = text(&status_file)?.trim();
            let status = status
                .parse()
                .map_err(|_| format!("{name}/{status_file}: `{status}` is not an exit status"))?;
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
            Ok(Program {
                name: program.to_string(),
                status,
                log: text(&format!("{program}.log"))?.to_string(),
                component,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if programs.is_empty() {
        return Err(format!("scenario {name} has no compiled program"));
    }
    Ok(Scenario {
        name: name.to_string(),
        expectations,
        observations,
        programs,
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
            scenarios(&file("zena-revision", b"abc\n")).unwrap_err(),
            "the bundle holds no scenario"
        );
    }
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Wasmtime run as a step of the build.
//!
//! `wcmp-scenario-wasmtime <sources> <compiled> <out> [<source-bundle>]`
//! runs every scenario the build compiled through Wasmtime and writes
//! what it saw to `<out>/<scenario>/observations.txt`: the Wasmtime
//! stage on the first line, then each call with its outcome and each
//! line printed. A scenario that stops before `pass` is an outcome, not
//! a failure of the step. The step fails only when a scenario cannot be
//! read or the observations cannot be written. The compiler
//! component's `read-source` answers from `<source-bundle>`, and has no
//! file to answer with when there is none.
//!
//! `wcmp-scenario-wasmtime compile <compiler> <source-bundle> <entry>
//! <out>` compiles the Zena module `<entry>` with the compiler
//! component `<compiler>`, against the world Zena derives from it, and
//! writes the component to `<out>`. The entry module's path is its file
//! name. A compile the compiler refuses prints its diagnostics and
//! exits with 1.

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let arguments: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let outcome = match arguments.as_slice() {
        [command, compiler, bundle, entry, out] if command.as_os_str() == "compile" => {
            native::compile(compiler, bundle, entry, out).await
        }
        [sources, compiled, out] => native::run(sources, compiled, out, None).await,
        [sources, compiled, out, bundle] => native::run(sources, compiled, out, Some(bundle)).await,
        _ => {
            eprintln!(
                "usage: wcmp-scenario-wasmtime <sources> <compiled> <out> [<source-bundle>]\n       wcmp-scenario-wasmtime compile <compiler> <source-bundle> <entry> <out>"
            );
            std::process::exit(2);
        }
    };
    if let Err(error) = outcome {
        eprintln!("wcmp-scenario-wasmtime: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::Path;

    use wcmp_scenario::SourceBundle;
    use wcmp_scenario_wasmtime::{Error, Result, Scenario, WasmtimeRun};

    /// The name of the file each scenario's observations go to.
    const OBSERVATIONS: &str = "observations.txt";

    /// The bytes of `path`.
    fn read(path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// The source bundle at `path`, or an empty one.
    fn sources(path: Option<&Path>) -> Result<SourceBundle> {
        let Some(path) = path else {
            return Ok(SourceBundle::default());
        };
        SourceBundle::parse(&read(path)?).map_err(|error| Error::Layout {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })
    }

    pub async fn compile(compiler: &Path, bundle: &Path, entry: &Path, out: &Path) -> Result<()> {
        let wasmtime = WasmtimeRun::with_sources(sources(Some(bundle))?)?;
        let source = String::from_utf8_lossy(&read(entry)?).into_owned();
        let entry_path = entry
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match wasmtime
            .compile(&read(compiler)?, &source, &entry_path, "", "")
            .await?
        {
            Ok(bytes) => std::fs::write(out, bytes).map_err(|source| Error::Io {
                path: out.to_path_buf(),
                source,
            }),
            Err(diagnostics) => {
                eprint!("{diagnostics}");
                std::process::exit(1);
            }
        }
    }

    pub async fn run(
        sources_dir: &Path,
        compiled: &Path,
        out: &Path,
        bundle: Option<&Path>,
    ) -> Result<()> {
        let scenarios = Scenario::find(sources_dir, compiled)?;
        let wasmtime = WasmtimeRun::with_sources(sources(bundle)?)?;
        for scenario in &scenarios {
            let observations = wasmtime.run(scenario).await?;
            let directory = out.join(&scenario.name);
            let io = |path: &Path| {
                let path = path.to_path_buf();
                move |source| Error::Io { path, source }
            };
            std::fs::create_dir_all(&directory).map_err(io(&directory))?;
            let path = directory.join(OBSERVATIONS);
            std::fs::write(&path, observations.to_string()).map_err(io(&path))?;
            let verdict = &observations.verdict;
            if verdict.reason.is_empty() {
                println!("wasmtime: {}: {}", scenario.name, verdict.stage);
            } else {
                println!(
                    "wasmtime: {}: {}: {}",
                    scenario.name, verdict.stage, verdict.reason
                );
            }
        }
        Ok(())
    }
}

/// Wasmtime does not run in the browser, so there is nothing to run on
/// the web target.
#[cfg(target_arch = "wasm32")]
fn main() {}

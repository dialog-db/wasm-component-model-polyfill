//! The Wasmtime run as a step of the build.
//!
//! `wcmp-wasmtime <sources> <compiled> <out>` runs every scenario the
//! build compiled through Wasmtime and writes what it saw to
//! `<out>/<scenario>/observations.txt`: the Wasmtime stage on the
//! first line, then each call with its outcome and each line printed.
//! A scenario that stops before `pass` is an outcome, not a failure of
//! the step. The step fails only when a scenario cannot be read or the
//! observations cannot be written.

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let arguments: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let [sources, compiled, out] = arguments.as_slice() else {
        eprintln!("usage: wcmp-wasmtime <sources> <compiled> <out>");
        std::process::exit(2);
    };
    if let Err(error) = native::run(sources, compiled, out).await {
        eprintln!("wcmp-wasmtime: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::Path;

    use wcmp_wasmtime::{Error, Result, Scenario, WasmtimeRun};

    /// The name of the file each scenario's observations go to.
    const OBSERVATIONS: &str = "observations.txt";

    pub async fn run(sources: &Path, compiled: &Path, out: &Path) -> Result<()> {
        let scenarios = Scenario::find(sources, compiled)?;
        let wasmtime = WasmtimeRun::new()?;
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

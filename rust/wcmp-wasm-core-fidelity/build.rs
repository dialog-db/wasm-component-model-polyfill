//! Lists the scripts of the fidelity suite, from the pinned
//! specification test suite that `WCMP_SPEC_TESTSUITE` names.
//!
//! The flake sets the variable on every derivation that compiles the
//! suite's tests, to the store path of its `spec-testsuite` input. The
//! build writes `scripts.rs` into `OUT_DIR`: a static that embeds every
//! script, and a macro that expands to one test for each. Without the
//! variable the list is empty, so a plain `cargo build` of the workspace
//! still compiles, and the one test that checks the list fails instead.
//!
//! The suite holds the scripts at the top of the test suite, which are the
//! core specification, and the scripts of each proposal directory whose
//! feature the capability lexicon names. The other directories hold what
//! no lexicon name covers: `legacy/` the legacy exception handling,
//! `custom/` the custom-section annotations of the text format, and every
//! other proposal a feature outside the lexicon.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// The variable that names the root of the pinned test suite.
const VARIABLE: &str = "WCMP_SPEC_TESTSUITE";

/// The proposal directories under `proposals/` whose feature is a name of
/// the capability lexicon.
const PROPOSALS: [&str; 1] = ["threads"];

fn main() {
    println!("cargo::rerun-if-env-changed={VARIABLE}");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("scripts.rs");
    let scripts = match env::var_os(VARIABLE) {
        Some(root) => scripts(Path::new(&root)),
        None => Vec::new(),
    };
    fs::write(&out, generate(&scripts))
        .unwrap_or_else(|error| panic!("writing {}: {error}", out.display()));
}

/// Every script of the suite under `root`: its path relative to `root`,
/// and its absolute path, in the order of their relative paths.
fn scripts(root: &Path) -> Vec<(String, PathBuf)> {
    println!("cargo::rerun-if-changed={}", root.display());
    let mut scripts = wast_files(root, "");
    for proposal in PROPOSALS {
        let directory = format!("proposals/{proposal}");
        scripts.extend(wast_files(&root.join(&directory), &format!("{directory}/")));
    }
    scripts.sort();
    scripts
}

/// The `.wast` files directly inside `directory`, each with its path
/// relative to the root of the suite, `prefix` followed by its name.
fn wast_files(directory: &Path, prefix: &str) -> Vec<(String, PathBuf)> {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{VARIABLE}: reading {}: {error}", directory.display()));
    entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("{VARIABLE}: {error}"))
                .path()
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "wast")
        })
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| panic!("{VARIABLE}: {} is not UTF-8", path.display()));
            (format!("{prefix}{name}"), path)
        })
        .collect()
}

/// The name of the test of the script at `path`: `it_passes_` and the path
/// without its extension, each other character an underscore.
fn test_name(path: &str) -> String {
    let stem = path.strip_suffix(".wast").unwrap_or(path);
    let words = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("it_passes_{words}")
}

/// The source of `scripts.rs` for `scripts`.
fn generate(scripts: &[(String, PathBuf)]) -> String {
    let mut names = scripts
        .iter()
        .map(|(path, _)| test_name(path))
        .collect::<Vec<_>>();
    names.sort();
    if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
        panic!("{VARIABLE}: two scripts share the test name {}", pair[0]);
    }

    let mut source = String::new();
    source.push_str("/// Every script of the suite, in the order of their paths.\n");
    source.push_str("pub static SCRIPTS: &[Script] = &[\n");
    for (path, file) in scripts {
        writeln!(
            source,
            "    Script::new({path:?}, include_str!({:?})),",
            file.display().to_string()
        )
        .expect("writing to a string");
    }
    source.push_str("];\n\n");
    source.push_str("/// One test for each script of the suite. See `fidelity_tests!`.\n");
    source.push_str("#[doc(hidden)]\n#[macro_export]\nmacro_rules! __script_tests {\n");
    source.push_str("    ($engine:path, $expected:path) => {\n");
    for (path, _) in scripts {
        let name = test_name(path);
        writeln!(
            source,
            "        #[$crate::__private::test]\n        async fn {name}() {{\n            $crate::check_script(&$engine(), {path:?}, $expected).await;\n        }}"
        )
        .expect("writing to a string");
    }
    source.push_str("    };\n}\n");
    source
}

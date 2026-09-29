//! One script of the pinned specification test suite.

/// A `.wast` script of the suite: its path under the root of the test
/// suite, such as `address.wast` or `proposals/threads/atomic.wast`, and
/// its source.
#[derive(Clone, Copy, Debug)]
pub struct Script {
    path: &'static str,
    source: &'static str,
}

impl Script {
    /// The script at `path`, whose source is `source`.
    pub const fn new(path: &'static str, source: &'static str) -> Self {
        Self { path, source }
    }

    /// The path of the script under the root of the test suite.
    pub const fn path(&self) -> &'static str {
        self.path
    }

    /// The source of the script.
    pub const fn source(&self) -> &'static str {
        self.source
    }

    /// The script of the suite at `path`, where the suite holds one.
    pub fn find(path: &str) -> Option<Script> {
        SCRIPTS.iter().copied().find(|script| script.path == path)
    }
}

// `SCRIPTS`, and the macro that expands to one test for each script: see
// the build script.
include!(concat!(env!("OUT_DIR"), "/scripts.rs"));

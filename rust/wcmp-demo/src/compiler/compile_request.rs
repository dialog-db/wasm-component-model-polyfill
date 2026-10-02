// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One compile's request.

/// One compile: an entry module, files beside it, and the world it
/// must match.
pub struct CompileRequest<'a> {
    /// The path of the entry module. Relative imports resolve against
    /// it.
    pub entry_path: &'a str,
    /// The text of the entry module.
    pub source: &'a str,
    /// Other files of this compile, by path, such as an author's source
    /// that the entry module imports by a relative path.
    pub files: &'a [(String, String)],
    /// A WIT document that holds the world, or empty for the world Zena
    /// derives from the program.
    pub wit: &'a str,
    /// The world in `wit` the program must match.
    pub world: &'a str,
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The toolchain's source bundle.

use std::collections::BTreeMap;

use crate::error::{Error, Result};

/// The files a compile with the compiler component can read through its
/// `read-source` import, by the path the compiler asks for: Zena's
/// standard library under `/stdlib`, and each directory of WIT under its
/// own path, as the text Zena's `readWitSource` makes of a directory.
///
/// The build packs the bundle with `zena/compiler/bundle.sh`. Each file
/// is a line `file <path> <length>`, then its `<length>` bytes, then a
/// newline, the format of the scenario bundle too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceBundle {
    files: BTreeMap<String, String>,
}

impl SourceBundle {
    /// Read a bundle from its bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Bundle`] when a header is malformed, a file runs past
    /// its length or is not followed by a newline, or a file is not
    /// UTF-8.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let malformed = |reason: String| Error::Bundle(reason);
        let mut files = BTreeMap::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let newline = rest
                .iter()
                .position(|&byte| byte == b'\n')
                .ok_or_else(|| malformed("a header has no end".to_string()))?;
            let header = core::str::from_utf8(&rest[..newline])
                .map_err(|_| malformed("a header is not UTF-8".to_string()))?;
            let mut words = header.split(' ');
            let (Some("file"), Some(path), Some(length), None) =
                (words.next(), words.next(), words.next(), words.next())
            else {
                return Err(malformed(format!("`{header}` is not a header")));
            };
            let length: usize = length
                .parse()
                .map_err(|_| malformed(format!("`{header}` has no length")))?;
            rest = &rest[newline + 1..];
            if rest.len() <= length || rest[length] != b'\n' {
                return Err(malformed(format!("{path} runs past its length")));
            }
            let text = core::str::from_utf8(&rest[..length])
                .map_err(|_| malformed(format!("{path} is not UTF-8")))?;
            files.insert(path.to_string(), text.to_string());
            rest = &rest[length + 1..];
        }
        Ok(SourceBundle { files })
    }

    /// The text of the file at `path`, or `None` when the bundle has no
    /// such file. This is what `read-source` answers.
    pub fn read(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    /// The number of files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the bundle holds no file.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_each_file_by_its_path() {
        let bundle =
            SourceBundle::parse(b"file /a.zena 3\nabc\nfile /dir 5\nx\ny\nz\n").unwrap();
        assert_eq!(bundle.read("/a.zena"), Some("abc"));
        assert_eq!(bundle.read("/dir"), Some("x\ny\nz"));
        assert_eq!(bundle.len(), 2);
    }

    #[wcmp_macros::test]
    fn it_answers_none_for_a_missing_path() {
        let bundle = SourceBundle::parse(b"file /a.zena 3\nabc\n").unwrap();
        assert_eq!(bundle.read("/b.zena"), None);
        assert_eq!(bundle.read("/a"), None);
    }

    #[wcmp_macros::test]
    fn it_refuses_a_file_that_runs_past_its_length() {
        assert!(matches!(
            SourceBundle::parse(b"file /a.zena 9\nabc\n"),
            Err(Error::Bundle(_))
        ));
        assert!(matches!(
            SourceBundle::parse(b"file /a.zena 2\nabc\n"),
            Err(Error::Bundle(_))
        ));
    }

    #[wcmp_macros::test]
    fn it_refuses_a_malformed_header() {
        assert!(SourceBundle::parse(b"files /a 1\na\n").is_err());
        assert!(SourceBundle::parse(b"file /a\na\n").is_err());
    }
}

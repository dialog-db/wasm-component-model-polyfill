// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One invocation: its options, and the WIT they select.

use std::path::{Path, PathBuf};

use proc_macro2::Span;
use wit_parser::{PackageId, Resolve, WorldId};

use crate::options::Options;

/// One invocation of the macro: its options, the WIT they read, and
/// the world they select.
pub struct Config {
    /// The options.
    pub options: Options,
    /// The WIT, with every feature gate on.
    pub resolve: Resolve,
    /// The world the invocation generates.
    pub world: WorldId,
    /// Every file the WIT was read from, which the generated code names
    /// so that a change to one rebuilds the invoking crate.
    pub files: Vec<PathBuf>,
}

impl Config {
    /// Read the WIT `options` name, from the paths relative to `root`,
    /// the directory of the invoking crate's `Cargo.toml`. With no path
    /// and no inline source, read `root/wit`.
    pub fn new(options: Options, root: &Path) -> syn::Result<Self> {
        let call_site = Span::call_site();
        let failed = |error: anyhow::Error| syn::Error::new(call_site, format!("{error:#}"));

        // Every item behind a feature gate generates, as Wasmtime's
        // macro reads them: as though every feature is on.
        let mut resolve = Resolve {
            all_features: true,
            ..Resolve::default()
        };
        let mut packages: Vec<PackageId> = Vec::new();
        let mut files = Vec::new();
        let mut push = |resolve: &mut Resolve, path: &Path| -> anyhow::Result<()> {
            let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            let (package, sources) = resolve.push_path(&path)?;
            packages.push(package);
            files.extend(sources.paths().map(Path::to_path_buf));
            Ok(())
        };
        if options.paths.is_empty() {
            let default = root.join("wit");
            if default.exists() {
                push(&mut resolve, &default).map_err(failed)?;
            }
        } else {
            for path in &options.paths {
                push(&mut resolve, &root.join(path.value())).map_err(failed)?;
            }
        }
        if let Some(inline) = &options.inline {
            packages.clear();
            packages.push(resolve.push_str("macro-input", inline).map_err(failed)?);
        }

        let world_name = options.world.as_ref().map(|world| world.value());
        let world = resolve
            .select_world(&packages, world_name.as_deref())
            .map_err(failed)?;
        Ok(Self {
            options,
            resolve,
            world,
            files,
        })
    }
}

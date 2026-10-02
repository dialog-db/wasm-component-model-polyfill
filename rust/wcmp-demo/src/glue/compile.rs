// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An element's or a route's compile.

use super::{
    ELEMENT_WIT, ELEMENT_WORLD, ROUTE_WIT, ROUTE_WORLD, element_file, element_glue, form_of,
    route_file, route_glue,
};
use crate::compiler::CompileRequest;

/// An element's or a route's compile: the glue as the entry module,
/// the author's source beside it, and the world.
pub struct Compile {
    /// The glue's path.
    pub entry_path: String,
    /// The glue.
    pub glue: String,
    /// The author's source, at its path.
    pub files: Vec<(String, String)>,
    /// The WIT document.
    pub wit: &'static str,
    /// The world.
    pub world: &'static str,
}

impl Compile {
    /// The compile of the element `tag` from `source`.
    ///
    /// # Errors
    ///
    /// A message when the source exports neither a class that extends
    /// `Element` nor a `render` function.
    pub fn element(tag: &str, source: &str) -> Result<Self, String> {
        let file = element_file(tag);
        let form = form_of(source).ok_or_else(|| {
            format!(
                "{file}: Error: the source exports no element: export a class that extends Element, or a render function"
            )
        })?;
        Ok(Compile {
            entry_path: format!("{tag}.glue.zena"),
            glue: element_glue(&file, &form),
            files: vec![(file, source.to_string())],
            wit: ELEMENT_WIT,
            world: ELEMENT_WORLD,
        })
    }

    /// The compile of the route `pattern` from `source`.
    pub fn route(pattern: &str, source: &str) -> Self {
        let file = route_file(pattern);
        Compile {
            entry_path: file.replace(".zena", ".glue.zena"),
            glue: route_glue(pattern, &file),
            files: vec![(file, source.to_string())],
            wit: ROUTE_WIT,
            world: ROUTE_WORLD,
        }
    }

    /// The request to hand the compiler.
    pub fn request(&self) -> CompileRequest<'_> {
        CompileRequest {
            entry_path: &self.entry_path,
            source: &self.glue,
            files: &self.files,
            wit: self.wit,
            world: self.world,
        }
    }
}

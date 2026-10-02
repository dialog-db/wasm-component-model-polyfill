// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A view: what an element's `render` returns, read back into a tree.
//!
//! WIT types cannot recurse, so a render returns its tree flattened: a
//! `list<node>` in document order, each node naming its parent by its
//! index in the list. [`View::from_nodes`] reads that list back into a
//! tree. [`matches`] pairs the children of two views the way the
//! element's diff does: by key where a child has one, and by position
//! among the children without one.

mod kind;
mod property;
#[allow(clippy::module_inception)]
mod view;
mod view_error;

pub use kind::Kind;
pub use property::Property;
pub use view::View;
pub use view_error::ViewError;

/// Pair each of `new` with the index of one of `old`, or with none.
///
/// A child with a key pairs with the child of `old` with the same key.
/// A child without one pairs with the child of `old` in the same
/// position among the children without a key. A child of `old` pairs at
/// most once.
pub fn matches(old: &[Option<&str>], new: &[Option<&str>]) -> Vec<Option<usize>> {
    let mut used = vec![false; old.len()];
    let unkeyed: Vec<usize> = (0..old.len())
        .filter(|&index| old[index].is_none())
        .collect();
    let mut next_unkeyed = 0;
    new.iter()
        .map(|key| {
            let found = match key {
                Some(key) => old
                    .iter()
                    .enumerate()
                    .find(|(index, other)| !used[*index] && *other == &Some(*key))
                    .map(|(index, _)| index),
                None => {
                    let found = unkeyed.get(next_unkeyed).copied();
                    next_unkeyed += 1;
                    found
                }
            };
            if let Some(index) = found {
                used[index] = true;
            }
            found
        })
        .collect()
}

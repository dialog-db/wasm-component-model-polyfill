// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The parameter list or the result list of a typed function.

use super::component_type::ComponentType;

/// A tuple of typed values that is the parameter list or the result
/// list of a function, as Wasmtime's `ComponentNamedList` is.
///
/// Tuples of 0 to 17 values implement it. A typed function takes both
/// of its lists as tuples, so a function with one result returns a
/// tuple of one value:
///
/// ```rust
/// # async fn call(func: wcmp::Func, store: &mut wcmp::Store<()>) -> wcmp::Result<()> {
/// let add = func.typed::<(u32, u32), (u32,)>()?;
/// let (sum,) = add.call(store, (2, 3)).await?;
/// # let _ = sum;
/// # Ok(())
/// # }
/// ```
pub trait ComponentNamedList: ComponentType {}

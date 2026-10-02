// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The handles to the objects of a store, and the externs among them.

mod external;
mod func;
mod global;
mod instance;
mod memory;
mod table;
mod tag;

pub use external::Extern;
pub use func::Func;
pub use global::Global;
pub use instance::Instance;
pub use memory::Memory;
pub use table::Table;
pub use tag::Tag;

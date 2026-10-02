// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The boundary type model: the types of values and of externs.

mod extern_type;
mod func_type;
mod global_type;
mod heap_type;
mod memory_type;
mod mutability;
mod ref_type;
mod table_type;
mod tag_type;
mod type_handle;
mod val_type;

pub use extern_type::ExternType;
pub use func_type::FuncType;
pub use global_type::GlobalType;
pub use heap_type::HeapType;
pub use memory_type::MemoryType;
pub use mutability::Mutability;
pub use ref_type::RefType;
pub use table_type::TableType;
pub use tag_type::TagType;
pub use type_handle::TypeHandle;
pub use val_type::ValType;

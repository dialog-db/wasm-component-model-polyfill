// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The numbers the backend gives concrete heap types.

use core::cell::RefCell;
use core::fmt::Write;
use std::collections::HashMap;

use wasmparser::{
    CompositeInnerType, FieldType, PackedIndex, RecGroup, StorageType, SubType, UnpackedIndex,
};
use wcmp_wasm_core::TypeHandle;
use wcmp_wasm_core::backend::RawTypeHandle;

/// The hierarchy the values of a concrete type belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hierarchy {
    /// A function type: its values are function references.
    Func,
    /// A struct or an array type: its values are internal references.
    Any,
    /// A continuation type: its values are continuation references.
    Cont,
}

/// The numbers of the concrete heap types the backend has described.
///
/// The browser takes two concrete types for one type when their recursion
/// groups have the same shape, and each sits at the same place in its
/// group. That is the iso-recursive equality of Wasm GC. The registry
/// numbers each distinct recursion group once, the first time a module
/// defines it, and keeps it for the life of the backend. So two handles are
/// equal exactly when the browser takes them for one type.
#[derive(Debug, Default)]
pub struct TypeRegistry {
    inner: RefCell<Registry>,
}

/// The state of the registry.
#[derive(Debug, Default)]
struct Registry {
    /// The first number of each recursion group, by the group's shape.
    groups: HashMap<String, u64>,
    /// The hierarchy of each number.
    hierarchies: Vec<Hierarchy>,
}

impl TypeRegistry {
    /// The handles of the types of `group`, which a module defines at type
    /// index `start`, after the types `earlier` that it defined before.
    pub fn intern(&self, group: &RecGroup, start: u32, earlier: &[TypeHandle]) -> Vec<TypeHandle> {
        let len = group.types().len();
        let shape = shape(group, start, earlier);
        let mut registry = self.inner.borrow_mut();
        let first = match registry.groups.get(&shape) {
            Some(first) => *first,
            None => {
                let first = registry.hierarchies.len() as u64;
                registry.hierarchies.extend(group.types().map(hierarchy));
                registry.groups.insert(shape, first);
                first
            }
        };
        (0..len as u64)
            .map(|offset| TypeHandle::from_raw(first + offset))
            .collect()
    }

    /// The hierarchy of `handle`, or `None` where the registry did not
    /// number it.
    pub fn hierarchy(&self, handle: TypeHandle) -> Option<Hierarchy> {
        let registry = self.inner.borrow();
        usize::try_from(handle.raw())
            .ok()
            .and_then(|index| registry.hierarchies.get(index))
            .copied()
    }
}

/// The hierarchy of the values of `ty`.
fn hierarchy(ty: &SubType) -> Hierarchy {
    match ty.composite_type.inner {
        CompositeInnerType::Func(_) => Hierarchy::Func,
        CompositeInnerType::Struct(_) | CompositeInnerType::Array(_) => Hierarchy::Any,
        CompositeInnerType::Cont(_) => Hierarchy::Cont,
    }
}

/// The shape of `group`: a text that two groups share exactly when the
/// browser takes them for one group.
///
/// A reference to a type inside the group is its place in the group. A
/// reference to a type before the group is that type's number in the
/// registry.
fn shape(group: &RecGroup, start: u32, earlier: &[TypeHandle]) -> String {
    let end = start.saturating_add(group.types().len() as u32);
    let mut shape = Shape {
        text: String::new(),
        start,
        end,
        earlier,
    };
    for ty in group.types() {
        shape.sub_type(ty);
    }
    shape.text
}

/// The writer of a shape.
struct Shape<'a> {
    text: String,
    start: u32,
    end: u32,
    earlier: &'a [TypeHandle],
}

impl Shape<'_> {
    fn word(&mut self, word: impl core::fmt::Display) {
        // Writing to a `String` does not fail.
        let _ = write!(self.text, "{word} ");
    }

    fn sub_type(&mut self, ty: &SubType) {
        self.word(if ty.is_final { "(final" } else { "(sub" });
        if let Some(supertype) = ty.supertype_idx {
            self.packed(supertype);
        }
        let composite = &ty.composite_type;
        if composite.shared {
            self.word("shared");
        }
        if let Some(descriptor) = composite.descriptor_idx {
            self.word("descriptor");
            self.packed(descriptor);
        }
        if let Some(describes) = composite.describes_idx {
            self.word("describes");
            self.packed(describes);
        }
        match &composite.inner {
            CompositeInnerType::Func(func) => {
                self.word("func");
                for param in func.params() {
                    self.val_type(*param);
                }
                self.word("->");
                for result in func.results() {
                    self.val_type(*result);
                }
            }
            CompositeInnerType::Array(array) => {
                self.word("array");
                self.field(array.0);
            }
            CompositeInnerType::Struct(structure) => {
                self.word("struct");
                for field in structure.fields.iter() {
                    self.field(*field);
                }
            }
            CompositeInnerType::Cont(cont) => {
                self.word("cont");
                self.packed(cont.0);
            }
        }
        self.word(")");
    }

    fn field(&mut self, field: FieldType) {
        if field.mutable {
            self.word("mut");
        }
        match field.element_type {
            StorageType::I8 => self.word("i8"),
            StorageType::I16 => self.word("i16"),
            StorageType::Val(ty) => self.val_type(ty),
        }
    }

    fn val_type(&mut self, ty: wasmparser::ValType) {
        match ty {
            wasmparser::ValType::Ref(ty) => {
                self.word(if ty.is_nullable() {
                    "(ref null"
                } else {
                    "(ref"
                });
                match ty.heap_type() {
                    wasmparser::HeapType::Abstract { shared, ty } => {
                        if shared {
                            self.word("shared");
                        }
                        self.word(format_args!("{ty:?}"));
                    }
                    wasmparser::HeapType::Concrete(index) => self.unpacked(index),
                    wasmparser::HeapType::Exact(index) => {
                        self.word("exact");
                        self.unpacked(index);
                    }
                }
                self.word(")");
            }
            other => self.word(other),
        }
    }

    fn packed(&mut self, index: PackedIndex) {
        self.unpacked(index.unpack());
    }

    fn unpacked(&mut self, index: UnpackedIndex) {
        if let Some(index) = index.as_module_index() {
            if (self.start..self.end).contains(&index) {
                self.word(format_args!("local{}", index - self.start));
            } else if let Some(handle) = self.earlier.get(index as usize) {
                self.word(format_args!("type{}", handle.raw()));
            } else {
                // The engine accepted the module before the backend read
                // it, so no index points past the types defined so far.
                self.word(format_args!("unknown{index}"));
            }
        } else if let Some(index) = index.as_rec_group_index() {
            self.word(format_args!("local{index}"));
        } else {
            self.word("unknown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recursion groups of the type section of `bytes`, each interned
    /// in `registry`, in order.
    fn intern_all(registry: &TypeRegistry, bytes: &[u8]) -> Vec<TypeHandle> {
        let mut handles = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let Ok(wasmparser::Payload::TypeSection(reader)) = payload {
                for group in reader {
                    let group = group.expect("the group reads");
                    let interned = registry.intern(&group, handles.len() as u32, &handles);
                    handles.extend(interned);
                }
            }
        }
        handles
    }

    #[wcmp_macros::test]
    fn it_gives_one_number_to_one_type_in_two_modules() {
        let registry = TypeRegistry::default();
        let first = intern_all(
            &registry,
            wcmp_macros::wasm!(
                r#"
                (module
                  (type $pair (struct (field i32) (field i32)))
                  (type $list (struct (field (ref null $pair)))))
                "#
            ),
        );
        let second = intern_all(
            &registry,
            wcmp_macros::wasm!(
                r#"
                (module
                  (type $other (func))
                  (type $pair (struct (field i32) (field i32)))
                  (type $list (struct (field (ref null $pair)))))
                "#
            ),
        );

        assert_eq!(first[0], second[1], "one struct shape is one type");
        assert_eq!(first[1], second[2], "a reference to an earlier type");
        assert_ne!(second[0], second[1]);
        assert_eq!(registry.hierarchy(second[0]), Some(Hierarchy::Func));
        assert_eq!(registry.hierarchy(second[1]), Some(Hierarchy::Any));
    }

    #[wcmp_macros::test]
    fn it_tells_apart_a_type_in_a_recursion_group_from_one_outside() {
        let registry = TypeRegistry::default();
        let handles = intern_all(
            &registry,
            wcmp_macros::wasm!(
                r#"
                (module
                  (type $alone (struct (field i32)))
                  (rec
                    (type $inside (struct (field i32)))
                    (type $next (struct (field (ref null $inside))))))
                "#
            ),
        );

        assert_eq!(handles.len(), 3);
        assert_ne!(handles[0], handles[1], "a group of two is another group");
    }
}

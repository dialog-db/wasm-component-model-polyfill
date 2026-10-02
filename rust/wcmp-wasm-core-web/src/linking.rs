// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Which import of a refused instantiation does not link, as the backend
//! tells it from its own imports object and the types it knows.
//!
//! Each engine words a refused import in its own way. V8 names it by its
//! place, as `Import #3`. JavaScriptCore names it as `host:notify`.
//! SpiderMonkey names a function as `'host.notify'`, and names no import at
//! all for a global, a table, or a memory of the wrong type. So the backend
//! never reads the engine's message. Where the engine refused an
//! instantiation for its imports, the backend reads the imports object it
//! gave the engine, as the JavaScript API reads it, and checks each extern
//! against the type of its import.
//!
//! The engine alone decides whether an extern links. The check here only
//! names the import that the engine refused, and never refuses one itself.

use js_sys::{Object, WebAssembly};
use wasm_bindgen::{JsCast, JsValue};
use wcmp_wasm_core::{
    Extern, ExternType, FuncType, HeapType, ImportType, MemoryType, Mutability, Result, TableType,
    ValType,
};

use crate::js;
use crate::objects::Objects;
use crate::type_registry::{Hierarchy, TypeRegistry};

/// Whether an extern links to an import, as far as the backend can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The extern links: its type is the type of the import.
    Links,
    /// No engine links the extern to the import.
    Refused,
    /// The backend cannot tell. The type of the extern is not known, or
    /// the answer rests on the subtypes that a module declares, which the
    /// boundary of a module does not describe.
    Open,
}

/// The import that the instantiation of a module with the imports `types`,
/// given `imports` in the imports object `object`, failed on with `error`,
/// where the backend can tell which one.
///
/// The JavaScript API reads the imports in order. It throws a `TypeError`
/// where the namespace of an import is not an object, and a `LinkError`
/// where the value of an import is not an extern of its type. So for either
/// error, the failed import is the first whose namespace or value is not the
/// one the backend gave. For a `LinkError`, it is next the first import
/// whose extern cannot link, and last the one import whose extern the
/// backend cannot check. Any other error, and a `LinkError` that none of
/// these explains, names no import.
pub fn failed<'a>(
    error: &JsValue,
    types: &'a [ImportType],
    imports: &[Extern],
    object: &Object,
    objects: &Objects,
    registry: &TypeRegistry,
) -> Option<&'a ImportType> {
    let linking = error.is_instance_of::<WebAssembly::LinkError>();
    if !linking && !error.is_instance_of::<js_sys::TypeError>() {
        return None;
    }
    let pairs = || types.iter().zip(imports);
    if let Some((ty, _)) = pairs().find(|(ty, import)| !given(object, ty, import, objects)) {
        return Some(ty);
    }
    if !linking {
        return None;
    }
    let fits = pairs()
        .map(|(ty, import)| (ty, fit(ty.ty(), import, objects, registry)))
        .collect::<Vec<_>>();
    if let Some((ty, _)) = fits.iter().find(|(_, fit)| *fit == Fit::Refused) {
        return Some(ty);
    }
    let mut open = fits.iter().filter(|(_, fit)| *fit == Fit::Open);
    match (open.next(), open.next()) {
        (Some((ty, _)), None) => Some(ty),
        _ => None,
    }
}

/// Whether the imports object `object` holds, for the import `ty`, a
/// namespace object whose value is the object of `import`.
fn given(object: &Object, ty: &ImportType, import: &Extern, objects: &Objects) -> bool {
    let Ok(namespace) = js::get(object, ty.module()) else {
        return false;
    };
    if !namespace.is_object() {
        return false;
    }
    let (Ok(value), Ok(expected)) = (js::get(&namespace, ty.name()), value(import, objects)) else {
        return false;
    };
    Object::is(&value, &expected)
}

/// The JavaScript value of `external`.
pub fn value(external: &Extern, objects: &Objects) -> Result<JsValue> {
    Ok(match external {
        Extern::Func(func) => objects.func(*func)?.function.clone().into(),
        Extern::Global(global) => objects.global(*global)?.global.clone().into(),
        Extern::Table(table) => objects.table(*table)?.table.clone().into(),
        Extern::Memory(memory) => objects.memory(*memory)?.memory.clone().into(),
        Extern::Tag(tag) => objects.tag(*tag)?.tag.clone(),
    })
}

/// Whether `external` links to an import of type `ty`.
pub fn fit(ty: &ExternType, external: &Extern, objects: &Objects, registry: &TypeRegistry) -> Fit {
    let fit = match (ty, external) {
        (ExternType::Func(expected), Extern::Func(func)) => objects.func(*func).map(|object| {
            object
                .ty
                .as_ref()
                .map_or(Fit::Open, |found| func_fit(expected, found, registry))
        }),
        (ExternType::Global(expected), Extern::Global(global)) => {
            objects.global(*global).map(|object| {
                let found = object.ty;
                if found == *expected {
                    Fit::Links
                } else if found.mutability() != expected.mutability()
                    || expected.mutability() == Mutability::Var
                {
                    // A mutable global links only to its own type.
                    Fit::Refused
                } else {
                    answer(subtype(found.content(), expected.content(), registry))
                }
            })
        }
        (ExternType::Table(expected), Extern::Table(table)) => objects
            .table(*table)
            .map(|object| table_fit(expected, &object.ty, &object.table)),
        (ExternType::Memory(expected), Extern::Memory(memory)) => objects
            .memory(*memory)
            .map(|object| memory_fit(expected, &object.ty, &object.memory)),
        (ExternType::Tag(expected), Extern::Tag(tag)) => objects.tag(*tag).map(|object| {
            if object.ty == *expected {
                Fit::Links
            } else {
                Fit::Refused
            }
        }),
        _ => Ok(Fit::Refused),
    };
    fit.unwrap_or(Fit::Open)
}

/// Whether a function of type `found` links to an import of type
/// `expected`.
///
/// A function links where its type is a subtype of the import's, which
/// needs the parameters of the import to be subtypes of the function's,
/// and the results of the function to be subtypes of the import's. Two
/// different types that meet this are subtypes only where a module
/// declares one the other's supertype, which the backend does not know.
fn func_fit(expected: &FuncType, found: &FuncType, registry: &TypeRegistry) -> Fit {
    if found == expected {
        return Fit::Links;
    }
    if found.params().len() != expected.params().len()
        || found.results().len() != expected.results().len()
    {
        return Fit::Refused;
    }
    let params = expected
        .params()
        .iter()
        .zip(found.params())
        .map(|(expected, found)| subtype(expected, found, registry));
    let results = found
        .results()
        .iter()
        .zip(expected.results())
        .map(|(found, expected)| subtype(found, expected, registry));
    match answer(all(params.chain(results))) {
        Fit::Refused => Fit::Refused,
        _ => Fit::Open,
    }
}

/// Whether a table of type `found`, whose object is `table`, links to an
/// import of type `expected`: the same element type and addressing, at
/// least the import's minimum of elements now, and a maximum within the
/// import's.
fn table_fit(expected: &TableType, found: &TableType, table: &WebAssembly::Table) -> Fit {
    if found.element() != expected.element() || found.is_64() != expected.is_64() {
        return Fit::Refused;
    }
    let Some(size) = js::get(table, "length").ok().as_ref().and_then(js::count) else {
        return Fit::Open;
    };
    limits_fit(
        (expected.minimum(), expected.maximum()),
        (size, found.maximum()),
    )
}

/// Whether a memory of type `found`, whose object is `memory`, links to an
/// import of type `expected`: the same addressing and sharing, at least the
/// import's minimum of pages now, and a maximum within the import's.
fn memory_fit(expected: &MemoryType, found: &MemoryType, memory: &WebAssembly::Memory) -> Fit {
    if found.is_64() != expected.is_64() || found.is_shared() != expected.is_shared() {
        return Fit::Refused;
    }
    let Some(bytes) = js::get(memory, "buffer")
        .and_then(|buffer| js::get(&buffer, "byteLength"))
        .ok()
        .as_ref()
        .and_then(js::count)
    else {
        return Fit::Open;
    };
    limits_fit(
        (expected.minimum(), expected.maximum()),
        (bytes / MemoryType::PAGE_SIZE, found.maximum()),
    )
}

/// Whether the limits `found`, a size and a maximum, meet the limits
/// `expected` of an import, a minimum and a maximum.
fn limits_fit(expected: (u64, Option<u64>), found: (u64, Option<u64>)) -> Fit {
    let (minimum, maximum) = expected;
    let (size, found_maximum) = found;
    let within = match (maximum, found_maximum) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(maximum), Some(found)) => found <= maximum,
    };
    if size >= minimum && within {
        Fit::Links
    } else {
        Fit::Refused
    }
}

/// The fit that the answer `answer` to "is it a subtype?" gives.
fn answer(answer: Option<bool>) -> Fit {
    match answer {
        Some(true) => Fit::Links,
        Some(false) => Fit::Refused,
        None => Fit::Open,
    }
}

/// `Some(false)` where any of `answers` is, `Some(true)` where each is,
/// and `None` otherwise.
fn all(answers: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    let mut known = true;
    for answer in answers {
        match answer {
            Some(false) => return Some(false),
            Some(true) => {}
            None => known = false,
        }
    }
    known.then_some(true)
}

/// Whether `sub` is a subtype of `sup`, or `None` where the answer rests
/// on the subtypes that a module declares.
fn subtype(sub: &ValType, sup: &ValType, registry: &TypeRegistry) -> Option<bool> {
    match (sub, sup) {
        (ValType::Ref(sub), ValType::Ref(sup)) => {
            if sub.nullable && !sup.nullable {
                return Some(false);
            }
            heap_subtype(sub.heap, sup.heap, registry)
        }
        _ => Some(sub == sup),
    }
}

/// The hierarchy of a heap type, named by its top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Top {
    Func,
    Extern,
    Any,
    Exn,
    Cont,
}

/// The hierarchy of `heap`, or `None` for a concrete type that the
/// registry did not number.
fn top(heap: HeapType, registry: &TypeRegistry) -> Option<Top> {
    Some(match heap {
        HeapType::Func | HeapType::NoFunc => Top::Func,
        HeapType::Extern | HeapType::NoExtern => Top::Extern,
        HeapType::Any
        | HeapType::Eq
        | HeapType::I31
        | HeapType::Struct
        | HeapType::Array
        | HeapType::None => Top::Any,
        HeapType::Exn | HeapType::NoExn => Top::Exn,
        HeapType::Cont | HeapType::NoCont => Top::Cont,
        HeapType::Concrete(handle) => match registry.hierarchy(handle)? {
            Hierarchy::Func => Top::Func,
            Hierarchy::Any => Top::Any,
            Hierarchy::Cont => Top::Cont,
        },
    })
}

/// Whether the heap type `sub` is a subtype of `sup`, or `None` where the
/// answer rests on the subtypes that a module declares, or on whether a
/// concrete type is a struct or an array.
fn heap_subtype(sub: HeapType, sup: HeapType, registry: &TypeRegistry) -> Option<bool> {
    if sub == sup {
        return Some(true);
    }
    if top(sub, registry)? != top(sup, registry)? {
        return Some(false);
    }
    let bottom = |heap| {
        matches!(
            heap,
            HeapType::NoFunc
                | HeapType::NoExtern
                | HeapType::None
                | HeapType::NoExn
                | HeapType::NoCont
        )
    };
    if bottom(sub) {
        return Some(true);
    }
    match (sub, sup) {
        (_, HeapType::Func | HeapType::Extern | HeapType::Any | HeapType::Exn | HeapType::Cont) => {
            Some(true)
        }
        (
            HeapType::I31 | HeapType::Struct | HeapType::Array | HeapType::Concrete(_),
            HeapType::Eq,
        ) => Some(true),
        (HeapType::Concrete(_), HeapType::Struct | HeapType::Array | HeapType::Concrete(_)) => None,
        _ => Some(false),
    }
}

#[cfg(test)]
mod tests {
    use wcmp_wasm_core::RefType;

    use super::*;

    #[wcmp_macros::test]
    fn it_orders_the_abstract_heap_types_of_each_hierarchy() {
        let registry = TypeRegistry::default();
        for (sub, sup, answer) in [
            (HeapType::NoFunc, HeapType::Func, Some(true)),
            (HeapType::Func, HeapType::NoFunc, Some(false)),
            (HeapType::I31, HeapType::Eq, Some(true)),
            (HeapType::Struct, HeapType::Any, Some(true)),
            (HeapType::Any, HeapType::Eq, Some(false)),
            (HeapType::Struct, HeapType::Array, Some(false)),
            (HeapType::None, HeapType::Array, Some(true)),
            (HeapType::Func, HeapType::Extern, Some(false)),
            (HeapType::NoExtern, HeapType::Any, Some(false)),
            (HeapType::Exn, HeapType::Exn, Some(true)),
        ] {
            assert_eq!(
                heap_subtype(sub, sup, &registry),
                answer,
                "{sub} below {sup}"
            );
        }
    }

    #[wcmp_macros::test]
    fn it_refuses_a_function_of_another_arity_and_leaves_a_subtype_open() {
        let registry = TypeRegistry::default();
        let expected = FuncType::new([ValType::I32], []);
        assert_eq!(
            func_fit(&expected, &FuncType::new([], []), &registry),
            Fit::Refused
        );
        assert_eq!(
            func_fit(&expected, &FuncType::new([ValType::I64], []), &registry),
            Fit::Refused
        );
        assert_eq!(func_fit(&expected, &expected, &registry), Fit::Links);

        // A function that takes any function reference is a subtype of one
        // that takes only a null one where a module declares it so. The
        // other way round, it never is.
        let any = FuncType::new([ValType::Ref(RefType::FUNCREF)], []);
        let null = FuncType::new([ValType::Ref(RefType::NULLFUNCREF)], []);
        assert_eq!(func_fit(&null, &any, &registry), Fit::Open);
        assert_eq!(func_fit(&any, &null, &registry), Fit::Refused);
    }

    #[wcmp_macros::test]
    fn it_reads_the_limits_of_a_table_or_a_memory_against_its_size_now() {
        assert_eq!(limits_fit((1, None), (2, None)), Fit::Links);
        assert_eq!(limits_fit((3, None), (2, Some(8))), Fit::Refused);
        assert_eq!(limits_fit((1, Some(4)), (2, None)), Fit::Refused);
        assert_eq!(limits_fit((1, Some(4)), (2, Some(8))), Fit::Refused);
        assert_eq!(limits_fit((1, Some(4)), (2, Some(4))), Fit::Links);
    }
}

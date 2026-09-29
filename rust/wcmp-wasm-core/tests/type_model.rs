//! The boundary type model expresses every value type of Wasm 3.0.

use std::collections::HashSet;

use wcmp_wasm_core::backend::RawTypeHandle;
use wcmp_wasm_core::{FuncType, HeapType, RefType, TagType, TypeHandle, Val, ValType};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Every abstract heap type, with its name in the text format.
const ABSTRACT: [(HeapType, &str); 14] = [
    (HeapType::Func, "func"),
    (HeapType::Extern, "extern"),
    (HeapType::Any, "any"),
    (HeapType::Eq, "eq"),
    (HeapType::I31, "i31"),
    (HeapType::Struct, "struct"),
    (HeapType::Array, "array"),
    (HeapType::Exn, "exn"),
    (HeapType::Cont, "cont"),
    (HeapType::NoFunc, "nofunc"),
    (HeapType::NoExtern, "noextern"),
    (HeapType::None, "none"),
    (HeapType::NoExn, "noexn"),
    (HeapType::NoCont, "nocont"),
];

#[wcmp_macros::test]
fn it_expresses_every_number_and_vector_type() {
    let names: Vec<_> = [
        ValType::I32,
        ValType::I64,
        ValType::F32,
        ValType::F64,
        ValType::V128,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(names, ["i32", "i64", "f32", "f64", "v128"]);
}

#[wcmp_macros::test]
fn it_expresses_a_reference_to_every_abstract_heap_type() {
    for (heap, name) in ABSTRACT {
        let nullable = ValType::Ref(RefType::new(true, heap));
        let non_null = ValType::Ref(RefType::new(false, heap));
        assert_eq!(nullable.to_string(), format!("(ref null {name})"));
        assert_eq!(non_null.to_string(), format!("(ref {name})"));
        assert_ne!(nullable, non_null);
    }
}

#[wcmp_macros::test]
fn it_spells_the_shorthand_reference_types() {
    let shorthands = [
        (ValType::FUNCREF, HeapType::Func),
        (ValType::EXTERNREF, HeapType::Extern),
        (ValType::ANYREF, HeapType::Any),
        (ValType::EQREF, HeapType::Eq),
        (ValType::I31REF, HeapType::I31),
        (ValType::STRUCTREF, HeapType::Struct),
        (ValType::ARRAYREF, HeapType::Array),
        (ValType::EXNREF, HeapType::Exn),
        (ValType::CONTREF, HeapType::Cont),
        (ValType::NULLFUNCREF, HeapType::NoFunc),
        (ValType::NULLEXTERNREF, HeapType::NoExtern),
        (ValType::NULLREF, HeapType::None),
        (ValType::NULLEXNREF, HeapType::NoExn),
        (ValType::NULLCONTREF, HeapType::NoCont),
    ];
    for (shorthand, heap) in shorthands {
        assert_eq!(shorthand, ValType::Ref(RefType::new(true, heap)));
    }
}

#[wcmp_macros::test]
fn it_expresses_a_reference_to_a_concrete_type() {
    let handle = TypeHandle::from_raw(3);
    let ty = ValType::Ref(RefType {
        nullable: false,
        heap: HeapType::Concrete(handle),
    });
    assert_eq!(ty.to_string(), "(ref type#3)");
    assert_eq!(
        ty.ref_type().map(|ref_type| ref_type.heap),
        Some(HeapType::Concrete(handle))
    );
}

#[wcmp_macros::test]
fn it_makes_a_null_function_reference_for_a_concrete_type() {
    let concrete = HeapType::Concrete(TypeHandle::from_raw(4));
    assert!(matches!(Val::null(concrete), Val::FuncRef(None)));
    assert!(matches!(
        Val::default_for_ty(&ValType::Ref(RefType::new(true, concrete))),
        Some(Val::FuncRef(None))
    ));
    assert!(Val::default_for_ty(&ValType::Ref(RefType::new(false, concrete))).is_none());
}

#[wcmp_macros::test]
fn it_prints_and_compares_type_handles() {
    let one = TypeHandle::from_raw(1);
    let same = TypeHandle::from_raw(1);
    let other = TypeHandle::from_raw(2);

    assert_eq!(one, same);
    assert_ne!(one, other);
    assert_eq!(one.to_string(), "type#1");
    assert_eq!(format!("{one:?}"), "TypeHandle(type#1)");
    let set: HashSet<_> = [one, same, other].into_iter().collect();
    assert_eq!(set.len(), 2);
    assert_eq!(one.raw(), 1);
}

#[wcmp_macros::test]
fn it_reads_the_parameter_types_of_a_tag() {
    let payload = [
        ValType::I32,
        ValType::Ref(RefType::new(
            false,
            HeapType::Concrete(TypeHandle::from_raw(9)),
        )),
    ];
    let tag = TagType::new(FuncType::new(payload, []));
    assert_eq!(tag.params(), payload);
    assert!(tag.ty().results().is_empty());
}

//! Tags, an extern kind.

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    Capability, Engine, ExportType, Extern, ExternType, FuncType, ImportType, TagType, Val, ValType,
};

use crate::support;

/// A module describes a tag it imports and a tag it exports, with the
/// parameters of each.
pub async fn it_describes_a_tag_at_the_boundary(engine: &Engine) {
    if !support::declares(engine, &[Capability::Exceptions]) {
        return;
    }
    let module = support::module(
        engine,
        wasm!(
            r#"
            (module
              (import "peer" "fault" (tag (param i32 f64)))
              (tag (export "signal") (param i64)))
            "#
        ),
    )
    .await;

    assert_eq!(
        module.imports().cloned().collect::<Vec<_>>(),
        [ImportType::new(
            "peer",
            "fault",
            ExternType::Tag(TagType::new(FuncType::new(
                [ValType::I32, ValType::F64],
                []
            ))),
        )]
    );
    assert_eq!(
        module.exports().cloned().collect::<Vec<_>>(),
        [ExportType::new(
            "signal",
            ExternType::Tag(TagType::new(FuncType::new([ValType::I64], []))),
        )]
    );
}

/// A tag that one instance exports is linked into another. The host reads
/// its parameters, and a guest throws with the tag in the first instance
/// and catches the exception in the second.
pub async fn it_links_a_tag_from_one_instance_into_another(engine: &Engine) {
    if !support::declares(engine, &[Capability::Exceptions]) {
        return;
    }
    let mut store = support::store(engine, ());
    let thrower = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (tag $failure (export "failure") (param i32))
              (func (export "fail") (param i32)
                local.get 0
                throw $failure))
            "#
        ),
        &[],
    )
    .await;

    let failure = thrower
        .get_export(&mut store, "failure")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_tag)
        .expect("the instance exports its tag");
    let ty = failure.ty(&store).expect("the tag belongs to the store");
    assert_eq!(ty.params(), [ValType::I32]);

    let fail = support::func(&mut store, thrower, "fail");
    let catcher = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (import "thrower" "failure" (tag $failure (param i32)))
              (import "thrower" "fail" (func $fail (param i32)))
              (func (export "attempt") (param i32) (result i32)
                block $caught (result i32)
                  try_table (catch $failure $caught)
                    local.get 0
                    call $fail
                  end
                  i32.const -1
                end))
            "#
        ),
        &[failure.into(), fail.into()],
    )
    .await;

    let attempt = support::func(&mut store, catcher, "attempt");
    let caught = support::call(&mut store, attempt, &[Val::I32(7)], &[ValType::I32]);
    assert_eq!(
        caught[0].i32(),
        Some(7),
        "the catcher receives the payload of the exception the thrower threw"
    );
}

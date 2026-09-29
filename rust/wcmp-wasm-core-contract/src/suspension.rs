//! Host suspension, where a backend does not declare it.

use core::task::Poll;

use wcmp_macros::wasm;
use wcmp_wasm_core::{Capability, Engine, Error, Func, FuncType, Val, ValType};

use crate::support;

/// Where the engine does not declare `host_suspension`, a suspending host
/// function and a resumable call are each
/// [`Error::Unsupported`] with `host_suspension`.
pub async fn it_refuses_host_suspension_where_it_is_not_declared(engine: &Engine) {
    if support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let mut store = support::store(engine, ());

    let suspending =
        Func::new_suspending(&mut store, FuncType::new([], [ValType::I32]), |_, _, _| {
            Ok(Poll::Pending)
        });
    assert!(
        matches!(
            suspending,
            Err(Error::Unsupported(Capability::HostSuspension))
        ),
        "{suspending:?}"
    );

    let instance = support::instance(
        &mut store,
        wasm!(r#"(module (func (export "answer") (result i32) i32.const 42))"#),
        &[],
    )
    .await;
    let answer = support::func(&mut store, instance, "answer");
    let resumable = answer
        .call_resumable(&mut store, &[], &mut [Val::I32(0)])
        .await;
    assert!(
        matches!(
            resumable,
            Err(Error::Unsupported(Capability::HostSuspension))
        ),
        "{resumable:?}"
    );
}

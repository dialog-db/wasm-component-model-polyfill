//! Host functions.

use core::fmt;

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    Caller, Capability, Engine, Error, Extern, Func, FuncType, TrapKind, Val, ValType,
};

use crate::support;

/// What the store of the test of re-entry holds: the guest function the
/// host function calls back into, and the results each depth returned.
#[derive(Default)]
struct Descent {
    down: Option<Func>,
    returns: Vec<(i32, i32, i32)>,
}

/// A host function calls back into the guest, and the guest calls the same
/// host function again, four times over. Each depth keeps its own
/// arguments and returns its own results.
pub async fn it_enters_a_host_function_again_at_any_depth(engine: &Engine) {
    let mut store = support::store(engine, Descent::default());
    let descend = Func::new(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32, ValType::I32]),
        |mut caller: Caller<'_, Descent>, params, results| {
            let depth = argument(params)?;
            let (count, sum) = if depth == 0 {
                (0, 0)
            } else {
                let down = caller
                    .data()
                    .down
                    .ok_or_else(|| anyhow::anyhow!("the guest function is not set"))?;
                let mut inner = [Val::I32(0), Val::I32(0)];
                down.call(&mut caller, &[Val::I32(depth - 1)], &mut inner)?;
                (argument(&inner[..1])? + 1, argument(&inner[1..])? + depth)
            };
            // The call below this depth ran, and returned, while this depth
            // waited. Its arguments are still this depth's own.
            anyhow::ensure!(argument(params)? == depth, "the arguments changed");
            results[0] = Val::I32(count);
            results[1] = Val::I32(sum);
            caller.data_mut().returns.push((depth, count, sum));
            Ok(())
        },
    )
    .expect("the store makes a host function");

    let instance = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (import "host" "descend" (func $descend (param i32) (result i32 i32)))
              (func (export "down") (param i32) (result i32 i32)
                local.get 0
                call $descend))
            "#
        ),
        &[descend.into()],
    )
    .await;
    let down = support::func(&mut store, instance, "down");
    store.data_mut().down = Some(down);

    let results = support::call(
        &mut store,
        down,
        &[Val::I32(4)],
        &[ValType::I32, ValType::I32],
    );
    assert_eq!((results[0].i32(), results[1].i32()), (Some(4), Some(10)));
    assert_eq!(
        store.data().returns,
        [(0, 0, 0), (1, 1, 1), (2, 2, 3), (3, 3, 6), (4, 4, 10)],
        "each depth returned its own results, the deepest first"
    );
}

/// The one `i32` of `values`.
fn argument(values: &[Val]) -> anyhow::Result<i32> {
    match values {
        [value] => value
            .i32()
            .ok_or_else(|| anyhow::anyhow!("{value:?} is not an i32")),
        _ => anyhow::bail!("{} values where one i32 was expected", values.len()),
    }
}

/// The error the failing host function returns.
#[derive(Debug)]
struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the host refuses")
    }
}

impl core::error::Error for Refusal {}

/// A guest calls a host function that fails, inside a `try_table` with
/// `catch_all`. The call fails with [`TrapKind::Host`] and the host's own
/// error, unchanged, and the guest's handler does not run.
///
/// Where the engine does not declare `exceptions`, the guest calls the
/// failing host function without a handler, and the call fails the same
/// way.
pub async fn it_traps_with_the_host_error_that_no_guest_can_catch(engine: &Engine) {
    let mut store = support::store(engine, ());
    let fail = Func::new(&mut store, FuncType::new([], []), |_, _, _| {
        Err(anyhow::Error::new(Refusal))
    })
    .expect("the store makes a host function");

    let bytes: &[u8] = if support::declares(engine, &[Capability::Exceptions]) {
        wasm!(
            r#"
            (module
              (import "host" "fail" (func $fail))
              (global $handled (export "handled") (mut i32) (i32.const 0))
              (func (export "guarded")
                block $caught
                  try_table (catch_all $caught)
                    call $fail
                  end
                  return
                end
                i32.const 1
                global.set $handled))
            "#
        )
    } else {
        wasm!(
            r#"
            (module
              (import "host" "fail" (func $fail))
              (global $handled (export "handled") (mut i32) (i32.const 0))
              (func (export "guarded")
                call $fail))
            "#
        )
    };
    let instance = support::instance(&mut store, bytes, &[fail.into()]).await;
    let guarded = support::func(&mut store, instance, "guarded");

    match guarded.call(&mut store, &[], &mut []) {
        Err(Error::Trap(TrapKind::Host(error))) => {
            assert!(
                error.downcast_ref::<Refusal>().is_some(),
                "the trap carries the host's own error: {error:?}"
            );
            assert_eq!(error.to_string(), "the host refuses");
        }
        other => panic!("the call fails with the host's error: {other:?}"),
    }

    let handled = instance
        .get_export(&mut store, "handled")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_global)
        .expect("the instance exports its global");
    let handled = handled
        .get(&mut store)
        .expect("the global belongs to the store");
    assert_eq!(handled.i32(), Some(0), "the guest's handler did not run");
}

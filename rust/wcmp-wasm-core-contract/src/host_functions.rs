// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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

/// A guest calls a host function of ten parameters, of each number type,
/// and the host function returns three results. Each argument reaches the
/// host in its place, and each result reaches the guest in its place.
pub async fn it_calls_a_host_function_of_more_than_eight_parameters(engine: &Engine) {
    let mut store = support::store(engine, ());
    let params = [
        ValType::I32,
        ValType::I64,
        ValType::F32,
        ValType::F64,
        ValType::I32,
        ValType::I64,
        ValType::F32,
        ValType::F64,
        ValType::I32,
        ValType::I64,
    ];
    let tally = Func::new(
        &mut store,
        FuncType::new(params, [ValType::I64, ValType::F64, ValType::I32]),
        |_, params, results| {
            let (mut integers, mut floats) = (0i64, 0f64);
            for (place, value) in params.iter().enumerate() {
                // Each argument counts times its place, so an argument
                // out of its place changes the sums.
                let place = place as i64 + 1;
                match value {
                    Val::I32(value) => integers += i64::from(*value) * place,
                    Val::I64(value) => integers += *value * place,
                    Val::F32(bits) => floats += f64::from(f32::from_bits(*bits)) * place as f64,
                    Val::F64(bits) => floats += f64::from_bits(*bits) * place as f64,
                    other => anyhow::bail!("{other:?} is not a number"),
                }
            }
            results[0] = Val::I64(integers);
            results[1] = Val::F64(floats.to_bits());
            results[2] = Val::I32(params.len() as i32);
            Ok(())
        },
    )
    .expect("the store makes a host function");

    let instance = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (import "host" "tally"
                (func $tally
                  (param i32 i64 f32 f64 i32 i64 f32 f64 i32 i64)
                  (result i64 f64 i32)))
              (func (export "relay")
                (param i32 i64 f32 f64 i32 i64 f32 f64 i32 i64)
                (result i64 f64 i32)
                local.get 0
                local.get 1
                local.get 2
                local.get 3
                local.get 4
                local.get 5
                local.get 6
                local.get 7
                local.get 8
                local.get 9
                call $tally))
            "#
        ),
        &[tally.into()],
    )
    .await;
    let relay = support::func(&mut store, instance, "relay");

    let results = support::call(
        &mut store,
        relay,
        &[
            Val::I32(-1),
            Val::I64(1 << 40),
            Val::F32(0.5f32.to_bits()),
            Val::F64(0.25f64.to_bits()),
            Val::I32(7),
            Val::I64(-3),
            Val::F32(2.0f32.to_bits()),
            Val::F64((-1.5f64).to_bits()),
            Val::I32(i32::MAX),
            Val::I64(i64::from(u32::MAX)),
        ],
        &[ValType::I64, ValType::F64, ValType::I32],
    );
    let integers =
        -1 + (1i64 << 40) * 2 + 7 * 5 - 3 * 6 + i64::from(i32::MAX) * 9 + i64::from(u32::MAX) * 10;
    let floats = 0.5 * 3.0 + 0.25 * 4.0 + 2.0 * 7.0 - 1.5 * 8.0;
    assert_eq!(results[0].i64(), Some(integers));
    assert_eq!(results[1].f64(), Some(floats));
    assert_eq!(results[2].i32(), Some(10));
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

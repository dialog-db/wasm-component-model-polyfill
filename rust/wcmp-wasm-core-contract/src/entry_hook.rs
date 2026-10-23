// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The entry hook of a store.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wcmp_macros::wasm;
use wcmp_wasm_core::{AsContextMut, Engine, Error, Val, ValType};

use crate::support;

/// A module whose export answers its argument.
const ECHO: &[u8] = wasm!(
    r#"
    (module
      (func (export "echo") (param i32) (result i32)
        local.get 0))
    "#
);

/// The engine calls the store's entry hook once before a call into a
/// guest and once after it, and not at all once the hook is removed.
pub async fn it_calls_the_entry_hook_around_a_call_into_a_guest(engine: &Engine) {
    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, ECHO, &[]).await;
    let echo = support::func(&mut store, instance, "echo");
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    store
        .as_context_mut()
        .set_entry_hook(Some(Arc::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })));

    let results = support::call(&mut store, echo, &[Val::I32(7)], &[ValType::I32]);
    assert!(
        matches!(results.as_slice(), [Val::I32(7)]),
        "the call answers its argument"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the hook runs before the call and after it"
    );

    store.as_context_mut().set_entry_hook(None);
    support::call(&mut store, echo, &[Val::I32(8)], &[ValType::I32]);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a store with no hook calls none"
    );
}

/// An error from the entry hook is the error of the call it runs
/// around, before the guest runs and after it returned.
pub async fn it_fails_a_call_with_the_error_of_its_entry_hook(engine: &Engine) {
    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, ECHO, &[]).await;
    let echo = support::func(&mut store, instance, "echo");
    let fail_after = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let (failing, counted) = (fail_after.clone(), calls.clone());
    store
        .as_context_mut()
        .set_entry_hook(Some(Arc::new(move |_| {
            // The first run of each call is before the guest runs.
            let before = counted.fetch_add(1, Ordering::SeqCst) % 2 == 0;
            if before != failing.load(Ordering::SeqCst) {
                Err(Error::Backend {
                    message: "the hook failed".to_owned(),
                })
            } else {
                Ok(())
            }
        })));

    let mut results = [Val::I32(0)];
    let before = echo.call(&mut store, &[Val::I32(7)], &mut results);
    assert!(
        matches!(&before, Err(Error::Backend { message }) if message == "the hook failed"),
        "a hook that fails before the guest runs fails the call, got {before:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "and the guest does not run, so the hook does not run after it"
    );

    calls.store(0, Ordering::SeqCst);
    fail_after.store(true, Ordering::SeqCst);
    let after = echo.call(&mut store, &[Val::I32(8)], &mut results);
    assert!(
        matches!(&after, Err(Error::Backend { message }) if message == "the hook failed"),
        "a hook that fails after the guest returned fails the call, got {after:?}"
    );
}

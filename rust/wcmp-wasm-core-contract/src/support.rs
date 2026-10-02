// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the tests of the contract share.

use wcmp_wasm_core::{Capability, Engine, Extern, Func, Instance, Module, Store, Val, ValType};

/// Whether `engine` declares every capability of `capabilities`.
pub fn declares(engine: &Engine, capabilities: &[Capability]) -> bool {
    capabilities
        .iter()
        .all(|capability| engine.capabilities().contains(*capability))
}

/// A new store of `engine` that owns `data`.
pub fn store<T: wcmp_wasm_core::MaybeSend + 'static>(engine: &Engine, data: T) -> Store<T> {
    Store::new(engine, data).expect("the engine makes a store")
}

/// The module `bytes`, compiled asynchronously on `engine`.
pub async fn module(engine: &Engine, bytes: &[u8]) -> Module {
    Module::compile(engine, bytes)
        .await
        .expect("the engine compiles the module")
}

/// An instance of `bytes` in `store`, with `imports`.
pub async fn instance<T: 'static>(
    store: &mut Store<T>,
    bytes: &[u8],
    imports: &[Extern],
) -> Instance {
    let module = module(store.engine(), bytes).await;
    Instance::instantiate(store, &module, imports)
        .await
        .expect("the module instantiates")
}

/// The function `instance` exports as `name`.
pub fn func<T: 'static>(store: &mut Store<T>, instance: Instance, name: &str) -> Func {
    instance
        .get_export(store, name)
        .expect("the instance belongs to the store")
        .and_then(Extern::into_func)
        .unwrap_or_else(|| panic!("the instance exports a function `{name}`"))
}

/// The results of a call of `func` with `params`, one for each of the
/// `results` types the function gives.
pub fn call<T: 'static>(
    store: &mut Store<T>,
    func: Func,
    params: &[Val],
    results: &[ValType],
) -> Vec<Val> {
    let mut outputs = results
        .iter()
        .map(|ty| Val::default_for_ty(ty).unwrap_or(Val::I32(0)))
        .collect::<Vec<_>>();
    func.call(store, params, &mut outputs)
        .unwrap_or_else(|error| panic!("the call succeeds: {error}"));
    outputs
}

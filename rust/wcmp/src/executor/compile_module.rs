// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compiling the core modules of a component against the engine.
//!
//! The runtime layer compiles asynchronously on every backend. A
//! native backend finishes at once. In the browser the backend
//! compiles with `WebAssembly.compile`, the browser's asynchronous
//! path, because the synchronous `WebAssembly.Module` constructor is
//! refused on the main thread above a size limit.
//!
//! A component carries several core modules, and the browser compiles
//! each on its own threads once `WebAssembly.compile` is called. The
//! compiles of one component are therefore all issued before any is
//! awaited, so the browser works on them side by side and the
//! component waits for the slowest rather than for their sum.
//!
//! Modules of one component with the same bytes are compiled once and
//! share the one module built from them: a compiled module is
//! immutable, and each instantiation of it is its own.

use core::future::Future;
use core::pin::Pin;
use core::task::Poll;

use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::internal::{EngineInternal, ErrorInternal};
use crate::runtime_layer::{Module as RuntimeModule, substrate_failure};

/// Compile `bytes`, one core module, against `engine`. A module the
/// runtime substrate refuses fails as the substrate's failure.
pub async fn compile_module(engine: &Engine, bytes: &[u8]) -> Result<RuntimeModule> {
    compile_modules(engine, &[bytes])
        .await?
        .pop()
        .ok_or_else(|| Error::internal("compiling one core module answered no module"))
}

/// Compile every core module in `modules` against `engine`, and answer
/// them in the same order. Modules with the same bytes are compiled
/// once and answered as clones of one module. The compiles are issued
/// together and awaited as one. The translator already validated the
/// modules; a failure here means the runtime substrate refused a valid
/// module.
pub async fn compile_modules(engine: &Engine, modules: &[&[u8]]) -> Result<Vec<RuntimeModule>> {
    let (distinct, slots) = distinct_modules(modules);
    let compiled = all(distinct
        .iter()
        .map(|bytes| RuntimeModule::compile(engine.inner(), bytes)))
    .await
    .map_err(substrate_failure)?;
    Ok(slots
        .into_iter()
        .map(|slot| compiled[slot].clone())
        .collect())
}

/// The distinct byte strings of `modules` in order of first
/// appearance, and for each module the index of its bytes among them.
fn distinct_modules<'a>(modules: &[&'a [u8]]) -> (Vec<&'a [u8]>, Vec<usize>) {
    let mut distinct: Vec<&'a [u8]> = Vec::with_capacity(modules.len());
    let mut index: std::collections::HashMap<&'a [u8], usize> =
        std::collections::HashMap::with_capacity(modules.len());
    let slots = modules
        .iter()
        .map(|&bytes| {
            *index.entry(bytes).or_insert_with(|| {
                distinct.push(bytes);
                distinct.len() - 1
            })
        })
        .collect();
    (distinct, slots)
}

/// Drive every future in `futures` to completion together, and answer
/// their values in the order the futures came, or the first failure
/// any of them meets. Each is polled once before the first wait, which
/// is what starts every compile before the browser is waited on for
/// any of them. On the first failure the rest are dropped unfinished.
async fn all<F, T, E>(futures: impl Iterator<Item = F>) -> core::result::Result<Vec<T>, E>
where
    F: Future<Output = core::result::Result<T, E>>,
{
    let mut pending: Vec<(usize, Pin<Box<F>>)> = futures.map(Box::pin).enumerate().collect();
    let mut finished: Vec<Option<T>> = pending.iter().map(|_| None).collect();
    core::future::poll_fn(move |cx| {
        let mut index = 0;
        while index < pending.len() {
            match pending[index].1.as_mut().poll(cx) {
                Poll::Ready(Ok(value)) => {
                    finished[pending[index].0] = Some(value);
                    pending.swap_remove(index);
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => index += 1,
            }
        }
        if pending.is_empty() {
            Poll::Ready(Ok(finished.drain(..).flatten().collect()))
        } else {
            Poll::Pending
        }
    })
    .await
}

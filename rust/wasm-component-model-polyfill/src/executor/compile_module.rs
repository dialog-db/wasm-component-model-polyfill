//! Compiling the core modules of a component against the engine.
//!
//! The runtime layer's `Module::new` is synchronous. On native that
//! is the whole story. In the browser the synchronous
//! `WebAssembly.Module` constructor is refused on the main thread
//! above a size limit, so the backend first compiles the bytes with
//! `WebAssembly.compile`, the browser's asynchronous path, and keeps
//! the result on the engine keyed by the bytes. The runtime layer's
//! constructor then takes that entry, and a constructor that finds
//! none compiles synchronously.
//!
//! A component carries several core modules, and the browser compiles
//! each on its own threads once `WebAssembly.compile` is called. The
//! compiles of one component are therefore all issued before any is
//! awaited, so the browser works on them side by side and the
//! component waits for the slowest rather than for their sum.
//!
//! Two things follow from the handoff through the engine. An entry
//! serves one constructor, so modules of one component with the same
//! bytes are compiled once and share the one module built from it;
//! a compiled module is immutable, and each instantiation of it is
//! its own. And an entry nobody takes stays on the engine for its
//! lifetime, so a batch that ends before every module is built — one
//! compile failed, or the caller dropped the future — discards the
//! entries of the compiles that did finish.

use wasm_runtime_layer::Module as RuntimeModule;

use crate::engine::Engine;
use crate::error::{Error, InstantiationError, Result};
use crate::internal::{EngineInternal, ErrorInternal};

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
/// once and answered as clones of one module. In the browser the
/// asynchronous compiles are issued together and awaited as one; on
/// native each module compiles synchronously in turn. The translator
/// already validated the modules; a failure here means the runtime
/// substrate refused a valid module.
pub async fn compile_modules(engine: &Engine, modules: &[&[u8]]) -> Result<Vec<RuntimeModule>> {
    let (distinct, slots) = distinct_modules(modules);
    #[cfg(target_arch = "wasm32")]
    let _pending = precompile_all(engine, &distinct).await?;
    let compiled = distinct
        .iter()
        .map(|bytes| {
            RuntimeModule::new(engine.inner(), bytes)
                .map_err(InstantiationError::SubstrateFailure)
                .map_err(Error::from)
        })
        .collect::<Result<Vec<_>>>()?;
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

/// The entries a batch of asynchronous compiles left on the engine
/// that no constructor has taken yet. Dropping it discards them: a
/// constructor that ran has already taken its entry, so what is left
/// is what the batch will not build. Nothing else runs between the
/// last constructor and the drop, so no other batch's entry for the
/// same bytes can be in the map by then.
#[cfg(target_arch = "wasm32")]
struct Precompiled<'a> {
    backend: js_wasm_runtime_layer::Engine,
    finished: Vec<&'a [u8]>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for Precompiled<'_> {
    fn drop(&mut self) {
        for bytes in &self.finished {
            self.backend.discard_precompiled(bytes);
        }
    }
}

/// Compile every byte string in `distinct` asynchronously against
/// `engine`, and answer the entries they left once all finished. On
/// the first failure the rest are dropped unfinished, which leaves no
/// entry, and the entries of those that finished are discarded.
#[cfg(target_arch = "wasm32")]
async fn precompile_all<'a>(engine: &Engine, distinct: &[&'a [u8]]) -> Result<Precompiled<'a>> {
    let backend = engine.inner().clone().into_backend();
    let mut pending = Precompiled {
        backend: backend.clone(),
        finished: Vec::with_capacity(distinct.len()),
    };
    all(
        distinct.iter().map(|&bytes| {
            let backend = backend.clone();
            async move { backend.precompile(bytes).await.map(|()| bytes) }
        }),
        &mut pending.finished,
    )
    .await
    .map_err(InstantiationError::SubstrateFailure)
    .map_err(Error::from)?;
    Ok(pending)
}

/// Drive every future in `futures` to completion together, pushing
/// each one's value onto `finished` as it completes, and answer the
/// first failure any of them meets. Each is polled once before the
/// first wait, which is what starts every compile before the browser
/// is waited on for any of them.
#[cfg(target_arch = "wasm32")]
async fn all<F, T, E>(
    futures: impl Iterator<Item = F>,
    finished: &mut Vec<T>,
) -> core::result::Result<(), E>
where
    F: core::future::Future<Output = core::result::Result<T, E>>,
{
    use core::task::Poll;

    let mut pending: Vec<core::pin::Pin<Box<F>>> = futures.map(Box::pin).collect();
    core::future::poll_fn(move |cx| {
        let mut index = 0;
        while index < pending.len() {
            match pending[index].as_mut().poll(cx) {
                Poll::Ready(Ok(value)) => {
                    finished.push(value);
                    pending.swap_remove(index);
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => index += 1,
            }
        }
        if pending.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    })
    .await
}

//! A built-in that the reference lets wait inside a guest call.

use std::sync::Arc;

use anyhow::anyhow;

use crate::runtime_layer::{Func as RuntimeFunc, FuncType, Val as RuntimeVal, ValType};
use crate::store::{StoreContext, StoreContextInternalExt};

use super::block_step::BlockStep;
use super::suspend_seam::SuspendSeam;
use super::switch_module::SwitchModule;

/// The first part of one blocking built-in, as a host function body.
type Begin<T> = dyn Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> anyhow::Result<BlockStep<T>>
    + Send
    + Sync
    + 'static;

/// The whole of one blocking built-in, as a host function body, for
/// a built-in that waits in a way of its own where its thread cannot
/// suspend its stack.
type Whole<T> = dyn Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> anyhow::Result<Option<Vec<RuntimeVal>>>
    + Send
    + Sync
    + 'static;

/// Whether one call of a built-in names a thread to switch to, as a
/// host function body.
type Switches<T> = dyn Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> bool + Send + Sync + 'static;

/// A built-in that the reference lets wait inside a guest call:
/// `waitable-set.wait`, a synchronous stream or future copy or
/// cancel, the synchronous start of a call into another component,
/// `thread.yield`, the five thread built-ins that suspend or switch,
/// and the synchronous lower of a host `async` function.
///
/// The value holds the built-in's core type and its first part,
/// which answers a [`BlockStep`]. It becomes one of two things for
/// the guest to import:
///
/// - [`trampoline`](Self::trampoline), a host function that runs the
///   first part and, when it waits, waits through the suspend seam's
///   nested turn and then runs the finish part. This is what a guest
///   imports when the engine has no provider.
/// - [`parts`](Self::parts), the try and the finish host functions
///   that the switch module's shim for the built-in calls. This is
///   what the shim a guest imports under a provider stands on. The
///   try part runs the first part, records the wait on the thread,
///   and answers whether the built-in is ready, and the shim
///   suspends the thread when it is not. The finish part runs once
///   the condition holds. [`SuspendSeam::try_block`] and
///   [`SuspendSeam::finish_block`] state the rules.
///
/// A built-in made [`with_fallback`](Self::with_fallback) brings the
/// whole of itself for a thread that cannot suspend its stack: the
/// host trampoline of a store with no provider, and what its try part
/// runs, under a provider, for a thread that runs on another thread's
/// stack or of a task that must not block. Its first part then serves
/// only a thread that suspends. The thread built-ins that suspend or
/// switch are such built-ins, because a switch made where the thread
/// cannot suspend runs the thread it names from inside itself, rather
/// than leaving it to the frame that resumed the switching thread.
///
/// A built-in made [`switching`](Self::switching) also answers, for
/// one call, whether that call names a thread to switch to. Under a
/// provider, a thread of a task that must not block suspends in the
/// shim for such a call if it runs on a stack of its own. It does not
/// take the fallback. The switch hands control to the named thread
/// and waits on nothing, so it blocks nothing, and Wasmtime suspends
/// the thread there too.
pub struct BlockingBuiltin<T: 'static> {
    ty: FuncType,
    begin: Arc<Begin<T>>,
    fallback: Option<Arc<Whole<T>>>,
    switches: Option<Arc<Switches<T>>>,
}

impl<T: 'static> Clone for BlockingBuiltin<T> {
    fn clone(&self) -> Self {
        Self {
            ty: self.ty.clone(),
            begin: self.begin.clone(),
            fallback: self.fallback.clone(),
            switches: self.switches.clone(),
        }
    }
}

impl<T: 'static> BlockingBuiltin<T> {
    /// A blocking built-in of core type `ty`, whose first part is
    /// `begin`.
    pub fn new(
        ty: FuncType,
        begin: impl Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> anyhow::Result<BlockStep<T>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            ty,
            begin: Arc::new(begin),
            fallback: None,
            switches: None,
        }
    }

    /// A blocking built-in of core type `ty`, whose first part is
    /// `begin` for a thread that suspends its stack, and whose whole
    /// is `fallback` for a thread that cannot. The fallback answers
    /// `None` when it left the rest of itself to the scheduler as a
    /// plan, having parked its step for the plan's outcome.
    pub fn with_fallback(
        ty: FuncType,
        begin: impl Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> anyhow::Result<BlockStep<T>>
        + Send
        + Sync
        + 'static,
        fallback: impl Fn(
            &mut StoreContext<'_, T>,
            &[RuntimeVal],
        ) -> anyhow::Result<Option<Vec<RuntimeVal>>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            ty,
            begin: Arc::new(begin),
            fallback: Some(Arc::new(fallback)),
            switches: None,
        }
    }

    /// The same built-in, where `switches` answers whether one call of
    /// it names a thread to switch to.
    pub fn switching(
        mut self,
        switches: impl Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.switches = Some(Arc::new(switches));
        self
    }

    /// The built-in's core type, which is its shim's type too.
    pub fn ty(&self) -> &FuncType {
        &self.ty
    }

    /// The host function a guest imports when the engine has no
    /// provider: the whole built-in, which waits where it stands.
    pub fn trampoline(&self, store: &mut StoreContext<'_, T>) -> RuntimeFunc {
        let begin = self.begin.clone();
        let fallback = self.fallback.clone();
        RuntimeFunc::new(
            store.internal().runtime_mut(),
            self.ty.clone(),
            move |store_ctx, args, results| {
                let mut store = StoreContext::new(store_ctx);
                let values = match &fallback {
                    Some(whole) => whole(&mut store, args)?.ok_or_else(|| {
                        anyhow!("a blocking built-in left a plan with no provider to run it")
                    })?,
                    None => SuspendSeam::block(&mut store, &*begin, args)?,
                };
                deliver(results, values)
            },
        )
    }

    /// The try and the finish host functions the built-in's shim
    /// calls under a provider.
    pub fn parts(&self, store: &mut StoreContext<'_, T>) -> (RuntimeFunc, RuntimeFunc) {
        let begin = self.begin.clone();
        let fallback = self.fallback.clone();
        let switches = self.switches.clone();
        let try_part = RuntimeFunc::new(
            store.internal().runtime_mut(),
            FuncType::new(self.ty.params().iter().copied(), [ValType::I32]),
            move |store_ctx, args, results| {
                let mut store = StoreContext::new(store_ctx);
                if store.internal().dropped() {
                    // The owner dropped the store while this thread's
                    // resume was under way, and the store was kept
                    // only for this call. The shim traps on the
                    // answer, so the store is freed once the stack
                    // has unwound.
                    if let Some(provider) = store.internal().provider() {
                        provider.release_dropped(&mut store);
                    }
                    results[0] = RuntimeVal::I32(SwitchModule::DROPPED);
                    return Ok(());
                }
                let ready = SuspendSeam::try_block(
                    &mut store,
                    &*begin,
                    fallback.as_ref().map(|whole| &**whole as _),
                    switches.as_ref().map(|switches| &**switches as _),
                    args,
                )?;
                results[0] = RuntimeVal::I32(i32::from(ready));
                Ok(())
            },
        );
        let finish_part = RuntimeFunc::new(
            store.internal().runtime_mut(),
            self.ty.clone(),
            move |store_ctx, _args, results| {
                let mut store = StoreContext::new(store_ctx);
                let values = SuspendSeam::finish_block(&mut store)?;
                deliver(results, values)
            },
        );
        (try_part, finish_part)
    }
}

/// Write a built-in's results into the slots the runtime layer
/// handed its host function.
fn deliver(results: &mut [RuntimeVal], values: Vec<RuntimeVal>) -> anyhow::Result<()> {
    if values.len() != results.len() {
        return Err(anyhow!(
            "a blocking built-in produced {} results for a core type with {}",
            values.len(),
            results.len()
        ));
    }
    for (slot, value) in results.iter_mut().zip(values) {
        *slot = value;
    }
    Ok(())
}

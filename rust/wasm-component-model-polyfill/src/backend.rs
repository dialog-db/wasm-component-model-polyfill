//! Selection of the runtime-layer backend the polyfill is built
//! against.
//!
//! The choice is target-conditional and an implementation detail —
//! the public API never names the backend directly. Native targets
//! use the Wasmtime backend (which participates only as a core-Wasm
//! engine; the polyfill's component-level work is implemented above
//! the runtime layer, not delegated to `wasmtime::component`).
//! `wasm32-unknown-unknown` uses the browser's native `WebAssembly`
//! interface via the `js_wasm` backend. This module is workspace-
//! private; nothing inside it is re-exported by `lib.rs`.
//!
//! # One divergence between the two backends
//!
//! A native engine calls a host function at any depth: a host
//! function already on the stack is called again with no more
//! ceremony than any other. The browser cannot. A host function
//! there is one JavaScript function object over one Rust closure,
//! and the arguments and the results buffer of a call belong to
//! that call alone, so a second call made while the first is still
//! running has nowhere to put them. The browser backend detects that
//! second call and refuses it; the message it carries and the
//! reasoning behind it are recorded in that backend's `PATCHES.md`.
//!
//! What counts as one host function is one trampoline of the
//! translated component, and the translator merges trampolines that
//! are the same. Most of them name the component instance they act
//! for: a lowered import through its canonical options, and a
//! built-in the guest imports directly, such as `resource.drop`,
//! `task.return`, the waitable-set built-ins, `subtask.drop` or
//! `thread.yield`, through its own field. So two `canon
//! resource.drop` of one resource type in one instance, two `canon
//! waitable-set.wait` over the same options, or two identical lowers
//! of one import are each a single host function, and the same import
//! or built-in in another component instance is a host function of
//! its own. The intrinsics a fused adapter calls name no instance.
//! The prepare intrinsic names only the callee's memory. The
//! synchronous and asynchronous start intrinsics name only the
//! callee's callback and post-return function. The intrinsics that
//! enter and leave a synchronous call and the two resource transfers
//! name nothing, and a transcoder names only the two memories it
//! copies between. Each of them is one host function for every
//! adapter that names the same things, whichever caller instance the
//! adapter serves.
//!
//! The polyfill reaches the divergence two ways. One is a nested
//! turn, and the rule for it is broad: without a suspend provider,
//! a nested turn cannot enter any host function already on the
//! stack. A blocking built-in or intrinsic that finds no suspend
//! provider runs turns of the store from inside itself, so for as
//! long as the block lasts its own trampoline is on the stack, and so
//! is every host function the frames above it are inside. Work those
//! turns run is ordinary guest work and may call any of them again.
//! The import the block is inside is one: a synchronous lower of a
//! host `async` import blocks inside that import's trampoline, and an
//! item of the turn that calls the same import calls it a second
//! time. The host function of an outer nesting level is another: a
//! turn nested inside a turn leaves the outer block on the stack as
//! well, so the inner turn cannot enter the import or built-in the
//! outer block is inside either. An adapter intrinsic is a third, and
//! it needs no import called twice by one instance: two different
//! caller instances that synchronously lower the same asynchronous
//! callee export both reach the one `sync-start-call` of that callee,
//! so the second caller's call is a second call of the host function
//! the first caller's block is inside. Each of those runs natively
//! and fails in the browser. A call to a host function no frame on
//! the stack is inside runs the same way on both targets.
//!
//! The other is a destructor. A `resource.drop` is a host function
//! of its own, and it runs the resource's destructor from inside
//! itself, so a destructor that drops a second handle of the same
//! resource type in the same instance calls that same host function
//! a second time, through whichever of the instance's
//! `resource.drop` definitions of that type it names. That call too
//! runs natively and fails in the browser, with no nested turn in
//! it at all.
//!
//! [`substrate_failure`] is where the refusal becomes a polyfill
//! error: it answers the `ReentrantHostCall` scheduler cause, so the
//! failure reaches a host as a cause it can branch on rather than as
//! a backend string. Every call of a guest function goes through it,
//! or through [`reentrant_refusal`] where the site already has a
//! structured error of its own for an ordinary failure — a
//! canonical-ABI crossing, or a destructor. A site that stringified
//! the failure instead would leave the cause unreadable, because a
//! string is not something a host can branch on.
//!
//! The cause is recorded only when the call has no earlier host
//! failure recorded yet: the backend keeps the first failure of a
//! call, because a guest may catch the exception and trap again. A
//! guest that caught the exception of an earlier failing host
//! function and then makes a re-entrant call therefore reports that
//! earlier failure, not the re-entrant cause, although the call is
//! refused all the same.

use crate::error::{Error, InstantiationError, SchedulerCause};

#[cfg(not(target_arch = "wasm32"))]
pub type Backend = wasmtime_runtime_layer::Engine;

#[cfg(target_arch = "wasm32")]
pub type Backend = js_wasm_runtime_layer::Engine;

/// The polyfill error the browser backend's refusal of a re-entrant
/// host call becomes, and nothing at all for any other failure.
///
/// The refusal names a limitation of the target rather than anything
/// the call did, and a host may want to tell it apart from a guest
/// that trapped. It travels as the backend's own error type, under
/// whatever context the frames between the refusal and here added,
/// so this reads it back off the error the call failed with. A site
/// that answered the refusal once already and is handing its own
/// error on — a destructor the host wrote, say — carries the
/// polyfill error instead, so that shape is read back too.
///
/// A guest-call site with a structured error of its own for an
/// ordinary failure asks this first and keeps its own error for
/// everything else, which is how a refusal avoids being reported as
/// whatever the site happened to be doing. [`substrate_failure`] is
/// the same question with a substrate failure as the other answer.
/// Workspace-internal.
pub fn reentrant_refusal(error: &anyhow::Error) -> Option<Error> {
    #[cfg(target_arch = "wasm32")]
    if error
        .downcast_ref::<js_wasm_runtime_layer::ReentrantHostCall>()
        .is_some()
    {
        return Some(Error::Scheduler(SchedulerCause::ReentrantHostCall));
    }
    match error.downcast_ref::<Error>() {
        Some(Error::Scheduler(SchedulerCause::ReentrantHostCall)) => {
            Some(Error::Scheduler(SchedulerCause::ReentrantHostCall))
        }
        _ => None,
    }
}

/// The polyfill error a runtime-layer failure of a guest call
/// becomes.
///
/// Every such failure is a substrate failure, with one exception:
/// the browser backend's refusal of a re-entrant host call, which
/// [`reentrant_refusal`] answers with a cause of its own. The native
/// backend cannot produce that refusal, because a native engine
/// calls a host function at any depth. Workspace-internal.
pub fn substrate_failure(error: anyhow::Error) -> Error {
    match reentrant_refusal(&error) {
        Some(refusal) => refusal,
        None => Error::from(InstantiationError::SubstrateFailure(error)),
    }
}

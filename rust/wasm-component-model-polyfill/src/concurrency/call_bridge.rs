//! The two generated functions that carry one call between
//! components.

use wasm_runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};

use crate::abi::layout::FlatType;

use super::caller_kind::CallerKind;
use super::thread_id::ThreadId;

/// Everything the polyfill needs to carry one prepared call between
/// two components.
///
/// The fused adapter compiler generates two functions per such call
/// and hands them to the prepare intrinsic as function references.
/// The start function takes the caller's flat arguments, lifts them
/// in the caller, and lowers them into the callee. The return
/// function takes the arguments of the callee's `task.return`, or a
/// synchronously lifted callee's flat results, and lowers them into
/// the caller. The adapter's own code handles the reallocs, the
/// may-leave flag, and the fresh context slots around each realloc;
/// the polyfill only calls the two functions at the right moments.
///
/// The bridge lives on the subtask record of the call, because the
/// return function runs when the callee's `task.return` does, which
/// can be in a later turn than the one the call started in.
pub struct CallBridge {
    /// The start function, called once, from inside the item that
    /// starts the callee's implicit thread.
    pub start: RuntimeFunc,
    /// The caller's flat arguments, which the start function takes.
    pub arguments: Vec<RuntimeVal>,
    /// The return function, called when the callee produces its
    /// result.
    pub return_: RuntimeFunc,
    /// How the caller takes the result.
    pub caller: CallerKind,
    /// Whether the callee's function type carries the `async`
    /// effect, which is what decides whether its task waits at its
    /// instance's entry gate.
    pub callee_async_typed: bool,
    /// The caller's thread, whose task is the scope the return
    /// function runs in and the caller the start intrinsic returns
    /// to.
    pub caller_thread: ThreadId,
    /// The flat result types the caller takes, which the start
    /// intrinsic fills in from its own core signature: the return
    /// function produces exactly these. Empty for a caller that
    /// takes its result through a return pointer or a status word.
    pub caller_results: Vec<FlatType>,
    /// What the return function produced: the caller's flat results,
    /// which a synchronous start hands back as it returns. Empty
    /// until the call resolves, and empty afterwards for a caller
    /// that takes its result through a return pointer.
    pub flat_results: Vec<RuntimeVal>,
}

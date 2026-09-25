//! The two forms of `canon lift async`, and what each does when its
//! core function returns.
//!
//! An export lifted `async` delivers its result through `task.return`
//! and not by returning it. The lift comes in two forms, and they
//! differ in what the core function returns and in what that return
//! means:
//!
//! - The callback form returns a status word. The word goes to the
//!   callback loop of [`CallbackTask`], which re-enters the export's
//!   callback once per event until a word says the task is over.
//! - The stackful form names no callback. Its core function runs as
//!   the task's implicit thread and returns nothing, and it blocks
//!   inside built-ins as it needs. Its return ends the implicit
//!   thread, which is the reference's `exit_implicit_thread` in
//!   `canon_lift`.
//!
//! A task ends when its implicit thread does. A stackful task can hold
//! explicit threads beside its implicit one, and none of them runs
//! when its core function returns: an explicit thread whose start has
//! not run yet, and one suspended in the provider, is the task's
//! pending work, which leaves with the task. So the return of a
//! stackful task's core function ends the task. A task that has not
//! resolved by then fails with the
//! no-result cause, which is Wasmtime's message "async-lifted export
//! failed to produce a result". The callback loop's exit code ends
//! a callback task the same way, through the same function.
//!
//! Only the callback form needs the exclusive thread of its
//! instance: the reference's `needs_exclusive` is `not opts.async or
//! opts.callback`. A stackful task passes the entry gate and runs
//! beside any other task of the instance.
//!
//! The implicit thread of a stackful task starts through the store's
//! suspend provider when there is one, on a stack of its own, and a
//! block inside it suspends that stack until the block's condition
//! holds. With no provider it starts as a direct call on the real
//! stack, and a block inside it takes the nested turn of the suspend
//! seam, so a block that only a frame below can release fails with
//! the stack-switch cause. A stackful export that never blocks
//! behaves the same on every target.

use wasm_runtime_layer::Val as RuntimeVal;

use crate::concurrency::TaskId;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, TaskCause};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::callback_task::{CallbackTask, status_word};

/// The form of one call's `canon lift async`, and so what its core
/// function returns and what that return does.
#[derive(Clone)]
pub enum AsyncLift {
    /// The callback form: the core function returns a status word,
    /// which goes to this loop.
    Callback(CallbackTask),
    /// The stackful form: the core function returns nothing, and its
    /// return ends the implicit thread of this task.
    Stackful(TaskId),
}

impl AsyncLift {
    /// How many flat results the core function returns: the status
    /// word of the callback form, and nothing for the stackful form.
    pub fn result_count(&self) -> usize {
        match self {
            Self::Callback(_) => 1,
            Self::Stackful(_) => 0,
        }
    }

    /// Whether the task needs the exclusive thread of its instance,
    /// which is the reference's `needs_exclusive` for an `async`
    /// lift: only the callback form does.
    pub fn needs_exclusive(&self) -> bool {
        matches!(self, Self::Callback(_))
    }

    /// Act on what the core function returned, once the task's scope
    /// is off the stack. The callback form hands its status word to
    /// the loop. The stackful form ends the task's implicit thread.
    pub fn returned<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        core_results: &[RuntimeVal],
    ) -> Result<()> {
        match self {
            Self::Callback(loop_) => loop_.handle_status_word(store, status_word(core_results)?),
            Self::Stackful(task) => exit_implicit_thread(store, *task),
        }
    }
}

/// End the implicit thread of an `async`-lifted task whose scope is
/// already off the stack. No other thread of the task runs, so its
/// record leaves the store, and a task that has not resolved fails
/// with the no-result cause. A borrow the guest did not drop fails a
/// task that did resolve.
pub fn exit_implicit_thread<T: 'static>(
    store: &mut StoreContext<'_, T>,
    task: TaskId,
) -> Result<()> {
    let resolved = store.internal().export_task_resolved(task)?;
    let borrows = store.internal().end_export_task(task)?;
    if !resolved {
        return Err(Error::Task(TaskCause::NoResult));
    }
    match borrows {
        Ok(()) => Ok(()),
        Err(count) => Err(Error::from(AbiError {
            position: AbiPosition::Result,
            valtype: None,
            cause: AbiCause::OutstandingBorrows {
                count: count as usize,
            },
        })),
    }
}

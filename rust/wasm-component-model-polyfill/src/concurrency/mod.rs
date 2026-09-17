//! The store's cooperative scheduler and its task, subtask, thread,
//! waitable, and instance records.
//!
//! [`Scheduler`] is the loop the store owns. It holds the ready
//! queues and the entry gate; the turn that runs an item lives on
//! the store, because an item runs against the store. [`Driver`] is
//! a host future that polls it: one poll is a turn, guest code runs
//! only inside a turn, and every entry point that reaches a guest is
//! a driver. [`Accessor`] is what the store's `run_concurrent` entry
//! hands its closure: the one way a future that does not borrow the
//! store still reaches the store's host data, and only during a
//! poll. [`SchedulerState`] is the half of the scheduler a
//! trampoline can reach from inside a runtime-layer closure: the
//! waker of the running turn and the flag that says one is running.
//!
//! [`HostTask`] is one call of a host `async` function: a body the
//! store polls once per turn, and the lowering that carries what it
//! produced into the subtask that awaits it. The store polls a body
//! with an [`Accessor`] of its own, so a body that has to read the
//! host data reaches it the way a `run_concurrent` closure does.
//! [`CallStatus`] is the word the call returns to the guest, and
//! [`LowerKind`] is which lowering the guest called through.
//!
//! A task is the record of one call into an export; a subtask is the
//! record of one call out through an import; a thread is one guest
//! execution, and every task has at least one, its implicit thread.
//! A waitable is a handle a guest can wait on, and a waitable set is
//! a group of waitables one thread waits on together. The store keeps
//! one table of each, a record per component instance, and a stack of
//! current scopes, where a scope is a task or a subtask:
//!
//! - [`TaskTables`] is the whole of that state, reached through the
//!   store's handle tables so that a trampoline or an intrinsic can
//!   consult it from inside a runtime-layer closure.
//! - [`Scope`] is one entry of the stack. `Func::call` pushes the
//!   export's task, a host trampoline pushes the subtask of the
//!   guest's call and keeps it on the stack while the host side
//!   runs, and an adapter's enter and exit intrinsics push and pop
//!   the callee's task.
//! - [`Task`](task::Task), [`Subtask`](subtask::Subtask),
//!   [`Thread`](thread::Thread),
//!   [`WaitableSet`](waitable_set::WaitableSet), and
//!   [`InstanceRecord`](instance_record::InstanceRecord) are the
//!   records themselves, reached through the accessors on
//!   [`TaskTables`].
//! - [`WaitableId`] names one waitable and [`Event`] is what one
//!   delivers. A waitable's own state lives on its record, as
//!   [`WaitableState`](waitable_state::WaitableState); a subtask is
//!   the only kind of waitable the polyfill builds today.
//!
//! A synchronous call is a task with one thread, so the synchronous
//! baseline is the case of one task per instance at a time.

mod accessor;
mod call_status;
mod driver;
mod event;
mod event_code;
mod host_future;
mod host_result_lowering;
mod host_task;
mod host_task_body;
mod instance_id;
mod instance_record;
mod item;
mod item_action;
mod item_kind;
mod lower_kind;
mod outcome;
mod readiness;
mod record_table;
mod scheduler;
mod scheduler_state;
mod scope;
mod subtask;
mod subtask_id;
mod subtask_state;
mod task;
mod task_id;
mod task_result;
mod task_state;
mod task_tables;
mod thread;
mod thread_id;
mod waitable_id;
mod waitable_set;
mod waitable_set_id;
mod waitable_state;
mod yield_wake;

// The records themselves are reached through the accessors on
// `TaskTables`, so only the names other modules spell are
// re-exported here.
pub use accessor::Accessor;
pub use call_status::CallStatus;
pub use driver::Driver;
pub use event::Event;
pub use host_task::HostTask;
pub use instance_id::InstanceId;
pub use item::Item;
pub use item_kind::ItemKind;
pub use lower_kind::LowerKind;
pub use outcome::Outcome;
pub use scheduler::Scheduler;
pub use scheduler_state::SchedulerState;
pub use scope::Scope;
pub use subtask_id::SubtaskId;
pub use subtask_state::SubtaskState;
pub use task_id::TaskId;
pub use task_tables::TaskTables;
pub use thread_id::ThreadId;
pub use waitable_id::WaitableId;
pub use waitable_set_id::WaitableSetId;
pub use yield_wake::YieldWake;

//! The store's task, subtask, thread, waitable, and instance records.
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

mod event;
mod event_code;
mod instance_id;
mod instance_record;
mod readiness;
mod record_table;
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

// The records themselves are reached through the accessors on
// `TaskTables`, so only the names other modules spell are
// re-exported here.
pub use event::Event;
pub use instance_id::InstanceId;
pub use scope::Scope;
pub use subtask_id::SubtaskId;
pub use subtask_state::SubtaskState;
pub use task_id::TaskId;
pub use task_tables::TaskTables;
pub use thread_id::ThreadId;
pub use waitable_id::WaitableId;
pub use waitable_set_id::WaitableSetId;

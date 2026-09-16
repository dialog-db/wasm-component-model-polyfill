//! The store's task, subtask, thread, and instance records.
//!
//! A task is the record of one call into an export; a subtask is the
//! record of one call out through an import; a thread is one guest
//! execution, and every task has at least one, its implicit thread.
//! The store keeps one table of each, a record per component
//! instance, and a stack of current scopes, where a scope is a task
//! or a subtask:
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
//!   [`Thread`](thread::Thread), and
//!   [`InstanceRecord`](instance_record::InstanceRecord) are the
//!   records themselves, reached through the accessors on
//!   [`TaskTables`].
//!
//! A synchronous call is a task with one thread, so the synchronous
//! baseline is the case of one task per instance at a time.

mod instance_id;
mod instance_record;
mod readiness;
mod record_table;
mod scope;
mod subtask;
mod subtask_event;
mod subtask_id;
mod subtask_state;
mod task;
mod task_id;
mod task_result;
mod task_state;
mod task_tables;
mod thread;
mod thread_id;

// The records themselves are reached through the accessors on
// `TaskTables`, so only the names other modules spell are
// re-exported here.
pub use instance_id::InstanceId;
pub use scope::Scope;
pub use subtask_id::SubtaskId;
pub use subtask_state::SubtaskState;
pub use task_id::TaskId;
pub use task_tables::TaskTables;
pub use thread_id::ThreadId;

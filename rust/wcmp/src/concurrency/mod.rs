//! The store's cooperative scheduler and its task, subtask, thread,
//! waitable, and instance records.
//!
//! [`Scheduler`] is the loop the store owns. It holds the ready
//! queues and the entry gate; the turn that runs an item lives on
//! the store, because an item runs against the store. [`Driver`] is
//! a host future that polls it: one poll is a turn, guest code runs
//! only inside a turn, and every entry point that reaches a guest is
//! a driver. [`Accessor`] is what the store's `run_concurrent` entry
//! hands its closure: a token carrying the store's identity, which
//! is the one way a future that does not borrow the store still
//! reaches the store's host data, and only during a poll.
//! [`PollScope`] is where the store waits for it — the slot the
//! store leaves its context in for the length of one poll, which is
//! what makes the accessor a token rather than a borrow.
//! [`SchedulerState`] is the part of the scheduler that lives
//! behind the store's handle tables rather than in the store's data:
//! the waker of the running turn and the count of the turns that
//! are running, which a resource trampoline and a lift/lower
//! context reach without the store's context.
//! [`TurnGuard`] is what a turn holds that state through:
//! an item can panic, and the mark a turn leaves has to go back
//! whether the turn returned or unwound.
//!
//! [`HostTask`] is one call of a host `async` function: a body the
//! store polls in the turn after each wake, and the lowering that carries what it
//! produced into the subtask that awaits it. The store polls a body
//! inside a [`PollScope`] of its own and hands it an [`Accessor`],
//! so a body that has to read the host data reaches it the way a
//! `run_concurrent` closure does — and, because the accessor
//! borrows nothing, can hold one across its awaits. The call that
//! produces the body runs inside such a scope too, so a registration
//! that reads the host data before it builds its future reaches the
//! store there as well.
//! [`CallStatus`] is the word the call returns to the guest, and
//! [`LowerKind`] is which lowering the guest called through.
//!
//! [`SuspendSeam`] is the scheduler's one suspend capability: a
//! blocking built-in asks it to suspend the current guest thread
//! until a readiness condition holds. A [`Readiness`] is that
//! condition as data: the thread's record holds it while the thread
//! waits, and evaluating it only reads the store. A
//! [`BlockingBuiltin`] is such a built-in, split into a first part
//! that answers a [`BlockStep`] and a finish part. Under a provider
//! it reaches the guest as the switch module's shim, which suspends
//! a thread that runs on a stack of its own; the scheduler keeps the
//! thread as a [`ParkedThread`] and resumes it once its condition
//! holds. Every other block runs a nested turn from inside the guest
//! call. That nested turn is not the nesting [`SchedulerState`]
//! counts: a host task's body that reaches the store through its
//! accessor enters a turn of its own and raises that count, while
//! the seam's fallback deliberately does not, because a nested turn
//! is not a driver and polls with the waker the outer turn recorded.
//!
//! [`SuspendProvider`] is the contract a mechanism that switches
//! guest stacks meets, and [`EntryStatus`] is where a thread entry
//! stopped when the provider handed control back. [`EntryFinish`] is
//! what the frame that started a thread entry runs once the entry
//! finishes, which under a provider can be after the thread
//! suspended and resumed. The switch module is the core module the
//! providers switch stacks with: shims that suspend in WebAssembly in
//! place of a blocking built-in's trampoline, and wrappers that hand
//! a thread entry's results to the host. It takes one form per
//! provider, a [`SwitchForm`]. [`StackSwitchingProvider`] fills the
//! contract with the instructions of the WebAssembly stack-switching
//! proposal, over instances of the switch module the store keeps for
//! its whole life, and [`SwitchProbe`] is what an engine runs when it
//! is constructed to learn whether it can. In the browser,
//! `JspiProvider` fills it through JavaScript Promise Integration,
//! over instances of the other form, and [`JspiProbe`] is what an
//! engine runs next to learn whether the browser offers that.
//! [`StoreProvider`] is the one a store runs its threads through.
//!
//! The JSPI provider resumes a thread on a microtask, never inside the
//! call that asks for it. A resume is therefore made only from a turn
//! of a driver, and the turn waits for the thread, an [`InFlight`]
//! thread, before it runs anything else. A frame inside a guest call
//! that has to resume a thread leaves the resumption to the store, and
//! the trampoline it runs in leaves a [`Plan`] for the rest of its
//! work: its shim suspends the thread it runs in, the scheduler runs
//! the plan from a turn, and then resumes the thread. The rest of a
//! blocking built-in's nested-turn fallback is a [`SeamWait`], which a
//! plan can carry.
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
//!   the callee's task. A start intrinsic that runs an `async`-typed
//!   callee from inside its own frame marks that nested start on the
//!   stack for as long as the callee runs.
//! - [`Task`](task::Task), [`Subtask`](subtask::Subtask),
//!   [`Thread`](thread::Thread),
//!   [`WaitableSet`](waitable_set::WaitableSet), and
//!   [`InstanceRecord`](instance_record::InstanceRecord) are the
//!   records themselves, reached through the accessors on
//!   [`TaskTables`].
//! - [`WaitableId`] names one waitable and [`Event`] is what one
//!   delivers. A waitable's own state lives on its record, as
//!   [`WaitableState`](waitable_state::WaitableState). A waitable is
//!   a subtask or a stream or future end.
//! - [`CopyEnd`](copy_end::CopyEnd) is the record of one stream or
//!   future end, named by an [`EndId`] and of one [`EndKind`], and
//!   [`SharedRecord`](shared_record::SharedRecord) is the state its
//!   two ends share. The store keeps one table of each.
//! - [`ErrorContextRecord`](error_context_record::ErrorContextRecord)
//!   is the record of one error context, named by an
//!   [`ErrorContextId`]: the debug message, the count of the guest
//!   handles that name it, and whether the host holds it. The store
//!   keeps one table of them. [`ErrorContextAny`] and
//!   [`ErrorContext`] are the untyped and the typed value the host
//!   holds one through; neither has an operation.
//!
//! A host writes a stream or a future through a producer: a
//! [`StreamProducer`] or a [`FutureProducer`], handed to
//! [`StreamReader::new`] or [`FutureReader::new`], which create the
//! shared record with the producer as its writable end and return the
//! readable end. The scheduler holds the producer, erased to the host
//! end the store polls, and a guest's read on the other end is a host
//! task while the producer is pending. [`Destination`] is the buffer
//! of the read a stream's producer serves, and [`StreamResult`] what
//! one of its polls answers. [`StreamAny`] and [`FutureAny`] carry a
//! readable end the host holds as a [`Val`](crate::Val), without the
//! payload type in Rust: each converts to and from its typed reader,
//! checking the payload type, and closes the end.
//!
//! A host reads a stream or a future through a consumer: a
//! [`StreamConsumer`] or a [`FutureConsumer`], handed to
//! [`StreamReader::pipe`] or [`FutureReader::pipe`] with a readable
//! end the host holds, which a guest handed over through a typed
//! call or a typed host function. The scheduler holds the consumer,
//! erased to the host end the store polls, keyed by the readable
//! end, and a guest's write on the other end is a host task while the
//! consumer is pending. [`Source`] is the buffer of the write a
//! consumer serves. A stream or future the host created and piped to
//! itself is one host task that copies from the producer to the
//! consumer, with no guest involved.
//!
//! A reader the host holds and will not read ends through
//! [`StreamReader::close`] or [`FutureReader::close`], and an untyped
//! value through [`StreamAny::close`] or [`FutureAny::close`]. Each
//! drops the readable end and tells the writer, through the one close
//! of the executor. Several values can name one end, because cloning
//! an untyped value or converting it copies the end's identity, as in
//! Wasmtime; once one of them lowers, pipes, or closes the end, the
//! others are refused those uses, save the ones
//! [`CopyCause::NotHeldByHost`](crate::CopyCause::NotHeldByHost)
//! lets through as Wasmtime does. [`GuardedStreamReader`] and
//! [`GuardedFutureReader`] pair a reader with an accessor and close
//! it when they drop inside a poll of the store. A reader that ends
//! no other way leaks its end until the store drops, and dropping the
//! store drops every shared record, every end, and every producer and
//! consumer without polling them.
//!
//! A synchronous call is a task with one thread, so the synchronous
//! baseline is the case of one task per instance at a time.

mod accessor;
mod block_step;
mod blocking_builtin;
mod call_bridge;
mod call_status;
mod caller_kind;
mod copy_buffer;
mod copy_end;
mod copy_result;
mod copy_state;
mod deferred_work;
mod destination;
mod discarded_work;
mod driver;
mod end_direction;
mod end_id;
mod end_kind;
mod entry_finish;
mod entry_status;
mod error_context;
mod error_context_any;
mod error_context_id;
mod error_context_record;
mod event;
mod event_code;
mod event_slot;
mod future_any;
mod future_consumer;
mod future_producer;
mod future_reader;
mod guarded_future_reader;
mod guarded_stream_reader;
mod host_consumer;
mod host_future;
mod host_reader;
mod host_result_lowering;
mod host_task;
mod host_task_body;
mod host_task_set;
mod host_writer;
mod in_flight;
mod instance_id;
mod instance_record;
mod item;
mod item_action;
mod item_kind;
mod jspi_probe;
#[cfg(target_arch = "wasm32")]
mod jspi_provider;
mod lower_kind;
mod outcome;
mod pairing;
mod parked_thread;
mod pending_block;
mod plan;
mod poll_scope;
mod readiness;
mod record_table;
mod scheduler;
mod scheduler_state;
mod scope;
mod seam_wait;
mod shared_record;
mod source;
mod stack_switching_provider;
mod store_provider;
mod stream_any;
mod stream_consumer;
mod stream_producer;
mod stream_reader;
mod stream_result;
mod subtask;
mod subtask_id;
mod subtask_state;
mod suspend_provider;
mod suspend_seam;
mod switch_form;
mod switch_module;
mod switch_probe;
mod task;
mod task_id;
mod task_result;
mod task_state;
mod task_tables;
mod thread;
mod thread_id;
mod thread_start;
mod thread_table;
mod turn_guard;
mod waitable_id;
mod waitable_set;
mod waitable_set_id;
mod waitable_state;
mod wake_slot;
mod yield_wake;

// The records themselves are reached through the accessors on
// `TaskTables`, so only the names other modules spell are
// re-exported here.
pub use accessor::Accessor;
pub use block_step::BlockStep;
pub use blocking_builtin::BlockingBuiltin;
pub use call_bridge::CallBridge;
pub use call_status::CallStatus;
pub use caller_kind::CallerKind;
pub use copy_buffer::CopyBuffer;
pub use copy_state::CopyState;
pub use destination::Destination;
pub use driver::Driver;
pub use end_id::EndId;
pub use end_kind::EndKind;
pub use entry_finish::EntryFinish;
pub use entry_status::EntryStatus;
pub use error_context::ErrorContext;
pub use error_context_any::ErrorContextAny;
pub use error_context_id::ErrorContextId;
pub use event::Event;
// An event code is spelled outside this module only by the tests that
// leave a copy event on an end by hand; the built-ins that finish a
// copy reach the code through the task tables.
#[cfg(test)]
pub use event_code::EventCode;
pub use event_slot::EventSlot;
pub use future_any::FutureAny;
pub use future_consumer::FutureConsumer;
pub use future_producer::FutureProducer;
pub use future_reader::FutureReader;
pub use guarded_future_reader::GuardedFutureReader;
pub use guarded_stream_reader::GuardedStreamReader;
pub use host_consumer::HostConsumer;
pub use host_future::HostFuture;
pub use host_task::HostTask;
pub use host_task_body::HostTaskBody;
pub use host_writer::HostWriter;
pub use in_flight::InFlight;
pub use instance_id::InstanceId;
pub use item::Item;
pub use item_kind::ItemKind;
pub use jspi_probe::JspiProbe;
#[cfg(target_arch = "wasm32")]
pub use jspi_provider::JspiProvider;
pub use lower_kind::LowerKind;
pub use outcome::Outcome;
pub use pairing::Pairing;
pub use parked_thread::ParkedThread;
pub use pending_block::PendingBlock;
pub use plan::Plan;
pub use poll_scope::PollScope;
pub use readiness::Readiness;
pub use scheduler::Scheduler;
pub use scheduler_state::SchedulerState;
pub use scope::Scope;
pub use seam_wait::SeamWait;
pub use source::Source;
pub use stack_switching_provider::StackSwitchingProvider;
pub use store_provider::StoreProvider;
pub use stream_any::StreamAny;
pub use stream_consumer::StreamConsumer;
pub use stream_producer::StreamProducer;
pub use stream_reader::StreamReader;
pub use stream_result::StreamResult;
pub use subtask_id::SubtaskId;
pub use subtask_state::SubtaskState;
pub use suspend_provider::SuspendProvider;
pub use suspend_seam::SuspendSeam;
// The switch module and its form are spelled outside this module only
// by their tests: each provider builds its modules itself.
#[cfg(all(test, target_arch = "x86_64", target_os = "linux"))]
pub use switch_form::SwitchForm;
#[cfg(all(test, target_arch = "x86_64", target_os = "linux"))]
pub use switch_module::SwitchModule;
pub use switch_probe::SwitchProbe;
pub use task_id::TaskId;
pub use task_result::ResultChannel;
// Where a task's result went is read back only by the tests of the
// built-in that puts it there; every caller of `Task::resolve` in the
// crate takes the value from the call it is resolving, and a call
// whose task outlives it takes the value from the channel instead.
#[cfg(test)]
pub use task_result::TaskResult;
pub use task_state::TaskState;
pub use task_tables::TaskTables;
pub use thread_id::ThreadId;
pub use thread_start::ThreadStart;
pub use turn_guard::TurnGuard;
pub use waitable_id::WaitableId;
pub use waitable_set_id::WaitableSetId;
pub use wake_slot::WakeSlot;
pub use yield_wake::YieldWake;

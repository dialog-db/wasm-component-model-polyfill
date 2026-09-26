//! A guest thread suspended in the provider.

use wasm_runtime_layer::Val as RuntimeVal;

use crate::error::Result;
use crate::store::StoreContext;

use super::entry_finish::EntryFinish;
use super::scope::Scope;
use super::task_id::TaskId;
use super::thread_id::ThreadId;

/// What runs once a parked thread's entry finishes, boxed with the
/// `Send` bound the native target puts on everything a store holds.
#[cfg(not(target_arch = "wasm32"))]
type BoxedFinish<T> = Box<
    dyn FnOnce(&mut StoreContext<'_, T>, Result<Vec<RuntimeVal>>) -> Result<()> + Send + 'static,
>;

/// What runs once a parked thread's entry finishes, boxed. The
/// browser drops the `Send` bound.
#[cfg(target_arch = "wasm32")]
type BoxedFinish<T> =
    Box<dyn FnOnce(&mut StoreContext<'_, T>, Result<Vec<RuntimeVal>>) -> Result<()> + 'static>;

/// A guest thread suspended in the provider, as the scheduler keeps
/// it until it resumes.
///
/// A thread entry runs on a stack of its own. The frames of the host
/// that started it — an item of a turn, or a trampoline that made a
/// nested start — go on once the thread suspends, and the part of
/// them that would have run after the entry returned cannot wait on
/// the real stack for it. That part is the thread's finish, and it
/// waits here until the thread's entry finishes after some later
/// resumption.
///
/// The stack of current scopes is one stack, because synchronous
/// calls nest on the real stack. A thread that suspends leaves the
/// real stack with the scopes it pushed on it: the task it runs, the
/// callee tasks of the synchronous calls it is inside, and the
/// subtasks of the calls it made. They come off the stack as it
/// suspends and wait here, with the explicit threads running among
/// them, and they go back on top of whatever the stack holds when
/// the thread resumes.
pub struct ParkedThread<T: 'static> {
    /// The task the thread belongs to. The thread goes with it when
    /// the task's record leaves the store.
    pub task: TaskId,
    /// The scopes the thread had pushed, outermost first.
    pub scopes: Vec<Scope>,
    /// The explicit threads that were running among those scopes,
    /// each with its position counted from the first of them.
    pub running: Vec<(usize, ThreadId)>,
    /// Whether a resumption of the thread is queued.
    pub queued: bool,
    /// The order the thread last suspended in, among the threads of
    /// the store: a thread a nested start began suspends before the
    /// thread whose trampoline began it, which is still running then.
    pub number: u64,
    /// Whether a plan holds the thread: it suspended so that the
    /// scheduler could run the plan, which resumes it once done, and
    /// no queued resumption does.
    pub held: bool,
    finish: BoxedFinish<T>,
}

impl<T: 'static> ParkedThread<T> {
    /// A thread of `task` that suspended with `scopes` and `running`
    /// taken off the stack, and `finish` to run once its entry
    /// finishes.
    pub fn new(
        task: TaskId,
        scopes: Vec<Scope>,
        running: Vec<(usize, ThreadId)>,
        finish: impl EntryFinish<T>,
    ) -> Self {
        Self {
            task,
            scopes,
            running,
            queued: false,
            number: 0,
            held: false,
            finish: Box::new(finish),
        }
    }

    /// Run the thread's finish with what its entry produced.
    pub fn finish(
        self,
        store: &mut StoreContext<'_, T>,
        entry: Result<Vec<RuntimeVal>>,
    ) -> Result<()> {
        (self.finish)(store, entry)
    }
}

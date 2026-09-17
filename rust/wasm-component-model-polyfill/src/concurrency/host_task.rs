//! One call of a host `async` function, as the store holds it.

use crate::error::Result;
use crate::value::Val;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use super::host_future::HostFuture;
use super::host_task_result::HostTaskResult;
use super::subtask_id::SubtaskId;

/// The boxed future of one host task, with the `Send` bound the
/// native target puts on everything a store holds.
#[cfg(not(target_arch = "wasm32"))]
type BoxedFuture = Pin<Box<dyn core::future::Future<Output = Result<Vec<Val>>> + Send + 'static>>;

/// The boxed future of one host task. The browser drops the `Send`
/// bound: see [`HostFuture`].
#[cfg(target_arch = "wasm32")]
type BoxedFuture = Pin<Box<dyn core::future::Future<Output = Result<Vec<Val>>> + 'static>>;

/// One call of a host `async` function, as the store holds it.
///
/// The store owns the future and polls it once per turn with the
/// driver's waker, so a wake the executor delivers reaches the
/// driver that is running the store. When the future completes, the
/// turn fills the task's result slot and queues the lowering of the
/// result into the subtask that awaits it.
pub struct HostTask {
    future: BoxedFuture,
    subtask: SubtaskId,
    handle_index: u32,
    result: HostTaskResult,
}

impl HostTask {
    /// Hand `future` to the store as the host task of `subtask`.
    ///
    /// `handle_index` is where the subtask sits in the calling
    /// instance's handle table, which is the first payload of the
    /// subtask event the guest receives when the task completes.
    /// `result` is the slot the completed future's value is left in.
    pub fn new(
        subtask: SubtaskId,
        handle_index: u32,
        result: HostTaskResult,
        future: impl HostFuture,
    ) -> Self {
        Self {
            future: Box::pin(future),
            subtask,
            handle_index,
            result,
        }
    }

    /// The subtask this host task resolves.
    pub fn subtask(&self) -> SubtaskId {
        self.subtask
    }

    /// Where the subtask sits in the calling instance's handle
    /// table.
    pub fn handle_index(&self) -> u32 {
        self.handle_index
    }

    /// The slot the completed future's value is left in.
    pub fn result(&self) -> HostTaskResult {
        self.result.clone()
    }

    /// Poll the future with `waker`, which is the waker of the turn
    /// that is running.
    pub fn poll(&mut self, waker: &Waker) -> Poll<Result<Vec<Val>>> {
        let mut context = Context::from_waker(waker);
        self.future.as_mut().poll(&mut context)
    }
}

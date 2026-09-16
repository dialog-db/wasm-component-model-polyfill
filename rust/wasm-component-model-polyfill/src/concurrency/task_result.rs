//! Where a task's result goes when the task resolves.

use std::sync::{Arc, Mutex};

use crate::value::Val;

/// Where a task's result goes when the task resolves.
///
/// A task whose caller is on the stack — every call of the
/// synchronous baseline — leaves its result in the record, and the
/// caller takes it as the call returns. A task whose caller is not
/// on the stack is given the caller's channel instead, and the
/// result is sent through it as the task resolves.
#[derive(Clone, Debug)]
pub enum TaskResult {
    /// The task has not resolved yet and no caller is waiting on a
    /// channel.
    Pending,
    /// The result the task returned, held in the record until its
    /// caller takes it. The inner `Option` is absent for a function
    /// that declares no result.
    Returned(Option<Val>),
    /// The channel of a caller that is not on the stack. The slot is
    /// filled once, with the result the task returned. Reserved for
    /// the drivers that let a task outlive the call that started it;
    /// nothing constructs this variant yet.
    #[allow(dead_code)]
    Channel(Arc<Mutex<Option<Option<Val>>>>),
}

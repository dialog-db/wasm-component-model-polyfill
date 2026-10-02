// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One call of a host `async` function, as the store holds it.

use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use crate::error::{Error, Result};
use crate::executor::release_subtask;
use crate::internal::{AccessorInternal, ErrorInternal};
use crate::resource::TableId;
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;
use crate::value::Val;

use super::accessor::Accessor;
use super::host_future::HostFuture;
use super::host_result_lowering::HostResultLowering;
use super::host_task_body::HostTaskBody;
use super::item::Item;
use super::item_kind::ItemKind;
use super::poll_scope::PollScope;
use super::subtask_id::SubtaskId;

/// The boxed lowering of one host task's result, with the `Send`
/// bound the native target puts on everything a store holds.
#[cfg(not(target_arch = "wasm32"))]
type BoxedLowering<T> =
    Box<dyn FnOnce(&mut StoreContext<'_, T>, Result<Vec<Val>>) -> Result<()> + Send + 'static>;

/// The boxed lowering of one host task's result. The browser drops
/// the `Send` bound: see [`HostResultLowering`].
#[cfg(target_arch = "wasm32")]
type BoxedLowering<T> =
    Box<dyn FnOnce(&mut StoreContext<'_, T>, Result<Vec<Val>>) -> Result<()> + 'static>;

/// One call of a host `async` function, as the store holds it.
///
/// The runtime layer gives a host trampoline a synchronous closure
/// and nothing else, so the trampoline cannot run the call's future
/// itself. It gives the task to the store and returns to the guest,
/// and the store polls it in the turn after each wake, with a waker
/// of its own that passes the wake on to the driver's, so a wake the
/// executor delivers reaches the driver that is running the store.
/// When the body completes, the turn queues the lowering of the
/// result into the subtask that awaits it.
///
/// A call can also end through cancellation. `subtask.cancel` marks
/// the task as aborted and drops nothing. The next turn that polls
/// the host tasks drops the body without polling it, and the subtask
/// resolves as cancelled before it returned. A body that completed
/// before the abort has left the store already, and its result lowers
/// as usual.
///
/// A guest's copy against an end the host serves runs as a host task
/// too. Its body polls the host's producer, and what the turn queues
/// when the body completes is the delivery of what the producer
/// produced into the guest's buffer, which completes the copy. Such a
/// task resolves no subtask. A failure of it is a trap, as a host
/// call's failure is, unless the task is the host's own work: a pipe
/// whose two ends the host serves touches no guest, and its failure
/// ends the turn without poisoning the store.
pub struct HostTask<T: 'static> {
    body: Box<dyn HostTaskBody<T>>,
    lowering: BoxedLowering<T>,
    /// The subtask the task resolves, or `None` for a copy.
    subtask: Option<SubtaskId>,
    /// The handle table of the instance that made the call, once the
    /// call has started and the caller holds an entry for its
    /// subtask there.
    caller_table: Option<TableId>,
    /// Whether the task is the host's own work, which touches no
    /// guest.
    host_only: bool,
}

impl<T: 'static> HostTask<T> {
    /// The host task of the call `subtask` records, running `body`
    /// and lowering what it produces through `lowering`.
    pub fn new(
        subtask: SubtaskId,
        lowering: impl HostResultLowering<T>,
        body: impl HostTaskBody<T>,
    ) -> Self {
        Self {
            body: Box::new(body),
            lowering: Box::new(lowering),
            subtask: Some(subtask),
            caller_table: None,
            host_only: false,
        }
    }

    /// The host task of a call whose body needs nothing from the
    /// store: a plain future, polled with the accessor ignored.
    pub fn from_future(
        subtask: SubtaskId,
        lowering: impl HostResultLowering<T>,
        future: impl HostFuture,
    ) -> Self {
        Self::new(subtask, lowering, FutureBody(Box::pin(future)))
    }

    /// The host task of a guest's copy against an end the host
    /// serves: `body` polls the host's side of the copy, and
    /// `delivery` carries what it produced into the guest's buffer
    /// once it is ready. The vector the body produces is empty,
    /// because what the producer produced stays with the end until
    /// the delivery takes it.
    pub fn copy(delivery: impl HostResultLowering<T>, body: impl HostTaskBody<T>) -> Self {
        Self {
            body: Box::new(body),
            lowering: Box::new(delivery),
            subtask: None,
            caller_table: None,
            host_only: false,
        }
    }

    /// Mark the task as the host's own work, which touches no guest:
    /// a pipe from a producer of the host's to a consumer of the
    /// host's. What its completion queues is host work too, and runs
    /// in a store a trap poisoned, where no guest work item runs.
    pub fn host_only(mut self) -> Self {
        self.host_only = true;
        self
    }

    /// The subtask this host task resolves, or `None` for the task of
    /// a copy, which resolves none.
    pub fn subtask(&self) -> Option<SubtaskId> {
        self.subtask
    }

    /// Record the handle table of the instance that made the call,
    /// where the caller holds an entry for the call's subtask.
    ///
    /// The entry exists only once the call is known not to have
    /// finished at once. A call whose first poll resolved reports no
    /// subtask, so no entry is made for it and the guest's own call
    /// is still on the stack to take a failure.
    pub fn started_in(&mut self, table: TableId) {
        self.caller_table = Some(table);
    }

    /// Poll the body against `store`, with `waker`, which is the
    /// waker of the turn that is running.
    ///
    /// This is where the store goes into the thread's slot and comes
    /// out again: the poll runs inside a [`PollScope`], so the
    /// accessor the body is handed — and any accessor the body kept
    /// from an earlier poll — reaches the store for the length of a
    /// closure it runs through it, and reaches nothing once this
    /// poll has returned. Workspace-internal.
    #[tracing::instrument(level = "trace", name = "host task poll", skip_all)]
    pub fn poll(
        &mut self,
        store: &mut StoreContext<'_, T>,
        waker: &Waker,
    ) -> Poll<Result<Vec<Val>>> {
        let accessor = Accessor::new(store.internal().id());
        let mut context = Context::from_waker(waker);
        let _poll = PollScope::enter(store, waker);
        self.body.poll(&accessor, &mut context)
    }

    /// Drop the task's body, which is how a call its caller cancelled
    /// ends, and answer the subtask the call resolves. Nothing
    /// crosses: the host is given no way to return a value after the
    /// drop.
    ///
    /// The drop runs inside a [`PollScope`], with `waker` as the
    /// waker of the turn that is running, as a poll does. A body's
    /// `Drop` can reach the store through an accessor it kept, and it
    /// reaches it here as it would in a poll. The turn that polls the
    /// host tasks is the only caller, so the drop always happens in a
    /// turn.
    pub fn abort(self, store: &mut StoreContext<'_, T>, waker: &Waker) -> Option<SubtaskId> {
        let Self { body, subtask, .. } = self;
        let _poll = PollScope::enter(store, waker);
        drop(body);
        subtask
    }

    /// Lower `outcome` into the subtask that awaits it, here and
    /// now. The trampoline that started the call takes this path
    /// when the first poll resolved the body: the guest is still on
    /// the stack, so a failure fails its call.
    pub fn lower(self, store: &mut StoreContext<'_, T>, outcome: Result<Vec<Val>>) -> Result<()> {
        (self.lowering)(store, outcome)
    }

    /// End the call this task serves on `error`, which is a trap of
    /// the guest task that made the call, and answer the error for
    /// the turn to end with.
    ///
    /// The call never returned, and the guest task that made it can
    /// never resolve its subtask, so the store is poisoned and the
    /// polling driver reports the error. The subtask's record and the
    /// caller's entry for it leave the store first, and the handles
    /// the guest lent for the call come back with them.
    pub fn trap(self, store: &mut StoreContext<'_, T>, error: Error) -> Error {
        let Self {
            subtask,
            caller_table,
            ..
        } = self;
        if let Some(subtask) = subtask {
            release_subtask(store, subtask, caller_table);
        }
        store.internal().poison();
        error
    }

    /// The item that lowers `outcome` into the subtask that awaits
    /// it in a later turn: the crossing runs where every other piece
    /// of guest work runs, and the subtask then resolves and takes
    /// on the subtask event a thread waiting on it takes delivery
    /// of.
    ///
    /// `outcome` is what the body produced. A value crosses through
    /// the lowering, the subtask returns, and the guest takes
    /// delivery of the subtask event.
    ///
    /// A body that failed is a call that never returned, and a
    /// crossing that fails is a call whose result the guest cannot
    /// be given. Both are a trap of the guest task that made the
    /// call. The trap poisons the store and ends the turn, so the
    /// driver whose turn runs this item reports it, whichever driver
    /// that is and whether or not the call that started the guest
    /// task has already returned. The subtask does not resolve, as
    /// [`Self::trap`] states.
    ///
    /// The task of a copy queues the delivery instead. A body and a
    /// delivery that succeed have completed the copy, and a failure
    /// of either is a trap by the same rule. The failure of a task that
    /// is the host's own work is not: it ends the turn, and leaves the
    /// store unpoisoned.
    pub fn lowering_item(self, outcome: Result<Vec<Val>>) -> Item<T> {
        let Self {
            lowering,
            subtask,
            caller_table,
            host_only,
            ..
        } = self;
        let Some(subtask) = subtask else {
            let item = Item::new(
                ItemKind::HostCopyDelivery,
                move |store: &mut StoreContext<'_, T>| match lowering(store, outcome) {
                    Ok(()) => Ok(()),
                    // The failure of the host's own work touches no
                    // guest, so it is no trap and leaves the store as
                    // it was. It still ends the turn, and the driver
                    // that is polling reports it.
                    Err(error) if host_only => Err(error),
                    Err(error) => {
                        store.internal().poison();
                        Err(error)
                    }
                },
            );
            return if host_only { item.host_only() } else { item };
        };
        Item::new(
            ItemKind::HostResultLowering,
            move |store: &mut StoreContext<'_, T>| {
                let crossing = match outcome {
                    Ok(values) => lowering(store, Ok(values)),
                    Err(error) => Err(error),
                };
                let Err(error) = crossing else {
                    let mut guard = store
                        .internal()
                        .tables()
                        .lock()
                        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
                    // The call is over, and the subtask's readiness
                    // is the subtask event a thread waiting on it
                    // takes delivery of. Returning records it from
                    // the subtask's own handle, which a subtask this
                    // item resolves always has: it was entered in
                    // the caller's table when the first poll left
                    // the body running.
                    guard.tasks.subtask_returned(subtask)?;
                    return Ok(());
                };
                release_subtask(store, subtask, caller_table);
                store.internal().poison();
                Err(error)
            },
        )
    }
}

/// A plain future as the body of a host task. It reaches nothing of
/// the store, so every poll ignores the accessor.
struct FutureBody<F>(Pin<Box<F>>);

impl<T: 'static, F: HostFuture> HostTaskBody<T> for FutureBody<F> {
    fn poll(
        &mut self,
        _accessor: &Accessor<T>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Vec<Val>>> {
        core::future::Future::poll(self.0.as_mut(), context)
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use std::sync::{Arc, Mutex};

    use crate::engine::Engine;
    use crate::resource::TableId;

    use super::super::lower_kind::LowerKind;
    use super::super::subtask_state::SubtaskState;
    use crate::store::Store;

    use super::*;
    use crate::store::{StoreContextInternalExt, StoreInternalExt};

    /// A body that reads the store's host data through the accessor
    /// the poll hands it, one poll after it started: the poll that
    /// reads runs inside a turn, which is where the body of a host
    /// task is polled from once its call has returned to the guest.
    struct ReadsHostData {
        polled: usize,
        seen: Arc<Mutex<Option<String>>>,
    }

    impl HostTaskBody<String> for ReadsHostData {
        fn poll(
            &mut self,
            accessor: &Accessor<String>,
            _context: &mut Context<'_>,
        ) -> Poll<Result<Vec<Val>>> {
            self.polled += 1;
            if self.polled == 1 {
                return Poll::Pending;
            }
            // The borrow the closure is given outlives nothing, so
            // what the body keeps is a clone.
            match accessor.with(|store| store.data().clone()) {
                Ok(data) => {
                    *self.seen.lock().expect("what the body read") = Some(data);
                    Poll::Ready(Ok(vec![Val::U32(1)]))
                }
                Err(error) => Poll::Ready(Err(error)),
            }
        }
    }

    #[wcmp_macros::test]
    async fn it_reaches_the_host_data_through_the_accessor_the_poll_hands_it() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");
        let table = TableId::fresh();
        let subtask = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .push_subtask()
            .expect("room under the record cap");
        let seen = Arc::new(Mutex::new(None));
        let lowered = Arc::new(Mutex::new(None));
        let slot = lowered.clone();

        let status = store
            .internal()
            .context()
            .internal()
            .start_host_task(
                HostTask::new(
                    subtask,
                    move |_store: &mut StoreContext<'_, String>, outcome: Result<Vec<Val>>| {
                        *slot.lock().expect("the lowering's slot") =
                            Some(outcome.expect("the body's value"));
                        Ok(())
                    },
                    ReadsHostData {
                        polled: 0,
                        seen: seen.clone(),
                    },
                ),
                table,
                LowerKind::Async,
            )
            .expect("start the host task");

        assert_eq!(
            status.state(),
            SubtaskState::Started.value(),
            "the body was not ready on its first poll"
        );
        assert!(
            seen.lock().expect("what the body read").is_none(),
            "the body has not read the host data yet"
        );

        // The turn that polls the body a second time, and the one
        // that runs the lowering it queued.
        store.internal().turn(Waker::noop()).expect("a turn");
        store.internal().turn(Waker::noop()).expect("a turn");

        assert_eq!(
            seen.lock().expect("what the body read").as_deref(),
            Some("host data"),
            "the body read the host data through the accessor the poll handed it"
        );
        assert_eq!(
            lowered.lock().expect("the lowering's slot").as_deref(),
            Some(&[Val::U32(1)][..]),
            "the body's value crossed through the lowering"
        );
        assert!(
            !store.internal().turn_in_flight(),
            "the turn the body ran a closure inside left the outer turn as it found it"
        );
    }

    /// A future that is pending the first time it is polled and
    /// ready afterwards: one await for a body to hold something
    /// across.
    #[derive(Default)]
    struct PendingOnce(bool);

    impl Future for PendingOnce {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                return Poll::Ready(());
            }
            self.0 = true;
            Poll::Pending
        }
    }

    /// A body that is a future of its own, holding the accessor
    /// across an await.
    ///
    /// The accessor borrows nothing, so the body clones the token
    /// the first poll hands it into a future that owns it. That
    /// future awaits, and reaches the store's host data on the
    /// other side of the await — in a later poll, with a later
    /// borrow of the store in the thread's slot, through the token
    /// it has been holding all along.
    ///
    /// The boxed future is `Send` on both targets because
    /// everything it holds is; the native half of the host-task
    /// bound requires it, and the browser half is content with it.
    struct HoldsTheAccessor {
        started: Option<BodyFuture>,
        seen: Arc<Mutex<Option<String>>>,
    }

    /// The future [`HoldsTheAccessor`] builds on its first poll: the
    /// body's own future, owning the accessor it awaits across.
    type BodyFuture = Pin<Box<dyn Future<Output = Result<Vec<Val>>> + Send>>;

    impl HostTaskBody<String> for HoldsTheAccessor {
        fn poll(
            &mut self,
            accessor: &Accessor<String>,
            context: &mut Context<'_>,
        ) -> Poll<Result<Vec<Val>>> {
            let seen = self.seen.clone();
            let started = self.started.get_or_insert_with(|| {
                let accessor = accessor.clone();
                Box::pin(async move {
                    PendingOnce::default().await;
                    // The borrow the closure is given outlives
                    // nothing, so what the body keeps is a clone.
                    let data = accessor.with(|store| store.data().clone())?;
                    *seen.lock().expect("what the body read") = Some(data);
                    Ok(vec![Val::U32(7)])
                })
            });
            started.as_mut().poll(context)
        }
    }

    #[wcmp_macros::test]
    async fn it_holds_the_accessor_across_an_await_and_reaches_the_host_data_later() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");
        let table = TableId::fresh();
        let subtask = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .push_subtask()
            .expect("room under the record cap");
        let seen = Arc::new(Mutex::new(None));
        let lowered = Arc::new(Mutex::new(None));
        let slot = lowered.clone();

        let status = store
            .internal()
            .context()
            .internal()
            .start_host_task(
                HostTask::new(
                    subtask,
                    move |_store: &mut StoreContext<'_, String>, outcome: Result<Vec<Val>>| {
                        *slot.lock().expect("the lowering's slot") =
                            Some(outcome.expect("the body's value"));
                        Ok(())
                    },
                    HoldsTheAccessor {
                        started: None,
                        seen: seen.clone(),
                    },
                ),
                table,
                LowerKind::Async,
            )
            .expect("start the host task");

        assert_eq!(
            status.state(),
            SubtaskState::Started.value(),
            "the body's future is waiting at its await"
        );
        assert!(
            seen.lock().expect("what the body read").is_none(),
            "the body has not reached the other side of the await yet"
        );

        // The turn that polls the body past its await, and the one
        // that runs the lowering it queued.
        store.internal().turn(Waker::noop()).expect("a turn");
        store.internal().turn(Waker::noop()).expect("a turn");

        assert_eq!(
            seen.lock().expect("what the body read").as_deref(),
            Some("host data"),
            "the accessor the future has been holding since its first poll \
             reached the host data in a later poll"
        );
        assert_eq!(
            lowered.lock().expect("the lowering's slot").as_deref(),
            Some(&[Val::U32(7)][..]),
            "the body's value crossed through the lowering"
        );
    }
}

//! One call of a host `async` function, as the store holds it.

use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use crate::error::{Error, Result};
use crate::store::Store;
use crate::value::Val;

use super::accessor::Accessor;
use super::host_future::HostFuture;
use super::host_result_lowering::HostResultLowering;
use super::host_task_body::HostTaskBody;
use super::item::Item;
use super::item_kind::ItemKind;
use super::subtask_id::SubtaskId;

/// The boxed lowering of one host task's result, with the `Send`
/// bound the native target puts on everything a store holds.
#[cfg(not(target_arch = "wasm32"))]
type BoxedLowering<T> =
    Box<dyn FnOnce(&mut Store<T>, Result<Vec<Val>>) -> Result<()> + Send + 'static>;

/// The boxed lowering of one host task's result. The browser drops
/// the `Send` bound: see [`HostResultLowering`].
#[cfg(target_arch = "wasm32")]
type BoxedLowering<T> = Box<dyn FnOnce(&mut Store<T>, Result<Vec<Val>>) -> Result<()> + 'static>;

/// One call of a host `async` function, as the store holds it.
///
/// The runtime layer gives a host trampoline a synchronous closure
/// and nothing else, so the trampoline cannot run the call's future
/// itself. It gives the task to the store and returns to the guest,
/// and the store polls it once per turn with the driver's waker, so
/// a wake the executor delivers reaches the driver that is running
/// the store. When the body completes, the turn queues the lowering
/// of the result into the subtask that awaits it.
pub struct HostTask<T: 'static> {
    body: Box<dyn HostTaskBody<T>>,
    lowering: BoxedLowering<T>,
    subtask: SubtaskId,
    handle_index: u32,
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
            subtask,
            handle_index: 0,
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

    /// The subtask this host task resolves.
    pub fn subtask(&self) -> SubtaskId {
        self.subtask
    }

    /// Where the subtask sits in the calling instance's handle
    /// table, which is the first payload of the subtask event the
    /// guest receives when the call completes.
    pub fn handle_index(&self) -> u32 {
        self.handle_index
    }

    /// Record where the subtask sits in the calling instance's
    /// handle table. The index exists only once the call is known
    /// not to have finished at once: a call whose first poll
    /// resolved reports no subtask, so no entry is made for it.
    pub fn set_handle_index(&mut self, handle_index: u32) {
        self.handle_index = handle_index;
    }

    /// Poll the body with `waker`, which is the waker of the turn
    /// that is running, and `accessor`, which reaches the store for
    /// the length of a closure the body runs through it.
    pub fn poll(&mut self, accessor: &Accessor<'_, T>, waker: &Waker) -> Poll<Result<Vec<Val>>> {
        let mut context = Context::from_waker(waker);
        self.body.poll(accessor, &mut context)
    }

    /// Lower `outcome` into the subtask that awaits it, here and
    /// now. The trampoline that started the call takes this path
    /// when the first poll resolved the body: the guest is still on
    /// the stack, so a failure fails its call.
    pub fn lower(self, store: &mut Store<T>, outcome: Result<Vec<Val>>) -> Result<()> {
        (self.lowering)(store, outcome)
    }

    /// The item that lowers `outcome` into the subtask that awaits
    /// it in a later turn: the crossing runs where every other piece
    /// of guest work runs, and the subtask then resolves and takes
    /// on the subtask event a thread waiting on it takes delivery
    /// of.
    ///
    /// `outcome` is what the body produced. A value crosses through
    /// the lowering and the subtask returns. A failure is a call
    /// that never returned, so the subtask resolves as a
    /// cancellation and nothing crosses, which is what the same
    /// failure on the first poll does; the difference is only where
    /// the guest learns of it. The guest's call was on the stack
    /// then and the failure travelled out to it, and here the guest
    /// has been told the call started, so what it takes delivery of
    /// is the subtask event carrying the cancelled state.
    ///
    /// A crossing that fails is a call whose result the guest cannot
    /// be given. The subtask resolves as a cancellation all the same
    /// and takes on its event, exactly as a failed body resolves it:
    /// the guest was told the call started, it holds the subtask in
    /// its handle table, and a subtask left `started` with no event
    /// would leave a thread waiting on it waiting for ever. The
    /// failure itself belongs to no caller — the guest's call
    /// returned turns ago and nothing of it is on the stack — so
    /// once the subtask is resolved it ends the turn and reaches
    /// whichever driver polled it. A subtask record that vanished
    /// between the poll that completed the body and the turn that
    /// ran this item ends the turn the same way, with nothing left
    /// to resolve.
    pub fn lowering_item(self, outcome: Result<Vec<Val>>) -> Item<T> {
        let Self {
            lowering,
            subtask,
            handle_index,
            ..
        } = self;
        Item::new(ItemKind::HostResultLowering, move |store: &mut Store<T>| {
            // A body that failed is a call that never returned, so
            // nothing crosses: the lowering has no value to take
            // into the guest. A crossing that fails is the same
            // thing seen from the other side, so the subtask
            // resolves the same way and the failure travels on
            // afterwards.
            let produced = outcome.is_ok();
            let crossing = match outcome {
                Ok(values) => lowering(store, Ok(values)),
                Err(_) => Ok(()),
            };
            let mut guard = store
                .tables
                .lock()
                .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
            if produced && crossing.is_ok() {
                guard.tasks.subtask_returned(subtask)?;
            } else {
                guard.tasks.subtask_cancelled(subtask)?;
            }
            // The call is over either way, and the subtask's
            // readiness is the subtask event a thread waiting on it
            // takes delivery of. The event carries the subtask's
            // index in the caller instance's handle table and the
            // state it resolved to.
            guard.tasks.record_subtask_event(subtask, handle_index)?;
            crossing
        })
    }
}

/// A plain future as the body of a host task. It reaches nothing of
/// the store, so every poll ignores the accessor.
struct FutureBody<F>(Pin<Box<F>>);

impl<T: 'static, F: HostFuture> HostTaskBody<T> for FutureBody<F> {
    fn poll(
        &mut self,
        _accessor: &Accessor<'_, T>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Vec<Val>>> {
        core::future::Future::poll(self.0.as_mut(), context)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::engine::Engine;
    use crate::resource::TableId;

    use super::super::lower_kind::LowerKind;
    use super::super::subtask_state::SubtaskState;
    use super::*;

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
            accessor: &Accessor<'_, String>,
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
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");
        let table = TableId::fresh();
        let subtask = store.tables.lock().expect("tables").tasks.push_subtask();
        let seen = Arc::new(Mutex::new(None));
        let lowered = Arc::new(Mutex::new(None));
        let slot = lowered.clone();

        let status = store
            .start_host_task(
                HostTask::new(
                    subtask,
                    move |_store: &mut Store<String>, outcome: Result<Vec<Val>>| {
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
        store.turn(Waker::noop()).expect("a turn");
        store.turn(Waker::noop()).expect("a turn");

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
            !store.turn_in_flight().expect("the store's turn state"),
            "the turn the body ran a closure inside left the outer turn as it found it"
        );
    }
}

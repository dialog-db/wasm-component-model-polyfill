//! A token that reaches the store from inside a poll.

use core::marker::PhantomData;

use crate::error::Result;
use crate::store::{StoreContext, StoreId};

use super::poll_scope::PollScope;

/// A token that reaches the store from inside a poll.
///
/// The token carries the identity of one store and nothing else. It
/// borrows nothing, so it has no lifetime and a future can own one
/// and hold it across its awaits. What it reaches, it reaches
/// through the slot the store leaves itself in while a poll is
/// running: around each poll of the `run_concurrent` closure, and
/// around each poll of a host task's body, the store puts its
/// context in the slot and takes it out again before the poll
/// returns. [`Accessor::with`] takes the store out of that slot,
/// runs a closure against it, and puts it back.
///
/// Everything the store offers reaches a body this way: the host
/// data and the resource table a synchronous host function reaches
/// through its call, and the rest of the store besides. The borrow
/// the closure is given outlives nothing, so a value taken from the
/// host data must be cloned out of it.
///
/// These reaches fail rather than lend the store:
///
/// - A reach made outside any poll of the store, and a reach made
///   with the token of another store, fail with the
///   store-not-in-poll cause. The slot holds a store only while a
///   poll of that store is running.
/// - A reach made with a token whose `T` is not the store's host
///   data type fails with the store-not-in-poll cause too. The
///   token names a store this thread is not polling: no store has
///   that identity and that host data. Such a token is buildable,
///   because [`Accessor::new`] takes a store's identity and a
///   store's identity is a value anyone holding the store can
///   read, so the reach checks the type rather than trusting it.
/// - A reach made from inside another reach's closure fails with
///   the recursive-driver cause: the outer reach holds the store,
///   and the slot it left behind still names it.
///
/// A nested `run_concurrent` entered from inside a reach fails with
/// the recursive-driver cause as well: the store is inside a turn
/// while the closure runs, so the guest work the closure reaches
/// runs where every other piece of guest work runs. The other two
/// drivers — a call into an export, an instantiation — cannot be
/// entered from there at all, because each takes a `&mut Store<T>`
/// and the closure holds a [`StoreContext`], which nothing turns
/// back into the store the host owns.
pub struct Accessor<T: 'static> {
    store: StoreId,
    /// The host data the token's reaches are typed by. The token
    /// holds none of it, so it is `Send` and `Sync` whatever `T` is:
    /// what it can reach, it can reach only on the thread whose slot
    /// holds the store, and a store's identity is never reused.
    data: PhantomData<fn() -> T>,
}

impl<T: 'static> Accessor<T> {
    /// A token for the store `store` names. Workspace-internal; the
    /// store hands one to its `run_concurrent` closure and to every
    /// poll of a host task's body.
    ///
    /// Nothing here ties `T` to the host data of the store `store`
    /// names, and nothing needs to: a token whose `T` is not that
    /// store's host data type reaches nothing. Every reach matches
    /// the type against the one the running poll recorded, and a
    /// token that fails that match is refused with the
    /// store-not-in-poll cause.
    pub fn new(store: StoreId) -> Self {
        Self {
            store,
            data: PhantomData,
        }
    }

    /// Run `body` against the store this token names.
    ///
    /// The store is reachable only for the length of the call.
    /// `body` takes the store by a borrow it cannot hold on to, so
    /// anything read out of the host data must be cloned out.
    ///
    /// The store is inside a turn while `body` runs, so the guest
    /// work `body` reaches runs where every other piece of guest
    /// work runs. A nested `run_concurrent` entered from there
    /// therefore fails with the recursive-driver cause, and so does
    /// this token used again from inside `body`. A reach made where
    /// no poll of this store is running fails with the
    /// store-not-in-poll cause.
    pub fn with<R>(&self, body: impl FnOnce(&mut StoreContext<'_, T>) -> R) -> Result<R> {
        PollScope::reach(self.store, |store, waker| store.run_in_turn(waker, body))?
    }
}

impl<T: 'static> Clone for Accessor<T> {
    fn clone(&self) -> Self {
        Self::new(self.store)
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll, Waker};

    use crate::engine::Engine;
    use crate::error::{Error, SchedulerCause};
    use crate::store::Store;

    use super::*;

    /// Poll `future` once, as an executor would.
    fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
        let mut context = Context::from_waker(waker);
        future.as_mut().poll(&mut context)
    }

    /// How a driver entered from inside a closure came out.
    fn cause(outcome: Poll<Result<()>>) -> String {
        match outcome {
            Poll::Ready(Err(error)) => error.to_string(),
            Poll::Ready(Ok(())) => "the driver succeeded".to_owned(),
            Poll::Pending => "the driver returned pending".to_owned(),
        }
    }

    /// The message a recursive reach into the store carries.
    fn recursive() -> String {
        Error::Scheduler(SchedulerCause::RecursiveDriver).to_string()
    }

    /// The message a reach made where no poll of the store is
    /// running carries.
    fn outside_a_poll() -> String {
        Error::Scheduler(SchedulerCause::StoreNotInPoll).to_string()
    }

    #[wcmp_macros::test]
    async fn it_reaches_the_host_data_and_returns_the_closures_value() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");

        let seen = store
            .run_concurrent(async |accessor| {
                accessor
                    .with(|store| {
                        store.data_mut().push_str(" reached");
                        // The borrow the closure is given outlives
                        // nothing, so what the host keeps is a clone.
                        store.data().clone()
                    })
                    .expect("reach the host data")
            })
            .await
            .expect("run the closure");

        assert_eq!(
            seen, "host data reached",
            "the entry returns what the closure returned"
        );
        assert_eq!(
            store.data(),
            "host data reached",
            "what the closure wrote to the host data stayed there"
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_an_accessor_used_inside_another_accessors_closure() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        let seen = store
            .run_concurrent(async |accessor| {
                accessor
                    .with(|_store| {
                        accessor
                            .with(|_store| ())
                            .expect_err("the inner reach is refused")
                            .to_string()
                    })
                    .expect("the outer reach succeeds")
            })
            .await
            .expect("run the closure");

        assert_eq!(
            seen,
            recursive(),
            "an accessor used inside another accessor's closure fails"
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_reach_made_outside_any_poll_of_the_store() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // The token outlives the poll it was handed to: it borrows
        // nothing, so nothing stops it leaving the entry. What it
        // reaches is the slot, and the slot is empty out here.
        let escaped = store
            .run_concurrent(async |accessor| accessor.clone())
            .await
            .expect("run the closure");

        assert_eq!(
            escaped
                .with(|_store| ())
                .expect_err("the reach is refused")
                .to_string(),
            outside_a_poll(),
            "no poll of the store is running, so the reach is refused"
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_reach_with_the_token_of_another_store() {
        let engine = Engine::new().expect("engine");
        let mut first = Store::new(&engine, ()).expect("store");
        let mut second = Store::new(&engine, ()).expect("store");

        let elsewhere = first
            .run_concurrent(async |accessor| accessor.clone())
            .await
            .expect("run the closure");

        let seen = second
            .run_concurrent(async move |_accessor| {
                elsewhere
                    .with(|_store| ())
                    .expect_err("the reach is refused")
                    .to_string()
            })
            .await
            .expect("run the closure");

        assert_eq!(
            seen,
            outside_a_poll(),
            "the slot holds the other store, so the reach is refused"
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_reach_with_a_token_typed_by_other_host_data() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");

        // A token is built from a store's identity, which says
        // which store it is and not what host data that store
        // holds. This one names a store whose host data is a
        // `String` and asks for a `u32`; lending it the store would
        // read the `String` as a `u32`.
        let mistyped: Accessor<u32> = Accessor::new(store.id());

        let seen = store
            .run_concurrent(async move |_accessor| {
                mistyped
                    .with(|store| *store.data())
                    .expect_err("the reach is refused")
                    .to_string()
            })
            .await
            .expect("run the closure");

        assert_eq!(
            seen,
            outside_a_poll(),
            "the poll that is running holds a store with other host \
             data, so the reach is refused"
        );
    }

    // The browser target aborts on a panic instead of unwinding,
    // so there is nothing to catch there and the test is native
    // only.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_ends_the_turn_an_embedders_closure_panicked_out_of() {
        use super::super::driver::Driver;

        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        {
            // The panic is the point of the test, so its report is
            // kept out of the test's output.
            let hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut entry = Box::pin(store.run_concurrent(async |accessor| {
                    accessor.with(|_store| panic!("the embedder's closure panicked"))
                }));
                poll_once(&mut entry, Waker::noop())
            }));
            std::panic::set_hook(hook);

            assert!(
                unwound.is_err(),
                "the closure's panic unwound the reach into the store"
            );
        }

        assert!(
            !store.turn_in_flight(),
            "the turn the closure ran inside is over"
        );
        let mut driver = Box::pin(Driver::new(store.context(), None, |_store, _waker| {
            Some(Ok(()))
        }));
        assert!(
            matches!(poll_once(&mut driver, Waker::noop()), Poll::Ready(Ok(()))),
            "a driver entered after the panic is not refused"
        );
    }

    #[wcmp_macros::test]
    async fn it_reaches_the_store_again_after_a_reach_that_panicked() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // The store the panicking reach took out of the slot went
        // back into it during the unwind, so the poll it unwound
        // inside can reach the store again. The browser aborts on a
        // panic, so the panic itself is native only; what the test
        // asserts on both targets is the reach that follows it.
        let seen = store
            .run_concurrent(async |accessor| {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let hook = std::panic::take_hook();
                    std::panic::set_hook(Box::new(|_| {}));
                    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        accessor.with(|_store| panic!("the embedder's closure panicked"))
                    }));
                    std::panic::set_hook(hook);
                    assert!(unwound.is_err(), "the closure's panic unwound the reach");
                }
                accessor.with(|_store| "reached again")
            })
            .await
            .expect("run the closure");

        assert_eq!(
            seen.expect("the reach after the panic succeeds"),
            "reached again",
            "the store went back into the slot as the panicking reach unwound"
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_nested_run_concurrent_entered_from_inside_the_closure() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        let seen = store
            .run_concurrent(async |accessor| {
                accessor
                    .with(|store: &mut StoreContext<'_, ()>| {
                        let mut nested = Box::pin(store.reborrow().run_concurrent(async |_| ()));
                        cause(poll_once(&mut nested, Waker::noop()))
                    })
                    .expect("reach the store")
            })
            .await
            .expect("run the closure");

        assert_eq!(
            seen,
            recursive(),
            "the store is inside a turn while the closure runs, so a driver \
             entered from there is refused"
        );
    }
}

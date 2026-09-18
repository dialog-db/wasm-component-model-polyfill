//! An accessor that reaches the store from inside a poll.

use core::task::Waker;
use std::sync::{Arc, Mutex, TryLockError};

use crate::error::{Error, Result, SchedulerCause};
use crate::store::StoreContext;

/// What the `run_concurrent` entry lends its accessor: the store,
/// and the waker of the poll that is running. The entry lends both
/// for the whole life of its own future, and the lock around them is
/// what makes a second reach into the store, from inside the first,
/// fail instead of alias.
struct Lent<'a, T: 'static> {
    store: StoreContext<'a, T>,
    waker: Waker,
}

/// An accessor that reaches the store from inside a poll.
///
/// The store's `run_concurrent` entry hands one of these to the
/// closure it runs. The closure's future does not borrow the store:
/// it reaches the store's host data only inside a closure that the
/// accessor runs, during a poll, through [`Accessor::with`]. A value
/// taken from the host data must be cloned out of that closure,
/// because the closure's argument outlives nothing.
///
/// An accessor used inside another accessor's closure fails with the
/// recursive-driver cause, and so does a nested `run_concurrent`
/// entered from inside one: the store is inside a turn while the
/// closure runs, so the guest work the closure reaches runs where
/// every other piece of guest work runs. The other two drivers — a
/// call into an export, an instantiation — cannot be entered from
/// there at all, because each takes a `&mut Store<T>` and the
/// closure holds a [`StoreContext`], which nothing turns back into
/// the store the host owns.
pub struct Accessor<'a, T: 'static> {
    lent: Arc<Mutex<Lent<'a, T>>>,
}

impl<'a, T: 'static> Accessor<'a, T> {
    /// Lend `store` to a fresh accessor.
    ///
    /// The poll that is running has not started yet, so the waker
    /// starts as one that does nothing. Workspace-internal; the
    /// store's `run_concurrent` entry is the only caller.
    pub fn new(store: StoreContext<'a, T>) -> Self {
        Self {
            lent: Arc::new(Mutex::new(Lent {
                store,
                waker: Waker::noop().clone(),
            })),
        }
    }

    /// Record `waker` as the waker of the poll that is running, so
    /// that a host task started from inside the closure is polled
    /// with the waker that reaches the entry's own future.
    /// Workspace-internal.
    pub fn attend(&self, waker: &Waker) -> Result<()> {
        self.borrow(|lent| lent.waker = waker.clone())
    }

    /// Reach the lent store without entering a turn. The
    /// `run_concurrent` entry runs its turns through this: a turn
    /// marks itself as running, so reaching the store through
    /// [`Accessor::with`] here would refuse itself.
    /// Workspace-internal.
    pub fn lend<R>(&self, body: impl FnOnce(&mut StoreContext<'_, T>) -> R) -> Result<R> {
        self.borrow(|lent| body(&mut lent.store))
    }

    /// Run `body` against the store this accessor reaches.
    ///
    /// The store is reachable only for the length of the call.
    /// `body` takes the store by a borrow it cannot hold on to, so
    /// anything read out of the host data must be cloned out.
    ///
    /// The store is inside a turn while `body` runs. A nested
    /// `run_concurrent` entered from there therefore fails with the
    /// recursive-driver cause, and so does this accessor used again
    /// from inside `body`. The other two drivers — a call into an
    /// export, an instantiation — take a `&mut Store<T>`, which the
    /// [`StoreContext`] `body` holds cannot produce, so neither can
    /// be written here.
    pub fn with<R>(&self, body: impl FnOnce(&mut StoreContext<'_, T>) -> R) -> Result<R> {
        self.borrow(|lent| {
            let waker = lent.waker.clone();
            lent.store.run_in_turn(&waker, body)
        })?
    }

    /// Reach what the entry lent. A reach from inside another reach
    /// is the recursive-driver cause: the store is already borrowed
    /// by the outer one.
    fn borrow<R>(&self, body: impl FnOnce(&mut Lent<'a, T>) -> R) -> Result<R> {
        let mut lent = match self.lent.try_lock() {
            Ok(lent) => lent,
            Err(TryLockError::WouldBlock) => {
                return Err(Error::Scheduler(SchedulerCause::RecursiveDriver));
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(Error::internal("the accessor's store lock is poisoned"));
            }
        };
        Ok(body(&mut lent))
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    use crate::engine::Engine;
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
            let accessor = Accessor::new(store.context());
            // The panic is the point of the test, so its report is
            // kept out of the test's output.
            let hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                accessor.with(|_store| panic!("the embedder's closure panicked"))
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

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A token that reaches the store from inside a poll.

use crate::error::Result;
use crate::internal::AccessorInternal;
use crate::projection::{Access, HasData, HasSelf};
use crate::store::StoreContextInternalExt;
use crate::store::StoreId;

use super::poll_scope::PollScope;

/// A token that reaches the store from inside a poll.
///
/// The token carries the identity of one store and a projection of its
/// host data, as Wasmtime's `Accessor` does, and nothing else. It
/// borrows nothing, so it has no lifetime and a future can own one
/// and hold it across its awaits. What it reaches, it reaches
/// through the slot the store leaves itself in while a poll is
/// running: around each poll of the `run_concurrent` closure, around
/// each poll of a host task's body, and around the call of a
/// concurrent registration's closure that produces such a body, the
/// store puts its context in the slot and takes it out again before
/// the poll returns. [`Accessor::with`] takes the store out of that
/// slot, runs a closure against it, and puts it back.
/// [`Accessor::try_with`] does the same, and answers a reach that fails
/// with an error rather than a panic.
///
/// Everything the store offers reaches a body this way: the host
/// data and the resource table a synchronous host function reaches
/// through its call, and the rest of the store besides. The closure
/// is handed an [`Access`], which pairs the store with the token's
/// projection: [`Access::get`] gives the view the projection `D`
/// names, and [`Access::store`] the store's context. The token a
/// driver hands out projects to the whole host data, [`HasSelf`].
/// [`Accessor::with_getter`] gives a token of the same store with
/// another projection, which is how a host crate that serves an
/// interface on a view of the host data reaches that view. The borrow
/// the closure is given outlives nothing, so a value taken from the
/// host data must be cloned out of it.
///
/// These reaches fail rather than lend the store, with a panic in
/// [`Accessor::with`] and with the cause named below in
/// [`Accessor::try_with`]:
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
/// Wasmtime's `with` panics in the first and the last case, and
/// returns the closure's value, as the polyfill's `with` does. So a
/// host's `accessor.with(|access| ..)?` means the same on both
/// runtimes: the `?` is the closure's own. `try_with` is the polyfill's
/// own extension, for a host that would rather handle a failed reach
/// than panic. Wasmtime has no counterpart, and code written for
/// Wasmtime never needs it.
///
/// A nested `run_concurrent` entered from inside a reach fails with
/// the recursive-driver cause as well: the store is inside a turn
/// while the closure runs, so the guest work the closure reaches
/// runs where every other piece of guest work runs. The other two
/// drivers — a call into an export, an instantiation — cannot be
/// entered from there at all, because each takes a `&mut Store<T>`
/// and the closure holds a [`StoreContext`](crate::StoreContext),
/// which nothing turns back into the store the host owns.
///
/// The token is `Clone`, which Wasmtime's is not: the polyfill's host
/// futures are `'static`, so a future that reaches the store after an
/// await owns a copy of the token rather than a borrow of it.
pub struct Accessor<T: 'static, D: HasData + ?Sized = HasSelf<T>> {
    store: StoreId,
    /// The projection the token's reaches hand their closure. A
    /// function pointer holds none of the host data, so the token is
    /// `Send` and `Sync` whatever `T` and `D` are: what it can reach,
    /// it can reach only on the thread whose slot holds the store,
    /// and a store's identity is never reused.
    get_data: fn(&mut T) -> D::Data<'_>,
}

impl<T: 'static, D: HasData + ?Sized> Accessor<T, D> {
    /// Run `body` against the store this token names, and answer what
    /// it returns, as Wasmtime's `with` does.
    ///
    /// The store is reachable only for the length of the call, as
    /// [`try_with`](Self::try_with) states.
    ///
    /// # Panics
    ///
    /// When the reach fails, which [`try_with`](Self::try_with) answers
    /// with an error instead: a reach from inside another reach's
    /// closure, a reach made where no poll of this store is running, and
    /// a reach with a token whose host data type is not the store's.
    /// The message is the error's. In the browser a panic aborts the
    /// page.
    #[track_caller]
    pub fn with<R>(&self, body: impl FnOnce(Access<'_, T, D>) -> R) -> R {
        match self.try_with(body) {
            Ok(value) => value,
            Err(error) => panic!("{error}"),
        }
    }

    /// Run `body` against the store this token names, and answer what
    /// it returns, or the error a reach that fails meets.
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
    ///
    /// The polyfill's own extension: Wasmtime's accessor has only
    /// [`with`](Self::with).
    pub fn try_with<R>(&self, body: impl FnOnce(Access<'_, T, D>) -> R) -> Result<R> {
        let get_data = self.get_data;
        PollScope::reach(self.store, |store, waker| {
            store.internal().run_in_turn(waker, |store| {
                body(Access::new(store.internal().reborrow(), get_data))
            })
        })?
    }

    /// The projection this token hands its reaches. The name is
    /// Wasmtime's.
    pub fn getter(&self) -> fn(&mut T) -> D::Data<'_> {
        self.get_data
    }

    /// A token of the same store whose reaches hand their closure the
    /// projection `get_data`. The name is Wasmtime's.
    pub fn with_getter<D2: HasData + ?Sized>(
        &self,
        get_data: fn(&mut T) -> D2::Data<'_>,
    ) -> Accessor<T, D2> {
        Accessor {
            store: self.store,
            get_data,
        }
    }
}

impl<T: 'static> AccessorInternal<T> for Accessor<T> {
    fn new(store: StoreId) -> Self {
        Accessor {
            store,
            get_data: |data| data,
        }
    }
}

impl<T: 'static, D: HasData + ?Sized> Clone for Accessor<T, D> {
    fn clone(&self) -> Self {
        Accessor {
            store: self.store,
            get_data: self.get_data,
        }
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
    use crate::store::{StoreContextInternalExt, StoreInternalExt};

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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");

        let seen = store
            .run_concurrent(async |accessor| {
                accessor
                    .try_with(|mut access| {
                        access.data_mut().push_str(" reached");
                        // The borrow the closure is given outlives
                        // nothing, so what the host keeps is a clone.
                        access.data().clone()
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
    async fn it_returns_the_closures_value_from_with_as_wasmtime_does() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, 20_u32).expect("store");

        let seen = store
            .run_concurrent(async |accessor| {
                accessor.with(|mut access| {
                    *access.data_mut() += 1;
                    *access.data()
                })
            })
            .await
            .expect("run the closure");

        assert_eq!(seen, 21, "`with` handed back the closure's value itself");
    }

    #[wcmp_macros::test]
    async fn it_passes_the_closures_own_error_to_the_question_mark_after_with() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // Upstream `wasi-webgpu-wasmtime`'s `map-async` has this shape:
        // the closure's own `Result`, and a `?` after `with`. With
        // `with` returning the closure's value, the `?` is the
        // closure's, as it is on Wasmtime, and its error is not
        // dropped.
        fn map_async(accessor: &Accessor<()>) -> core::result::Result<(), String> {
            accessor.with(|_access| -> core::result::Result<(), String> {
                Err("the buffer is already mapped".to_owned())
            })?;
            Ok(())
        }

        let seen = store
            .run_concurrent(async |accessor| map_async(accessor))
            .await
            .expect("run the closure");

        assert_eq!(
            seen,
            Err("the buffer is already mapped".to_owned()),
            "the closure's error reached the caller through the `?`"
        );
    }

    // The browser target aborts on a panic instead of unwinding, so
    // there is nothing to catch there and the test is native only. The
    // causes themselves are tested through `try_with` on both targets.
    #[cfg(not(target_arch = "wasm32"))]
    #[wcmp_macros::test]
    async fn it_panics_with_the_cause_when_with_reaches_outside_a_poll() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let escaped = store
            .run_concurrent(async |accessor| accessor.clone())
            .await
            .expect("run the closure");

        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let unwound = std::panic::catch_unwind(|| escaped.with(|_access| ()));
        std::panic::set_hook(hook);

        let payload = unwound.expect_err("the reach outside a poll panics");
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            message,
            outside_a_poll(),
            "the panic carries the cause `try_with` answers with"
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_an_accessor_used_inside_another_accessors_closure() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        let seen = store
            .run_concurrent(async |accessor| {
                accessor
                    .try_with(|_access| {
                        accessor
                            .try_with(|_access| ())
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
                .try_with(|_access| ())
                .expect_err("the reach is refused")
                .to_string(),
            outside_a_poll(),
            "no poll of the store is running, so the reach is refused"
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_reach_with_the_token_of_another_store() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut first = Store::new(&engine, ()).expect("store");
        let mut second = Store::new(&engine, ()).expect("store");

        let elsewhere = first
            .run_concurrent(async |accessor| accessor.clone())
            .await
            .expect("run the closure");

        let seen = second
            .run_concurrent(async move |_accessor| {
                elsewhere
                    .try_with(|_access| ())
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");

        // A token is built from a store's identity, which says
        // which store it is and not what host data that store
        // holds. This one names a store whose host data is a
        // `String` and asks for a `u32`; lending it the store would
        // read the `String` as a `u32`.
        let mistyped: Accessor<u32> = Accessor::new(store.internal().id());

        let seen = store
            .run_concurrent(async move |_accessor| {
                mistyped
                    .try_with(|access| *access.data())
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

        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        {
            // The panic is the point of the test, so its report is
            // kept out of the test's output.
            let hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut entry = Box::pin(store.run_concurrent(async |accessor| {
                    accessor.try_with(|_access| panic!("the embedder's closure panicked"))
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
            !store.internal().turn_in_flight(),
            "the turn the closure ran inside is over"
        );
        let mut driver = Box::pin(Driver::run(store.internal().context(), |_store, _waker| {
            Some(Ok(()))
        }));
        assert!(
            matches!(poll_once(&mut driver, Waker::noop()), Poll::Ready(Ok(()))),
            "a driver entered after the panic is not refused"
        );
    }

    #[wcmp_macros::test]
    async fn it_reaches_the_store_again_after_a_reach_that_panicked() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
                        accessor.try_with(|_access| panic!("the embedder's closure panicked"))
                    }));
                    std::panic::set_hook(hook);
                    assert!(unwound.is_err(), "the closure's panic unwound the reach");
                }
                accessor.try_with(|_access| "reached again")
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        let seen = store
            .run_concurrent(async |accessor| {
                accessor
                    .try_with(|mut access| {
                        let store = access.store();
                        let mut reborrowed = store.internal().reborrow();
                        let mut nested =
                            Box::pin(reborrowed.internal().run_concurrent(async |_| ()));
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

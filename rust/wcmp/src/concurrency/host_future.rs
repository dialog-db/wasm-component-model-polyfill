// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The bound the future of a host `async` function carries.

use core::future::Future;

use crate::error::Result;
use crate::value::Val;

/// The bound the future of a host `async` function carries.
///
/// One call of a host `async` function produces a future the store
/// owns and polls. The runtime layer hands a host trampoline a
/// synchronous closure and nothing else, so the trampoline cannot
/// run the future itself: it gives the future to the store and
/// returns to the guest, and later turns poll it. The future is
/// therefore `'static` and does not borrow the store.
///
/// The `Send` half of the bound is the one per-target line. It is
/// required natively, so that a store stays `Send` as it is today.
/// It is absent in the browser: a JavaScript promise wrapped as a
/// future is not `Send`, and awaiting one is the whole purpose of a
/// browser host function.
///
/// `Out` is what the future produces. The default is the value
/// vector one call answers with, which is what a store polls a host
/// task for and what the future of an untyped registration produces.
/// The future of a typed registration produces the closure's own
/// return type instead, and the registration turns that into the
/// vector, so one trait carries both forms and the per-target line
/// stays in one place.
#[cfg(not(target_arch = "wasm32"))]
pub trait HostFuture<Out = Result<Vec<Val>>>: Future<Output = Out> + Send + 'static {}

#[cfg(not(target_arch = "wasm32"))]
impl<Out, F> HostFuture<Out> for F where F: Future<Output = Out> + Send + 'static {}

/// The bound the future of a host `async` function carries. See the
/// native definition for what it is, what `Out` is, and why the
/// `Send` half is absent here.
#[cfg(target_arch = "wasm32")]
pub trait HostFuture<Out = Result<Vec<Val>>>: Future<Output = Out> + 'static {}

#[cfg(target_arch = "wasm32")]
impl<Out, F> HostFuture<Out> for F where F: Future<Output = Out> + 'static {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Take a future that satisfies the bound and give it back. A
    /// future that does not satisfy the bound of the target being
    /// built fails to compile here.
    fn accepts<F: HostFuture>(future: F) -> F {
        future
    }

    /// Take a future whose value is the return of a typed
    /// registration's closure and give it back. The `Out` parameter
    /// is what lets the one bound cover that form as well as the
    /// value vector.
    fn accepts_typed<F: HostFuture<Result<u32>>>(future: F) -> F {
        future
    }

    /// The bound takes the future of a typed registration, whose
    /// value is the closure's own return rather than the value
    /// vector the registration turns it into.
    #[wcmp_macros::test]
    async fn it_takes_a_future_of_a_typed_registrations_return() {
        let future = accepts_typed(async { Ok(7u32) });

        assert_eq!(
            future.await.expect("the future's value"),
            7,
            "the future the bound took runs and produces the closure's return"
        );
    }

    /// The native bound takes a future that is `Send`, which is what
    /// keeps a store `Send` while it holds one.
    #[cfg(not(target_arch = "wasm32"))]
    #[wcmp_macros::test]
    async fn it_takes_a_send_future_on_the_native_target() {
        fn assert_send<F: Send>(future: F) -> F {
            future
        }

        let future = assert_send(accepts(async { Ok(vec![Val::U32(7)]) }));

        assert_eq!(
            future.await.expect("the future's value"),
            vec![Val::U32(7)],
            "the future the native bound took runs and produces its value"
        );
    }

    /// The browser bound takes a future that is not `Send`: a
    /// JavaScript promise wrapped as a future is not, and neither is
    /// a block that awaits one. Awaiting a promise is the whole
    /// purpose of a browser host function, which is why the `Send`
    /// half of the bound is absent on this target.
    #[cfg(target_arch = "wasm32")]
    #[wcmp_macros::test]
    async fn it_takes_a_js_future_in_the_browser() {
        let promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::from_f64(7.0));
        let future = accepts(async move {
            let resolved = wasm_bindgen_futures::JsFuture::from(promise)
                .await
                .expect("the promise resolves");
            assert_eq!(resolved.as_f64(), Some(7.0), "the promise's value arrives");
            Ok(vec![Val::U32(7)])
        });

        assert_eq!(
            future.await.expect("the future's value"),
            vec![Val::U32(7)],
            "the future the browser bound took runs and produces its value"
        );
    }
}

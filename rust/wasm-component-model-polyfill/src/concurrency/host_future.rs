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
#[cfg(not(target_arch = "wasm32"))]
pub trait HostFuture: Future<Output = Result<Vec<Val>>> + Send + 'static {}

#[cfg(not(target_arch = "wasm32"))]
impl<F> HostFuture for F where F: Future<Output = Result<Vec<Val>>> + Send + 'static {}

/// The bound the future of a host `async` function carries. See the
/// native definition for what it is and why the `Send` half is
/// absent here.
#[cfg(target_arch = "wasm32")]
pub trait HostFuture: Future<Output = Result<Vec<Val>>> + 'static {}

#[cfg(target_arch = "wasm32")]
impl<F> HostFuture for F where F: Future<Output = Result<Vec<Val>>> + 'static {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Take a future that satisfies the bound and give it back. A
    /// future that does not satisfy the bound of the target being
    /// built fails to compile here.
    fn accepts<F: HostFuture>(future: F) -> F {
        future
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

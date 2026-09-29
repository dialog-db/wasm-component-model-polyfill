//! The future a backend returns for an asynchronous operation.

use core::future::Future;
use core::pin::Pin;

/// A boxed future that is `Send` on every target but `wasm32`.
///
/// A method of the backend contract cannot be `async`, because the engine
/// holds the backend behind dynamic dispatch. It returns this type instead.
/// Natively the future is `Send`, so a host can await it on a multi-threaded
/// executor. In the browser it awaits JavaScript promises, which are not
/// `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A boxed future that is `Send` on every target but `wasm32`.
///
/// A method of the backend contract cannot be `async`, because the engine
/// holds the backend behind dynamic dispatch. It returns this type instead.
/// Natively the future is `Send`, so a host can await it on a multi-threaded
/// executor. In the browser it awaits JavaScript promises, which are not
/// `Send`.
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

//! The bound the action of a queued item carries.

use crate::store::Store;

/// The bound the action of a queued item carries.
///
/// An item outlives the driver that queued it: dropping a driver's
/// future cancels nothing, so the action sits in the store until
/// some turn runs it. The action is therefore `'static`, and it
/// reaches the store it runs against through the argument the
/// scheduler hands it rather than by borrowing one.
///
/// The `Send` half of the bound is the one per-target line. A store
/// stays `Send` natively, so an action queued into it must be `Send`
/// too. In the browser the bound is absent: the whole polyfill runs
/// on one thread there, and an action that captures a JavaScript
/// value is not `Send` and does not need to be.
#[cfg(not(target_arch = "wasm32"))]
pub trait ItemAction<T: 'static>: FnOnce(&mut Store<T>) + Send + 'static {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: 'static, F> ItemAction<T> for F where F: FnOnce(&mut Store<T>) + Send + 'static {}

/// The bound the action of a queued item carries. See the native
/// definition for what it is and why the `Send` half is absent here.
#[cfg(target_arch = "wasm32")]
pub trait ItemAction<T: 'static>: FnOnce(&mut Store<T>) + 'static {}

#[cfg(target_arch = "wasm32")]
impl<T: 'static, F> ItemAction<T> for F where F: FnOnce(&mut Store<T>) + 'static {}

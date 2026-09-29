//! `Sync` where the target has threads, and nothing where it does not.

/// `Sync` on every target but `wasm32`, and no bound on `wasm32`.
///
/// Natively, one engine is shared between threads, as Wasmtime's is, so a
/// backend and its compiled modules must be `Sync`. In the browser, a
/// backend holds JavaScript values, which never leave the thread that made
/// them. Every type implements this trait wherever it would implement
/// `Sync`, so a backend never implements it by hand.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSync: Sync {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync + ?Sized> MaybeSync for T {}

/// `Sync` on every target but `wasm32`, and no bound on `wasm32`.
///
/// Natively, one engine is shared between threads, as Wasmtime's is, so a
/// backend and its compiled modules must be `Sync`. In the browser, a
/// backend holds JavaScript values, which never leave the thread that made
/// them. Every type implements this trait wherever it would implement
/// `Sync`, so a backend never implements it by hand.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSync {}

#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSync for T {}

//! The bound what runs after a thread entry carries.

use crate::error::Result;
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreContext;

/// The bound what runs after a thread entry carries: the part of the
/// frame that started the entry which would have run once the entry
/// returned.
///
/// It is handed the entry's core results, or the trap the entry
/// raised, and answers what the frame would have answered. Under a
/// provider it runs when the entry finishes, which can be after the
/// thread suspended and a later turn resumed it, so it is `'static`
/// and reaches the store through its argument.
///
/// The `Send` half of the bound is the one per-target line, for the
/// reason [`ItemAction`](super::item_action::ItemAction) gives.
#[cfg(not(target_arch = "wasm32"))]
pub trait EntryFinish<T: 'static>:
    FnOnce(&mut StoreContext<'_, T>, Result<Vec<RuntimeVal>>) -> Result<()> + Send + 'static
{
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: 'static, F> EntryFinish<T> for F where
    F: FnOnce(&mut StoreContext<'_, T>, Result<Vec<RuntimeVal>>) -> Result<()> + Send + 'static
{
}

/// The bound what runs after a thread entry carries. See the native
/// definition for what it is and why the `Send` half is absent here.
#[cfg(target_arch = "wasm32")]
pub trait EntryFinish<T: 'static>:
    FnOnce(&mut StoreContext<'_, T>, Result<Vec<RuntimeVal>>) -> Result<()> + 'static
{
}

#[cfg(target_arch = "wasm32")]
impl<T: 'static, F> EntryFinish<T> for F where
    F: FnOnce(&mut StoreContext<'_, T>, Result<Vec<RuntimeVal>>) -> Result<()> + 'static
{
}

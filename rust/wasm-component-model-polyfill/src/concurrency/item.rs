//! One item of the scheduler's ready queues.

use crate::store::Store;

use super::item_action::ItemAction;
use super::item_kind::ItemKind;

/// The boxed action of one item, with the `Send` bound the native
/// target puts on everything a store holds.
#[cfg(not(target_arch = "wasm32"))]
type BoxedAction<T> = Box<dyn FnOnce(&mut Store<T>) + Send + 'static>;

/// The boxed action of one item. The browser drops the `Send` bound:
/// see [`ItemAction`].
#[cfg(target_arch = "wasm32")]
type BoxedAction<T> = Box<dyn FnOnce(&mut Store<T>) + 'static>;

/// One item of the scheduler's ready queues.
///
/// An item is a piece of work the store holds until a turn runs it.
/// It runs to its next yield point and returns; whatever it produces
/// it leaves in the store, because the driver whose turn ran it is
/// not necessarily the driver that queued it. Dropping a driver's
/// future cancels nothing, and dropping the store drops every item
/// unrun.
pub struct Item<T: 'static> {
    kind: ItemKind,
    action: BoxedAction<T>,
}

impl<T: 'static> Item<T> {
    /// Build an item of `kind` that runs `action` against the store.
    pub fn new(kind: ItemKind, action: impl ItemAction<T>) -> Self {
        Self {
            kind,
            action: Box::new(action),
        }
    }

    /// What this item does when it runs.
    pub fn kind(&self) -> ItemKind {
        self.kind
    }

    /// Run the item against `store`, consuming it.
    pub fn run(self, store: &mut Store<T>) {
        (self.action)(store);
    }
}

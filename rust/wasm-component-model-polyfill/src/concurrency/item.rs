//! One item of the scheduler's ready queues.

use crate::error::Result;
use crate::store::StoreContext;

use super::instance_id::InstanceId;
use super::item_action::ItemAction;
use super::item_kind::ItemKind;
use super::task_id::TaskId;

/// The boxed action of one item, with the `Send` bound the native
/// target puts on everything a store holds.
#[cfg(not(target_arch = "wasm32"))]
type BoxedAction<T> = Box<dyn FnOnce(&mut StoreContext<'_, T>) -> Result<()> + Send + 'static>;

/// The boxed action of one item. The browser drops the `Send` bound:
/// see [`ItemAction`].
#[cfg(target_arch = "wasm32")]
type BoxedAction<T> = Box<dyn FnOnce(&mut StoreContext<'_, T>) -> Result<()> + 'static>;

/// One item of the scheduler's ready queues.
///
/// An item is a piece of work the store holds until a turn runs it.
/// It runs to its next yield point and returns; whatever it produces
/// it leaves in the store, because the driver whose turn ran it is
/// not necessarily the driver that queued it. An item that fails
/// ends the turn that ran it, and the driver that polled that turn
/// sees the failure: [`ItemAction`] says which failures those are.
/// Dropping a driver's future cancels nothing, and dropping the
/// store drops every item unrun.
pub struct Item<T: 'static> {
    kind: ItemKind,
    instance: Option<InstanceId>,
    task: Option<TaskId>,
    action: BoxedAction<T>,
}

impl<T: 'static> Item<T> {
    /// Build an item of `kind` that runs `action` against the store.
    /// The item names no instance until
    /// [`in_instance`](Self::in_instance) says which one it belongs
    /// to, and no task until [`for_task`](Self::for_task) says whose
    /// work it is.
    pub fn new(kind: ItemKind, action: impl ItemAction<T>) -> Self {
        Self {
            kind,
            instance: None,
            task: None,
            action: Box::new(action),
        }
    }

    /// Say which component instance this item's work belongs to.
    ///
    /// A task that must not block gives way only to the ready work
    /// of its own instance, so a turn run for such a task asks each
    /// item whose instance it is. An item that names none is not
    /// that instance's work and such a turn leaves it queued.
    ///
    /// Today the entry gate is the one caller, so a
    /// [`ItemKind::TaskStart`] item is the one kind that arrives
    /// tagged. A [`ItemKind::HostResultLowering`] item names no
    /// instance on purpose: it is the host's work and no component
    /// instance's. The other two kinds are untagged only because
    /// nothing builds them yet. A [`ItemKind::Callback`] item and a
    /// [`ItemKind::ThreadResumption`] item are both the instance's
    /// own work, so each must be tagged where it is built, or a turn
    /// held to that instance will silently skip the very work the
    /// task is waiting for.
    pub fn in_instance(mut self, instance: InstanceId) -> Self {
        self.instance = Some(instance);
        self
    }

    /// Say which task this item's work belongs to.
    ///
    /// An item queued for a task is that task's pending work, and a
    /// task's pending work goes with its record: the scheduler drops
    /// every item that names a task whose record leaves the store,
    /// so no item can run against a task that is not there. An item
    /// that belongs to no one task — a host task's result lowering,
    /// the resumption of a thread — names none, and nothing drops
    /// it.
    pub fn for_task(mut self, task: TaskId) -> Self {
        self.task = Some(task);
        self
    }

    /// What this item does when it runs.
    pub fn kind(&self) -> ItemKind {
        self.kind
    }

    /// The component instance this item's work belongs to, when it
    /// names one.
    pub fn instance(&self) -> Option<InstanceId> {
        self.instance
    }

    /// The task this item's work belongs to, when it names one.
    pub fn task(&self) -> Option<TaskId> {
        self.task
    }

    /// Run the item against `store`, consuming it.
    pub fn run(self, store: &mut StoreContext<'_, T>) -> Result<()> {
        (self.action)(store)
    }
}

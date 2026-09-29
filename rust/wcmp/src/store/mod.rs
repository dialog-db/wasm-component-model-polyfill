//! The polyfill's owner of guest state, what that store carries,
//! and the borrow of it that guest work runs against.
//!
//! [`Store`] is the unit of isolation between independent component
//! instances. It owns the core store the runtime layer gives it, and
//! nothing else: everything else it carries rides in that store's
//! data.
//!
//! [`StoreData`] is that data — the host's own value of type `T`,
//! and beside it the store as the polyfill knows it: the handle
//! tables, the scheduler with its suspend seam, the registered
//! destructors, and the store's identity. It sits there because of
//! what a host trampoline holds: the runtime layer hands a
//! trampoline the core store's context and nothing else, and bounds
//! every trampoline `Send + Sync`, which the scheduler is not in the
//! browser.
//!
//! [`StoreContext`] is a borrow of that core store, and so of all of
//! it. It is what a turn, a queued item, a host task's lowering, and
//! a blocking built-in all run against, so each of them runs the
//! same way from a driver's poll and from inside a trampoline.
//! PDD018 asks for that: a suspended guest thread resumes outside
//! any poll of a driver, so the scheduler's state has to be
//! reachable with no driver on the stack.
//!
//! [`ResourceRecord`] is what the store knew about one resource
//! type at one moment. An instantiation takes one per resource type
//! it registers and puts them back when its plan fails, so that a
//! failed instantiation leaves the store as it found it.
//!
//! [`StoreId`] is the process-unique identity a store mints at
//! construction, which an instance records and checks.
mod resource_record;
#[allow(clippy::module_inception)]
mod store;
mod store_context;
mod store_data;
mod store_id;

pub use resource_record::ResourceRecord;
pub use store::Store;
pub use store::internal::StoreInternalExt;
pub use store_context::StoreContext;
pub use store_context::internal::StoreContextInternalExt;
pub use store_data::StoreData;
pub use store_id::StoreId;

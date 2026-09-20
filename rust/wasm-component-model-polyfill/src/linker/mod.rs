//! The polyfill's build-up of the host environment a component links
//! against.
//!
//! [`Linker`] organises host items by interface: a
//! [`LinkerInstance`] borrowed from a [`Linker`] is the unit of "a
//! single interface's worth of host items," addressed by a
//! [`PackageName`] and an [`InterfaceIdentifier`].
//!
//! The module exposes the *addressing* surface, the host-item
//! registration modes a [`LinkerInstance`] offers, and the
//! identifier-resolution logic that walks a component's imports and
//! matches each against the registered linker instances. A function
//! item is registered typed or untyped, and synchronous or
//! concurrent; the registration records which of the latter two it is
//! as its [`HostFuncKind`], because the link rule and the trampoline
//! read it. A component whose imports have no matching registration
//! fails cleanly with [`Error::Link`].
//!
//! [`PackageName`]: crate::PackageName
//! [`InterfaceIdentifier`]: crate::InterfaceIdentifier
//! [`Error::Link`]: crate::Error::Link

mod component_value;
mod host_call;
mod host_func;
mod host_func_kind;
mod host_resource;
#[allow(clippy::module_inception)]
mod linker;
mod linker_instance;
mod module_matching;
mod registration;
mod resolve;

pub use component_value::{ComponentParameters, ComponentResult, ComponentValue};
pub use host_call::HostCall;
pub use host_func::HostFuncBody;
pub use host_func_kind::HostFuncKind;
pub use host_resource::{DestructorBody, HostResource};
pub use linker::Linker;
pub use linker_instance::LinkerInstance;
pub use registration::InstanceRegistration;
pub use resolve::{ImportBinding, Resolution};

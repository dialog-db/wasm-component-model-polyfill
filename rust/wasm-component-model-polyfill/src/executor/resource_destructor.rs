//! Where a resource's destructor comes from: a host closure for an
//! imported resource, or a core function of the defining instance for
//! a locally-defined one.

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::Func as RuntimeFunc;

use crate::linker::DestructorBody;

/// The destructor a `resource.drop` runs after it removes the handle.
pub enum ResourceDestructor<T> {
    /// The host closure registered for an imported resource.
    Host(Arc<DestructorBody<T>>),
    /// The core function a locally-defined resource names, bound when
    /// the defining core instance exists. `None` inside the slot means
    /// the resource declares no destructor, and a drop invokes nothing.
    Local(Arc<Mutex<Option<RuntimeFunc>>>),
}

impl<T> Clone for ResourceDestructor<T> {
    fn clone(&self) -> Self {
        match self {
            ResourceDestructor::Host(body) => ResourceDestructor::Host(body.clone()),
            ResourceDestructor::Local(slot) => ResourceDestructor::Local(slot.clone()),
        }
    }
}

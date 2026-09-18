//! Where a resource's destructor comes from: a host closure for an
//! imported resource, or a core function of the defining instance for
//! a locally-defined one.

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::Func as RuntimeFunc;

use crate::concurrency::InstanceId;
use crate::linker::DestructorBody;

/// The destructor a `resource.drop` runs after it removes the handle.
pub enum ResourceDestructor<T> {
    /// The host closure registered for an imported resource. The
    /// host implements the resource, so the task the destructor runs
    /// as belongs to no component instance.
    Host(Arc<DestructorBody<T>>),
    /// The core function a locally-defined resource names, bound when
    /// the defining core instance exists, with the component instance
    /// that defines it. The reference lifts the destructor in the
    /// instance that implements the resource, so that is the instance
    /// the destructor's task belongs to. `None` inside the slot means
    /// the resource declares no destructor, and a drop invokes
    /// nothing.
    Local {
        /// The destructor itself, once the `DefineResource` directive
        /// has bound it.
        function: Arc<Mutex<Option<RuntimeFunc>>>,
        /// The component instance that defines the resource.
        instance: InstanceId,
    },
}

impl<T> ResourceDestructor<T> {
    /// The component instance the destructor's task belongs to:
    /// the defining instance of a locally-defined resource, and
    /// none at all for a resource the host implements.
    pub fn instance(&self) -> Option<InstanceId> {
        match self {
            ResourceDestructor::Host(_) => None,
            ResourceDestructor::Local { instance, .. } => Some(*instance),
        }
    }
}

impl<T> Clone for ResourceDestructor<T> {
    fn clone(&self) -> Self {
        match self {
            ResourceDestructor::Host(body) => ResourceDestructor::Host(body.clone()),
            ResourceDestructor::Local { function, instance } => ResourceDestructor::Local {
                function: function.clone(),
                instance: *instance,
            },
        }
    }
}

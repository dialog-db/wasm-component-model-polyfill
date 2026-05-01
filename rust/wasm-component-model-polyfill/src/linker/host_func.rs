//! Per-import host-function payload stored on a [`LinkerInstance`].
//!
//! `HostFunc<T>` is the runtime carrier for what a developer
//! registers via either [`LinkerInstance::func_new`] (untyped, takes
//! `Val` slices) or [`LinkerInstance::func_wrap`] (typed, statically
//! shaped). Both paths converge on this single type so the host-
//! trampoline code in [`crate::executor::instantiate`] dispatches
//! against one shape.
//!
//! [`LinkerInstance`]: super::LinkerInstance
//! [`LinkerInstance::func_new`]: super::LinkerInstance::func_new
//! [`LinkerInstance::func_wrap`]: super::LinkerInstance::func_wrap

use std::sync::Arc;

use crate::component::FunctionType;
use crate::error::Result;
use crate::value::Val;

/// One registered host function inside an [`crate::LinkerInstance`].
///
/// Wraps the `Fn`-trait-object closure and the declared
/// [`FunctionType`] the developer named (untyped) or the polyfill
/// derived (typed). Both registration paths produce the same
/// `HostFunc` so the trampoline dispatcher only sees one shape.
#[derive(Clone)]
pub struct HostFunc<T> {
    /// The signature the registration declares. The resolver checks
    /// this against the import's declared type at link time.
    pub signature: FunctionType,
    /// The closure this registration carries. The polyfill's host-
    /// trampoline implementation calls this with the lifted host-
    /// side argument list and a buffer for the returned values.
    pub call: Arc<HostFuncBody<T>>,
}

/// The closure type a [`HostFunc`] holds.
///
/// The closure takes a `&mut T` (the store's host-data slot), a
/// slice of host-lifted [`Val`] arguments, and a mutable slice the
/// implementation fills with the host's `Val` results. The result
/// slice is sized by the polyfill from the registration's declared
/// signature.
pub type HostFuncBody<T> =
    dyn Fn(&mut T, &[Val], &mut [Val]) -> Result<()> + Send + Sync + 'static;

impl<T> HostFunc<T> {
    /// Construct a host-function payload from its signature and a
    /// closure.
    pub fn new(
        signature: FunctionType,
        call: impl Fn(&mut T, &[Val], &mut [Val]) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            signature,
            call: Arc::new(call),
        }
    }
}

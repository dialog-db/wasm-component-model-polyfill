//! A resumable call of the browser backend that runs.

use core::any::Any;
use std::rc::Rc;

use wcmp_wasm_core::backend::BackendResumption;

use crate::flight::Flight;
use crate::returns::Returns;

/// A resumable call that runs, started or resumed, until a wait sees its
/// next stop: its flight, and how the host reads the results of the call
/// at its end.
///
/// The handle holds no store. Where a wait drops, the flight parks the next
/// time its stack would reach the store, and the next wait takes it up.
/// Where the handle drops, nothing can take the flight up any more: it
/// parks and never runs again, unless the store dropped first.
pub struct WebResumption {
    pub flight: Rc<Flight>,
    pub returns: Returns,
}

impl BackendResumption for WebResumption {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

//! The error of a host function, on its way through Wasmi.

use core::fmt;

/// The error a host function returned, wrapped so that it crosses Wasmi
/// and comes back out unchanged.
///
/// Wasmi carries a host function's error out of the call that ran the
/// guest as a host error of its own, which it can hand back by its type.
/// The backend wraps the error in this type on the way in, and finds the
/// wrapper again on the way out, so the host receives exactly the error its
/// function returned, and no other error of Wasmi's is taken for one.
pub struct HostError(pub anyhow::Error);

impl fmt::Debug for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl wasmi::errors::HostError for HostError {}

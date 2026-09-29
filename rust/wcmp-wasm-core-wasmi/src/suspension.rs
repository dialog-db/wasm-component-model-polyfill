//! The answer "not yet" of a suspending host function, on its way through
//! Wasmi.

use core::fmt;

/// The error a suspending host function returns to Wasmi when its body
/// answers "not yet".
///
/// Inside a resumable call, Wasmi sets the call aside at any error of a
/// host function that a WebAssembly frame called, and hands back a
/// `ResumableCallHostTrap`. The backend suspends the call only where the
/// error is this marker, and turns any other error into the trap it stands
/// for. Everywhere else, the marker leaves Wasmi as the error of the call:
/// a call that is not resumable, such as the call a host function makes
/// back into a guest, or a host function that the root frame of a
/// resumable call tail-calls, which Wasmi does not set aside. The call
/// then traps with `TrapKind::Host`, with this marker's message, as it
/// does on every backend.
#[derive(Debug)]
pub struct Suspension;

impl fmt::Display for Suspension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "a suspending host function answered \"not yet\" where its call cannot suspend: \
             the call is not a resumable call, a frame of the host lies between the start of \
             the resumable call and the host function, or the root frame of the resumable call \
             tail-calls the host function",
        )
    }
}

impl wasmi::errors::HostError for Suspension {}

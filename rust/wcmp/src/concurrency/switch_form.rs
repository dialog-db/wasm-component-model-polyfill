//! Which provider's form of the switch module a store generates.

/// Which provider's form of the switch module a store generates.
///
/// Both forms have the same shims and entry wrappers, and differ
/// only in how a shim suspends and how a thread starts and resumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchForm {
    /// The form that suspends with the instructions of the
    /// WebAssembly stack-switching proposal, and keeps each suspended
    /// thread in a table of continuations.
    StackSwitching,
    /// The form that suspends through JavaScript Promise Integration:
    /// a shim suspends by calling an import made with
    /// `WebAssembly.Suspending`, and a thread starts through
    /// `WebAssembly.promising`.
    Jspi,
}

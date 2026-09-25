//! Which provider fills an [`Engine`]'s suspend capability.
//!
//! [`Engine`]: crate::Engine

/// Which provider fills an [`Engine`]'s suspend capability, as
/// [`Engine::suspend_provider`] answers it.
///
/// A blocking built-in suspends the guest thread it runs on until
/// the thing it waits for is ready. Wasmtime serves that by
/// switching fibers, which always exist. The polyfill runs a guest
/// on the one real stack, so it needs a provider to set that stack
/// aside and resume it later. Where no provider exists, the
/// polyfill runs the waiting work in a nested turn above the
/// blocked call instead. A block whose releasing work lies on a
/// frame below it then fails with the stack-switch cause, which is
/// [`SchedulerCause::StackSwitchNeeded`].
///
/// The engine selects the provider once, when it is constructed,
/// and keeps the answer for its life. It takes the first that
/// holds of:
///
/// 1. No provider, when the host turned the provider off through
///    [`EngineConfig::suspend_provider`].
/// 2. The stack-switching provider, when the engine runs a probe
///    thread that suspends and resumes through the WebAssembly
///    stack-switching instructions.
/// 3. The JSPI provider, when the browser offers JavaScript Promise
///    Integration.
/// 4. No provider.
///
/// The switch probe passes on the native engine on x86_64 Linux,
/// where Wasmtime implements the stack-switching proposal. The JSPI
/// probe does not exist yet, so an engine answers
/// [`None`](Self::None) in the browser and on every other native
/// platform today.
///
/// Wasmtime has no counterpart to this answer, because its fibers
/// always exist, so the names are the polyfill's own.
///
/// The enum is non-exhaustive: a provider for another engine can
/// join the two here.
///
/// [`Engine`]: crate::Engine
/// [`Engine::suspend_provider`]: crate::Engine::suspend_provider
/// [`EngineConfig::suspend_provider`]: crate::EngineConfig::suspend_provider
/// [`SchedulerCause::StackSwitchNeeded`]: crate::SchedulerCause::StackSwitchNeeded
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SuspendProviderKind {
    /// The stack-switching provider: the WebAssembly stack-switching
    /// instructions, where the engine implements them. A thread it
    /// suspends resumes synchronously.
    StackSwitching,
    /// The JSPI provider: JavaScript Promise Integration, in a
    /// browser that ships it. A thread it suspends resumes on a
    /// microtask.
    Jspi,
    /// No provider: a blocking built-in runs the waiting work in a
    /// nested turn above the blocked call.
    None,
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
/// 2. The stack-switching provider, when the backend declares stack
///    switching and runs a probe thread that suspends and resumes
///    through the WebAssembly stack-switching instructions.
/// 3. The host-suspension provider, when the backend declares host
///    suspension: a host function that can answer "not yet", and a
///    call of a guest that the host resumes later. The browser's
///    backend declares it where the browser offers JavaScript
///    Promise Integration.
/// 4. No provider.
///
/// The switch probe passes on the Wasmtime backend on x86_64 Linux,
/// where Wasmtime implements the stack-switching proposal. The
/// browser's backend declares host suspension in every current
/// browser. An engine answers [`None`](Self::None) on a backend that
/// declares neither, such as Wasmtime on another native platform or
/// the browser's backend in an older browser such as Safari 26.
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
    /// The host-suspension provider: a suspending host function and
    /// a resumable call, where the backend declares them. A thread it
    /// suspends resumes when the driver of the store awaits it, which
    /// in the browser is on a microtask.
    HostSuspension,
    /// No provider: a blocking built-in runs the waiting work in a
    /// nested turn above the blocked call.
    None,
}

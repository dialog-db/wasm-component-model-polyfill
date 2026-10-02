// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![warn(missing_docs)]

//! The Wasmi backend of the runtime layer of the Wasm Component Model
//! Polyfill.
//!
//! This is the practical native backend: Wasmi is an interpreter, small and
//! quick to start, with no compiler to trust. A host makes an engine over it
//! with [`Wasmi::new`] and
//! [`Engine::with_backend`](wcmp_wasm_core::Engine::with_backend), and from
//! then on sees only the types of `wcmp_wasm_core`.
//!
//! Wasmi builds for every target, `wasm32` included, so this crate does
//! too, and a page can run the polyfill over it. No test lane runs that
//! configuration.
//!
//! # Building
//!
//! Wasmi dispatches its instructions with tail calls wherever it is built
//! optimized, and with a loop otherwise. Its debug assertions keep the
//! compiler from making those calls tail calls, so a Wasmi built optimized
//! with debug assertions overflows the native stack on a long run
//! (wasmi-labs/wasmi#2039). A host whose profile builds its dependencies
//! optimized with debug assertions, as a development profile with
//! `opt-level = 3` for every package does, turns them off for Wasmi:
//!
//! ```toml
//! [profile.dev.package.wasmi]
//! debug-assertions = false
//! ```
//!
//! # Compilation and instantiation
//!
//! Wasmi compiles and instantiates synchronously. The asynchronous compile
//! and instantiation of the runtime layer finish at once: each future is
//! ready on its first poll. Each compile makes a module of its own, and the
//! backend keeps no cache of modules by their bytes. Wasmi validates the
//! whole module when it compiles it, and translates each function the first
//! time it runs.
//!
//! A module that Wasmi refuses because it needs a capability the backend
//! does not declare is
//! [`Error::Unsupported`](wcmp_wasm_core::Error::Unsupported), with the name
//! of the capability. The backend finds the capability with
//! [`missing_capability`](wcmp_wasm_core::backend::missing_capability),
//! which asks `wasmparser`'s validator, whose feature names the capability
//! lexicon repeats: the module must validate under every feature, and the
//! least set of features under which it validates must hold a capability
//! the backend lacks. Where
//! the module needs several, the error names the one of them that comes
//! last in the lexicon, which is the proposal that builds on the others: a
//! module with GC types and a tag is `gc`. Any other refusal is
//! [`Error::Compile`](wcmp_wasm_core::Error::Compile), with Wasmi's message.
//!
//! # The boundary
//!
//! Wasmi's type model is Wasm 2.0's: its only reference types are
//! `funcref` and `externref`, and it has no tags. The backend describes the
//! imports and exports of a module in that model. Where the host asks for
//! anything above it, such as a global of a GC reference type or the type
//! of a tag, the backend returns `Unsupported` with the capability that
//! type needs.
//!
//! # References
//!
//! A handle of the runtime layer is plain data: it is `Copy`, and the host
//! never releases it. Each store keeps every object a handle names for the
//! life of the store, in a table that the handle indexes. Wasmi has no
//! collector: a function, an `externref`, and every other object of a store
//! lives as long as the store, so a handle stays good as long as its store.
//!
//! # Host functions
//!
//! A host function is a Wasmi host function whose body runs the host's
//! closure with the store the guest runs in. The closure can call back into
//! the guest through that store, and the guest can call the same host
//! function again. Each call has its own arguments and its own results.
//!
//! Wasmi bounds a guest's own calls, but not a descent through host
//! functions: each host function that calls back into the guest runs Wasmi
//! again on the native stack. The backend bounds that descent itself. At
//! most 64 calls of host functions run in a store, each inside the last,
//! and a guest's call of a 65th traps with
//! [`TrapKind::StackOverflow`](wcmp_wasm_core::TrapKind::StackOverflow),
//! before the host function runs. The host function that made the call
//! back into the guest sees that trap as the error of its call. Each round
//! trip takes about 3 KiB of the native stack in an optimized build, and
//! up to 16 KiB where nothing is optimized, so the deepest descent fits in
//! the 2 MiB stack that Rust gives a new thread by default, with room for
//! the host functions' own frames.
//!
//! An error from the closure traps the guest with
//! [`TrapKind::Host`](wcmp_wasm_core::TrapKind::Host), carrying the error
//! unchanged. Wasmi has no exception handling, so no guest catches the
//! trap.
//!
//! # Memory
//!
//! Every memory method checks its range against the current size of the
//! memory before it touches a byte, and refuses a range outside it with
//! [`Error::MemoryOutOfBounds`](wcmp_wasm_core::Error::MemoryOutOfBounds).
//! `with_bytes` lends the bytes of the memory themselves, and copies
//! nothing. `Memory::copy` is one `memmove` from one memory to the other,
//! with no buffer on the host. Wasmi has no shared memory.
//!
//! Wasmi panics when it grows a 64-bit memory whose maximum is 2^48 pages,
//! the largest the specification allows. It counts that maximum in bytes in
//! a `u64` each time the memory grows, and 2^48 pages of 64 KiB overflow it
//! (`wasmi_core` 2.0.0, `crates/core/src/memory/mod.rs:165`). The backend
//! refuses to make such a memory, with
//! [`Error::TypeMismatch`](wcmp_wasm_core::Error::TypeMismatch), and
//! refuses the host's growth of one a guest exports, with
//! [`Error::Grow`](wcmp_wasm_core::Error::Grow). It cannot stop a guest
//! that declares such a memory from running `memory.grow` on it
//! (`crates/wasmi/src/engine/executor/handler/exec.rs:407` reaches the same
//! panic). On x86-64 that panic aborts the process, because Wasmi runs
//! the guest in `extern "sysv64"` functions, which do not unwind
//! (`crates/wasmi/src/engine/executor/handler/exec/macros.rs:32`).
//! The module is valid, so the backend compiles it: a host that runs
//! untrusted memory64 modules over this backend checks their memories'
//! maxima itself.
//!
//! # Traps
//!
//! Wasmi's `TrapCode` uses Wasmtime's names for the core traps. Each is the
//! [`TrapKind`](wcmp_wasm_core::TrapKind) of the same name, whose message
//! is Wasmtime's, and never Wasmi's own words. That includes `OutOfFuel`,
//! although the backend does not turn fuel on, so Wasmi never raises it. A
//! trap of Wasmi's that has no kind of the runtime layer, such as a failed
//! allocation of the host, is `Other`, with Wasmi's message.
//!
//! # Capabilities
//!
//! The backend declares what Wasmi implements, as its README states:
//! multi-memory, memory64, tail calls, and relaxed SIMD. It does not
//! declare garbage collection, exception handling, typed function
//! references, threads, or stack switching, which Wasmi does not implement.
//!
//! It declares host suspension, which Wasmi's resumable calls fill.
//!
//! # Host suspension
//!
//! A resumable call is Wasmi's `call_resumable`. A suspending host function
//! that answers "not yet" returns a marker error to Wasmi, which sets the
//! call aside and hands back a `ResumableCallHostTrap`. That handle owns
//! the stack of the call, and not the store, so any number of calls wait
//! at once in one store, and the host resumes them in any order, with the
//! results of the host function. A resumption runs the call to its next
//! suspension or its end. Both finish synchronously: each future is ready
//! on its first poll.
//!
//! Wasmi sets a resumable call aside at any error of a host function that a
//! WebAssembly frame called, and the backend suspends the call only at the
//! marker. Any other error is the trap it stands for, as in a call that is
//! not resumable, and the call is gone.
//!
//! A suspension succeeds only where WebAssembly frames alone lie between
//! the start of the resumable call and the suspending host function. A host
//! function between the two calls back into the guest with a call that is
//! not resumable, and Wasmi does not set such a call aside: the marker
//! leaves it as its error, and the call traps with
//! [`TrapKind::Host`](wcmp_wasm_core::TrapKind::Host), with a message that
//! says the call cannot suspend, as on every backend. So does a call
//! that is not resumable at all, and a suspending host function that the
//! root frame of a resumable call tail-calls, whose call Wasmi does not set
//! aside either.
//!
//! When a store drops, its waiting calls drop without a resumption. A
//! waiting call does not reach its store when it drops: it gives its stack
//! back to the engine. So it can drop before or after its store.

mod backend;
mod context;
mod convert;
mod errors;
mod host_error;
mod module;
mod resumption;
mod state;
mod store;
mod suspended_call;
mod suspension;
mod values;

pub use crate::backend::Wasmi;

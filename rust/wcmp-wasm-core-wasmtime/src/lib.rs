// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![cfg(not(target_arch = "wasm32"))]
#![warn(missing_docs)]

//! The Wasmtime backend of the runtime layer of the Wasm Component Model
//! Polyfill.
//!
//! This backend is the control. It runs beside the other backends so that
//! a person can compare their results with Wasmtime's. A host makes an
//! engine over it with [`Wasmtime::new`] and
//! [`Engine::with_backend`](wcmp_wasm_core::Engine::with_backend), and from
//! then on sees only the types of `wcmp_wasm_core`.
//!
//! Wasmtime does not run in the browser, so on `wasm32` this crate is
//! empty.
//!
//! # Compilation and instantiation
//!
//! Wasmtime compiles and instantiates synchronously. The asynchronous
//! compile and instantiation of the runtime layer finish at once: each
//! future is ready on its first poll. Each compile makes a module of its
//! own. The backend keeps no cache of modules by their bytes, and it does
//! not turn on Wasmtime's compilation cache.
//!
//! A module Wasmtime refuses because it needs a capability the backend
//! does not declare, such as stack switching where Wasmtime's compiler does
//! not serve it, is
//! [`Error::Unsupported`](wcmp_wasm_core::Error::Unsupported), with the
//! capability that
//! [`missing_capability`](wcmp_wasm_core::backend::missing_capability)
//! names. Any other refusal is
//! [`Error::Compile`](wcmp_wasm_core::Error::Compile), with Wasmtime's
//! message.
//!
//! # The boundary
//!
//! The backend describes the imports and exports of a module and nothing
//! else. It never refuses a module for an item that does not cross the
//! boundary: whether a module compiles is Wasmtime's decision alone. A
//! concrete heap type at the boundary is a
//! [`TypeHandle`](wcmp_wasm_core::TypeHandle). The backend numbers each
//! concrete type the first time it describes it, and keeps the type for the
//! life of the backend, so two handles are equal exactly when Wasmtime takes
//! them for the same type.
//!
//! # References and their roots
//!
//! A handle of the runtime layer is plain data: it is `Copy`, and the host
//! never releases it. So each store keeps every object a handle names for
//! the life of the store, in a table that the handle indexes:
//!
//! - A function, a memory, a global, a table, a tag, and an instance are
//!   Wasmtime handles, which the Wasmtime store owns anyway.
//! - An `externref`, an internal reference (`anyref` and everything below
//!   it, including an `i31ref`), and an `exnref` are GC references. The
//!   store roots each one with an `OwnedRooted` the moment it crosses to the
//!   host: a result of a call, an argument of a host function, the value of
//!   a global or of a table element, a reference the host makes, and the
//!   exception of an uncaught throw. The root holds until the store drops,
//!   and the collector frees the object after that. A reference that
//!   crosses twice takes two slots.
//! - A reference that crosses from the host to a guest is rooted only for
//!   that crossing, in a scope that ends when the operation returns.
//!
//! Wasmtime's embedding API does not yet carry a continuation reference
//! (bytecodealliance/wasmtime#10248). The backend lets continuation types
//! through the boundary of a module, and refuses with
//! [`Error::Backend`](wcmp_wasm_core::Error::Backend) any operation that
//! would move a continuation reference between the host and a guest,
//! where Wasmtime itself would panic.
//!
//! # Host functions
//!
//! A host function is a Wasmtime host function whose body runs the host's
//! closure with the store the guest runs in. The closure can call back into
//! the guest through that store, and the guest can call the same host
//! function again, at any depth. Each call has its own arguments and its
//! own results. An error from the closure traps the guest with
//! [`TrapKind::Host`](wcmp_wasm_core::TrapKind::Host), carrying the error
//! unchanged. Wasmtime lets a guest catch only an exception, and never a
//! host error, so no guest catches the trap.
//!
//! # Memory
//!
//! Every memory method checks its range against the current size of the
//! memory before it touches a byte, and refuses a range outside it with
//! [`Error::MemoryOutOfBounds`](wcmp_wasm_core::Error::MemoryOutOfBounds).
//! `with_bytes` lends the bytes of an unshared memory themselves, and copies
//! nothing. `Memory::copy` between two unshared memories is one `memmove`
//! from one memory to the other, with no buffer on the host.
//!
//! Wasmtime lends a shared memory only as cells that must be reached with
//! atomic operations, because another agent can write it at any time. The
//! backend follows the same rule: it never lends a shared memory as a slice.
//! `with_bytes` copies the range with atomic reads and lends the copy, and
//! every read and write of a shared memory is atomic.
//!
//! # Traps
//!
//! A core trap of Wasmtime's is the [`TrapKind`](wcmp_wasm_core::TrapKind)
//! of the same name, with Wasmtime's message. That includes `OutOfFuel` and
//! `Interrupt`, although the backend turns on neither fuel nor epoch
//! interruption, so Wasmtime raises neither. An exception that no guest
//! catches, Wasmtime's `ThrownException`, is `UncaughtException`, with the
//! exception rooted in the store as an `exnref`. A trap that is not a core
//! trap is `Other`, with Wasmtime's message.
//!
//! # Capabilities
//!
//! The backend declares every Wasm feature of the lexicon that Wasmtime
//! implements: multi-memory, memory64, tail calls, exception handling,
//! typed function references, garbage collection, relaxed SIMD, and threads.
//! It turns on stack switching where Wasmtime supports it, on x86-64 Linux
//! and macOS, and declares it there.
//!
//! It does not declare host suspension. Wasmtime's `Func::call_async`
//! borrows the store until the call finishes, so one store has at most one
//! suspended call. Every method of host suspension returns
//! [`Error::Unsupported`](wcmp_wasm_core::Error::Unsupported) with
//! `host_suspension`.

mod backend;
mod context;
mod convert;
mod errors;
mod host_error;
mod memory_object;
mod module;
mod state;
mod store;
mod type_registry;
mod values;

pub use crate::backend::Wasmtime;

#![cfg(target_arch = "wasm32")]
#![warn(missing_docs)]

//! The browser backend of the runtime layer of the Wasm Component Model
//! Polyfill, over the WebAssembly JavaScript API.
//!
//! A host makes an engine over it with [`Web::new`] and
//! [`Engine::with_backend`](wcmp_wasm_core::Engine::with_backend), and from
//! then on sees only the types of `wcmp_wasm_core`. The JavaScript API
//! exists only in the browser, so on every other target this crate is
//! empty.
//!
//! The backend never makes a function from a string of source. It reaches
//! the JavaScript API through `js-sys` and `wasm-bindgen` imports alone, and
//! JavaScript carries only what that API alone can do: a compile, an
//! instantiation, and the reads and writes of its objects.
//!
//! # Compilation and instantiation
//!
//! The asynchronous compile is `WebAssembly.compile`, and every
//! instantiation is `WebAssembly.instantiate`. The browser refuses a
//! synchronous compile or instantiation of a module above its limit, 8 MB in
//! Chromium, on the main thread, and these two load such a module. The
//! synchronous compile, `new WebAssembly.Module`, is for small modules that a
//! backend or a host generates. Above the limit it fails with
//! [`Error::Compile`](wcmp_wasm_core::Error::Compile) and the browser's
//! message. Each compile makes a module of its own. The backend keeps no
//! cache of modules by their bytes.
//!
//! # The boundary
//!
//! The JavaScript API names the imports and exports of a module, but not
//! their types. So the backend reads the types of the boundary from the
//! bytes of the module, after the browser accepted it. It reads the type,
//! import, function, table, memory, tag, global, and export sections, and
//! nothing after them. It never refuses a module for an item that does not
//! cross the boundary: whether a module compiles is the browser's decision
//! alone.
//!
//! A concrete heap type at the boundary is a
//! [`TypeHandle`](wcmp_wasm_core::TypeHandle). The backend numbers each
//! distinct recursion group the first time a module defines it, and keeps
//! the number for the life of the backend. So two handles are equal exactly
//! when the browser takes them for one type.
//!
//! # Externs and linking
//!
//! A function, a memory, a global, a table, and a tag are each the
//! JavaScript object the browser made for it. A tag is an extern kind like
//! the others: the host imports, exports, and links one, and reads its
//! parameter types from the boundary of the module that exports it. An
//! instantiation passes each extern's own object in the imports object. So
//! an exported function reaches the import of another instance as the
//! function object of the export itself, and a call between two instances
//! is a call from WebAssembly to WebAssembly.
//!
//! # References and their roots
//!
//! The host reads three kinds of reference. A `funcref` is the function
//! itself, which the host can call. An `externref` that the host made is a
//! fresh, empty JavaScript object that stands for the host's value, so the
//! backend knows it again when a guest hands it back. An `i31ref` is a
//! `Number`. A GC object is an opaque JavaScript object: the host holds it,
//! tests it for null, and gives it back to a guest of the same store.
//!
//! A handle of the runtime layer is plain data: it is `Copy`, and the host
//! never releases it. So each store keeps every object a handle names in a
//! list that the handle indexes, for the life of the store. The entry holds
//! the object's JavaScript value, which roots it: the browser's collector
//! does not free a value that `wasm-bindgen` holds for Rust. Each
//! `externref`, `anyref`, and function reference is rooted the moment it
//! crosses to the host: a result of a call, the value of a global or of a
//! table element, and a reference the host makes. A function that crosses
//! again keeps its handle, and so does an `externref` the host made. Any
//! other reference that crosses twice takes two entries. When the store
//! drops, every entry drops, and the collector frees each value that
//! nothing else holds. A reference that crosses from the host to a guest
//! is not rooted for the crossing: the entry of its handle already roots
//! it.
//!
//! The JavaScript API carries no `v128`, no `exnref`, and no continuation
//! reference between JavaScript and a guest. So the host calls a function
//! whose type holds a `v128` or an `exnref` through a carrier: a small
//! generated module that imports the function, and takes and gives a
//! `v128` as two `i64` halves and an `exnref` as its index in a table of
//! exceptions. The store owns that table, and it roots each `exnref` that
//! crosses to the host for the life of the store, as the lists root every
//! other reference. The backend knows the type of an export, and not the
//! type of a function reference a guest handed out, so only an export gets
//! a carrier. A call that would carry one of these values any other way,
//! and the value of a global or a table element of such a type, is
//! [`Error::TypeMismatch`](wcmp_wasm_core::Error::TypeMismatch). A guest
//! passes each of them to another guest directly, and a module with each
//! of them at its boundary loads.
//!
//! # Capabilities
//!
//! The backend runs one small probe for each Wasm feature of the lexicon
//! when it is made: a module that uses the feature, which
//! `WebAssembly.validate` accepts or refuses. It declares each capability
//! whose probe the browser accepts. A browser without a feature loads the
//! backend and declares less. The backend also reads the two functions of
//! JavaScript Promise Integration, `WebAssembly.Suspending` and
//! `WebAssembly.promising`, and keeps them.
//!
//! The backend does not yet make host functions, and it does not declare
//! [`host_suspension`](wcmp_wasm_core::Capability::HostSuspension). A host
//! function needs a generated wrapper module, so that a host error traps the
//! guest instead of throwing an exception that a guest can catch.
//! [`Func::new`](wcmp_wasm_core::Func::new) is
//! [`Error::Backend`](wcmp_wasm_core::Error::Backend) until then. A trap is
//! [`TrapKind::Other`](wcmp_wasm_core::TrapKind::Other), with the browser's
//! message.

mod backend;
mod boundary;
mod carrier;
mod errors;
mod js;
mod jspi;
mod module;
mod objects;
mod probes;
mod store;
mod type_registry;
mod values;

pub use crate::backend::Web;

// The crate's own unit tests run in a browser, as every other test binary
// of the workspace does.
#[cfg(test)]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

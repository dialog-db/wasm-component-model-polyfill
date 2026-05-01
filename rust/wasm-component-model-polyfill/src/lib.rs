#![warn(missing_docs)]

//! A polyfill that brings the WebAssembly Component Model (wasip3) to
//! web browsers where only Wasm Core is presently supported.
//!
//! The crate exposes:
//!
//! - [`Engine`] and [`Store`] — the compilation context and the
//!   owner of guest state, the foundations every higher-level type
//!   is built against.
//! - [`Component`] — a parsed component value with accessors over
//!   its declared imports and exports.
//! - [`PackageName`] and [`InterfaceIdentifier`] — the addressing
//!   surface for component imports and exports.
//! - [`ValueType`] — the structural identity of every value type a
//!   parsed component can declare.
//! - [`Linker`], [`LinkerInstance`], [`Instance`], and [`Func`] —
//!   the build-up and runtime surface for linking a component
//!   against a host environment, instantiating it into a [`Store`],
//!   and calling its exports.
//! - [`Val`], [`ValField`], and [`ResourceHandle`] — the polyfill's
//!   component-level value enum and its compound-variant payloads.
//! - [`Error`] and [`Result`] — the polyfill's single error enum and
//!   the `Result` alias every public function returns.

mod abi;
mod backend;
mod component;
mod engine;
mod error;
mod executor;
mod identifier;
mod instance;
mod linker;
mod resource;
mod store;
mod types;
mod value;

pub use crate::component::{
    Component, ComponentExport, ComponentImport, ExternType, ExternalName, FunctionParameter,
    FunctionType, InstanceItem, InstanceType,
};
pub use crate::engine::Engine;
pub use crate::error::{
    AbiCause, AbiError, AbiPosition, Error, InstantiationError, LinkError, Result, TypeMismatch,
    TypeMismatchPosition, TypeRendering,
};
pub use crate::identifier::{IdentifierParseError, InterfaceIdentifier, PackageName};
pub use crate::instance::{Func, Instance};
pub use crate::linker::{
    ComponentParameters, ComponentResult, ComponentValue, Linker, LinkerInstance,
};
pub use crate::store::Store;
pub use crate::types::{
    EnumType, FlagsType, ListType, OptionType, PrimitiveType, RecordField, RecordType,
    ResourceType, ResultType, TupleType, ValueType, VariantCase, VariantType,
};
pub use crate::resource::{ResourceHandle, ResourceTypeId};
pub use crate::value::{Val, ValField};

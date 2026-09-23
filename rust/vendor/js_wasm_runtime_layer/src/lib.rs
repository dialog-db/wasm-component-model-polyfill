#![warn(missing_docs)]
#![warn(clippy::missing_docs_in_private_items)]
#![cfg_attr(not(feature = "std"), no_std)]

//! `js_wasm_runtime_layer` implements the `wasm_runtime_layer` abstraction interface over WebAssembly runtimes for your web browser's WebAssembly runtime.

extern crate alloc;

use alloc::{boxed::Box, format, rc::Rc, string::String, sync::Arc};
use core::{
    cell::{RefCell, RefMut},
    error::Error,
    fmt,
};
use smallvec::SmallVec;

use anyhow::{bail, Result};
use fxhash::FxHashMap;
use js_sys::{JsString, Object, Reflect, Uint8Array, WebAssembly};
use wasm_bindgen_futures::JsFuture;
use slab::Slab;
use wasm_bindgen::{JsCast, JsValue};
use wasm_runtime_layer::{
    backend::{AsContext, AsContextMut, Extern, Ref, Val, WasmEngine, WasmExternRef, WasmGlobal},
    GlobalType, RefType, ValType,
};

/// The default amount of arguments and return values for which to allocate
/// stack space.
const DEFAULT_ARGUMENT_SIZE: usize = 4;

/// A vector which allocates up to the default number of arguments on the stack.
type ArgumentVec<T> = SmallVec<[T; DEFAULT_ARGUMENT_SIZE]>;

/// Conversion to and from JavaScript
mod conversion;
/// Functions
mod func;
/// Instances
mod instance;
/// Memories
mod memory;
/// WebAssembly modules
mod module;
/// Stores all the WebAssembly state for a given collection of modules with a similar lifetime
mod store;
/// WebAssembly tables
mod table;

pub use func::Func;
// PATCH (wcmp): the failure a re-entrant host call answers with.
pub use func::ReentrantHostCall;
pub use instance::Instance;
pub use memory::Memory;
pub use module::Module;
pub use store::{Store, StoreContext, StoreContextMut, StoreInner};
pub use table::Table;

use self::{
    conversion::{FromJs, ToJs, ToStoredJs},
    module::{ModuleInner, ParsedModule},
};

/// Helper to convert a `JsValue` into a proper error, as well as making it `Send` + `Sync`
#[derive(Debug, Clone)]
pub(crate) struct JsErrorMsg {
    /// A string representation of the error message
    message: String,
}

impl fmt::Display for JsErrorMsg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl Error for JsErrorMsg {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

impl From<&JsValue> for JsErrorMsg {
    fn from(value: &JsValue) -> Self {
        // PATCH (wcmp): a value whose `message` is not a string — a
        // `WebAssembly.Exception` a guest threw, for one — reaches the
        // debug rendering rather than a panic. `Reflect::get` answers
        // `Ok(undefined)` for a property the object does not have, so
        // upstream's `expect` fires on every such value.
        if let Some(v) = value.dyn_ref::<JsString>() {
            Self { message: v.into() }
        } else if let Some(message) = Reflect::get(value, &"message".into())
            .ok()
            .and_then(|v| v.as_string())
        {
            Self { message }
        } else if value.is_instance_of::<WebAssembly::Exception>() {
            // PATCH (wcmp): an exception a guest threw and did not
            // catch is handed to the host as the exception object
            // itself, which carries no message. The native backend
            // reports the same failure as `thrown Wasm exception`, so
            // this one says it too and a caller reads one wording on
            // either backend.
            Self {
                message: String::from("thrown Wasm exception"),
            }
        } else {
            Self {
                message: format!("{value:?}"),
            }
        }
    }
}

impl From<JsValue> for JsErrorMsg {
    fn from(value: JsValue) -> Self {
        Self::from(&value)
    }
}

impl WasmEngine for Engine {
    type ExternRef = ExternRef;
    type Func = Func;
    type Global = Global;
    type Instance = Instance;
    type Memory = Memory;
    type Module = Module;
    type Store<T: 'static> = Store<T>;
    type StoreContext<'a, T: 'static> = StoreContext<'a, T>;
    type StoreContextMut<'a, T: 'static> = StoreContextMut<'a, T>;
    type Table = Table;
}

/// Handle used to retain the lifetime of Js passed objects and drop them at an appropriate time.
///
/// Most commonly this is to ensure a closure with captures does not get dropped by Rust while a
/// reference to it exists in the world of Js.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct DropResource(Box<dyn fmt::Debug>);

impl DropResource {
    /// Creates a new drop resource from anything that implements `std::fmt::Debug`
    ///
    /// In general, any trait can be used here, but `std::fmt::Debug` is the most common and allows
    /// easy introspection of the values being held on to.
    pub fn new(value: impl 'static + fmt::Debug) -> Self {
        Self(Box::new(value))
    }
}

#[derive(Default, Debug, Clone)]
/// Runtime for WebAssembly
pub struct Engine {
    /// Inner state of the engine
    ///
    /// May be accessed at any time, but not recursively
    inner: Rc<RefCell<EngineInner>>,
}

impl Engine {
    // /// Borrow the engine
    // pub(crate) fn borrow(&self) -> Ref<EngineInner> {
    //     self.inner.borrow()
    // }

    /// Mutably borrow the engine
    pub(crate) fn borrow_mut(&self) -> RefMut<'_, EngineInner> {
        self.inner.borrow_mut()
    }

    /// PATCH (wcmp): compile `bytes` with `WebAssembly.compile`, the
    /// browser's asynchronous path, and keep the result so that the next
    /// `Module::new` on this engine with the same bytes uses it instead of
    /// the synchronous `WebAssembly.Module` constructor. The synchronous
    /// constructor is refused on the main thread above a size limit in
    /// some browsers; the asynchronous path has no such limit.
    pub async fn precompile(&self, bytes: &[u8]) -> Result<()> {
        let promise = WebAssembly::compile(&Uint8Array::from(bytes).into());
        let module = JsFuture::from(promise)
            .await
            .map_err(JsErrorMsg::from)?
            .dyn_into::<WebAssembly::Module>()
            .map_err(JsErrorMsg::from)?;
        self.borrow_mut()
            .precompiled
            .insert(bytes.into(), module);
        Ok(())
    }

    /// PATCH (wcmp): drop the module [`Engine::precompile`] kept for
    /// `bytes`, if no `Module::new` has taken it yet. Only `Module::new`
    /// otherwise removes an entry, and it adds a module to the engine
    /// that is never removed either, so a caller that compiled a batch
    /// of modules and gives up before building them all discards the
    /// rest here.
    pub fn discard_precompiled(&self, bytes: &[u8]) {
        self.borrow_mut().precompiled.remove(bytes);
    }

    /// PATCH (wcmp): how many modules [`Engine::precompile`] kept that
    /// no `Module::new` has taken yet. The count is what says whether
    /// a caller left an entry behind, which the test of a failed batch
    /// reads.
    pub fn precompiled_count(&self) -> usize {
        self.inner.borrow().precompiled.len()
    }
}

/// Holds the inner mutable state of the engine
#[derive(Default, Debug)]
pub(crate) struct EngineInner {
    /// Modules loaded into the engine
    ///
    /// This is a slab since the WasmModule needs to be `Send`, but the WebAssembly::Module is not.
    /// The engine is not `Send` or `Sync` so they are stored here instead.
    pub(crate) modules: Slab<ModuleInner>,
    /// PATCH (wcmp): modules compiled ahead of time through
    /// [`Engine::precompile`], keyed by their bytes. `Module::new` takes
    /// the entry for the same bytes instead of compiling synchronously.
    pub(crate) precompiled: FxHashMap<Box<[u8]>, WebAssembly::Module>,
}

impl EngineInner {
    /// Inserts a new module into the engine
    pub fn insert_module(&mut self, module: ModuleInner, parsed: Arc<ParsedModule>) -> Module {
        Module {
            id: self.modules.insert(module),
            parsed,
        }
    }
}

/// A global variable accessible as an import or export in a module
///
/// Stored within the store
#[derive(Debug, Clone)]
pub struct Global {
    /// The id of the global in the store
    pub(crate) id: usize,
}

/// Holds the inner state of the global
#[derive(Debug)]
pub(crate) struct GlobalInner {
    /// The global value
    value: WebAssembly::Global,
    /// The global type
    ty: GlobalType,
}

impl ToStoredJs for Global {
    type Repr = WebAssembly::Global;

    fn to_stored_js<T>(&self, store: &StoreInner<T>) -> Result<WebAssembly::Global> {
        let global = &store.globals[self.id];
        Ok(global.value.clone())
    }
}

impl Global {
    /// Creates a new global from a JS value
    pub(crate) fn from_exported_global<T>(
        store: &mut StoreInner<T>,
        value: JsValue,
        signature: GlobalType,
    ) -> Option<Self> {
        let global: &WebAssembly::Global = value.dyn_ref()?;

        Some(store.insert_global(GlobalInner {
            value: global.clone(),
            ty: signature,
        }))
    }
}

impl WasmGlobal<Engine> for Global {
    fn new(mut ctx: impl AsContextMut<Engine>, value: Val<Engine>, mutable: bool) -> Self {
        let mut ctx = ctx.as_context_mut();

        let ty = GlobalType::new(value.ty(), mutable);

        let desc = Object::new();

        Reflect::set(&desc, &"value".into(), &value.ty().to_js()).unwrap();
        Reflect::set(&desc, &"mutable".into(), &mutable.into()).unwrap();

        let value = value.to_stored_js(&ctx).unwrap();

        let global = GlobalInner {
            value: WebAssembly::Global::new(&desc, &value).unwrap(),
            ty,
        };

        ctx.insert_global(global)
    }

    fn ty(&self, ctx: impl AsContext<Engine>) -> GlobalType {
        ctx.as_context().globals[self.id].ty
    }

    fn set(&self, mut ctx: impl AsContextMut<Engine>, new_value: Val<Engine>) -> Result<()> {
        let store: &mut StoreInner<_> = &mut ctx.as_context_mut();

        let value = &new_value.to_stored_js(store)?;

        let inner = &mut store.globals[self.id];

        if !inner.ty.mutable() {
            bail!("Global is not mutable");
        }

        inner.value.set_value(value);

        Ok(())
    }

    fn get(&self, mut ctx: impl AsContextMut<Engine>) -> Val<Engine> {
        let store: &mut StoreInner<_> = &mut ctx.as_context_mut();
        let inner = &mut store.globals[self.id];

        let ty = inner.ty;
        let value = inner.value.value();

        value_from_js_typed(store, &ty.content(), value).unwrap()
    }
}

impl ToStoredJs for Val<Engine> {
    type Repr = JsValue;

    /// Convert the value enum to a JavaScript value
    fn to_stored_js<T>(&self, store: &StoreInner<T>) -> Result<JsValue> {
        match self {
            Val::I32(v) => Ok((*v).into()),
            Val::I64(v) => Ok((*v).into()),
            Val::F32(v) => Ok((*v).into()),
            Val::F64(v) => Ok((*v).into()),
            Val::V128(_) => {
                bail!("v128 values are not supported in the js_wasm_runtime_layer backend")
            }
            Val::FuncRef(None) => Ok(JsValue::NULL),
            Val::FuncRef(Some(func)) => {
                let v: &JsValue = store.funcs[func.id].func.as_ref();
                Ok(v.clone())
            }
            Val::ExternRef(_) => bail!(
                "extern references are not yet supported in the js_wasm_runtime_layer backend"
            ),
        }
    }
}

impl ToStoredJs for Ref<Engine> {
    type Repr = JsValue;

    /// Convert the reference enum to a JavaScript value
    fn to_stored_js<T>(&self, store: &StoreInner<T>) -> Result<JsValue> {
        match self {
            Ref::FuncRef(None) => Ok(JsValue::NULL),
            Ref::FuncRef(Some(func)) => {
                let v: &JsValue = store.funcs[func.id].func.as_ref();
                Ok(v.clone())
            }
            Ref::ExternRef(_) => bail!(
                "extern references are not yet supported in the js_wasm_runtime_layer backend"
            ),
        }
    }
}

#[derive(Debug, Clone)]
/// Extern host reference type
pub struct ExternRef {}

impl WasmExternRef<Engine> for ExternRef {
    fn new<T: 'static + Send + Sync>(_: impl AsContextMut<Engine>, _: T) -> Self {
        unimplemented!(
            "extern references are not yet supported in the js_wasm_runtime_layer backend"
        )
    }

    fn downcast<'a, 's: 'a, T: 'static, S: 'static>(
        &self,
        _: <Engine as WasmEngine>::StoreContext<'s, S>,
    ) -> Result<&'a T> {
        bail!("extern references are not yet supported in the js_wasm_runtime_layer backend")
    }
}

impl ToStoredJs for Extern<Engine> {
    type Repr = JsValue;
    fn to_stored_js<T>(&self, store: &StoreInner<T>) -> Result<JsValue> {
        match self {
            Extern::Global(v) => Ok(v.to_stored_js(store)?.into()),
            Extern::Table(v) => Ok(v.to_stored_js(store)?.into()),
            Extern::Memory(v) => Ok(v.to_stored_js(store)?.into()),
            Extern::Func(v) => Ok(v.to_stored_js(store)?.into()),
        }
    }
}

impl ToJs for ValType {
    type Repr = JsString;
    /// Convert the value type enum to a JavaScript descriptor
    ///
    /// See: <https://developer.mozilla.org/en-US/docs/WebAssembly/JavaScript_interface/Global/Global>
    fn to_js(&self) -> JsString {
        match self {
            ValType::I32 => "i32",
            ValType::I64 => "i64",
            ValType::F32 => "f32",
            ValType::F64 => "f64",
            ValType::V128 => "v128",
            ValType::FuncRef => "anyfunc",
            ValType::ExternRef => "externref",
        }
        .into()
    }
}

impl ToJs for RefType {
    type Repr = JsString;
    /// Convert the reference type enum to a JavaScript descriptor
    ///
    /// See: <https://developer.mozilla.org/en-US/docs/WebAssembly/JavaScript_interface/Global/Global>
    fn to_js(&self) -> JsString {
        match self {
            RefType::FuncRef => "anyfunc",
            RefType::ExternRef => "externref",
        }
        .into()
    }
}

impl FromJs for ValType {
    fn from_js(value: JsValue) -> Option<Self>
    where
        Self: Sized,
    {
        let s = value.as_string()?;

        let res = match &s[..] {
            "i32" => Self::I32,
            "i64" => Self::I64,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "v128" => Self::V128,
            "anyfunc" => Self::FuncRef,
            "externref" => Self::ExternRef,
            _ => {
                #[cfg(feature = "tracing")]
                tracing::error!("Invalid value type {s:?}");
                return None;
            }
        };

        Some(res)
    }
}

/// PATCH (wcmp): convert a JsValue for a call whose signature the
/// backend does not know.
///
/// A function reference that reached the host as an argument carries
/// no signature, so a result of a call to it has no declared type of
/// its own. The slice the caller supplied says what the caller
/// expects, and that is what the conversion follows, with one rule
/// on top: a `BigInt` is always an `i64`, because the JS API
/// represents that core type and no other as one. A caller that
/// cannot name a result's type asks for an `f64` and reads back
/// every number faithfully, since a number passed on to another wasm
/// call reaches an `i32`, an `f32`, or an `f64` through the JS API's
/// own coercion.
pub(crate) fn value_from_js_untyped<T>(
    store: &mut StoreInner<T>,
    ty: &ValType,
    value: JsValue,
) -> Option<Val<Engine>> {
    if value.is_bigint() {
        return Some(Val::I64(i64::from_js(value)?));
    }
    value_from_js_typed(store, ty, value)
}

/// Convert the JsValue into a Value of the supplied type
pub(crate) fn value_from_js_typed<T>(
    store: &mut StoreInner<T>,
    ty: &ValType,
    value: JsValue,
) -> Option<Val<Engine>> {
    match ty {
        ValType::I32 => Some(Val::I32(i32::from_js(value)?)),
        ValType::I64 => Some(Val::I64(i64::from_js(value)?)),
        ValType::F32 => Some(Val::F32(f32::from_js(value)?)),
        ValType::F64 => Some(Val::F64(f64::from_js(value)?)),
        ValType::V128 => {
            #[cfg(feature = "tracing")]
            tracing::error!("v128 values are not supported in the js_wasm_runtime_layer backend");
            None
        }
        // PATCH (wcmp): a function reference passed to a host
        // function arrives as the JS object of an exported wasm
        // function. The store records it so the host can call it
        // back; the JS API carries no signature with it, so the
        // record says so and the call takes its result types from
        // the caller. A null reference stays null.
        //
        // The store keeps one record per function object, and a
        // conversion of a function it already saw reads that one
        // back, so the calls of one store share the records their
        // first call made; see `StoreInner::func_of_reference`.
        ValType::FuncRef => {
            if value.is_null() || value.is_undefined() {
                return Some(Val::FuncRef(None));
            }
            let function: js_sys::Function = value.dyn_into().ok()?;
            Some(Val::FuncRef(Some(store.func_of_reference(function))))
        }
        ValType::ExternRef => {
            #[cfg(feature = "tracing")]
            tracing::error!(
                "conversion to a function or extern outside of a module is not permitted"
            );
            None
        }
    }
}

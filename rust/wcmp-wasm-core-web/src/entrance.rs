//! The function through which the store starts a resumable call of one
//! guest function.

use std::collections::HashMap;

use js_sys::{Array, Function, Object, Uint8Array, WebAssembly};
use wasm_bindgen::{JsCast, JsValue};
use wasm_encoder::{
    CodeSection, ConstExpr, EntityType, ExportKind, ExportSection, FunctionSection, GlobalSection,
    GlobalType, ImportSection, TypeSection, ValType,
};
use wcmp_wasm_core::Result;

use crate::errors;
use crate::js;
use crate::jspi::Jspi;

/// The function through which the store starts a resumable call of one
/// guest function: `WebAssembly.promising` over a generated module that
/// records the call's return, where a module can name the function's type.
///
/// A promising call answers a promise, and never its results. Where the
/// call returns in its first stretch, the browser has fulfilled that
/// promise by the time the call returns, but only a handler that the
/// browser runs on a microtask can read it. So the store calls the guest
/// function through an entrance module, which records the return where
/// the host reads it at once:
///
/// ```text
/// (module
///   (import "" "callee" (func $callee (param ...) (result ...)))
///   (global $done (export "done") (mut i32) (i32.const 0))
///   (global $r0 (export "r0") (mut ...)) ...   ;; one for each result
///   (func (export "enter") (param ... $token i32) (result ...)
///     ;; each parameter
///     call $callee
///     ;; each result into $r0 ...
///     local.get $token
///     global.set $done
///     ;; each result again))
/// ```
///
/// The host passes each call a token of its own, the number of its
/// flight. Right after the promising call returns, a call that did not
/// suspend returned where `$done` holds its token, and its results are in
/// the result globals. Otherwise it trapped, and only the promise says
/// why. A call nested inside the first stretch of another sets `$done` to
/// its own token, and the outer call sets it again when it returns, after
/// the nested call. So `$done` holds the token of the call that returned
/// last, and a call that trapped after a nested call returned never finds
/// its own token there.
///
/// The entrance returns the results too, so a call that suspended and
/// returns in a later stretch fulfils its promise with them, as the guest
/// function itself would.
///
/// Where the entrance cannot import the function, the store calls
/// `WebAssembly.promising` over the function itself, and a call reads its
/// end from the promise alone: a function whose type holds a reference to
/// a concrete type, which the generated module does not define, a function
/// reference whose type the backend does not know, or a function whose
/// type does not link to the type the module imports it with.
pub struct Entrance {
    /// `WebAssembly.promising` over the entrance's `enter`, or over the
    /// guest function itself where there is no entrance module.
    promising: Function,
    /// Where the entrance module records a return: its `done` global and
    /// its result globals.
    record: Option<(WebAssembly::Global, Vec<WebAssembly::Global>)>,
}

impl Entrance {
    /// The entrance of `function`, which the JavaScript API calls with the
    /// types `seen`, where a module can name them. `modules` keeps each
    /// entrance module the backend compiled, by its bytes.
    pub fn new(
        jspi: &Jspi,
        function: &Function,
        seen: Option<(Vec<ValType>, Vec<ValType>)>,
        modules: &mut HashMap<Vec<u8>, WebAssembly::Module>,
    ) -> Result<Self> {
        let Some((params, results)) = seen else {
            return Self::without_record(jspi, function);
        };
        let bytes = generate(&params, &results);
        let module = match modules.get(&bytes) {
            Some(module) => module.clone(),
            None => {
                let module = WebAssembly::Module::new(&Uint8Array::from(&bytes[..]).into())
                    .map_err(|error| errors::backend(errors::message(&error)))?;
                modules.insert(bytes, module.clone());
                module
            }
        };
        let imports = js::object(&[("callee", function.clone().into())])
            .and_then(|imports| js::object(&[("", imports.into())]))
            .map_err(|error| errors::call(&error))?;
        let exports = match WebAssembly::Instance::new(&module, &imports) {
            Ok(instance) => instance.exports(),
            Err(error) if error.is_instance_of::<WebAssembly::LinkError>() => {
                return Self::without_record(jspi, function);
            }
            Err(error) => return Err(errors::backend(errors::message(&error))),
        };
        let enter = export::<Function>(&exports, "enter")?;
        let done = export::<WebAssembly::Global>(&exports, "done")?;
        let globals = (0..results.len())
            .map(|index| export::<WebAssembly::Global>(&exports, &format!("r{index}")))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            promising: jspi.promising(&enter)?,
            record: Some((done, globals)),
        })
    }

    /// The entrance of `function` with no module: `WebAssembly.promising`
    /// over the function itself.
    fn without_record(jspi: &Jspi, function: &Function) -> Result<Self> {
        Ok(Self {
            promising: jspi.promising(function)?,
            record: None,
        })
    }

    /// The function to call, with the arguments of the guest function and,
    /// where [`Entrance::records`], the call's token last.
    pub fn promising(&self) -> &Function {
        &self.promising
    }

    /// Whether the entrance records a return, and takes a token.
    pub fn records(&self) -> bool {
        self.record.is_some()
    }

    /// The token of the call of the flight numbered `id`.
    pub fn token(id: u64) -> i32 {
        id as u32 as i32
    }

    /// What the call whose token is `token` returned, as the guest
    /// function itself returns it, where it was the last call of the
    /// entrance to return. Read right after a promising call that did not
    /// suspend, `None` means that the call trapped.
    pub fn returned(&self, token: i32) -> Option<JsValue> {
        let (done, globals) = self.record.as_ref()?;
        if done.value().as_f64() != Some(f64::from(token)) {
            return None;
        }
        Some(match globals.as_slice() {
            [] => JsValue::UNDEFINED,
            [global] => global.value(),
            globals => globals
                .iter()
                .map(WebAssembly::Global::value)
                .collect::<Array>()
                .into(),
        })
    }
}

/// The export `name` of `exports`, as a `T`.
fn export<T: JsCast>(exports: &Object, name: &str) -> Result<T> {
    js::get(exports, name)
        .ok()
        .and_then(|value| value.dyn_into::<T>().ok())
        .ok_or_else(|| errors::backend(format!("the entrance module exports no `{name}`")))
}

/// The entrance module of a function that takes `params` and gives
/// `results`. See [`Entrance`].
fn generate(params: &[ValType], results: &[ValType]) -> Vec<u8> {
    let mut module = wasm_encoder::Module::new();
    let mut types = TypeSection::new();
    types.ty().function(params.to_vec(), results.to_vec());
    types.ty().function(
        params
            .iter()
            .copied()
            .chain([ValType::I32])
            .collect::<Vec<_>>(),
        results.to_vec(),
    );
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("", "callee", EntityType::Function(0));
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(1);
    module.section(&functions);
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(0),
    );
    for ty in results {
        let (val_type, init) = holder(*ty);
        globals.global(
            GlobalType {
                val_type,
                mutable: true,
                shared: false,
            },
            &init,
        );
    }
    module.section(&globals);
    let mut exports = ExportSection::new();
    exports.export("enter", ExportKind::Func, 1);
    exports.export("done", ExportKind::Global, 0);
    for index in 0..results.len() as u32 {
        exports.export(&format!("r{index}"), ExportKind::Global, 1 + index);
    }
    module.section(&exports);

    let token = params.len() as u32;
    let first_local = token + 1;
    let mut body = wasm_encoder::Function::new(results.iter().map(|ty| (1, *ty)));
    let mut code = body.instructions();
    for local in 0..token {
        code.local_get(local);
    }
    code.call(0);
    for index in (0..results.len() as u32).rev() {
        code.local_set(first_local + index);
    }
    for index in 0..results.len() as u32 {
        code.local_get(first_local + index).global_set(1 + index);
    }
    code.local_get(token).global_set(0);
    for index in 0..results.len() as u32 {
        code.local_get(first_local + index);
    }
    code.end();
    let mut section = CodeSection::new();
    section.function(&body);
    module.section(&section);
    module.finish()
}

/// The type of the global that holds a result of `ty`, and its first value.
/// A reference global is nullable, since it holds nothing before the first
/// return.
fn holder(ty: ValType) -> (ValType, ConstExpr) {
    match ty {
        ValType::I32 => (ty, ConstExpr::i32_const(0)),
        ValType::I64 => (ty, ConstExpr::i64_const(0)),
        ValType::F32 => (ty, ConstExpr::f32_const(0.0f32.into())),
        ValType::F64 => (ty, ConstExpr::f64_const(0.0f64.into())),
        ValType::V128 => (ty, ConstExpr::v128_const(0)),
        ValType::Ref(reference) => (
            ValType::Ref(wasm_encoder::RefType {
                nullable: true,
                heap_type: reference.heap_type,
            }),
            ConstExpr::ref_null(reference.heap_type),
        ),
    }
}

//! The run of one script through the runtime layer.

use std::collections::HashMap;

use wast::core::WastArgCore;
use wast::parser;
use wast::token::Id;
use wast::{QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat};
use wcmp_wasm_core::{
    Capabilities, Engine, Error, Extern, ExternRef, HeapType, Instance, Module, Store, TrapKind,
    Val,
};

use crate::patterns;
use crate::script_run::ScriptRun;
use crate::spectest;
use crate::suite::Suite;
use crate::text;

/// Runs the script `source` on `engine`, directive by directive, in one
/// store of its own.
pub async fn run(engine: &Engine, source: &str) -> ScriptRun {
    let mut run = ScriptRun::new();
    let buffer = match text::buffer(source) {
        Ok(buffer) => buffer,
        Err(error) => {
            run.fail(1, format!("the script does not parse: {error}"));
            return run;
        }
    };
    let wast = match parser::parse::<Wast<'_>>(&buffer) {
        Ok(wast) => wast,
        Err(error) => {
            run.fail(1, format!("the script does not parse: {error}"));
            return run;
        }
    };
    // A refusal a declared capability lifts is measured against what the
    // script needs; see `Suite::lifting`.
    let needed = match Suite::of(source) {
        Ok(Suite::Capabilities(needed)) => needed,
        _ => Capabilities::empty(),
    };
    let mut runner = match Runner::new(engine, needed) {
        Ok(runner) => runner,
        Err(error) => {
            run.fail(
                1,
                format!("the engine makes no store for the script: {error}"),
            );
            return run;
        }
    };
    for directive in wast.directives {
        let line = text::line_of(source, directive.span());
        match runner.directive(directive).await {
            Ok(()) => run.pass(),
            Err(Miss::Fail(reason)) => run.fail(line, reason),
            Err(Miss::Skip(reason)) => run.skip(line, reason),
        }
    }
    run
}

/// Why a directive did not pass.
enum Miss {
    /// The engine did not do what the directive asserts.
    Fail(String),
    /// The directive does not apply to the backend, or the runtime layer
    /// cannot express it.
    Skip(String),
}

/// What the runner did with one directive.
type Step = Result<(), Miss>;

/// The results of a call, or of an instantiation or a read of a global,
/// or the error the runtime layer returned for it.
type Execution = Result<Vec<Val>, Error>;

/// The state of one script's run: its store, and the modules and instances
/// its directives defined.
struct Runner {
    store: Store<()>,
    /// The capabilities the script needs above the floor.
    needed: Capabilities,
    /// The externs of `spectest`, by name.
    spectest: HashMap<&'static str, Extern>,
    /// The instances `register` made importable, by the name they were
    /// registered under.
    registered: HashMap<String, Instance>,
    /// The instances the script named, by name.
    instances: HashMap<String, Instance>,
    /// The instance the last `module` or `module instance` made.
    current: Option<Instance>,
    /// The modules `module definition` named, by name.
    definitions: HashMap<String, Module>,
    /// The module the last `module definition` defined.
    definition: Option<Module>,
    /// The `externref` of each `ref.extern` number, so that one number is
    /// one reference.
    extern_refs: HashMap<u32, ExternRef>,
}

impl Runner {
    fn new(engine: &Engine, needed: Capabilities) -> wcmp_wasm_core::Result<Self> {
        let mut store = Store::new(engine, ())?;
        let spectest = spectest::externs(&mut store)?;
        Ok(Self {
            store,
            needed,
            spectest,
            registered: HashMap::new(),
            instances: HashMap::new(),
            current: None,
            definitions: HashMap::new(),
            definition: None,
            extern_refs: HashMap::new(),
        })
    }

    async fn directive(&mut self, directive: WastDirective<'_>) -> Step {
        match directive {
            WastDirective::Module(mut module) => {
                let name = module_name(&module)?;
                let module = self.compile(&mut module).await?;
                let instance = self.instantiate(&module).await?.map_err(|error| {
                    Miss::Fail(format!("the module does not instantiate: {error}"))
                })?;
                self.current = Some(instance);
                if let Some(name) = name {
                    self.instances.insert(name, instance);
                }
                Ok(())
            }
            WastDirective::ModuleDefinition(mut module) => {
                let name = module_name(&module)?;
                let module = self.compile(&mut module).await?;
                if let Some(name) = name {
                    self.definitions.insert(name, module.clone());
                }
                self.definition = Some(module);
                Ok(())
            }
            WastDirective::ModuleInstance {
                instance, module, ..
            } => {
                let module = match module {
                    Some(id) => self.definitions.get(id.name()).cloned(),
                    None => self.definition.clone(),
                }
                .ok_or_else(|| Miss::Fail("no module definition to instantiate".to_string()))?;
                let made = self.instantiate(&module).await?.map_err(|error| {
                    Miss::Fail(format!("the module does not instantiate: {error}"))
                })?;
                self.current = Some(made);
                if let Some(id) = instance {
                    self.instances.insert(id.name().to_string(), made);
                }
                Ok(())
            }
            WastDirective::AssertMalformed {
                module, message, ..
            } => self.assert_refused(module, "malformed", message).await,
            WastDirective::AssertInvalid {
                module, message, ..
            } => self.assert_refused(module, "invalid", message).await,
            WastDirective::AssertMalformedCustom { .. }
            | WastDirective::AssertInvalidCustom { .. } => Err(Miss::Skip(
                "asserts a fault of a custom section, which an engine does not read".to_string(),
            )),
            WastDirective::Register { name, module, .. } => {
                let instance = self.instance(module)?;
                self.registered.insert(name.to_string(), instance);
                Ok(())
            }
            WastDirective::Invoke(invoke) => match self.invoke(&invoke, None).await? {
                Ok(_) => Ok(()),
                Err(error) => Err(Miss::Fail(format!("the call failed: {error}"))),
            },
            WastDirective::AssertReturn { exec, results, .. } => {
                let values = match self.execute(exec, Some(results.len())).await? {
                    Ok(values) => values,
                    Err(error) => {
                        return Err(Miss::Fail(format!(
                            "expected results, and the runtime layer failed: {error}"
                        )));
                    }
                };
                self.compare(&results, &values)
            }
            WastDirective::AssertTrap { exec, message, .. } => {
                match self.execute(exec, None).await? {
                    Err(Error::Trap(kind)) if patterns::trap_matches(message, &kind) => Ok(()),
                    Err(Error::Trap(kind)) => Err(Miss::Fail(format!(
                        "expected the trap `{message}`, and the trap was {kind:?}: {kind}"
                    ))),
                    Err(error) => Err(Miss::Fail(format!(
                        "expected the trap `{message}`, and the error was: {error}"
                    ))),
                    Ok(values) => Err(Miss::Fail(format!(
                        "expected the trap `{message}`, and the call returned {values:?}"
                    ))),
                }
            }
            WastDirective::AssertExhaustion { call, message, .. } => {
                expect_trap(self.invoke(&call, None).await?, message, |kind| {
                    matches!(kind, TrapKind::StackOverflow)
                })
            }
            WastDirective::AssertException { exec, .. } => {
                expect_trap(self.execute(exec, None).await?, "an exception", |kind| {
                    matches!(kind, TrapKind::UncaughtException(_))
                })
            }
            WastDirective::AssertSuspension { exec, message, .. } => {
                expect_trap(self.execute(exec, None).await?, message, |kind| {
                    matches!(kind, TrapKind::UnhandledTag)
                })
            }
            WastDirective::AssertUnlinkable {
                mut module,
                message,
                ..
            } => self.assert_unlinkable(&mut module, message).await,
            WastDirective::Thread(_) | WastDirective::Wait { .. } => Err(Miss::Skip(
                "runs a thread of its own: the runtime layer runs no guest code on two host \
                 threads"
                    .to_string(),
            )),
        }
    }

    /// Compiles `module`, which the directive expects to be valid.
    async fn compile(&mut self, module: &mut QuoteWat<'_>) -> Result<Module, Miss> {
        let bytes = module
            .encode()
            .map_err(|error| Miss::Fail(format!("the module does not encode: {error}")))?;
        Module::compile(self.store.engine(), &bytes)
            .await
            .map_err(|error| Miss::Fail(format!("the engine refused a valid module: {error}")))
    }

    /// Instantiates `module` with its imports, or fails the directive
    /// where an import names nothing the script made importable.
    async fn instantiate(&mut self, module: &Module) -> Result<Result<Instance, Error>, Miss> {
        let imports = self.imports(module).map_err(Miss::Fail)?;
        Ok(Instance::instantiate(&mut self.store, module, &imports).await)
    }

    /// The externs for the imports of `module`, in order, or the import
    /// that names nothing importable.
    fn imports(&mut self, module: &Module) -> Result<Vec<Extern>, String> {
        let mut externs = Vec::new();
        for import in module.imports() {
            let found = if import.module() == "spectest" {
                self.spectest.get(import.name()).copied()
            } else {
                match self.registered.get(import.module()) {
                    Some(instance) => instance
                        .get_export(&mut self.store, import.name())
                        .map_err(|error| error.to_string())?,
                    None => None,
                }
            };
            let found = found.ok_or_else(|| {
                format!(
                    "unknown import `{}` `{}`: nothing importable has that name",
                    import.module(),
                    import.name()
                )
            })?;
            externs.push(found);
        }
        Ok(externs)
    }

    /// The instance `id` names, or the current instance.
    fn instance(&self, id: Option<Id<'_>>) -> Result<Instance, Miss> {
        match id {
            Some(id) => self
                .instances
                .get(id.name())
                .copied()
                .ok_or_else(|| Miss::Fail(format!("no instance named `{}`", id.name()))),
            None => self
                .current
                .ok_or_else(|| Miss::Fail("no instance to use".to_string())),
        }
    }

    /// Runs `exec`: a call, an instantiation, or a read of a global.
    /// `results` is the number of results the directive expects, where it
    /// expects any.
    async fn execute(
        &mut self,
        exec: WastExecute<'_>,
        results: Option<usize>,
    ) -> Result<Execution, Miss> {
        match exec {
            WastExecute::Invoke(invoke) => self.invoke(&invoke, results).await,
            WastExecute::Wat(mut wat) => {
                let bytes = wat
                    .encode()
                    .map_err(|error| Miss::Fail(format!("the module does not encode: {error}")))?;
                let module = match Module::compile(self.store.engine(), &bytes).await {
                    Ok(module) => module,
                    Err(error) => return Ok(Err(error)),
                };
                Ok(self.instantiate(&module).await?.map(|_| Vec::new()))
            }
            WastExecute::Get { module, global, .. } => {
                let instance = self.instance(module)?;
                let found = instance
                    .get_export(&mut self.store, global)
                    .map_err(|error| Miss::Fail(error.to_string()))?
                    .and_then(Extern::into_global)
                    .ok_or_else(|| Miss::Fail(format!("no global export `{global}`")))?;
                Ok(found.get(&mut self.store).map(|value| vec![value]))
            }
        }
    }

    /// Calls the function `invoke` names with its arguments. `results` is
    /// the number of results the directive expects, which counts the result
    /// slots where the engine does not know the function's type.
    async fn invoke(
        &mut self,
        invoke: &WastInvoke<'_>,
        results: Option<usize>,
    ) -> Result<Execution, Miss> {
        let instance = self.instance(invoke.module)?;
        let func = instance
            .get_export(&mut self.store, invoke.name)
            .map_err(|error| Miss::Fail(error.to_string()))?
            .and_then(Extern::into_func)
            .ok_or_else(|| Miss::Fail(format!("no function export `{}`", invoke.name)))?;
        let params = invoke
            .args
            .iter()
            .map(|arg| self.argument(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let count = match func.ty(&self.store) {
            Ok(Some(ty)) => ty.results().len(),
            _ => results.unwrap_or(0),
        };
        let mut outputs = vec![Val::I32(0); count];
        Ok(func
            .call(&mut self.store, &params, &mut outputs)
            .map(|()| outputs))
    }

    /// The value of the argument `arg`.
    fn argument(&mut self, arg: &WastArg<'_>) -> Result<Val, Miss> {
        let WastArg::Core(arg) = arg else {
            return Err(Miss::Fail(
                "a component value, in a core script".to_string(),
            ));
        };
        Ok(match arg {
            WastArgCore::I32(value) => Val::I32(*value),
            WastArgCore::I64(value) => Val::I64(*value),
            WastArgCore::F32(value) => Val::F32(value.bits),
            WastArgCore::F64(value) => Val::F64(value.bits),
            WastArgCore::V128(value) => Val::V128(u128::from_le_bytes(value.to_le_bytes())),
            // The null of a concrete type goes as a null function
            // reference, as the runtime layer defines it: the backend
            // gives the call the null of the parameter's own hierarchy.
            WastArgCore::RefNull(heap) => {
                Val::null(patterns::heap_type(heap).unwrap_or(HeapType::Func))
            }
            WastArgCore::RefExtern(number) => Val::ExternRef(Some(self.extern_ref(*number)?)),
            WastArgCore::RefHost(_) => {
                return Err(Miss::Skip(
                    "passes `ref.host`, a host object inside the internal hierarchy, which the \
                     host cannot make through the runtime layer"
                        .to_string(),
                ));
            }
        })
    }

    /// The `externref` of the number `number`, the same reference each time.
    fn extern_ref(&mut self, number: u32) -> Result<ExternRef, Miss> {
        if let Some(extern_ref) = self.extern_refs.get(&number) {
            return Ok(*extern_ref);
        }
        let extern_ref = ExternRef::new(&mut self.store, number)
            .map_err(|error| Miss::Fail(format!("the host cannot make an externref: {error}")))?;
        self.extern_refs.insert(number, extern_ref);
        Ok(extern_ref)
    }

    /// Whether `values` match the patterns `results`, one by one.
    fn compare(&self, results: &[WastRet<'_>], values: &[Val]) -> Step {
        let matched = results.len() == values.len()
            && results
                .iter()
                .zip(values)
                .all(|(expected, actual)| match expected {
                    WastRet::Core(expected) => patterns::matches(&self.store, expected, actual),
                    _ => false,
                });
        if matched {
            Ok(())
        } else {
            Err(Miss::Fail(format!(
                "expected {results:?}, and the results were {values:?}"
            )))
        }
    }

    /// Asserts that the engine refuses `module`, which is `fault`: malformed
    /// or invalid. A module whose text does not even encode is refused
    /// before an engine sees it, which the directive asserts too. A refusal
    /// that a capability the backend declares lifts does not apply to the
    /// backend, and is skipped: see [`Suite::lifting`].
    async fn assert_refused(
        &mut self,
        mut module: QuoteWat<'_>,
        fault: &str,
        message: &str,
    ) -> Step {
        let Ok(bytes) = module.encode() else {
            return Ok(());
        };
        if let Some(lifting) =
            Suite::lifting(self.store.engine().capabilities(), self.needed, &bytes)
        {
            return Err(Miss::Skip(format!(
                "asserts that a {fault} module is refused ({message}), which the backend's \
                 {} makes valid",
                Suite::Capabilities(lifting)
            )));
        }
        match Module::compile(self.store.engine(), &bytes).await {
            Err(Error::Compile { .. }) => Ok(()),
            Err(error) => Err(Miss::Fail(format!(
                "expected a {fault} module to be refused ({message}), and the error was: {error}"
            ))),
            Ok(_) => Err(Miss::Fail(format!(
                "expected a {fault} module to be refused ({message}), and the engine compiled it"
            ))),
        }
    }

    /// Asserts that `module` compiles and does not link.
    async fn assert_unlinkable(&mut self, module: &mut Wat<'_>, message: &str) -> Step {
        let bytes = module
            .encode()
            .map_err(|error| Miss::Fail(format!("the module does not encode: {error}")))?;
        let module = Module::compile(self.store.engine(), &bytes)
            .await
            .map_err(|error| Miss::Fail(format!("the engine refused a valid module: {error}")))?;
        let Ok(imports) = self.imports(&module) else {
            // An import that names nothing importable does not link.
            return Ok(());
        };
        match Instance::instantiate(&mut self.store, &module, &imports).await {
            Err(Error::Link { .. }) => Ok(()),
            Err(error) => Err(Miss::Fail(format!(
                "expected a link error ({message}), and the error was: {error}"
            ))),
            Ok(_) => Err(Miss::Fail(format!(
                "expected a link error ({message}), and the module instantiated"
            ))),
        }
    }
}

/// The name `module` defines itself by, where it has one.
fn module_name(module: &QuoteWat<'_>) -> Result<Option<String>, Miss> {
    match module {
        QuoteWat::Wat(Wat::Module(module)) => Ok(module.id.map(|id| id.name().to_string())),
        QuoteWat::QuoteModule(..) => Ok(None),
        QuoteWat::Wat(Wat::Component(_)) | QuoteWat::QuoteComponent(..) => {
            Err(Miss::Fail("a component, in a core script".to_string()))
        }
    }
}

/// Whether `execution` failed with a trap that `kind` accepts, which the
/// directive names `expected`.
fn expect_trap(execution: Execution, expected: &str, kind: impl Fn(&TrapKind) -> bool) -> Step {
    match execution {
        Err(Error::Trap(trap)) if kind(&trap) => Ok(()),
        Err(error) => Err(Miss::Fail(format!(
            "expected {expected}, and the error was: {error}"
        ))),
        Ok(values) => Err(Miss::Fail(format!(
            "expected {expected}, and the call returned {values:?}"
        ))),
    }
}

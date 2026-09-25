//! The switch module: the core module that stands between the
//! scheduler and the guests of a store, in the form of either
//! provider.

use std::borrow::Cow;

use wasm_encoder::{
    BlockType, CodeSection, ContType, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, Handle, HeapType, ImportSection, Instruction, Module,
    RefType, TableSection, TableType, TagKind, TagSection, TagType, TypeSection, ValType,
};
use wasm_runtime_layer::{FuncType, ValType as RuntimeValType};

use super::switch_form::SwitchForm;

/// The core module the polyfill generates for each store to switch
/// the stacks of its guest threads, in the form of one provider.
///
/// The polyfill builds the module's bytes in memory, so the switch
/// module is never fetched. It has two parts, the same in both
/// forms:
///
/// - A shim for each blocking built-in, which a guest imports in
///   place of the host trampoline. A suspension must have only
///   WebAssembly frames between it and the start of the thread's
///   stack, and a host trampoline is not WebAssembly, so the shim is
///   what suspends. It calls a host function that tries the built-in
///   and returns at once. When the built-in is ready the shim returns
///   what a second host function, the built-in's finish, computes.
///   Otherwise it suspends, and tries again when the thread resumes:
///
///   ```text
///   shim for a blocking built-in B:
///       loop:
///           status = host.try_B(args)     // returns at once, never blocks
///           if status is ready:
///               return host.finish_B(args) // the result the guest reads
///           suspend                        // the form's own suspend
///   ```
///
/// - An entry wrapper for each type of thread entry, the guest
///   function that starts a thread. The wrapper calls the entry and
///   hands its results to a host function before it returns, so the
///   scheduler reads them from the host side at once, whether or not
///   the entry suspended on the way:
///
///   ```text
///   entry wrapper for a thread entry E:
///       results = E(args)
///       host.finished(thread, results)
///   ```
///
/// The forms differ only in the form of `suspend`, and in how a
/// thread starts and resumes.
///
/// The stack-switching form, [`SwitchForm::StackSwitching`], uses the
/// instructions of the WebAssembly stack-switching proposal. It
/// defines one control tag, `$block`, which every shim suspends on,
/// and one table of continuations, indexed by thread, where a
/// suspended thread waits. It exports a start and a resume function
/// to the scheduler:
///
/// ```text
/// export start(thread, entry, args) -> status:
///     $entries[thread] = entry
///     return run(thread, cont.new(entry wrapper), args)
/// export resume(thread) -> status:
///     c = $threads[thread]; $threads[thread] = null
///     return run(thread, c)
///
/// run(thread, c, args):
///     resume c (on $block -> parked) args
///     return finished
///   parked(k):
///     $threads[thread] = k
///     return suspended
///
/// suspend in a shim:  suspend $block
/// ```
///
/// A resumption runs synchronously, and returns when the thread
/// suspends again or finishes. Any number of threads wait in the
/// table at once, each in its own slot, and a resume may name any of
/// them, so they resume in any order. A start or a resume made from a
/// host function that runs inside another thread starts or resumes a
/// continuation of its own on top of that thread, and returns to the
/// host function when it stops. A store that drops drops the table,
/// and the continuations in it are never resumed.
///
/// The JSPI form, [`SwitchForm::Jspi`], uses JavaScript Promise
/// Integration, and keeps no suspended thread itself: the browser
/// keeps each suspended stack. A shim suspends by calling one more
/// import, `suspend`, which the host makes with
/// `WebAssembly.Suspending`. A thread starts through
/// `WebAssembly.promising` over the form's start, which calls the
/// entry wrapper directly, so the stack a promising call begins holds
/// only WebAssembly frames from its start to the shim:
///
/// ```text
/// export start(thread, entry, args):
///     $entries[thread] = entry
///     entry wrapper(thread, args)
///
/// suspend in a shim:  host.suspend()   // a WebAssembly.Suspending import
/// ```
///
/// The function behind `suspend` answers a promise the host holds,
/// and the host resumes the thread by resolving it. The resumed shim
/// tries the built-in again, as in the other form. The shim calls
/// `suspend` only when the built-in is not ready, because a call of a
/// suspending import always suspends: Chromium 147 suspends even when
/// the function answers a plain value or a resolved promise, although
/// the proposal's overview states the opposite. A promising call
/// answers a promise and never the entry's results, and a wrapper in
/// JavaScript would put a frame that is not WebAssembly between the
/// start of the stack and the suspension, which traps. The entry
/// wrapper in WebAssembly is therefore how the host learns at once
/// that an entry finished. The form has no resume export: a thread
/// resumes when its promise resolves.
///
/// The entry reaches the wrapper through a table of functions,
/// indexed by thread, which the start fills, so that one wrapper
/// serves every entry of its type. Every table grows on demand to
/// hold the thread index a start names.
///
/// A module is described with [`shim`](Self::shim) and
/// [`entry`](Self::entry) and encoded with [`encode`](Self::encode).
/// The encoded module imports, from the module `host`:
///
/// - `try{i}` and `finish{i}` for shim `i`. The try takes the shim's
///   parameters and answers an `i32`, nonzero when the built-in is
///   ready. The finish takes the shim's parameters and answers its
///   results.
/// - `finished{j}` for entry type `j`, which takes the thread index
///   and then the entry's results.
/// - In the JSPI form, `suspend`, of type `[] -> []`, last.
///
/// It exports `shim{i}` for each shim and `start{j}` for each entry
/// type. `start{j}` takes the thread index, the entry as a `funcref`,
/// and the entry's parameters. In the stack-switching form it answers
/// [`FINISHED`](Self::FINISHED) or [`SUSPENDED`](Self::SUSPENDED), and
/// the module exports `resume` too, which takes a thread index and
/// answers the same. In the JSPI form `start{j}` answers nothing.
/// Every import and export is a function over number types and
/// `funcref`; the tag, the continuation types, and the tables stay
/// inside the module.
#[derive(Clone, Debug)]
pub struct SwitchModule {
    form: SwitchForm,
    shims: Vec<FuncType>,
    entries: Vec<FuncType>,
}

/// The type index of `[] -> []`, the type of the tag, of a
/// continuation that waits in the table, and of the JSPI form's
/// `suspend` import.
const UNIT: u32 = 0;

/// The type index of the continuation type over [`UNIT`], the type
/// of the table of continuations, in the stack-switching form.
const PARKED: u32 = 1;

/// The one control tag every shim suspends on, in the
/// stack-switching form.
const BLOCK: u32 = 0;

/// The table of continuations, one slot per thread, in the
/// stack-switching form.
const THREADS: u32 = 0;

impl SwitchModule {
    /// What a start or a resume of the stack-switching form answers
    /// when the thread finished.
    pub const FINISHED: i32 = 0;

    /// What a start or a resume of the stack-switching form answers
    /// when the thread suspended.
    pub const SUSPENDED: i32 = 1;

    /// A switch module of `form` with no shims and no entry wrappers.
    pub fn new(form: SwitchForm) -> Self {
        Self {
            form,
            shims: Vec::new(),
            entries: Vec::new(),
        }
    }

    /// The provider's form this module takes.
    pub fn form(&self) -> SwitchForm {
        self.form
    }

    /// Add a shim of type `ty` for a blocking built-in, and answer
    /// its index.
    pub fn shim(&mut self, ty: FuncType) -> u32 {
        self.shims.push(ty);
        index(self.shims.len() - 1)
    }

    /// Add an entry wrapper and a start for thread entries of type
    /// `ty`, and answer their index.
    pub fn entry(&mut self, ty: FuncType) -> u32 {
        self.entries.push(ty);
        index(self.entries.len() - 1)
    }

    /// The entry types, in the order of their indices.
    pub fn entry_types(&self) -> &[FuncType] {
        &self.entries
    }

    /// The number of shims.
    pub fn shim_count(&self) -> u32 {
        index(self.shims.len())
    }

    /// Encode the module.
    pub fn encode(&self) -> Vec<u8> {
        let stack_switching = self.form == SwitchForm::StackSwitching;
        let layout = Layout::of(self);
        let mut types = TypeSection::new();
        let mut imports = ImportSection::new();
        let mut functions = FunctionSection::new();
        let mut exports = ExportSection::new();
        let mut code = CodeSection::new();

        types.ty().function([], []);
        if stack_switching {
            types.ty().cont(&ContType(UNIT));
        }

        let shim_types = self
            .shims
            .iter()
            .map(|ty| {
                let try_type = types.len();
                types.ty().function(params(ty), [ValType::I32]);
                let shim_type = types.len();
                types.ty().function(params(ty), results(ty));
                (try_type, shim_type)
            })
            .collect::<Vec<_>>();
        let entry_types = self
            .entries
            .iter()
            .map(|ty| {
                let entry_type = types.len();
                types.ty().function(params(ty), results(ty));
                let wrapper_type = types.len();
                types
                    .ty()
                    .function(prefixed(&[ValType::I32], params(ty)), []);
                let cont_type = stack_switching.then(|| {
                    let cont_type = types.len();
                    types.ty().cont(&ContType(wrapper_type));
                    cont_type
                });
                let finished_type = types.len();
                types
                    .ty()
                    .function(prefixed(&[ValType::I32], results(ty)), []);
                let start_type = types.len();
                let status: &[ValType] = if stack_switching {
                    &[ValType::I32]
                } else {
                    &[]
                };
                types.ty().function(
                    prefixed(&[ValType::I32, ValType::FUNCREF], params(ty)),
                    status.iter().copied(),
                );
                EntryTypes {
                    entry: entry_type,
                    wrapper: wrapper_type,
                    cont: cont_type,
                    finished: finished_type,
                    start: start_type,
                }
            })
            .collect::<Vec<_>>();
        let resume_type = types.len();
        if stack_switching {
            types.ty().function([ValType::I32], [ValType::I32]);
        }

        for (i, (try_type, shim_type)) in shim_types.iter().enumerate() {
            imports.import("host", &format!("try{i}"), EntityType::Function(*try_type));
            imports.import(
                "host",
                &format!("finish{i}"),
                EntityType::Function(*shim_type),
            );
        }
        for (j, entry) in entry_types.iter().enumerate() {
            imports.import(
                "host",
                &format!("finished{j}"),
                EntityType::Function(entry.finished),
            );
        }
        if !stack_switching {
            imports.import("host", "suspend", EntityType::Function(UNIT));
        }

        let mut tables = TableSection::new();
        if stack_switching {
            tables.table(table_of(parked(true)));
        }
        tables.table(table_of(RefType::FUNCREF));

        let suspend = match self.form {
            SwitchForm::StackSwitching => Instruction::Suspend(BLOCK),
            SwitchForm::Jspi => Instruction::Call(layout.suspend_import()),
        };
        for (i, (ty, (_, shim_type))) in self.shims.iter().zip(&shim_types).enumerate() {
            let i = index(i);
            functions.function(*shim_type);
            code.function(&shim_body(
                ty,
                layout.try_import(i),
                layout.finish_import(i),
                &suspend,
            ));
            exports.export(&format!("shim{i}"), ExportKind::Func, layout.shim(i));
        }
        for (j, (ty, entry)) in self.entries.iter().zip(&entry_types).enumerate() {
            functions.function(entry.wrapper);
            code.function(&wrapper_body(
                ty,
                entry.entry,
                layout.entries_table(),
                layout.finished_import(index(j)),
            ));
        }
        for (j, (ty, entry)) in self.entries.iter().zip(&entry_types).enumerate() {
            let j = index(j);
            functions.function(entry.start);
            code.function(&match entry.cont {
                Some(cont_type) => start_body(ty, cont_type, layout.wrapper(j)),
                None => promising_start_body(ty, layout.entries_table(), layout.wrapper(j)),
            });
            exports.export(&format!("start{j}"), ExportKind::Func, layout.start(j));
        }

        let mut module = Module::new();
        if !stack_switching {
            module
                .section(&types)
                .section(&imports)
                .section(&functions)
                .section(&tables)
                .section(&exports)
                .section(&code);
            return module.finish();
        }

        functions.function(resume_type);
        code.function(&resume_body());
        exports.export("resume", ExportKind::Func, layout.resume());

        let mut tags = TagSection::new();
        tags.tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: UNIT,
        });

        let wrappers = (0..index(self.entries.len()))
            .map(|j| layout.wrapper(j))
            .collect::<Vec<_>>();
        let mut elements = ElementSection::new();
        elements.declared(Elements::Functions(Cow::Owned(wrappers)));

        module
            .section(&types)
            .section(&imports)
            .section(&functions)
            .section(&tables)
            .section(&tags)
            .section(&exports)
            .section(&elements)
            .section(&code);
        module.finish()
    }
}

/// The type indices one entry type adds to the module.
struct EntryTypes {
    /// The entry's own type, which the wrapper calls it through.
    entry: u32,
    /// The wrapper's type: the thread index, then the entry's
    /// parameters.
    wrapper: u32,
    /// The continuation type over the wrapper's type, in the
    /// stack-switching form.
    cont: Option<u32>,
    /// The type of the `finished` import: the thread index, then the
    /// entry's results.
    finished: u32,
    /// The type of the start export.
    start: u32,
}

/// Where each function and table of a module lands in its index
/// space. The functions are the imports first, then the shims, the
/// entry wrappers, the starts, and in the stack-switching form the
/// resume. The imports are the tries and finishes, the `finished`
/// imports, and in the JSPI form `suspend`.
struct Layout {
    form: SwitchForm,
    shims: u32,
    entries: u32,
}

impl Layout {
    fn of(module: &SwitchModule) -> Self {
        Self {
            form: module.form,
            shims: index(module.shims.len()),
            entries: index(module.entries.len()),
        }
    }

    fn try_import(&self, shim: u32) -> u32 {
        2 * shim
    }

    fn finish_import(&self, shim: u32) -> u32 {
        2 * shim + 1
    }

    fn finished_import(&self, entry: u32) -> u32 {
        2 * self.shims + entry
    }

    fn suspend_import(&self) -> u32 {
        2 * self.shims + self.entries
    }

    fn imports(&self) -> u32 {
        match self.form {
            SwitchForm::StackSwitching => 2 * self.shims + self.entries,
            SwitchForm::Jspi => 2 * self.shims + self.entries + 1,
        }
    }

    fn shim(&self, shim: u32) -> u32 {
        self.imports() + shim
    }

    fn wrapper(&self, entry: u32) -> u32 {
        self.imports() + self.shims + entry
    }

    fn start(&self, entry: u32) -> u32 {
        self.imports() + self.shims + self.entries + entry
    }

    fn resume(&self) -> u32 {
        self.imports() + self.shims + 2 * self.entries
    }

    /// The table of thread entries, one slot per thread, which
    /// follows the table of continuations in the stack-switching
    /// form and is the only table of the JSPI form.
    fn entries_table(&self) -> u32 {
        match self.form {
            SwitchForm::StackSwitching => THREADS + 1,
            SwitchForm::Jspi => 0,
        }
    }
}

/// The body of a shim of type `ty`, over its try and finish imports,
/// which suspends with `suspend`.
fn shim_body(
    ty: &FuncType,
    try_import: u32,
    finish_import: u32,
    suspend: &Instruction<'_>,
) -> Function {
    let arguments = index(ty.params().len());
    let mut body = Function::new([]);
    body.instruction(&Instruction::Loop(BlockType::Empty));
    for local in 0..arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::Call(try_import));
    body.instruction(&Instruction::If(BlockType::Empty));
    for local in 0..arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::Call(finish_import));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);
    body.instruction(suspend);
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body
}

/// The body of the entry wrapper for entries of type `ty`, which
/// reads the entry from `entries_table`. Local 0 is the thread index
/// and the entry's parameters follow it; the entry's results are
/// spilled to locals of their own, so that the `finished` import
/// receives the thread index first.
fn wrapper_body(
    ty: &FuncType,
    entry_type: u32,
    entries_table: u32,
    finished_import: u32,
) -> Function {
    let arguments = index(ty.params().len());
    let spilled = 1 + arguments;
    let mut body = Function::new(results(ty).map(|ty| (1, ty)));
    for local in 1..=arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::CallIndirect {
        type_index: entry_type,
        table_index: entries_table,
    });
    for result in (0..index(ty.results().len())).rev() {
        body.instruction(&Instruction::LocalSet(spilled + result));
    }
    body.instruction(&Instruction::LocalGet(0));
    for result in 0..index(ty.results().len()) {
        body.instruction(&Instruction::LocalGet(spilled + result));
    }
    body.instruction(&Instruction::Call(finished_import));
    body.instruction(&Instruction::End);
    body
}

/// The body of the stack-switching start for entries of type `ty`.
/// Local 0 is the thread index, local 1 the entry, and the entry's
/// parameters follow; one more local holds the continuation that
/// parked.
fn start_body(ty: &FuncType, cont_type: u32, wrapper: u32) -> Function {
    let arguments = index(ty.params().len());
    let parked_local = 2 + arguments;
    let entries = THREADS + 1;
    let mut body = Function::new([(1, ValType::Ref(parked(true)))]);
    grow_to_hold_thread(&mut body, THREADS, HeapType::Concrete(PARKED));
    grow_to_hold_thread(&mut body, entries, HeapType::FUNC);
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::TableSet(entries));
    body.instruction(&Instruction::Block(BlockType::Result(ValType::Ref(
        parked(false),
    ))));
    body.instruction(&Instruction::LocalGet(0));
    for local in 2..2 + arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::RefFunc(wrapper));
    body.instruction(&Instruction::ContNew(cont_type));
    run(&mut body, cont_type, parked_local);
    body
}

/// The body of the JSPI start for entries of type `ty`, which the
/// host calls through `WebAssembly.promising`. Local 0 is the thread
/// index, local 1 the entry, and the entry's parameters follow. It
/// puts the entry in the thread's slot of `entries_table` and calls
/// the wrapper, so the stack the promising call begins holds only
/// WebAssembly frames.
fn promising_start_body(ty: &FuncType, entries_table: u32, wrapper: u32) -> Function {
    let arguments = index(ty.params().len());
    let mut body = Function::new([]);
    grow_to_hold_thread(&mut body, entries_table, HeapType::FUNC);
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::TableSet(entries_table));
    body.instruction(&Instruction::LocalGet(0));
    for local in 2..2 + arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::Call(wrapper));
    body.instruction(&Instruction::End);
    body
}

/// The body of the resume. Local 0 is the thread index, and local 1
/// holds the continuation taken out of the thread's slot. The slot
/// is emptied before the thread runs, so a second resume of a thread
/// that is not suspended traps on the empty slot.
fn resume_body() -> Function {
    let parked_local = 1;
    let mut body = Function::new([(1, ValType::Ref(parked(true)))]);
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::TableGet(THREADS));
    body.instruction(&Instruction::LocalSet(parked_local));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::RefNull(HeapType::Concrete(PARKED)));
    body.instruction(&Instruction::TableSet(THREADS));
    body.instruction(&Instruction::Block(BlockType::Result(ValType::Ref(
        parked(false),
    ))));
    body.instruction(&Instruction::LocalGet(parked_local));
    run(&mut body, PARKED, parked_local);
    body
}

/// The common tail of a start and a resume: resume the continuation
/// on the stack, of type `cont_type`, inside the block the caller
/// opened, which is the handler of `$block`. A continuation that
/// returns answers [`SwitchModule::FINISHED`]. One that suspends
/// lands at the end of the block with its remainder on the stack,
/// which parks in the thread's slot, and the function answers
/// [`SwitchModule::SUSPENDED`].
fn run(body: &mut Function, cont_type: u32, parked_local: u32) {
    body.instruction(&Instruction::Resume {
        cont_type_index: cont_type,
        resume_table: Cow::Owned(vec![Handle::OnLabel {
            tag: BLOCK,
            label: 0,
        }]),
    });
    body.instruction(&Instruction::I32Const(SwitchModule::FINISHED));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalSet(parked_local));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(parked_local));
    body.instruction(&Instruction::TableSet(THREADS));
    body.instruction(&Instruction::I32Const(SwitchModule::SUSPENDED));
    body.instruction(&Instruction::End);
}

/// Grow `table` with nulls of `heap` until it holds the thread index
/// in local 0.
fn grow_to_hold_thread(body: &mut Function, table: u32, heap: HeapType) {
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::TableSize(table));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::RefNull(heap));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::TableSize(table));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::TableGrow(table));
    body.instruction(&Instruction::Drop);
    body.instruction(&Instruction::End);
}

/// A reference to a parked continuation.
fn parked(nullable: bool) -> RefType {
    RefType {
        nullable,
        heap_type: HeapType::Concrete(PARKED),
    }
}

/// A table of `element`, empty until it grows.
fn table_of(element: RefType) -> TableType {
    TableType {
        element_type: element,
        table64: false,
        minimum: 0,
        maximum: None,
        shared: false,
    }
}

/// `head`, then `tail`.
fn prefixed(head: &[ValType], tail: impl Iterator<Item = ValType>) -> Vec<ValType> {
    head.iter().copied().chain(tail).collect()
}

fn params(ty: &FuncType) -> impl ExactSizeIterator<Item = ValType> + Clone + '_ {
    ty.params().iter().map(|ty| value_type(*ty))
}

fn results(ty: &FuncType) -> impl ExactSizeIterator<Item = ValType> + Clone + '_ {
    ty.results().iter().map(|ty| value_type(*ty))
}

/// The encoder's name for a runtime-layer value type.
fn value_type(ty: RuntimeValType) -> ValType {
    match ty {
        RuntimeValType::I32 => ValType::I32,
        RuntimeValType::I64 => ValType::I64,
        RuntimeValType::F32 => ValType::F32,
        RuntimeValType::F64 => ValType::F64,
        RuntimeValType::V128 => ValType::V128,
        RuntimeValType::FuncRef => ValType::FUNCREF,
        RuntimeValType::ExternRef => ValType::EXTERNREF,
    }
}

/// A count or position of the module as a WebAssembly index. A
/// switch module has a handful of functions, far below the limit.
fn index(value: usize) -> u32 {
    u32::try_from(value).expect("a switch module has fewer than 2^32 functions")
}

#[cfg(test)]
mod tests {
    use wasm_runtime_layer::Module as RuntimeModule;

    use super::*;
    use crate::Engine;
    use crate::internal::EngineInternal;

    fn i32_to_i32() -> FuncType {
        FuncType::new([RuntimeValType::I32], [RuntimeValType::I32])
    }

    #[wcmp_macros::test]
    fn it_encodes_a_jspi_form_that_needs_no_stack_switching() {
        // The JSPI form has no tag, no continuation type, and no
        // table of continuations, so every engine compiles it, the
        // browser included, whether or not it switches stacks.
        let mut module = SwitchModule::new(SwitchForm::Jspi);
        module.shim(i32_to_i32());
        module.entry(i32_to_i32());
        let engine = Engine::new().expect("engine");
        let compiled = RuntimeModule::new(engine.inner(), &module.encode())
            .expect("the engine compiles the JSPI form");

        let mut imports = compiled
            .imports(engine.inner())
            .map(|import| format!("{}.{}", import.module, import.name))
            .collect::<Vec<_>>();
        imports.sort();
        assert_eq!(
            imports,
            [
                "host.finish0",
                "host.finished0",
                "host.suspend",
                "host.try0"
            ]
        );

        let mut exports = compiled
            .exports(engine.inner())
            .map(|export| export.name.to_owned())
            .collect::<Vec<_>>();
        exports.sort();
        assert_eq!(
            exports,
            ["shim0", "start0"],
            "the form has no resume: a thread resumes when its promise resolves"
        );
        assert_eq!(
            compiled
                .get_export(engine.inner(), "start0")
                .and_then(|ty| ty.try_into_func().ok()),
            Some(FuncType::new(
                [
                    RuntimeValType::I32,
                    RuntimeValType::FuncRef,
                    RuntimeValType::I32
                ],
                []
            )),
            "a start takes the thread index, the entry, and the entry's \
             parameters, and answers nothing"
        );
    }
}

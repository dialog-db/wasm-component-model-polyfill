//! The switch module: the core module that stands between the
//! scheduler and the guests of a store, in its stack-switching form.

use std::borrow::Cow;

use wasm_encoder::{
    BlockType, CodeSection, ContType, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, Handle, HeapType, ImportSection, Instruction, Module,
    RefType, TableSection, TableType, TagKind, TagSection, TagType, TypeSection, ValType,
};
use wasm_runtime_layer::{FuncType, ValType as RuntimeValType};

/// The core module the polyfill generates for each store to switch
/// the stacks of its guest threads, in the form that uses the
/// instructions of the WebAssembly stack-switching proposal.
///
/// The polyfill builds the module's bytes in memory, so the switch
/// module is never fetched. It has two parts:
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
///           suspend $block
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
/// The stack-switching form defines one control tag, `$block`, which
/// every shim suspends on, and one table of continuations, indexed by
/// thread, where a suspended thread waits. It exports a start and a
/// resume function to the scheduler:
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
/// The entry reaches the wrapper through a second table, of
/// functions and indexed by thread, which the start fills, so that
/// one wrapper serves every entry of its type. Both tables grow on
/// demand to hold the thread index a start names.
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
///
/// It exports `shim{i}` for each shim, `start{j}` for each entry type,
/// and `resume`. `start{j}` takes the thread index, the entry as a
/// `funcref`, and the entry's parameters. It and `resume` answer
/// [`FINISHED`](Self::FINISHED) or [`SUSPENDED`](Self::SUSPENDED).
/// Every import and export is a function over number types and
/// `funcref`; the tag, the continuation types, and the table of
/// continuations stay inside the module.
#[derive(Clone, Debug, Default)]
pub struct SwitchModule {
    shims: Vec<FuncType>,
    entries: Vec<FuncType>,
}

/// The type index of `[] -> []`, the type of the tag and of a
/// continuation that waits in the table.
const UNIT: u32 = 0;

/// The type index of the continuation type over [`UNIT`], the type
/// of the table of continuations.
const PARKED: u32 = 1;

/// The one control tag every shim suspends on.
const BLOCK: u32 = 0;

/// The table of continuations, one slot per thread.
const THREADS: u32 = 0;

/// The table of thread entries, one slot per thread.
const ENTRIES: u32 = 1;

impl SwitchModule {
    /// What a start or a resume answers when the thread finished.
    pub const FINISHED: i32 = 0;

    /// What a start or a resume answers when the thread suspended.
    pub const SUSPENDED: i32 = 1;

    /// A switch module with no shims and no entry wrappers.
    pub fn new() -> Self {
        Self::default()
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
        let layout = Layout::of(self);
        let mut types = TypeSection::new();
        let mut imports = ImportSection::new();
        let mut functions = FunctionSection::new();
        let mut exports = ExportSection::new();
        let mut code = CodeSection::new();

        types.ty().function([], []);
        types.ty().cont(&ContType(UNIT));

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
                let cont_type = types.len();
                types.ty().cont(&ContType(wrapper_type));
                let finished_type = types.len();
                types
                    .ty()
                    .function(prefixed(&[ValType::I32], results(ty)), []);
                let start_type = types.len();
                types.ty().function(
                    prefixed(&[ValType::I32, ValType::FUNCREF], params(ty)),
                    [ValType::I32],
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
        types.ty().function([ValType::I32], [ValType::I32]);

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

        let mut tables = TableSection::new();
        tables.table(table_of(parked(true)));
        tables.table(table_of(RefType::FUNCREF));

        let mut tags = TagSection::new();
        tags.tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: UNIT,
        });

        for (i, (ty, (_, shim_type))) in self.shims.iter().zip(&shim_types).enumerate() {
            let i = index(i);
            functions.function(*shim_type);
            code.function(&shim_body(
                ty,
                layout.try_import(i),
                layout.finish_import(i),
            ));
            exports.export(&format!("shim{i}"), ExportKind::Func, layout.shim(i));
        }
        for (j, (ty, entry)) in self.entries.iter().zip(&entry_types).enumerate() {
            functions.function(entry.wrapper);
            code.function(&wrapper_body(
                ty,
                entry.entry,
                layout.finished_import(index(j)),
            ));
        }
        for (j, (ty, entry)) in self.entries.iter().zip(&entry_types).enumerate() {
            let j = index(j);
            functions.function(entry.start);
            code.function(&start_body(ty, entry.cont, layout.wrapper(j)));
            exports.export(&format!("start{j}"), ExportKind::Func, layout.start(j));
        }
        functions.function(resume_type);
        code.function(&resume_body());
        exports.export("resume", ExportKind::Func, layout.resume());

        let wrappers = (0..index(self.entries.len()))
            .map(|j| layout.wrapper(j))
            .collect::<Vec<_>>();
        let mut elements = ElementSection::new();
        elements.declared(Elements::Functions(Cow::Owned(wrappers)));

        let mut module = Module::new();
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
    /// The continuation type over the wrapper's type.
    cont: u32,
    /// The type of the `finished` import: the thread index, then the
    /// entry's results.
    finished: u32,
    /// The type of the start export.
    start: u32,
}

/// Where each function of a module lands in the function index
/// space: the imports first, then the shims, the entry wrappers, the
/// starts, and the resume.
struct Layout {
    shims: u32,
    entries: u32,
}

impl Layout {
    fn of(module: &SwitchModule) -> Self {
        Self {
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

    fn imports(&self) -> u32 {
        2 * self.shims + self.entries
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
}

/// The body of a shim of type `ty`, over its try and finish imports.
fn shim_body(ty: &FuncType, try_import: u32, finish_import: u32) -> Function {
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
    body.instruction(&Instruction::Suspend(BLOCK));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body
}

/// The body of the entry wrapper for entries of type `ty`. Local 0
/// is the thread index and the entry's parameters follow it; the
/// entry's results are spilled to locals of their own, so that the
/// `finished` import receives the thread index first.
fn wrapper_body(ty: &FuncType, entry_type: u32, finished_import: u32) -> Function {
    let arguments = index(ty.params().len());
    let spilled = 1 + arguments;
    let mut body = Function::new(results(ty).map(|ty| (1, ty)));
    for local in 1..=arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::CallIndirect {
        type_index: entry_type,
        table_index: ENTRIES,
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

/// The body of the start for entries of type `ty`. Local 0 is the
/// thread index, local 1 the entry, and the entry's parameters
/// follow; one more local holds the continuation that parked.
fn start_body(ty: &FuncType, cont_type: u32, wrapper: u32) -> Function {
    let arguments = index(ty.params().len());
    let parked_local = 2 + arguments;
    let mut body = Function::new([(1, ValType::Ref(parked(true)))]);
    grow_to_hold_thread(&mut body, THREADS, HeapType::Concrete(PARKED));
    grow_to_hold_thread(&mut body, ENTRIES, HeapType::FUNC);
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::TableSet(ENTRIES));
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

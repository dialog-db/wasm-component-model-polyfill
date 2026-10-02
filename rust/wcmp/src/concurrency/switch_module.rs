// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The switch module: the core modules that stand between the
//! scheduler and the guests of a store, in the form of either
//! provider.

use std::borrow::Cow;

use wasm_encoder::{
    AbstractHeapType, BlockType, CodeSection, ConstExpr, ContType, ElementSection, Elements,
    EntityType, ExportKind, ExportSection, Function, FunctionSection, GlobalSection, GlobalType,
    Handle, HeapType, ImportSection, Instruction, Module, RefType, TableSection, TableType,
    TagKind, TagSection, TagType, TypeSection, ValType,
};

use crate::runtime_layer::{FuncType, HeapType as RuntimeHeapType, ValType as RuntimeValType};

use super::switch_form::SwitchForm;

/// The core modules the polyfill generates for each store to switch
/// the stacks of its guest threads, in the form of one provider, a
/// [`SwitchForm`].
///
/// The polyfill builds the bytes in memory, so the switch module is
/// never fetched. It has two parts, the same in both forms:
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
/// # The stack-switching form
///
/// The stack-switching form, [`SwitchForm::StackSwitching`], uses the
/// instructions of the WebAssembly stack-switching proposal. A thread
/// suspends on the one control tag `$block`, and the
/// handler that catches it must name that same tag. The guests of a
/// store call one another through fused adapters, so a thread that
/// one component started can suspend in the shim of another. The
/// polyfill learns the types of the shims and the entries only as
/// components are instantiated into the store, one instantiation at a
/// time. The switch module is therefore two kinds of core module:
///
/// - The base module, one instance per store, from
///   [`base`](Self::base). It defines `$block`, the table of
///   continuations where a suspended thread waits, and the pool of
///   workers described below. It exports `start`, `resume`, and
///   `suspend`, which is the one place a shim suspends.
/// - An extension module, from [`encode`](Self::encode), with the
///   shims and the entry wrappers of whatever types it is described
///   with. It imports `start` and `suspend` from the base module, so
///   every thread of the store suspends on the same tag and waits in
///   the same table, whichever module its shim and its wrapper came
///   from.
///
/// ```text
/// base module:
///     tag $block : [] -> []
///     tag $done  : [] -> [i32]
///     table $threads (ref null cont), $entries funcref, $idle (ref null cont)
///
///     worker(thread):
///         loop:
///             $entries[thread](thread)       // a wrapper of an extension
///             thread = suspend $done         // finished: wait for more work
///
///     export start(thread, wrapper) -> status:
///         $entries[thread] = wrapper
///         k = pop $idle, or cont.new(worker)
///         return run(thread, resume k with thread)
///     export resume(thread) -> status:
///         c = $threads[thread]; $threads[thread] = null
///         return run(thread, resume c)
///     export suspend():
///         suspend $block
///
///     run(thread, resumption):
///         the thread suspended on $block with k: $threads[thread] = k, answer suspended
///         the worker suspended on $done with k:  push k on $idle, answer finished
/// ```
///
/// A thread runs on a worker, a continuation of the base module's
/// `worker` function. A worker never returns: once its entry
/// finishes, it suspends on `$done` and waits in the pool of idle
/// workers, and the next start resumes it with the next thread
/// rather than making a continuation of its own. Wasmtime frees no
/// continuation before its store drops, and each one holds a stack
/// of its own, so the pool is what bounds a long-lived store's
/// memory: the store holds as many stacks as it ever had threads
/// alive at once, not one for every thread it ever started. A thread
/// that traps loses its worker, and a thread the store gives up on
/// while it waits keeps its worker until the store drops.
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
/// An extension module is described with [`shim`](Self::shim) and
/// [`entry`](Self::entry). It imports `start` and `suspend` from the
/// module `base`, and from the module `host`:
///
/// - `try{i}` and `finish{i}` for shim `i`. The try takes the shim's
///   parameters and answers an `i32`: positive when the built-in is
///   ready, zero when it is not, and [`DROPPED`](Self::DROPPED),
///   which is negative and on which the shim traps, when the store
///   was dropped while a resume of the thread was under way. The
///   finish takes the shim's parameters and answers its results.
/// - `finished{j}` for entry type `j`, which takes the thread index
///   and then the entry's results.
///
/// It exports `shim{i}` for each shim and `start{j}` for each entry
/// type. `start{j}` takes the thread index, the entry as a `funcref`,
/// and the entry's parameters, and answers what the base module's
/// `start` answers: [`FINISHED`](Self::FINISHED) or
/// [`SUSPENDED`](Self::SUSPENDED). It hands the entry and its
/// parameters to its wrapper through a table and globals of its own,
/// which the wrapper reads the moment the worker calls it. Every
/// import and export of both modules is a function over number types
/// and `funcref`; the tags, the continuation types, and the tables of
/// continuations stay inside the base module.
///
/// # The host-suspension form
///
/// The host-suspension form, [`SwitchForm::HostSuspension`], uses the
/// runtime layer's host suspension, and is one module with no base: it
/// keeps no suspended thread itself, because the backend keeps each
/// suspended call. A shim suspends by calling one more import,
/// `suspend`, which the host makes as a suspending host function that
/// answers "not yet". A thread starts as a resumable call of the
/// form's start, which calls the entry wrapper directly, so the stack
/// a resumable call begins holds only WebAssembly frames from its
/// start to the shim:
///
/// ```text
/// export start(thread, entry, args):
///     $entries[thread] = entry
///     entry wrapper(thread, args)
///
/// suspend in a shim:  host.suspend()   // a suspending host function
/// ```
///
/// The resumable call then ends suspended, and the host resumes the
/// thread by resuming that call. The resumed shim tries the built-in
/// again, as in the other form. The shim calls `suspend` only when the
/// built-in is not ready, because a call of a suspending import can
/// always suspend: in the browser, where the backend fills host
/// suspension with JavaScript Promise Integration, Chromium 147
/// suspends even when the function answers a plain value or a resolved
/// promise, although the proposal's overview states the opposite. A
/// resumable call in the browser ends only once a promise settles, and
/// a frame that is not WebAssembly between the start of the stack and
/// the suspension traps. The entry wrapper in WebAssembly is therefore
/// how the host learns at once that an entry finished. The form has no
/// resume export: a thread resumes when its call resumes.
///
/// The host-suspension form imports from the module `host` the tries,
/// the finishes, and the `finished` recorders the extension module
/// does, and `suspend`, of type `[] -> []`, last. It exports `shim{i}`
/// for each shim and `start{j}` for each entry type. `start{j}` takes
/// the thread index, the entry as a `funcref`, and the entry's
/// parameters, and answers nothing. The entry reaches the wrapper
/// through a table of functions, indexed by thread, which the start
/// fills and grows on demand.
#[derive(Clone, Debug)]
pub struct SwitchModule {
    form: SwitchForm,
    shims: Vec<FuncType>,
    entries: Vec<FuncType>,
}

// The base module's types, by index.

/// `[] -> []`: the type of `$block`, of `suspend`, and of a thread
/// that waits in the table.
const UNIT: u32 = 0;
/// The continuation type over [`UNIT`]: a suspended thread.
const PARKED: u32 = 1;
/// `[i32] -> []`: a worker, and every entry wrapper.
const WORKER: u32 = 2;
/// The continuation type over [`WORKER`]: a worker, fresh or idle.
const IDLE: u32 = 3;
/// `[] -> [i32]`: the type of `$done`.
const NEXT: u32 = 4;
/// `[i32 funcref] -> [i32]`: the type of `start`.
const START: u32 = 5;
/// `[i32] -> [i32]`: the type of `resume`.
const RESUME: u32 = 6;

// The base module's tags, tables, global, and functions, by index.

/// The tag a shim suspends on.
const BLOCK: u32 = 0;
/// The tag a worker suspends on once its entry finished.
const DONE: u32 = 1;
/// The table of suspended threads, one slot per thread.
const THREADS: u32 = 0;
/// The table of entry wrappers, one slot per thread.
const ENTRIES: u32 = 1;
/// The pool of idle workers.
const POOL: u32 = 2;
/// How many workers the pool holds.
const POOLED: u32 = 0;
/// How many workers the module has made.
const MADE: u32 = 1;
/// The worker function.
const WORKER_FUNC: u32 = 0;

impl SwitchModule {
    /// What a start or a resume of the stack-switching form answers
    /// when the thread finished.
    pub const FINISHED: i32 = 0;

    /// What a start or a resume of the stack-switching form answers
    /// when the thread suspended.
    pub const SUSPENDED: i32 = 1;

    /// What a try answers when the store its thread runs in was
    /// dropped while a resume of the thread was under way. The shim
    /// traps on it, so the stack unwinds where it suspended.
    pub const DROPPED: i32 = -1;

    /// A module of `form` with no shims and no entry wrappers: an
    /// extension module in the stack-switching form.
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

    /// Encode the base module of the stack-switching form, which is
    /// the same for every store.
    pub fn base() -> Vec<u8> {
        let mut types = TypeSection::new();
        types.ty().function([], []);
        types.ty().cont(&ContType(UNIT));
        types.ty().function([ValType::I32], []);
        types.ty().cont(&ContType(WORKER));
        types.ty().function([], [ValType::I32]);
        types
            .ty()
            .function([ValType::I32, ValType::FUNCREF], [ValType::I32]);
        types.ty().function([ValType::I32], [ValType::I32]);

        let mut functions = FunctionSection::new();
        functions.function(WORKER);
        functions.function(START);
        functions.function(RESUME);
        functions.function(UNIT);
        functions.function(NEXT);

        let mut tables = TableSection::new();
        tables.table(table_of(concrete(PARKED, true)));
        tables.table(table_of(RefType::FUNCREF));
        tables.table(table_of(concrete(IDLE, true)));

        let mut tags = TagSection::new();
        tags.tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: UNIT,
        });
        tags.tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: NEXT,
        });

        let mut globals = GlobalSection::new();
        for _ in [POOLED, MADE] {
            globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(0),
            );
        }

        let mut exports = ExportSection::new();
        exports.export("start", ExportKind::Func, 1);
        exports.export("resume", ExportKind::Func, 2);
        exports.export("suspend", ExportKind::Func, 3);
        exports.export("workers", ExportKind::Func, 4);

        let mut elements = ElementSection::new();
        elements.declared(Elements::Functions(Cow::Owned(vec![WORKER_FUNC])));

        let mut code = CodeSection::new();
        code.function(&worker_body());
        code.function(&base_start_body());
        code.function(&base_resume_body());
        let mut suspend = Function::new([]);
        suspend.instruction(&Instruction::Suspend(BLOCK));
        suspend.instruction(&Instruction::End);
        code.function(&suspend);
        let mut workers = Function::new([]);
        workers.instruction(&Instruction::GlobalGet(MADE));
        workers.instruction(&Instruction::End);
        code.function(&workers);

        let mut module = Module::new();
        module
            .section(&types)
            .section(&functions)
            .section(&tables)
            .section(&tags)
            .section(&globals)
            .section(&exports)
            .section(&elements)
            .section(&code);
        module.finish()
    }

    /// Encode the module this value describes: an extension module in
    /// the stack-switching form, and the whole module in the host-suspension form.
    pub fn encode(&self) -> Vec<u8> {
        match self.form {
            SwitchForm::StackSwitching => self.encode_extension(),
            SwitchForm::HostSuspension => self.encode_host_suspension(),
        }
    }

    /// Encode the extension module of the stack-switching form.
    fn encode_extension(&self) -> Vec<u8> {
        let layout = Layout::of(self);
        let mut types = TypeSection::new();
        let mut imports = ImportSection::new();
        let mut functions = FunctionSection::new();
        let mut globals = GlobalSection::new();
        let mut exports = ExportSection::new();
        let mut code = CodeSection::new();

        // 0: the base module's `start`, 1: its `suspend`, 2: every
        // wrapper's type.
        types
            .ty()
            .function([ValType::I32, ValType::FUNCREF], [ValType::I32]);
        types.ty().function([], []);
        types.ty().function([ValType::I32], []);
        let (base_start_type, suspend_type, wrapper_type) = (0, 1, 2);

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
                    finished: finished_type,
                    start: start_type,
                }
            })
            .collect::<Vec<_>>();

        imports.import("base", "start", EntityType::Function(base_start_type));
        imports.import("base", "suspend", EntityType::Function(suspend_type));
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
        tables.table(table_of(RefType::FUNCREF));

        // One global per parameter of each entry type: the start
        // leaves the entry's arguments there, and the wrapper reads
        // them as it begins.
        let mut argument_globals = Vec::with_capacity(self.entries.len());
        for ty in &self.entries {
            let first = globals.len();
            for param in params(ty) {
                globals.global(
                    GlobalType {
                        val_type: param,
                        mutable: true,
                        shared: false,
                    },
                    &zero(param),
                );
            }
            argument_globals.push(first);
        }

        for (i, (ty, (_, shim_type))) in self.shims.iter().zip(&shim_types).enumerate() {
            let i = index(i);
            functions.function(*shim_type);
            code.function(&shim_body(
                ty,
                layout.try_import(i),
                layout.finish_import(i),
                BASE_SUSPEND,
            ));
            exports.export(&format!("shim{i}"), ExportKind::Func, layout.shim(i));
        }
        for (j, (ty, entry)) in self.entries.iter().zip(&entry_types).enumerate() {
            functions.function(wrapper_type);
            code.function(&wrapper_body(
                ty,
                entry.entry,
                argument_globals[j],
                layout.finished_import(index(j)),
            ));
        }
        for (j, (ty, entry)) in self.entries.iter().zip(&entry_types).enumerate() {
            let j = index(j);
            functions.function(entry.start);
            code.function(&start_body(
                ty,
                argument_globals[j as usize],
                layout.wrapper(j),
            ));
            exports.export(&format!("start{j}"), ExportKind::Func, layout.start(j));
        }

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
            .section(&globals)
            .section(&exports)
            .section(&elements)
            .section(&code);
        module.finish()
    }
}

/// The type indices one entry type adds to an extension module.
struct EntryTypes {
    /// The entry's own type, which the wrapper calls it through.
    entry: u32,
    /// The type of the `finished` import: the thread index, then the
    /// entry's results.
    finished: u32,
    /// The type of the start export.
    start: u32,
}

/// Where each function of an extension module lands in the function
/// index space: the two base imports and the host imports first, then
/// the shims, the entry wrappers, and the starts.
struct Layout {
    shims: u32,
    entries: u32,
}

/// The base module's `start`, as an extension module imports it.
const BASE_START: u32 = 0;
/// The base module's `suspend`, as an extension module imports it.
const BASE_SUSPEND: u32 = 1;
/// How many functions an extension module imports from the base.
const BASE_IMPORTS: u32 = 2;

/// The table of entries of an extension module, one slot per thread.
const LOCAL_ENTRIES: u32 = 0;

impl Layout {
    fn of(module: &SwitchModule) -> Self {
        Self {
            shims: index(module.shims.len()),
            entries: index(module.entries.len()),
        }
    }

    fn try_import(&self, shim: u32) -> u32 {
        BASE_IMPORTS + 2 * shim
    }

    fn finish_import(&self, shim: u32) -> u32 {
        BASE_IMPORTS + 2 * shim + 1
    }

    fn finished_import(&self, entry: u32) -> u32 {
        BASE_IMPORTS + 2 * self.shims + entry
    }

    fn imports(&self) -> u32 {
        BASE_IMPORTS + 2 * self.shims + self.entries
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
}

/// The body of the base module's worker. Local 0 is the index of the
/// thread the worker runs. It calls the thread's wrapper, then
/// suspends on `$done` for the index of the next thread, for ever.
fn worker_body() -> Function {
    let mut body = Function::new([]);
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::CallIndirect {
        type_index: WORKER,
        table_index: ENTRIES,
    });
    body.instruction(&Instruction::Suspend(DONE));
    body.instruction(&Instruction::LocalSet(0));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body
}

/// The body of the base module's `start`. Local 0 is the thread
/// index and local 1 the thread's wrapper. Local 2 holds the worker
/// the thread runs on, and locals 3 and 4 what `run` parks.
fn base_start_body() -> Function {
    let worker = 2;
    let mut body = Function::new([
        (1, ValType::Ref(concrete(IDLE, true))),
        (1, ValType::Ref(concrete(PARKED, true))),
        (1, ValType::Ref(concrete(IDLE, true))),
    ]);
    grow_to_hold(&mut body, THREADS, HeapType::Concrete(PARKED), |body| {
        body.instruction(&Instruction::LocalGet(0));
    });
    grow_to_hold(&mut body, ENTRIES, HeapType::FUNC, |body| {
        body.instruction(&Instruction::LocalGet(0));
    });
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::TableSet(ENTRIES));

    // An idle worker when the pool holds one, and a fresh one
    // otherwise.
    body.instruction(&Instruction::GlobalGet(POOLED));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::GlobalGet(POOLED));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::GlobalSet(POOLED));
    body.instruction(&Instruction::GlobalGet(POOLED));
    body.instruction(&Instruction::TableGet(POOL));
    body.instruction(&Instruction::LocalSet(worker));
    body.instruction(&Instruction::GlobalGet(POOLED));
    body.instruction(&Instruction::RefNull(HeapType::Concrete(IDLE)));
    body.instruction(&Instruction::TableSet(POOL));
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::RefFunc(WORKER_FUNC));
    body.instruction(&Instruction::ContNew(IDLE));
    body.instruction(&Instruction::LocalSet(worker));
    body.instruction(&Instruction::GlobalGet(MADE));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::GlobalSet(MADE));
    body.instruction(&Instruction::End);

    run(&mut body, IDLE, |body| {
        body.instruction(&Instruction::LocalGet(0));
        body.instruction(&Instruction::LocalGet(worker));
    });
    body
}

/// The body of the base module's `resume`. Local 0 is the thread
/// index, and local 1 holds the continuation taken out of the
/// thread's slot. The slot is emptied before the thread runs, so a
/// second resume of a thread that is not suspended traps on the empty
/// slot.
fn base_resume_body() -> Function {
    let taken = 1;
    // Local 2 is unused, so that `run` finds its two locals at 3 and
    // 4 in both functions.
    let mut body = Function::new([
        (1, ValType::Ref(concrete(PARKED, true))),
        (1, ValType::I32),
        (1, ValType::Ref(concrete(PARKED, true))),
        (1, ValType::Ref(concrete(IDLE, true))),
    ]);
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::TableGet(THREADS));
    body.instruction(&Instruction::LocalSet(taken));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::RefNull(HeapType::Concrete(PARKED)));
    body.instruction(&Instruction::TableSet(THREADS));
    run(&mut body, PARKED, |body| {
        body.instruction(&Instruction::LocalGet(taken));
    });
    body
}

/// The common tail of the base module's `start` and `resume`: resume
/// the continuation `operands` push, of type `cont_type`, with a
/// handler for each tag. The two locals after the function's first
/// two hold what a handler receives.
///
/// A continuation that suspends on `$block` parks in the thread's
/// slot, and the function answers [`SwitchModule::SUSPENDED`]. A
/// worker that suspends on `$done` finished its entry, and it joins
/// the pool of idle workers, growing the pool when it is full; the
/// function answers [`SwitchModule::FINISHED`]. A worker never
/// returns, so the resumption itself never falls through.
fn run(body: &mut Function, cont_type: u32, operands: impl FnOnce(&mut Function)) {
    let (parked, idle) = (3, 4);
    body.instruction(&Instruction::Block(BlockType::Result(ValType::Ref(
        concrete(IDLE, false),
    ))));
    body.instruction(&Instruction::Block(BlockType::Result(ValType::Ref(
        concrete(PARKED, false),
    ))));
    operands(body);
    body.instruction(&Instruction::Resume {
        cont_type_index: cont_type,
        resume_table: Cow::Owned(vec![
            Handle::OnLabel {
                tag: BLOCK,
                label: 0,
            },
            Handle::OnLabel {
                tag: DONE,
                label: 1,
            },
        ]),
    });
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    // Suspended on `$block`.
    body.instruction(&Instruction::LocalSet(parked));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(parked));
    body.instruction(&Instruction::TableSet(THREADS));
    body.instruction(&Instruction::I32Const(SwitchModule::SUSPENDED));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);
    // Suspended on `$done`.
    body.instruction(&Instruction::LocalSet(idle));
    grow_to_hold(body, POOL, HeapType::Concrete(IDLE), |body| {
        body.instruction(&Instruction::GlobalGet(POOLED));
    });
    body.instruction(&Instruction::GlobalGet(POOLED));
    body.instruction(&Instruction::LocalGet(idle));
    body.instruction(&Instruction::TableSet(POOL));
    body.instruction(&Instruction::GlobalGet(POOLED));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::GlobalSet(POOLED));
    body.instruction(&Instruction::I32Const(SwitchModule::FINISHED));
    body.instruction(&Instruction::End);
}

/// The body of a shim of type `ty`, over its try and finish imports,
/// which suspends by calling `suspend`: the base module's `suspend`
/// in the stack-switching form, and the suspending host function
/// import in the host-suspension form. A try that answers
/// [`DROPPED`](SwitchModule::DROPPED) makes it trap, which no guest
/// handler catches, so the stack unwinds at once and runs no guest
/// code on the way.
fn shim_body(ty: &FuncType, try_import: u32, finish_import: u32, suspend: u32) -> Function {
    let arguments = index(ty.params().len());
    let answer = arguments;
    let mut body = Function::new([(1, ValType::I32)]);
    body.instruction(&Instruction::Loop(BlockType::Empty));
    for local in 0..arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::Call(try_import));
    body.instruction(&Instruction::LocalTee(answer));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32LtS);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(answer));
    body.instruction(&Instruction::If(BlockType::Empty));
    for local in 0..arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::Call(finish_import));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Call(suspend));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body
}

/// The body of the entry wrapper for entries of type `ty`. Local 0
/// is the thread index. The entry's arguments wait in the globals
/// from `arguments` on, and the entry in the thread's slot of the
/// module's table. The entry's results are spilled to locals of their
/// own, so that the `finished` import receives the thread index
/// first.
fn wrapper_body(ty: &FuncType, entry_type: u32, arguments: u32, finished_import: u32) -> Function {
    let spilled = 1;
    let mut body = Function::new(results(ty).map(|ty| (1, ty)));
    for global in 0..index(ty.params().len()) {
        body.instruction(&Instruction::GlobalGet(arguments + global));
    }
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::CallIndirect {
        type_index: entry_type,
        table_index: LOCAL_ENTRIES,
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
/// follow. The entry goes in the thread's slot of the module's table
/// and the parameters in the globals from `arguments` on, and the
/// base module's `start` runs the wrapper as the thread.
fn start_body(ty: &FuncType, arguments: u32, wrapper: u32) -> Function {
    let mut body = Function::new([]);
    grow_to_hold(&mut body, LOCAL_ENTRIES, HeapType::FUNC, |body| {
        body.instruction(&Instruction::LocalGet(0));
    });
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::TableSet(LOCAL_ENTRIES));
    for param in 0..index(ty.params().len()) {
        body.instruction(&Instruction::LocalGet(2 + param));
        body.instruction(&Instruction::GlobalSet(arguments + param));
    }
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::RefFunc(wrapper));
    body.instruction(&Instruction::Call(BASE_START));
    body.instruction(&Instruction::End);
    body
}

/// Grow `table` with nulls of `heap` until it holds the index the
/// instructions of `slot` push.
fn grow_to_hold(body: &mut Function, table: u32, heap: HeapType, slot: impl Fn(&mut Function)) {
    slot(body);
    body.instruction(&Instruction::TableSize(table));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::RefNull(heap));
    slot(body);
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::TableSize(table));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::TableGrow(table));
    body.instruction(&Instruction::Drop);
    body.instruction(&Instruction::End);
}

/// A reference to the base module's type `ty`.
fn concrete(ty: u32, nullable: bool) -> RefType {
    RefType {
        nullable,
        heap_type: HeapType::Concrete(ty),
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

/// The zero of a value type, which a global starts with.
fn zero(ty: ValType) -> ConstExpr {
    match ty {
        ValType::I32 => ConstExpr::i32_const(0),
        ValType::I64 => ConstExpr::i64_const(0),
        ValType::F32 => ConstExpr::f32_const(0.0f32.into()),
        ValType::F64 => ConstExpr::f64_const(0.0f64.into()),
        ValType::V128 => ConstExpr::v128_const(0),
        ValType::Ref(reference) => ConstExpr::ref_null(reference.heap_type),
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
///
/// The types of a switch module are those of the canonical ABI's flat
/// values and of the function references it passes, so a concrete
/// heap type never reaches it. The runtime layer keeps a concrete
/// type opaque, so one would become a `funcref`, and the engine would
/// refuse to link the module with a structured error.
fn value_type(ty: RuntimeValType) -> ValType {
    match ty {
        RuntimeValType::I32 => ValType::I32,
        RuntimeValType::I64 => ValType::I64,
        RuntimeValType::F32 => ValType::F32,
        RuntimeValType::F64 => ValType::F64,
        RuntimeValType::V128 => ValType::V128,
        RuntimeValType::Ref(reference) => {
            let ty = match reference.heap {
                RuntimeHeapType::Func | RuntimeHeapType::Concrete(_) => AbstractHeapType::Func,
                RuntimeHeapType::Extern => AbstractHeapType::Extern,
                RuntimeHeapType::Any => AbstractHeapType::Any,
                RuntimeHeapType::Eq => AbstractHeapType::Eq,
                RuntimeHeapType::I31 => AbstractHeapType::I31,
                RuntimeHeapType::Struct => AbstractHeapType::Struct,
                RuntimeHeapType::Array => AbstractHeapType::Array,
                RuntimeHeapType::Exn => AbstractHeapType::Exn,
                RuntimeHeapType::Cont => AbstractHeapType::Cont,
                RuntimeHeapType::NoFunc => AbstractHeapType::NoFunc,
                RuntimeHeapType::NoExtern => AbstractHeapType::NoExtern,
                RuntimeHeapType::None => AbstractHeapType::None,
                RuntimeHeapType::NoExn => AbstractHeapType::NoExn,
                RuntimeHeapType::NoCont => AbstractHeapType::NoCont,
            };
            ValType::Ref(RefType {
                nullable: reference.nullable,
                heap_type: HeapType::Abstract { shared: false, ty },
            })
        }
    }
}

/// A count or position of a module as a WebAssembly index. A switch
/// module has a handful of functions, far below the limit.
fn index(value: usize) -> u32 {
    u32::try_from(value).expect("a switch module has fewer than 2^32 functions")
}

impl SwitchModule {
    /// Encode the host-suspension form: one module, with no base, whose shims
    /// suspend through the host's `suspend` import and whose starts
    /// call the entry wrappers directly.
    fn encode_host_suspension(&self) -> Vec<u8> {
        let shims = index(self.shims.len());
        let entries = index(self.entries.len());
        // The imports are the tries and the finishes, the `finished`
        // recorders, and `suspend`; the shims, the wrappers, and the
        // starts follow them.
        let suspend_import = 2 * shims + entries;
        let imports_count = suspend_import + 1;
        let wrapper = |entry: u32| imports_count + shims + entry;

        let mut types = TypeSection::new();
        let mut imports = ImportSection::new();
        let mut functions = FunctionSection::new();
        let mut exports = ExportSection::new();
        let mut code = CodeSection::new();

        types.ty().function([], []);
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
                let finished_type = types.len();
                types
                    .ty()
                    .function(prefixed(&[ValType::I32], results(ty)), []);
                let start_type = types.len();
                types
                    .ty()
                    .function(prefixed(&[ValType::I32, ValType::FUNCREF], params(ty)), []);
                (entry_type, wrapper_type, finished_type, start_type)
            })
            .collect::<Vec<_>>();

        for (i, (try_type, shim_type)) in shim_types.iter().enumerate() {
            imports.import("host", &format!("try{i}"), EntityType::Function(*try_type));
            imports.import(
                "host",
                &format!("finish{i}"),
                EntityType::Function(*shim_type),
            );
        }
        for (j, (_, _, finished_type, _)) in entry_types.iter().enumerate() {
            imports.import(
                "host",
                &format!("finished{j}"),
                EntityType::Function(*finished_type),
            );
        }
        imports.import("host", "suspend", EntityType::Function(UNIT));

        let mut tables = TableSection::new();
        tables.table(table_of(RefType::FUNCREF));

        for (i, (ty, (_, shim_type))) in self.shims.iter().zip(&shim_types).enumerate() {
            let i = index(i);
            functions.function(*shim_type);
            code.function(&shim_body(ty, 2 * i, 2 * i + 1, suspend_import));
            exports.export(&format!("shim{i}"), ExportKind::Func, imports_count + i);
        }
        for (j, (ty, (entry_type, wrapper_type, _, _))) in
            self.entries.iter().zip(&entry_types).enumerate()
        {
            functions.function(*wrapper_type);
            code.function(&host_suspension_wrapper_body(
                ty,
                *entry_type,
                2 * shims + index(j),
            ));
        }
        for (j, (ty, (_, _, _, start_type))) in self.entries.iter().zip(&entry_types).enumerate() {
            let j = index(j);
            functions.function(*start_type);
            code.function(&resumable_start_body(ty, wrapper(j)));
            exports.export(
                &format!("start{j}"),
                ExportKind::Func,
                imports_count + shims + entries + j,
            );
        }

        let mut module = Module::new();
        module
            .section(&types)
            .section(&imports)
            .section(&functions)
            .section(&tables)
            .section(&exports)
            .section(&code);
        module.finish()
    }
}

/// The table of thread entries of the host-suspension form, its one
/// table.
const HOST_SUSPENSION_ENTRIES: u32 = 0;

/// The body of the host-suspension form's entry wrapper for entries of type
/// `ty`. Local 0 is the thread index and the entry's parameters follow
/// it; the entry's results are spilled to locals of their own, so that
/// the `finished` import receives the thread index first.
fn host_suspension_wrapper_body(ty: &FuncType, entry_type: u32, finished_import: u32) -> Function {
    let arguments = index(ty.params().len());
    let spilled = 1 + arguments;
    let mut body = Function::new(results(ty).map(|ty| (1, ty)));
    for local in 1..=arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::CallIndirect {
        type_index: entry_type,
        table_index: HOST_SUSPENSION_ENTRIES,
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

/// The body of the host-suspension start for entries of type `ty`, which the
/// host calls as a resumable call. Local 0 is the thread
/// index, local 1 the entry, and the entry's parameters follow. It
/// puts the entry in the thread's slot of the table and calls the
/// wrapper, so the stack the resumable call begins holds only
/// WebAssembly frames.
fn resumable_start_body(ty: &FuncType, wrapper: u32) -> Function {
    let arguments = index(ty.params().len());
    let mut body = Function::new([]);
    grow_to_hold(&mut body, HOST_SUSPENSION_ENTRIES, HeapType::FUNC, |body| {
        body.instruction(&Instruction::LocalGet(0));
    });
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::TableSet(HOST_SUSPENSION_ENTRIES));
    body.instruction(&Instruction::LocalGet(0));
    for local in 2..2 + arguments {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::Call(wrapper));
    body.instruction(&Instruction::End);
    body
}

#[cfg(test)]
mod tests {
    use crate::runtime_layer::{ExternType as RuntimeExternType, Module as RuntimeModule};

    use super::*;
    use crate::Engine;
    use crate::internal::EngineInternal;

    fn i32_to_i32() -> FuncType {
        FuncType::new([RuntimeValType::I32], [RuntimeValType::I32])
    }

    #[wcmp_macros::test]
    fn it_encodes_a_host_suspension_form_that_needs_no_stack_switching() {
        // The host-suspension form has no tag, no continuation type, and no
        // table of continuations, so every engine compiles it, the
        // browser included, whether or not it switches stacks.
        let mut module = SwitchModule::new(SwitchForm::HostSuspension);
        module.shim(i32_to_i32());
        module.entry(i32_to_i32());
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let compiled = RuntimeModule::new(engine.inner(), &module.encode())
            .expect("the engine compiles the host-suspension form");

        let mut imports = compiled
            .imports()
            .map(|import| format!("{}.{}", import.module(), import.name()))
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
            .exports()
            .map(|export| export.name().to_owned())
            .collect::<Vec<_>>();
        exports.sort();
        assert_eq!(
            exports,
            ["shim0", "start0"],
            "the form has no resume: a thread resumes when its call resumes"
        );
        assert_eq!(
            compiled
                .exports()
                .find(|export| export.name() == "start0")
                .map(|export| export.ty().clone()),
            Some(RuntimeExternType::Func(FuncType::new(
                [
                    RuntimeValType::I32,
                    RuntimeValType::FUNCREF,
                    RuntimeValType::I32
                ],
                []
            ))),
            "a start takes the thread index, the entry, and the entry's \
             parameters, and answers nothing"
        );
    }
}

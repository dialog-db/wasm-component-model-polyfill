// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The table `thread.new-indirect` reads start functions out of.
//!
//! The built-in names a start function by its index in a core table,
//! and the reference reads the function and checks its type when the
//! thread is created. The runtime layer cannot do either in the
//! browser: its table `get` there answers nothing, and a function
//! reference it hands the host carries no signature, because the
//! JavaScript API says nothing of one.
//!
//! So the polyfill asks WebAssembly. It carries a small core module,
//! the probe, and gives each extracted table an instance of it that
//! imports the table. The probe reads the entry and tells its type
//! with `ref.test`, and it hands the entry back as a `funcref`
//! result, which both backends turn into a function the host can
//! call. `ref.test` of a function type is part of the GC proposal,
//! which every current browser ships and Wasmtime enables by
//! default. Both take the same path, so a native run over Wasmtime
//! tests the one the browser takes.
//!
//! A backend without `gc` cannot compile the probe, and does not need
//! it: every native backend reads a table entry and the type of the
//! function in it. Without GC there are no recursion groups and no
//! subtypes, so two function types are the same exactly where their
//! parameters and results are, which is how `call_indirect` compares
//! them there. The host reads the entry and compares its type itself.

use anyhow::anyhow;

use crate::abi::layout::FlatType;
use crate::error::{Error, ThreadCause};
use crate::runtime_layer::{
    AsContextMut, Extern as RuntimeExtern, Func as RuntimeFunc, Instance as RuntimeInstance,
    Module as RuntimeModule, RuntimeError, Table, Val as RuntimeVal, ValType, into_anyhow,
};

/// The probe, as a core module binary. Its text is:
///
/// ```wat
/// (module
///   (type (func (param i32)))
///   (type (func (param i64)))
///   (type (func (param i32) (result i32)))
///   (type (func (param i32) (result funcref)))
///   (import "" "table" (table 0 funcref))
///   (func (type 2) (local funcref)
///     local.get 0
///     table.get 0
///     local.tee 1
///     ref.is_null
///     if
///       i32.const 0
///       return
///     end
///     local.get 1
///     ref.test (ref 0)
///     if
///       i32.const 1
///       return
///     end
///     local.get 1
///     ref.test (ref 1)
///     if
///       i32.const 2
///       return
///     end
///     i32.const 3)
///   (func (type 3)
///     local.get 0
///     table.get 0)
///   (export "classify" (func 0))
///   (export "get" (func 1)))
/// ```
///
/// `classify` answers what the entry at an index holds: [`EMPTY`], a
/// function of type `(i32) -> ()` ([`TAKES_I32`]), one of type
/// `(i64) -> ()` ([`TAKES_I64`]), or a function of any other type
/// ([`OTHER_TYPE`]). A type is compared the way `call_indirect`
/// compares it, by its canonical identity across modules. `get`
/// answers the entry itself.
pub const THREAD_START_PROBE: &[u8] = &[
    // The preamble.
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
    // The type section: the two start function types, then the types
    // of `classify` and `get`.
    0x01, 0x13, 0x04, //
    0x60, 0x01, 0x7f, 0x00, //
    0x60, 0x01, 0x7e, 0x00, //
    0x60, 0x01, 0x7f, 0x01, 0x7f, //
    0x60, 0x01, 0x7f, 0x01, 0x70, //
    // The import section: the table, as `"" "table"`.
    0x02, 0x0c, 0x01, 0x00, 0x05, b't', b'a', b'b', b'l', b'e', 0x01, 0x70, 0x00, 0x00,
    // The function section.
    0x03, 0x03, 0x02, 0x02, 0x03, //
    // The export section.
    0x07, 0x12, 0x02, //
    0x08, b'c', b'l', b'a', b's', b's', b'i', b'f', b'y', 0x00, 0x00, //
    0x03, b'g', b'e', b't', 0x00, 0x01, //
    // The code section: `classify`, then `get`.
    0x0a, 0x32, 0x02, //
    0x29, 0x01, 0x01, 0x70, //
    0x20, 0x00, 0x25, 0x00, 0x22, 0x01, 0xd1, 0x04, 0x40, 0x41, 0x00, 0x0f, 0x0b, //
    0x20, 0x01, 0xfb, 0x14, 0x00, 0x04, 0x40, 0x41, 0x01, 0x0f, 0x0b, //
    0x20, 0x01, 0xfb, 0x14, 0x01, 0x04, 0x40, 0x41, 0x02, 0x0f, 0x0b, //
    0x41, 0x03, 0x0b, //
    0x06, 0x00, 0x20, 0x00, 0x25, 0x00, 0x0b,
];

/// What `classify` answers for an entry that holds no function.
const EMPTY: i32 = 0;

/// What `classify` answers for a function of type `(i32) -> ()`.
const TAKES_I32: i32 = 1;

/// What `classify` answers for a function of type `(i64) -> ()`.
const TAKES_I64: i32 = 2;

/// What `classify` answers for a function of any other type.
const OTHER_TYPE: i32 = 3;

/// One table a `thread.new-indirect` reads start functions out of,
/// with the probe instance that reads it, or none where the host
/// reads it.
#[derive(Clone)]
pub struct ThreadStartTable {
    table: Table,
    probe: Option<(RuntimeFunc, RuntimeFunc)>,
}

impl ThreadStartTable {
    /// Give `table` an instance of `probe`, the compiled
    /// [`THREAD_START_PROBE`], or, with no probe, have the host read
    /// it, over a backend without `gc`.
    pub async fn new(
        mut store: impl AsContextMut,
        probe: Option<&RuntimeModule>,
        table: Table,
    ) -> Result<Self, RuntimeError> {
        let Some(probe) = probe else {
            return Ok(Self { table, probe: None });
        };
        let instance = RuntimeInstance::instantiate(&mut store, probe, &[table.into()]).await?;
        let mut export = |name: &str| match instance.get_export(&mut store, name)? {
            Some(RuntimeExtern::Func(func)) => Ok(func),
            _ => Err(RuntimeError::Backend {
                message: format!("the thread start probe exports no `{name}` function"),
            }),
        };
        Ok(Self {
            probe: Some((export("classify")?, export("get")?)),
            table,
        })
    }

    /// Read the start function at `index`, which takes a context
    /// value of type `context`: an `i32`, or an `i64` in a 64-bit
    /// memory.
    ///
    /// The reference traps when the index is out of bounds, when the
    /// entry holds no function, and when the function is not of the
    /// type `(context) -> ()`. Each fails here with Wasmtime's
    /// message for it. A failure of the probe itself is the
    /// substrate's, and comes back as the outer error.
    pub fn start_function(
        &self,
        mut store: impl AsContextMut,
        index: u32,
        context: FlatType,
    ) -> anyhow::Result<Result<RuntimeFunc, Error>> {
        if u64::from(index) >= self.table.size(&store).map_err(into_anyhow)? {
            return Ok(Err(Error::Thread(ThreadCause::StartFunctionOutOfBounds)));
        }
        let Some((classify, get)) = &self.probe else {
            return self.read_on_the_host(store, index, context);
        };
        let mut class = [RuntimeVal::I32(0)];
        classify
            .call(&mut store, &[RuntimeVal::I32(index as i32)], &mut class)
            .map_err(into_anyhow)?;
        let matches = match (&class[0], context) {
            (RuntimeVal::I32(EMPTY), _) => {
                return Ok(Err(Error::Thread(ThreadCause::StartFunctionUninitialized)));
            }
            (RuntimeVal::I32(TAKES_I32), FlatType::I32) => true,
            (RuntimeVal::I32(TAKES_I64), FlatType::I64) => true,
            (RuntimeVal::I32(TAKES_I32 | TAKES_I64 | OTHER_TYPE), _) => false,
            _ => return Err(anyhow!("the thread start probe answered no class")),
        };
        if !matches {
            return Ok(Err(Error::Thread(ThreadCause::StartFunctionType)));
        }
        let mut entry = [RuntimeVal::FuncRef(None)];
        get.call(&mut store, &[RuntimeVal::I32(index as i32)], &mut entry)
            .map_err(into_anyhow)?;
        match entry {
            [RuntimeVal::FuncRef(Some(function))] => Ok(Ok(function)),
            _ => Err(anyhow!(
                "the thread start probe handed back no function for an entry it classified"
            )),
        }
    }

    /// [`start_function`](Self::start_function) without the probe: the
    /// host reads the entry at `index`, which is in bounds, and
    /// compares the parameters and results of its function with
    /// `(context) -> ()`.
    fn read_on_the_host(
        &self,
        mut store: impl AsContextMut,
        index: u32,
        context: FlatType,
    ) -> anyhow::Result<Result<RuntimeFunc, Error>> {
        let entry = self
            .table
            .get(&mut store, u64::from(index))
            .map_err(into_anyhow)?;
        let function = match entry {
            RuntimeVal::FuncRef(Some(function)) => function,
            RuntimeVal::FuncRef(None) => {
                return Ok(Err(Error::Thread(ThreadCause::StartFunctionUninitialized)));
            }
            _ => return Err(anyhow!("a thread start table holds no function references")),
        };
        let ty = function
            .ty(&store)
            .map_err(into_anyhow)?
            .ok_or_else(|| anyhow!("the backend does not know the type of a table entry"))?;
        let param = match context {
            FlatType::I32 => ValType::I32,
            FlatType::I64 => ValType::I64,
            _ => return Ok(Err(Error::Thread(ThreadCause::StartFunctionType))),
        };
        if ty.params() == [param] && ty.results().is_empty() {
            Ok(Ok(function))
        } else {
            Ok(Err(Error::Thread(ThreadCause::StartFunctionType)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::compile_module;
    use crate::internal::EngineInternal;
    use crate::runtime_layer::Capability;
    use crate::store::StoreInternalExt;
    use crate::{Engine, Store};

    /// The probe's text, as the documentation of
    /// [`THREAD_START_PROBE`] gives it.
    const PROBE_TEXT: &[u8] = wcmp_macros::wasm!(
        r#"
        (module
          (type (func (param i32)))
          (type (func (param i64)))
          (type (func (param i32) (result i32)))
          (type (func (param i32) (result funcref)))
          (import "" "table" (table 0 funcref))
          (func (type 2) (local funcref)
            local.get 0
            table.get 0
            local.tee 1
            ref.is_null
            if
              i32.const 0
              return
            end
            local.get 1
            ref.test (ref 0)
            if
              i32.const 1
              return
            end
            local.get 1
            ref.test (ref 1)
            if
              i32.const 2
              return
            end
            i32.const 3)
          (func (type 3)
            local.get 0
            table.get 0)
          (export "classify" (func 0))
          (export "get" (func 1)))
        "#
    );

    /// A table of four entries: a function of each start type, one of
    /// another type, and an empty entry. Each start function stores
    /// the context it was given in the module's 64-bit memory, where
    /// `stored` reads it back.
    const STARTS: &[u8] = wcmp_macros::wasm!(
        r#"
        (module
          (memory i64 1)
          (table (export "table") 4 funcref)
          (func $takes-i32 (param i32)
            (i64.store (i64.const 0) (i64.extend_i32_u (local.get 0))))
          (func $takes-i64 (param i64)
            (i64.store (i64.const 0) (local.get 0)))
          (func $returns (param i32) (result i32) (local.get 0))
          (elem (table 0) (i32.const 0) func $takes-i32 $takes-i64 $returns)
          (func (export "stored") (result i64) (i64.load (i64.const 0))))
        "#
    );

    #[wcmp_macros::test]
    fn it_carries_the_probe_the_documentation_states() {
        assert_eq!(THREAD_START_PROBE, PROBE_TEXT);
    }

    /// Who reads a table: the probe, or the host.
    #[derive(Debug, Clone, Copy)]
    enum Reader {
        Probe,
        Host,
    }

    /// The readers the test backend runs: the probe where it declares
    /// `gc`, and natively the host, which the browser cannot be,
    /// because it does not tell the type of a table's function.
    fn readers(engine: &Engine) -> Vec<Reader> {
        let mut readers = Vec::new();
        if engine.inner().capabilities().contains(Capability::Gc) {
            readers.push(Reader::Probe);
        }
        if cfg!(not(target_arch = "wasm32")) {
            readers.push(Reader::Host);
        }
        readers
    }

    /// For each reader of the test backend, a store with an instance
    /// of [`STARTS`], its table behind the reader, and the function
    /// that reads what a start function stored.
    async fn starts() -> Vec<(Reader, Store<()>, ThreadStartTable, RuntimeFunc)> {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let starts = compile_module(&engine, STARTS)
            .await
            .expect("the start functions compile");
        let mut each = Vec::new();
        for reader in readers(&engine) {
            let probe = match reader {
                Reader::Probe => Some(
                    compile_module(&engine, THREAD_START_PROBE)
                        .await
                        .expect("the probe compiles"),
                ),
                Reader::Host => None,
            };
            let mut store: Store<()> = Store::new(&engine, ()).expect("store");
            let runtime = store.internal().inner_mut();
            let instance = RuntimeInstance::instantiate(&mut *runtime, &starts, &[])
                .await
                .expect("the start functions instantiate");
            let Some(RuntimeExtern::Table(table)) = instance
                .get_export(&mut *runtime, "table")
                .expect("the export")
            else {
                panic!("the module exports its table");
            };
            let Some(RuntimeExtern::Func(stored)) = instance
                .get_export(&mut *runtime, "stored")
                .expect("the export")
            else {
                panic!("the module exports `stored`");
            };
            let table = ThreadStartTable::new(&mut *runtime, probe.as_ref(), table)
                .await
                .expect("the reader");
            each.push((reader, store, table, stored));
        }
        each
    }

    /// Read the entry at `index` as a start function taking
    /// `context`.
    fn read(
        store: &mut Store<()>,
        table: &ThreadStartTable,
        index: u32,
        context: FlatType,
    ) -> Result<RuntimeFunc, Error> {
        table
            .start_function(store.internal().inner_mut(), index, context)
            .expect("the reader runs")
    }

    /// Call `function` with `context` and answer what it stored.
    fn run(
        store: &mut Store<()>,
        stored: &RuntimeFunc,
        function: &RuntimeFunc,
        context: RuntimeVal,
    ) -> i64 {
        let runtime = store.internal().inner_mut();
        function
            .call(&mut *runtime, &[context], &mut [])
            .expect("the start function runs");
        let mut value = [RuntimeVal::I64(0)];
        stored
            .call(&mut *runtime, &[], &mut value)
            .expect("the stored value reads");
        match value {
            [RuntimeVal::I64(value)] => value,
            other => panic!("`stored` answered {other:?}"),
        }
    }

    #[wcmp_macros::test]
    async fn it_reads_a_start_function_that_takes_an_i32() {
        for (reader, mut store, table, stored) in starts().await {
            let function = read(&mut store, &table, 0, FlatType::I32)
                .unwrap_or_else(|err| panic!("{reader:?}: an `(i32) -> ()`: {err:?}"));
            assert_eq!(
                run(&mut store, &stored, &function, RuntimeVal::I32(7)),
                7,
                "{reader:?}"
            );
        }
    }

    #[wcmp_macros::test]
    async fn it_reads_a_start_function_that_takes_an_i64() {
        // The context of a thread in a 64-bit memory is an address in
        // it, so a value past four gigabytes has to arrive whole.
        for (reader, mut store, table, stored) in starts().await {
            let function = read(&mut store, &table, 1, FlatType::I64)
                .unwrap_or_else(|err| panic!("{reader:?}: an `(i64) -> ()`: {err:?}"));
            let context = 0x1_0000_0007_i64;
            assert_eq!(
                run(&mut store, &stored, &function, RuntimeVal::I64(context)),
                context,
                "{reader:?}"
            );
        }
    }

    #[wcmp_macros::test]
    async fn it_fails_a_start_function_of_another_type_with_wasmtimes_message() {
        for (reader, mut store, table, _) in starts().await {
            for (index, context) in [(0, FlatType::I64), (1, FlatType::I32), (2, FlatType::I32)] {
                let err = read(&mut store, &table, index, context).err();
                assert!(
                    matches!(err, Some(Error::Thread(ThreadCause::StartFunctionType))),
                    "{reader:?}: entry {index} taken as `({context:?}) -> ()` gave {err:?}"
                );
            }
        }
    }

    #[wcmp_macros::test]
    async fn it_fails_an_empty_entry_with_wasmtimes_message() {
        for (reader, mut store, table, _) in starts().await {
            let err = read(&mut store, &table, 3, FlatType::I32).err();
            assert!(
                matches!(
                    err,
                    Some(Error::Thread(ThreadCause::StartFunctionUninitialized))
                ),
                "{reader:?}: got {err:?}"
            );
        }
    }

    #[wcmp_macros::test]
    async fn it_fails_an_index_past_the_end_of_the_table() {
        for (reader, mut store, table, _) in starts().await {
            let err = read(&mut store, &table, 4, FlatType::I32).err();
            assert!(
                matches!(
                    err,
                    Some(Error::Thread(ThreadCause::StartFunctionOutOfBounds))
                ),
                "{reader:?}: got {err:?}"
            );
        }
    }
}

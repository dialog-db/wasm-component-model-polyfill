//! The `spectest` module every script can import from.

use std::collections::HashMap;

use wcmp_wasm_core::{
    Extern, Func, FuncType, Global, GlobalType, Memory, MemoryType, Mutability, RefType, Result,
    Store, Table, TableType, Val, ValType,
};

/// The externs of the `spectest` module, made in `store`, by name.
///
/// They are the ones the reference interpreter's `spectest` module
/// exports: functions that print their arguments, which print nothing
/// here, four constant globals, a table, and a memory. The table addressed
/// with 64-bit numbers needs `memory64`, and the shared memory `threads`;
/// each is left out where the backend does not declare its capability, so
/// an import of it does not link, as it would not on that engine.
pub fn externs(store: &mut Store<()>) -> Result<HashMap<&'static str, Extern>> {
    let mut externs = HashMap::new();
    let prints: [(&str, &[ValType]); 7] = [
        ("print", &[]),
        ("print_i32", &[ValType::I32]),
        ("print_i64", &[ValType::I64]),
        ("print_f32", &[ValType::F32]),
        ("print_f64", &[ValType::F64]),
        ("print_i32_f32", &[ValType::I32, ValType::F32]),
        ("print_f64_f64", &[ValType::F64, ValType::F64]),
    ];
    for (name, params) in prints {
        let ty = FuncType::new(params.iter().copied(), []);
        let func = Func::new(&mut *store, ty, |_, _, _| Ok(()))?;
        externs.insert(name, Extern::Func(func));
    }

    let globals = [
        ("global_i32", ValType::I32, Val::I32(666)),
        ("global_i64", ValType::I64, Val::I64(666)),
        ("global_f32", ValType::F32, Val::F32(666.6_f32.to_bits())),
        ("global_f64", ValType::F64, Val::F64(666.6_f64.to_bits())),
    ];
    for (name, ty, value) in globals {
        let global = Global::new(&mut *store, GlobalType::new(ty, Mutability::Const), value)?;
        externs.insert(name, Extern::Global(global));
    }

    let table = Table::new(
        &mut *store,
        TableType::new(RefType::FUNCREF, 10, Some(20)),
        Val::FuncRef(None),
    )?;
    externs.insert("table", Extern::Table(table));
    if let Ok(table) = Table::new(
        &mut *store,
        TableType::new64(RefType::FUNCREF, 10, Some(20)),
        Val::FuncRef(None),
    ) {
        externs.insert("table64", Extern::Table(table));
    }

    let memory = Memory::new(&mut *store, MemoryType::new(1, Some(2)))?;
    externs.insert("memory", Extern::Memory(memory));
    if let Ok(memory) = Memory::new(&mut *store, MemoryType::shared(1, 2)) {
        externs.insert("shared_memory", Extern::Memory(memory));
    }
    Ok(externs)
}

//! The benchmarks themselves: what is measured, on which guest, and
//! with which payload.
//!
//! Each definition is written once. The runner drives it on a native
//! host and, compiled to `wasm32-unknown-unknown`, in a browser;
//! nothing below is written per target.

use wasm_component_model_polyfill::{
    Component, Engine, Func, Instance, Linker, Store, Val, ValField,
};

use crate::benchmark::Benchmark;
use crate::error::{Error, Result};
use crate::guests;
use crate::run::Run;

/// Every benchmark of the suite, in the order a report lists them:
/// the call floor first, then the canonical ABI's heap values, then
/// handles, composition, and parsing.
pub fn benchmarks() -> Vec<Benchmark> {
    [
        u32_call(),
        u32_call_typed(),
        string_roundtrip(),
        string_roundtrip_typed(),
        list_u8_roundtrip(),
        list_u8_roundtrip_typed(),
        list_u32_roundtrip(),
        list_record_roundtrip(),
        resource_handle(),
        composition_call(),
        component_new(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Instantiate `bytes` into a store of its own, and hand back the
/// engine, the store, and the instance.
///
/// Every benchmark calls this before its loop, so building a guest is
/// never part of a sample — except in `component-new`, where parsing
/// one is the thing measured.
async fn instantiate(bytes: &[u8]) -> Result<(Engine, Store<()>, Instance)> {
    let engine = Engine::new()?;
    let component = Component::new(&engine, bytes).await?;
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ())?;
    let instance = linker.instantiate(&mut store, &component).await?;
    Ok((engine, store, instance))
}

/// The instance's export named `name`, or the reason there is none.
fn export(instance: &Instance, name: &str) -> Result<Func> {
    instance
        .get_func(name)
        .ok_or_else(|| Error::Setup(format!("the guest exports no `{name}`")))
}

#[wcmp_macros::bench(
    guest = "the `guest` corpus fixture: `double: func(x: u32) -> u32`, lifted with neither a memory nor a realloc",
    payload = "one u32 in and one u32 out, as `Val`: the floor, with no memory traffic under it"
)]
async fn u32_call(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(guests::GUEST).await?;
    let double = export(&instance, "double")?;
    let arguments = [Val::U32(21)];
    while run.iterate() {
        double.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the `guest` corpus fixture: `double: func(x: u32) -> u32`, lifted with neither a memory nor a realloc",
    payload = "the same u32 in and out through `TypedFunc`, which is the floor without the untyped `Val` path's allocation"
)]
async fn u32_call_typed(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(guests::GUEST).await?;
    let double = export(&instance, "double")?.typed::<(u32,), u32>()?;
    while run.iterate() {
        double.call(&mut store, (21,)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-string: func(s: string) -> string`, which returns the pointer and length it was given",
    payload = "a UTF-8 string of the case's bytes, lowered into guest memory and lifted back out",
    cases = [64, 4096, 65536]
)]
async fn string_roundtrip(run: &mut Run) -> Result<()> {
    let size = usize::try_from(run.case().number()).unwrap_or(usize::MAX);
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
    let echo = export(&instance, "echo-string")?;
    let arguments = [Val::String("a".repeat(size))];
    run.moves_bytes(run.case().number());
    while run.iterate() {
        echo.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-string: func(s: string) -> string`, which returns the pointer and length it was given",
    payload = "the same UTF-8 string through `TypedFunc<(String,), String>`, cloned into each call because the call takes it by value",
    cases = [64, 4096, 65536]
)]
async fn string_roundtrip_typed(run: &mut Run) -> Result<()> {
    let size = usize::try_from(run.case().number()).unwrap_or(usize::MAX);
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
    let echo = export(&instance, "echo-string")?.typed::<(String,), String>()?;
    let payload = "a".repeat(size);
    run.moves_bytes(run.case().number());
    while run.iterate() {
        echo.call(&mut store, (payload.clone(),)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-u8: func(xs: list<u8>) -> list<u8>`, which returns the pointer and length it was given",
    payload = "the case's number of u8 elements, the untyped counterpart of a string of the same bytes",
    cases = [64, 4096, 65536]
)]
async fn list_u8_roundtrip(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
    let echo = export(&instance, "echo-list-u8")?;
    let elements: Vec<Val> = (0..count).map(|index| Val::U8(index as u8)).collect();
    let arguments = [Val::List(elements.into_boxed_slice())];
    run.moves_bytes(count);
    while run.iterate() {
        echo.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-u8: func(xs: list<u8>) -> list<u8>`, which returns the pointer and length it was given",
    payload = "the case's number of u8 elements through `TypedFunc<(Vec<u8>,), Vec<u8>>`, cloned into each call because the call takes them by value",
    cases = [64, 4096, 65536]
)]
async fn list_u8_roundtrip_typed(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
    let echo = export(&instance, "echo-list-u8")?.typed::<(Vec<u8>,), Vec<u8>>()?;
    let payload: Vec<u8> = (0..count).map(|index| index as u8).collect();
    run.moves_bytes(count);
    while run.iterate() {
        echo.call(&mut store, (payload.clone(),)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-u32: func(xs: list<u32>) -> list<u32>`, which returns the pointer and length it was given",
    payload = "the case's number of u32 elements, lowered in one write of the list's bytes and lifted in one read",
    cases = [16, 256, 4096]
)]
async fn list_u32_roundtrip(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
    let echo = export(&instance, "echo-list-u32")?;
    let elements: Vec<Val> = (0..count).map(|index| Val::U32(index as u32)).collect();
    let arguments = [Val::List(elements.into_boxed_slice())];
    run.moves_elements(count);
    while run.iterate() {
        echo.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-point: func(xs: list<point>) -> list<point>` over `record point { x: u32, y: u32 }`",
    payload = "the case's number of two-field records, whose field names are carried as owned strings on every crossing",
    cases = [16, 256, 4096]
)]
async fn list_record_roundtrip(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
    let echo = export(&instance, "echo-list-point")?;
    let elements: Vec<Val> = (0..count)
        .map(|index| {
            Val::Record(
                vec![
                    ValField {
                        name: "x".to_owned(),
                        value: Val::U32(index as u32),
                    },
                    ValField {
                        name: "y".to_owned(),
                        value: Val::U32(index as u32),
                    },
                ]
                .into_boxed_slice(),
            )
        })
        .collect();
    let arguments = [Val::List(elements.into_boxed_slice())];
    run.moves_elements(count);
    while run.iterate() {
        echo.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `resource` component: `make: func(rep: u32) -> own<thing>` and `dispose: func(h: own<thing>)`, with a destructor that counts",
    payload = "one owned handle per iteration: minted by the guest, held by the host, handed back, and dropped"
)]
async fn resource_handle(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(guests::RESOURCE).await?;
    let make = export(&instance, "make")?;
    let dispose = export(&instance, "dispose")?;
    let arguments = [Val::U32(7)];
    run.moves_elements(1);
    while run.iterate() {
        let results = make.call(&mut store, &arguments).await?;
        let handle = match results.first() {
            Some(Val::Own(handle)) => *handle,
            _ => {
                return Err(Error::Setup(
                    "`make` did not return an owned handle".to_owned(),
                ));
            }
        };
        dispose.call(&mut store, &[Val::Own(handle)]).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the `composition` corpus fixture: a socket's `run: func(x: u32) -> u32` reaching a plug's `double` through the adapter `wac plug` wrote between them",
    payload = "one u32 in and one u32 out, across two component instances instead of one"
)]
async fn composition_call(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(guests::COMPOSITION).await?;
    let call_run = export(&instance, "run")?;
    let arguments = [Val::U32(20)];
    while run.iterate() {
        call_run.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the smoke test's guests, one per case: the `guest`, `composition`, `maps`, and `fixed-lists` corpus fixtures",
    payload = "one component binary, parsed and translated by `Component::new`; the bytes are the binary's own size",
    cases = ["guest", "composition", "maps", "fixed-lists"]
)]
async fn component_new(run: &mut Run) -> Result<()> {
    let bytes = match run.case().name() {
        "guest" => guests::GUEST,
        "composition" => guests::COMPOSITION,
        "maps" => guests::MAPS,
        "fixed-lists" => guests::FIXED_LISTS,
        other => {
            return Err(Error::Setup(format!(
                "`{other}` is not one of the smoke test's guests"
            )));
        }
    };
    let engine = Engine::new()?;
    run.moves_bytes(bytes.len() as u64);
    while run.iterate() {
        Component::new(&engine, bytes).await?;
    }
    Ok(())
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Memory access.

use wcmp_macros::wasm;
use wcmp_wasm_core::{Capability, Engine, Error, Extern, Memory, MemoryType, Store, Val, ValType};

use crate::support;

/// The memory `instance` exports as `memory`.
fn exported_memory<T: 'static>(store: &mut Store<T>, instance: wcmp_wasm_core::Instance) -> Memory {
    instance
        .get_export(store, "memory")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_memory)
        .expect("the instance exports its memory")
}

/// A new memory of `ty` in `store`.
fn memory<T: 'static>(store: &mut Store<T>, ty: MemoryType) -> Memory {
    Memory::new(store, ty).expect("the store makes a memory")
}

/// The bytes 1 to `N`, a pattern no fresh memory holds.
fn pattern<const N: usize>() -> [u8; N] {
    core::array::from_fn(|index| index as u8 + 1)
}

/// The host writes a memory, and the guest reads what the host wrote. The
/// guest writes it, and the host reads what the guest wrote, a byte and a
/// little-endian scalar at a time or in bulk.
pub async fn it_reads_and_writes_a_memory(engine: &Engine) {
    let mut store = support::store(engine, ());
    let instance = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (memory (export "memory") 1)
              (func (export "load") (param i32) (result i32)
                local.get 0
                i32.load)
              (func (export "store") (param i32 i32)
                local.get 0
                local.get 1
                i32.store))
            "#
        ),
        &[],
    )
    .await;
    let memory = exported_memory(&mut store, instance);
    let load = support::func(&mut store, instance, "load");
    let store_i32 = support::func(&mut store, instance, "store");

    assert_eq!(
        memory.size(&store).expect("the memory is the store's"),
        65_536
    );

    memory
        .write(&mut store, 8, &[1, 2, 3, 4])
        .expect("the range lies inside the memory");
    let loaded = support::call(&mut store, load, &[Val::I32(8)], &[ValType::I32]);
    assert_eq!(
        loaded[0].i32(),
        Some(0x0403_0201),
        "the guest reads what the host wrote"
    );

    support::call(
        &mut store,
        store_i32,
        &[Val::I32(16), Val::I32(0x1122_3344)],
        &[],
    );
    assert_eq!(memory.load_u32(&store, 16).ok(), Some(0x1122_3344));
    assert_eq!(memory.load_u16(&store, 16).ok(), Some(0x3344));
    assert_eq!(memory.load_u8(&store, 17).ok(), Some(0x33));
    let mut bytes = [0; 4];
    memory
        .read(&store, 16, &mut bytes)
        .expect("the range lies inside the memory");
    assert_eq!(
        bytes,
        [0x44, 0x33, 0x22, 0x11],
        "the host reads what the guest wrote"
    );

    memory
        .store_u64(&mut store, 24, 0x0102_0304_0506_0708)
        .expect("the range lies inside the memory");
    assert_eq!(
        memory.load_u64(&store, 24).ok(),
        Some(0x0102_0304_0506_0708)
    );
    let loaded = support::call(&mut store, load, &[Val::I32(24)], &[ValType::I32]);
    assert_eq!(
        loaded[0].i32(),
        Some(0x0506_0708),
        "a store is little-endian"
    );
    memory
        .store_u32(&mut store, 32, 0xaabb_ccdd)
        .expect("the range lies inside the memory");
    memory
        .store_u16(&mut store, 36, 0xeeff)
        .expect("the range lies inside the memory");
    memory
        .store_u8(&mut store, 38, 0x99)
        .expect("the range lies inside the memory");
    let mut bytes = [0; 8];
    memory
        .read(&store, 32, &mut bytes)
        .expect("the range lies inside the memory");
    assert_eq!(bytes, [0xdd, 0xcc, 0xbb, 0xaa, 0xff, 0xee, 0x99, 0]);
}

/// A new memory of `ty` in `store`, a type that needs `capability`, where
/// the engine declares the capability. Where it does not, the store must
/// refuse the memory with [`Error::Unsupported`] and the capability, and
/// there is no memory.
fn memory_needing<T: 'static>(
    engine: &Engine,
    store: &mut Store<T>,
    capability: Capability,
    ty: MemoryType,
) -> Option<Memory> {
    if support::declares(engine, &[capability]) {
        return Some(memory(store, ty));
    }
    let refused = Memory::new(store, ty);
    assert!(
        matches!(&refused, Err(Error::Unsupported(named)) if *named == capability),
        "a memory of {ty:?} is refused with `Unsupported` and `{capability}`: {refused:?}"
    );
    None
}

/// The memories of `minimum` pages and at most `maximum` that a test runs
/// on: an unshared memory, a shared one where the engine declares
/// `threads`, and one addressed with 64-bit numbers where it declares
/// `memory64`. Where the engine lacks either capability, the store refuses
/// that memory with `Unsupported` and the capability.
fn memories_of<T: 'static>(
    engine: &Engine,
    store: &mut Store<T>,
    minimum: u32,
    maximum: u32,
) -> Vec<(&'static str, Memory)> {
    let mut memories = vec![(
        "an unshared memory",
        memory(store, MemoryType::new(minimum, Some(maximum))),
    )];
    let shared = MemoryType::shared(minimum, maximum);
    if let Some(memory) = memory_needing(engine, store, Capability::Threads, shared) {
        memories.push(("a shared memory", memory));
    }
    let wide = MemoryType::new64(minimum.into(), Some(maximum.into()));
    if let Some(memory) = memory_needing(engine, store, Capability::Memory64, wide) {
        memories.push(("a 64-bit memory", memory));
    }
    memories
}

/// A memory grows by whole pages up to its maximum, and returns its old
/// size. The new page holds what the host writes to it. A growth past the
/// maximum is a structured error, and leaves the memory as it was. That
/// holds for an unshared memory, a shared one, and one addressed with
/// 64-bit numbers.
pub async fn it_grows_a_memory_up_to_its_maximum(engine: &Engine) {
    let mut store = support::store(engine, ());
    for (name, memory) in memories_of(engine, &mut store, 1, 2) {
        assert_eq!(memory.grow(&mut store, 1).ok(), Some(1), "{name}");
        assert_eq!(memory.size(&store).ok(), Some(2 * 65_536), "{name}");
        memory
            .write(&mut store, 65_536 + 8, &[1, 2, 3, 4])
            .expect("the new page lies inside the memory");
        assert_eq!(
            memory.load_u32(&store, 65_536 + 8).ok(),
            Some(0x0403_0201),
            "{name}: the new page holds what the host wrote"
        );

        let refused = memory.grow(&mut store, 1);
        assert!(
            matches!(refused, Err(Error::Grow { delta: 1 })),
            "{name}: {refused:?}"
        );
        assert_eq!(memory.size(&store).ok(), Some(2 * 65_536), "{name}");
    }
}

/// The memories each test of ranges runs on, each of one page. See
/// [`memories_of`].
fn memories<T: 'static>(engine: &Engine, store: &mut Store<T>) -> Vec<(&'static str, Memory)> {
    memories_of(engine, store, 1, 1)
}

/// `with_bytes` lends a closure the bytes of the range asked for, and
/// hands back what the closure returns. A range of no bytes at the end of
/// the memory lends nothing, and is not an error.
pub async fn it_lends_the_bytes_of_a_range(engine: &Engine) {
    let mut store = support::store(engine, ());
    for (name, memory) in memories(engine, &mut store) {
        memory
            .write(&mut store, 100, &pattern::<16>())
            .expect("the range lies inside the memory");

        let lent = memory
            .with_bytes(&store, 100, 16, <[u8]>::to_vec)
            .expect("the range lies inside the memory");
        assert_eq!(lent, pattern::<16>(), "{name}");
        let lent = memory
            .with_bytes(&store, 104, 4, <[u8]>::to_vec)
            .expect("the range lies inside the memory");
        assert_eq!(lent, [5, 6, 7, 8], "{name}");
        let empty = memory
            .with_bytes(&store, 65_536, 0, |bytes| bytes.len())
            .expect("an empty range at the end lies inside the memory");
        assert_eq!(empty, 0, "{name}");
    }
}

/// Whether `result` is [`Error::MemoryOutOfBounds`] for `len` bytes at
/// `offset` of a memory of `size` bytes.
fn out_of_bounds<T: core::fmt::Debug>(
    result: wcmp_wasm_core::Result<T>,
    offset: u64,
    len: u64,
    size: u64,
) -> Result<(), String> {
    match result {
        Err(Error::MemoryOutOfBounds {
            offset: at,
            len: length,
            size: of,
        }) if (at, length, of) == (offset, len, size) => Ok(()),
        other => Err(format!(
            "{len} bytes at {offset} of {size}: expected `MemoryOutOfBounds`, got {other:?}"
        )),
    }
}

/// Every memory method refuses a range outside the memory with a
/// structured error that names the range and the size of the memory: a
/// range that runs past the end, a range that starts past it, and a range
/// whose end overflows 64 bits. The refusal writes nothing. A range that
/// ends exactly at the end of the memory is inside it.
pub async fn it_refuses_a_range_outside_the_memory(engine: &Engine) {
    let mut store = support::store(engine, ());
    let other = memory(&mut store, MemoryType::new(1, None));
    for (name, memory) in memories(engine, &mut store) {
        let size = memory.size(&store).expect("the memory is the store's");
        let mut failures = Vec::new();
        let mut check = |result: Result<(), String>| {
            if let Err(failure) = result {
                failures.push(failure);
            }
        };
        let mut buffer = [0; 4];

        // The ranges that end exactly at the end of the memory are inside.
        memory
            .read(&store, size - 4, &mut buffer)
            .expect("a range that ends at the end of the memory is inside it");
        memory
            .with_bytes(&store, size, 0, |_| ())
            .expect("an empty range at the end of the memory is inside it");

        for offset in [size - 3, size + 1, 1 << 32, u64::MAX - 1] {
            check(out_of_bounds(
                memory.read(&store, offset, &mut buffer),
                offset,
                4,
                size,
            ));
            check(out_of_bounds(
                memory.write(&mut store, offset, &[9; 4]),
                offset,
                4,
                size,
            ));
            check(out_of_bounds(
                memory.with_bytes(&store, offset, 4, <[u8]>::to_vec),
                offset,
                4,
                size,
            ));
            check(out_of_bounds(
                memory.load_u32(&store, offset),
                offset,
                4,
                size,
            ));
            check(out_of_bounds(
                memory.store_u32(&mut store, offset, 9),
                offset,
                4,
                size,
            ));
        }
        for offset in [size, u64::MAX] {
            check(out_of_bounds(
                memory.load_u8(&store, offset),
                offset,
                1,
                size,
            ));
            check(out_of_bounds(
                memory.store_u8(&mut store, offset, 9),
                offset,
                1,
                size,
            ));
        }
        for offset in [size - 1, u64::MAX - 1] {
            check(out_of_bounds(
                memory.load_u16(&store, offset),
                offset,
                2,
                size,
            ));
            check(out_of_bounds(
                memory.store_u16(&mut store, offset, 9),
                offset,
                2,
                size,
            ));
        }
        for offset in [size - 7, u64::MAX - 7] {
            check(out_of_bounds(
                memory.load_u64(&store, offset),
                offset,
                8,
                size,
            ));
            check(out_of_bounds(
                memory.store_u64(&mut store, offset, 9),
                offset,
                8,
                size,
            ));
        }
        check(out_of_bounds(
            memory.with_bytes(&store, 0, usize::MAX, |_| ()),
            0,
            usize::MAX as u64,
            size,
        ));
        check(out_of_bounds(
            Memory::copy(&mut store, &memory, size - 3, &other, 0, 4),
            size - 3,
            4,
            size,
        ));
        check(out_of_bounds(
            Memory::copy(&mut store, &other, 0, &memory, size - 3, 4),
            size - 3,
            4,
            size,
        ));
        check(out_of_bounds(
            Memory::copy(&mut store, &memory, 0, &memory, 0, u64::MAX),
            0,
            u64::MAX,
            size,
        ));
        assert!(failures.is_empty(), "{name}: {failures:#?}");

        let tail = memory
            .with_bytes(&store, size - 8, 8, <[u8]>::to_vec)
            .expect("the range lies inside the memory");
        assert_eq!(tail, [0; 8], "{name}: a refused write writes nothing");
        let head = other
            .with_bytes(&store, 0, 8, <[u8]>::to_vec)
            .expect("the range lies inside the memory");
        assert_eq!(head, [0; 8], "{name}: a refused copy writes nothing");
    }
}

/// `Memory::copy` copies bytes from the memory of one guest to the memory
/// of another in the same store, and the second guest reads them. Within
/// one memory, it copies a range onto an overlapping range as if through a
/// buffer, in either direction.
///
/// Where the engine declares `threads`, a copy passes through a shared
/// memory, from one shared memory to another, and within a shared memory
/// onto an overlapping range in either direction. Where it does not, the
/// store refuses a shared memory with `Unsupported` and `threads`.
pub async fn it_copies_between_two_memories_of_one_store(engine: &Engine) {
    let mut store = support::store(engine, ());
    let bytes = wasm!(
        r#"
        (module
          (memory (export "memory") 1)
          (func (export "load") (param i32) (result i32)
            local.get 0
            i32.load))
        "#
    );
    let first = support::instance(&mut store, bytes, &[]).await;
    let second = support::instance(&mut store, bytes, &[]).await;
    let source = exported_memory(&mut store, first);
    let destination = exported_memory(&mut store, second);
    let load = support::func(&mut store, second, "load");

    source
        .write(&mut store, 100, &pattern::<16>())
        .expect("the range lies inside the memory");
    Memory::copy(&mut store, &source, 100, &destination, 200, 16)
        .expect("both ranges lie inside their memories");
    let copied = destination
        .with_bytes(&store, 200, 16, <[u8]>::to_vec)
        .expect("the range lies inside the memory");
    assert_eq!(copied, pattern::<16>());
    let loaded = support::call(&mut store, load, &[Val::I32(200)], &[ValType::I32]);
    assert_eq!(
        loaded[0].i32(),
        Some(0x0403_0201),
        "the second guest reads the copy"
    );
    let kept = source
        .with_bytes(&store, 100, 16, <[u8]>::to_vec)
        .expect("the range lies inside the memory");
    assert_eq!(
        kept,
        pattern::<16>(),
        "the copy leaves its source as it was"
    );

    Memory::copy(&mut store, &source, 100, &source, 104, 16)
        .expect("both ranges lie inside the memory");
    let moved = source
        .with_bytes(&store, 100, 20, <[u8]>::to_vec)
        .expect("the range lies inside the memory");
    assert_eq!(
        moved,
        [
            1, 2, 3, 4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16
        ]
    );
    Memory::copy(&mut store, &source, 104, &source, 100, 16)
        .expect("both ranges lie inside the memory");
    let moved = source
        .with_bytes(&store, 100, 20, <[u8]>::to_vec)
        .expect("the range lies inside the memory");
    assert_eq!(
        moved,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 13, 14, 15, 16
        ]
    );

    Memory::copy(&mut store, &source, 65_536, &destination, 65_536, 0)
        .expect("an empty range at the end of a memory is inside it");

    let shared = MemoryType::shared(1, 1);
    let shared_pair = memory_needing(engine, &mut store, Capability::Threads, shared).zip(
        memory_needing(engine, &mut store, Capability::Threads, shared),
    );
    if let Some((shared, other_shared)) = shared_pair {
        Memory::copy(&mut store, &source, 100, &shared, 8, 16)
            .expect("both ranges lie inside their memories");
        Memory::copy(&mut store, &shared, 8, &destination, 300, 16)
            .expect("both ranges lie inside their memories");
        let copied = destination
            .with_bytes(&store, 300, 16, <[u8]>::to_vec)
            .expect("the range lies inside the memory");
        assert_eq!(
            copied,
            pattern::<16>(),
            "a copy passes through a shared memory"
        );

        Memory::copy(&mut store, &shared, 8, &shared, 12, 16)
            .expect("both ranges lie inside the memory");
        let moved = shared
            .with_bytes(&store, 8, 20, <[u8]>::to_vec)
            .expect("the range lies inside the memory");
        assert_eq!(
            moved,
            [
                1, 2, 3, 4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16
            ],
            "a shared memory copies onto an overlapping range later in it as if through a buffer"
        );
        Memory::copy(&mut store, &shared, 12, &shared, 8, 16)
            .expect("both ranges lie inside the memory");
        let moved = shared
            .with_bytes(&store, 8, 20, <[u8]>::to_vec)
            .expect("the range lies inside the memory");
        assert_eq!(
            moved,
            [
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 13, 14, 15, 16
            ],
            "a shared memory copies onto an overlapping range earlier in it as if through a \
             buffer"
        );

        Memory::copy(&mut store, &shared, 8, &other_shared, 40, 16)
            .expect("both ranges lie inside their memories");
        let copied = other_shared
            .with_bytes(&store, 40, 16, <[u8]>::to_vec)
            .expect("the range lies inside the memory");
        assert_eq!(
            copied,
            pattern::<16>(),
            "a copy passes from one shared memory to another"
        );
    }
}

/// A memory addressed with 64-bit numbers uses the same methods. The host
/// reads what a guest stored, and an offset past 4 GiB is a structured
/// error. A backend that lacks `memory64` refuses the module, and the
/// host's own 64-bit memory, with `Unsupported` and `memory64`.
pub async fn it_addresses_a_64_bit_memory_with_the_same_methods(engine: &Engine) {
    let bytes = wasm!(
        r#"
        (module
          (memory (export "memory") i64 1)
          (func (export "store") (param i64 i64)
            local.get 0
            local.get 1
            i64.store))
        "#
    );
    if support::refuses(engine, &[Capability::Memory64], &[bytes]).await {
        let mut store = support::store(engine, ());
        let wide = MemoryType::new64(1, None);
        memory_needing(engine, &mut store, Capability::Memory64, wide);
        return;
    }
    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, bytes, &[]).await;
    let memory = exported_memory(&mut store, instance);
    let store_i64 = support::func(&mut store, instance, "store");
    assert!(
        memory
            .ty(&store)
            .expect("the memory is the store's")
            .is_64(),
        "the memory is addressed with 64-bit numbers"
    );

    support::call(
        &mut store,
        store_i64,
        &[Val::I64(1000), Val::I64(0x0102_0304_0506_0708)],
        &[],
    );
    assert_eq!(
        memory.load_u64(&store, 1000).ok(),
        Some(0x0102_0304_0506_0708)
    );
    let far = 1 << 32;
    let refused = memory.load_u64(&store, far);
    assert!(
        out_of_bounds(refused, far, 8, 65_536).is_ok(),
        "an offset past 4 GiB is outside the memory"
    );
}

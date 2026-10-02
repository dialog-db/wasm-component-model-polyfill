// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! String transcoding between two guest memories.
//!
//! When a component contains other components, the translator's
//! fused adapter compiler emits one core module per cross-component
//! call site, and that adapter imports one transcoder per pair of
//! string encodings. A transcode is a crossing like any other: it
//! reads code units through one side's options and writes them
//! through the other's, so it runs on a [`BoundaryContext`] built
//! for the copy and names no memory of its own.
//!
//! The argument and result conventions follow the fused adapter
//! compiler's `transcode` signatures and Wasmtime's libcalls of the
//! same names. Pointers are offsets into the addressed side, and
//! lengths count code units of the respective encoding.
//!
//! A string can be as long as its memory, several GiB, so the host
//! never holds the whole of one. A transcode that only checks and
//! copies its source checks it a chunk at a time and then copies it
//! from one guest memory to the other with no buffer on the host. A
//! transcode that converts lends its source to the host a chunk at a
//! time, and writes what each chunk converts to before it reads the
//! next. Natively a lent chunk is the guest's own bytes; in the
//! browser it is one copy of the chunk.

use crate::abi::context::BoundaryContext;
use crate::abi::layout::FlatType;
use crate::error::{Error, Result};
use crate::executor::ir::TranscodeOp;
use crate::internal::ErrorInternal;
use crate::runtime_layer::Val as RuntimeVal;

/// The tag a "compact UTF-16" length carries when the string was
/// left as UTF-16 rather than deflated to Latin-1.
const UTF16_TAG: u32 = 1 << 31;

/// The most bytes of a source string the host is lent at once. What
/// one chunk converts to is at most twice its size, so a transcode
/// holds a few hundred KiB on the host however long the string is.
/// The size is even, so a chunk of UTF-16 ends on a whole code unit,
/// and above 4, so a chunk always holds a whole character.
const CHUNK: usize = 64 * 1024;

/// What a step of a walk over the source did with its chunk.
enum Step {
    /// It used the first this many bytes of the chunk, and the walk
    /// goes on from there.
    Next(usize),
    /// It is done, and the walk stops after the chunk.
    Stop,
}

/// Run one transcode on `ctx`, which reads through the source side
/// of the copy and writes through its own. Pointers and lengths
/// arrive at the width of the memory they address, `i32` for a
/// 32-bit memory and `i64` for a 64-bit one, and the results are
/// written back at the widths `result_widths` names.
pub fn transcode<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    op: TranscodeOp,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
    result_widths: &[FlatType],
) -> Result<()> {
    let set_results =
        |results: &mut [RuntimeVal], values: &[usize]| set_results(results, result_widths, values);
    match op {
        TranscodeOp::CopyUtf8 => {
            let (src, len, dst) = three(args)?;
            check_source(ctx, src, len)?;
            validate_utf8(ctx, src, len)?;
            copy(ctx, src, dst, len)
        }
        TranscodeOp::CopyUtf16 => {
            let (src, len, dst) = three(args)?;
            let bytes = utf16_bytes(len)?;
            check_source(ctx, src, bytes)?;
            validate_utf16(ctx, src, bytes)?;
            copy(ctx, src, dst, bytes)
        }
        TranscodeOp::CopyLatin1 => {
            let (src, len, dst) = three(args)?;
            check_source(ctx, src, len)?;
            copy(ctx, src, dst, len)
        }
        TranscodeOp::Latin1ToUtf16 => {
            let (src, len, dst) = three(args)?;
            check_source(ctx, src, len)?;
            walk(ctx, src, len, Some(dst), |bytes, _, out| {
                for b in bytes {
                    out.extend_from_slice(&u16::from(*b).to_le_bytes());
                }
                Ok(Step::Next(bytes.len()))
            })?;
            Ok(())
        }
        TranscodeOp::Utf8ToUtf16 => {
            let (src, len, dst) = three(args)?;
            check_source(ctx, src, len)?;
            validate_utf8(ctx, src, len)?;
            let units = utf8_to_utf16(ctx, src, len, dst)?;
            set_results(results, &[units])
        }
        TranscodeOp::Utf16ToUtf8 => {
            let (src, src_len, dst, dst_len, first_pass) = five(args)?;
            let bytes = utf16_bytes(src_len)?;
            check_source(ctx, src, bytes)?;
            let mut src_read = 0usize;
            let mut consumed = 0usize;
            let mut written = 0usize;
            walk(ctx, src, bytes, Some(dst), |chunk, last, out| {
                let units = whole_utf16(chunk, last);
                for ch in char::decode_utf16(units.clone()) {
                    let ch = ch.map_err(|_| invalid("invalid utf16 encoding"))?;
                    consumed += ch.len_utf16();
                    if first_pass != 0 && u32::from(ch) >= 0x80 {
                        return Ok(Step::Stop);
                    }
                    let remaining = dst_len - written;
                    if remaining < 4 && remaining < ch.len_utf8() {
                        return Ok(Step::Stop);
                    }
                    let mut buf = [0u8; 4];
                    let encoded = ch.encode_utf8(&mut buf).as_bytes();
                    out.extend_from_slice(encoded);
                    written += encoded.len();
                    src_read = consumed;
                }
                Ok(Step::Next(units.len() * 2))
            })?;
            set_results(results, &[src_read, written])
        }
        TranscodeOp::Latin1ToUtf8 => {
            let (src, src_len, dst, dst_len, first_pass) = five(args)?;
            check_source(ctx, src, src_len)?;
            let mut read_count = 0usize;
            let mut written = 0usize;
            walk(ctx, src, src_len, Some(dst), |bytes, _, out| {
                for b in bytes {
                    if first_pass != 0 && *b >= 0x80 {
                        return Ok(Step::Stop);
                    }
                    let ch = char::from(*b);
                    if written + ch.len_utf8() > dst_len {
                        return Ok(Step::Stop);
                    }
                    let mut buf = [0u8; 4];
                    let encoded = ch.encode_utf8(&mut buf).as_bytes();
                    out.extend_from_slice(encoded);
                    written += encoded.len();
                    read_count += 1;
                }
                Ok(Step::Next(bytes.len()))
            })?;
            set_results(results, &[read_count, written])
        }
        TranscodeOp::Utf16ToCompactProbablyUtf16 => {
            let (src, len, dst) = three(args)?;
            let bytes = utf16_bytes(len)?;
            check_source(ctx, src, bytes)?;
            if validate_utf16(ctx, src, bytes)? {
                walk(ctx, src, bytes, Some(dst), |chunk, _, out| {
                    // Every unit is at most 0xFF, so its low byte,
                    // which comes first, is the whole of it.
                    out.extend(chunk.chunks_exact(2).map(|pair| pair[0]));
                    Ok(Step::Next(chunk.len()))
                })?;
                set_results(results, &[len])
            } else {
                copy(ctx, src, dst, bytes)?;
                set_results(results, &[len | UTF16_TAG as usize])
            }
        }
        TranscodeOp::Utf8ToLatin1 => {
            let (src, len, dst) = three(args)?;
            check_source(ctx, src, len)?;
            validate_utf8(ctx, src, len)?;
            let mut read_count = 0usize;
            let written = walk(ctx, src, len, Some(dst), |bytes, last, out| {
                let mut stopped = false;
                let used = whole_utf8(bytes, last, |text| {
                    for ch in text.chars() {
                        if stopped {
                            return;
                        }
                        match u8::try_from(u32::from(ch)) {
                            Ok(b) => out.push(b),
                            Err(_) => {
                                stopped = true;
                                return;
                            }
                        }
                        read_count += ch.len_utf8();
                    }
                })?;
                Ok(if stopped {
                    Step::Stop
                } else {
                    Step::Next(used)
                })
            })?;
            set_results(results, &[read_count, written])
        }
        TranscodeOp::Utf16ToLatin1 => {
            let (src, len, dst) = three(args)?;
            let bytes = utf16_bytes(len)?;
            check_source(ctx, src, bytes)?;
            let written = walk(ctx, src, bytes, Some(dst), |chunk, _, out| {
                for pair in chunk.chunks_exact(2) {
                    match u8::try_from(u16::from_le_bytes([pair[0], pair[1]])) {
                        Ok(b) => out.push(b),
                        Err(_) => return Ok(Step::Stop),
                    }
                }
                Ok(Step::Next(chunk.len()))
            })?;
            set_results(results, &[written, written])
        }
        TranscodeOp::Utf8ToCompactUtf16 => {
            let (src, src_len, dst, _dst_len, latin1_so_far) = five(args)?;
            inflate_latin1(ctx, dst, latin1_so_far)?;
            check_source(ctx, src, src_len)?;
            validate_utf8(ctx, src, src_len)?;
            let units = utf8_to_utf16(ctx, src, src_len, dst + latin1_so_far * 2)?;
            set_results(results, &[units + latin1_so_far])
        }
        TranscodeOp::Utf16ToCompactUtf16 => {
            let (src, src_len, dst, _dst_len, latin1_so_far) = five(args)?;
            inflate_latin1(ctx, dst, latin1_so_far)?;
            let bytes = utf16_bytes(src_len)?;
            check_source(ctx, src, bytes)?;
            validate_utf16(ctx, src, bytes)?;
            copy(ctx, src, dst + latin1_so_far * 2, bytes)?;
            set_results(results, &[src_len + latin1_so_far])
        }
    }
}

/// Walk the `length` bytes at `offset` of the source side in chunks
/// of at most [`CHUNK`] bytes, lending each to `step` with whether it
/// ends the range and a buffer for what to write. A chunk `step` does
/// not use to its end is read again from where it stopped, which is
/// how a character two chunks split is seen whole. What `step` puts
/// in the buffer is written to the destination side from `dst` on,
/// chunk after chunk, and the count of bytes written is returned. A
/// walk that only checks its source passes no `dst` and writes
/// nothing.
fn walk<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    length: usize,
    dst: Option<usize>,
    mut step: impl FnMut(&[u8], bool, &mut Vec<u8>) -> Result<Step>,
) -> Result<usize> {
    let mut out = Vec::new();
    let mut read_so_far = 0usize;
    let mut written = 0usize;
    while read_so_far < length {
        let size = (length - read_so_far).min(CHUNK);
        let last = read_so_far + size == length;
        let stepped = ctx
            .with_source_bytes(offset + read_so_far, size, |bytes| {
                step(bytes, last, &mut out)
            })
            .map_err(|_| invalid("out-of-bounds string read in adapter"))??;
        if let Some(dst) = dst
            && !out.is_empty()
        {
            write(ctx, dst + written, &out)?;
            written += out.len();
            out.clear();
        }
        match stepped {
            Step::Next(0) => return Err(Error::internal("a transcode step used no bytes")),
            Step::Next(used) => read_so_far += used,
            Step::Stop => break,
        }
    }
    Ok(written)
}

/// Check that the whole source lies inside its memory before any of
/// it is read or written, so a source out of bounds writes nothing.
fn check_source<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    length: usize,
) -> Result<()> {
    if ctx.source_in_bounds(offset, length) {
        Ok(())
    } else {
        Err(invalid("out-of-bounds string read in adapter"))
    }
}

/// Check that the `length` bytes at `offset` of the source are UTF-8.
fn validate_utf8<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    length: usize,
) -> Result<()> {
    walk(ctx, offset, length, None, |bytes, last, _| {
        whole_utf8(bytes, last, |_| {}).map(Step::Next)
    })?;
    Ok(())
}

/// Check that the `length` bytes at `offset` of the source are
/// UTF-16, and return whether every code unit of them fits in
/// Latin-1.
fn validate_utf16<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    length: usize,
) -> Result<bool> {
    let mut latin1 = true;
    walk(ctx, offset, length, None, |chunk, last, _| {
        let units = whole_utf16(chunk, last);
        for ch in char::decode_utf16(units.clone()) {
            ch.map_err(|_| invalid("invalid utf16 encoding"))?;
        }
        latin1 = latin1 && units.clone().all(|unit| unit <= 0xFF);
        Ok(Step::Next(units.len() * 2))
    })?;
    Ok(latin1)
}

/// Convert the `length` bytes of UTF-8 at `offset` of the source to
/// UTF-16 at `dst`, and return how many code units were written.
fn utf8_to_utf16<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    length: usize,
    dst: usize,
) -> Result<usize> {
    let written = walk(ctx, offset, length, Some(dst), |bytes, last, out| {
        whole_utf8(bytes, last, |text| {
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        })
        .map(Step::Next)
    })?;
    Ok(written / 2)
}

/// Hand the UTF-8 text of `bytes` to `text` piece by piece, and
/// return how many bytes it covered. A character the end of the
/// chunk splits is left for the next chunk, which starts with it,
/// unless `last` says no chunk follows. Any other byte that is not
/// UTF-8 is an error.
fn whole_utf8(bytes: &[u8], last: bool, mut text: impl FnMut(&str)) -> Result<usize> {
    let mut used = 0usize;
    for piece in bytes.utf8_chunks() {
        text(piece.valid());
        used += piece.valid().len();
        let rest = piece.invalid();
        if rest.is_empty() {
            continue;
        }
        // An incomplete character is at most three bytes, and the
        // next chunk, which starts with it, sees it whole or proves
        // it invalid.
        if !last && used + rest.len() == bytes.len() {
            return Ok(used);
        }
        return Err(invalid("invalid utf8 encoding"));
    }
    Ok(used)
}

/// The code units of `chunk`, leaving out a high surrogate that ends
/// it unless `last` says no chunk follows: the low surrogate it pairs
/// with starts the next chunk, which reads the high one again.
fn whole_utf16(chunk: &[u8], last: bool) -> impl ExactSizeIterator<Item = u16> + Clone {
    let mut units = chunk.len() / 2;
    if !last
        && let [.., low, high] = chunk
        && (0xD800..=0xDBFF).contains(&u16::from_le_bytes([*low, *high]))
    {
        units -= 1;
    }
    chunk[..units * 2]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
}

/// The byte length of `units` UTF-16 code units.
fn utf16_bytes(units: usize) -> Result<usize> {
    units
        .checked_mul(2)
        .ok_or_else(|| invalid("out-of-bounds string read in adapter"))
}

/// Copy `length` bytes from `src` of the source to `dst` of the
/// destination, guest to guest. The source was checked first, so a
/// failure is the destination's.
fn copy<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    src: usize,
    dst: usize,
    length: usize,
) -> Result<()> {
    ctx.copy_from_source(src, dst, length)
        .map_err(|_| invalid("out-of-bounds string write in adapter"))
}

/// Inflate the first `count` Latin-1 bytes at `dst` into UTF-16
/// code units in place, a chunk at a time from the end, so nothing
/// is overwritten before it is read: the chunk at `start` lands at
/// twice `start`, past every byte still to be read. Both halves
/// address the side the copy writes to, not the side it reads from.
fn inflate_latin1<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    dst: usize,
    count: usize,
) -> Result<()> {
    let mut end = count;
    while end > 0 {
        let start = end.saturating_sub(CHUNK);
        let bytes = ctx
            .read_own_bytes(dst + start, end - start)
            .map_err(|_| invalid("out-of-bounds string read in adapter"))?;
        let units: Vec<u8> = bytes.iter().flat_map(|b| [*b, 0]).collect();
        write(ctx, dst + start * 2, &units)?;
        end = start;
    }
    Ok(())
}

fn write<T: 'static>(ctx: &mut BoundaryContext<'_, T>, offset: usize, bytes: &[u8]) -> Result<()> {
    ctx.write_own_bytes(offset, bytes)
        .map_err(|_| invalid("out-of-bounds string write in adapter"))
}

fn three(args: &[RuntimeVal]) -> Result<(usize, usize, usize)> {
    Ok((
        arg_usize(args, 0)?,
        arg_usize(args, 1)?,
        arg_usize(args, 2)?,
    ))
}

fn five(args: &[RuntimeVal]) -> Result<(usize, usize, usize, usize, usize)> {
    Ok((
        arg_usize(args, 0)?,
        arg_usize(args, 1)?,
        arg_usize(args, 2)?,
        arg_usize(args, 3)?,
        arg_usize(args, 4)?,
    ))
}

/// A pointer or length argument at the width of the memory it
/// addresses. A 64-bit offset that the host cannot address (a 32-bit
/// host with a memory past 4 GiB) is reported rather than truncated.
fn arg_usize(args: &[RuntimeVal], index: usize) -> Result<usize> {
    match args.get(index) {
        Some(RuntimeVal::I32(v)) => Ok(*v as u32 as usize),
        Some(RuntimeVal::I64(v)) => usize::try_from(*v as u64).map_err(|_| {
            Error::internal("the host cannot address a 64-bit memory offset of this size")
        }),
        _ => Err(Error::internal("intrinsic expected an integer argument")),
    }
}

/// Write the transcode's results at the widths the adapter's core
/// signature declares.
fn set_results(results: &mut [RuntimeVal], widths: &[FlatType], values: &[usize]) -> Result<()> {
    if results.len() != values.len() || widths.len() != values.len() {
        return Err(Error::internal("intrinsic result arity mismatch"));
    }
    for ((slot, width), value) in results.iter_mut().zip(widths).zip(values) {
        *slot = match width {
            FlatType::I64 => RuntimeVal::I64(*value as u64 as i64),
            _ => RuntimeVal::I32(*value as u32 as i32),
        };
    }
    Ok(())
}

fn invalid(message: &str) -> Error {
    Error::internal(message)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::abi::instance::BoundaryInstance;
    use crate::abi::options::BoundaryOptions;
    use crate::abi::runtime_state::AbiRuntimeState;
    use crate::abi::strategy::memory_accesses;
    use crate::engine::Engine;
    use crate::runtime_layer::{AsContextMut, Memory, MemoryType};
    use crate::store::{Store, StoreInternalExt};

    /// The pages of each memory of a test's copy: room for a string
    /// of several chunks in any encoding, twice over.
    const PAGES: u32 = 16;

    /// A store holding two memories, the source and the destination
    /// of one adapter's transcodes.
    struct TwoMemories {
        _engine: Engine,
        store: Store<()>,
        source: Memory,
        destination: Memory,
        source_options: BoundaryOptions,
        destination_options: BoundaryOptions,
    }

    impl TwoMemories {
        fn new() -> Self {
            let engine =
                Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
            let mut store: Store<()> = Store::new(&engine, ()).expect("store");
            let mut memory = || {
                Memory::new(
                    store.internal().inner_mut().as_context_mut(),
                    MemoryType::new(PAGES, None),
                )
                .expect("a guest memory")
            };
            let (source, destination) = (memory(), memory());
            let state = Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
                2,
                0,
                0,
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )));
            {
                let mut guard = state.lock().expect("runtime state");
                guard.memories[0] = Some(source);
                guard.memories[1] = Some(destination);
            }
            Self {
                _engine: engine,
                store,
                source,
                destination,
                source_options: BoundaryOptions::for_memory(0, &state).expect("source"),
                destination_options: BoundaryOptions::for_memory(1, &state).expect("destination"),
            }
        }

        /// Write `bytes` at `offset` of the source.
        fn put(&mut self, offset: usize, bytes: &[u8]) {
            let store = self.store.internal().inner_mut().as_context_mut();
            self.source
                .write(store, offset as u64, bytes)
                .expect("write the source");
        }

        /// Write `bytes` at `offset` of the destination.
        fn put_destination(&mut self, offset: usize, bytes: &[u8]) {
            let store = self.store.internal().inner_mut().as_context_mut();
            self.destination
                .write(store, offset as u64, bytes)
                .expect("write the destination");
        }

        /// The `length` bytes at `offset` of the destination.
        fn take(&mut self, offset: usize, length: usize) -> Vec<u8> {
            let mut bytes = vec![0u8; length];
            let store = self.store.internal().inner_mut().as_context_mut();
            self.destination
                .read(store, offset as u64, &mut bytes)
                .expect("read the destination");
            bytes
        }

        /// Run `op` with 32-bit `args`, and return its `results`.
        fn run(&mut self, op: TranscodeOp, args: &[usize], results: usize) -> Result<Vec<usize>> {
            let mut ctx = BoundaryContext::for_copy(
                self.store.internal().inner_mut().as_context_mut(),
                self.destination_options.clone(),
                self.source_options.clone(),
                BoundaryInstance::without_tables(None),
                None,
            );
            let args: Vec<RuntimeVal> = args.iter().map(|a| RuntimeVal::I32(*a as i32)).collect();
            let mut slots = vec![RuntimeVal::I32(0); results];
            transcode(
                &mut ctx,
                op,
                &args,
                &mut slots,
                &vec![FlatType::I32; results],
            )?;
            Ok(slots
                .iter()
                .map(|slot| match slot {
                    RuntimeVal::I32(v) => *v as u32 as usize,
                    _ => panic!("a 32-bit result"),
                })
                .collect())
        }
    }

    /// UTF-16 code units as little-endian bytes.
    fn utf16_le(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    /// Text of three chunks of UTF-8 whose first two chunk ends
    /// split a two-byte and a four-byte character.
    fn split_utf8() -> String {
        let mut text = "a".repeat(CHUNK - 1);
        text.push('é');
        text.push_str(&"b".repeat(CHUNK - 3));
        text.push('😀');
        text.push_str("日本語 and the rest");
        assert!(!text.is_char_boundary(CHUNK) && !text.is_char_boundary(2 * CHUNK - 1));
        text
    }

    /// Text of three chunks of UTF-16 whose first chunk ends between
    /// the two halves of a surrogate pair.
    fn split_utf16() -> String {
        let mut text = "a".repeat(CHUNK / 2 - 1);
        text.push('😀');
        text.push_str(&"ß".repeat(CHUNK / 2));
        text.push_str("日本語");
        text
    }

    #[wcmp_macros::test]
    fn it_transcodes_utf8_whose_characters_straddle_the_chunks() {
        let text = split_utf8();
        let mut copy = TwoMemories::new();
        copy.put(0, text.as_bytes());
        let len = text.len();

        copy.run(TranscodeOp::CopyUtf8, &[0, len, 0], 0)
            .expect("copy utf8");
        assert_eq!(copy.take(0, len), text.as_bytes());

        let [units] = copy
            .run(TranscodeOp::Utf8ToUtf16, &[0, len, 0], 1)
            .expect("utf8 to utf16")[..]
        else {
            panic!("one result");
        };
        let expected = utf16_le(&text);
        assert_eq!(units * 2, expected.len());
        assert_eq!(copy.take(0, expected.len()), expected);
    }

    #[wcmp_macros::test]
    fn it_transcodes_utf16_whose_surrogate_pair_straddles_the_chunks() {
        let text = split_utf16();
        let source = utf16_le(&text);
        let units = source.len() / 2;
        let mut copy = TwoMemories::new();
        copy.put(0, &source);

        copy.run(TranscodeOp::CopyUtf16, &[0, units, 0], 0)
            .expect("copy utf16");
        assert_eq!(copy.take(0, source.len()), source);

        let room = units * 3;
        let read_and_written = copy
            .run(TranscodeOp::Utf16ToUtf8, &[0, units, 0, room, 0], 2)
            .expect("utf16 to utf8");
        assert_eq!(read_and_written, [units, text.len()]);
        assert_eq!(copy.take(0, text.len()), text.as_bytes());

        let [tagged] = copy
            .run(TranscodeOp::Utf16ToCompactProbablyUtf16, &[0, units, 0], 1)
            .expect("utf16 to compact")[..]
        else {
            panic!("one result");
        };
        assert_eq!(
            tagged,
            units | UTF16_TAG as usize,
            "the string stays UTF-16"
        );
        assert_eq!(copy.take(0, source.len()), source);
    }

    #[wcmp_macros::test]
    fn it_stops_a_partial_utf16_to_utf8_at_the_room_it_was_given_past_a_chunk() {
        let text = split_utf16();
        let mut copy = TwoMemories::new();
        copy.put(0, &utf16_le(&text));
        let units = text.encode_utf16().count();
        // Room for the ASCII run and the emoji and three of the
        // two-byte characters after it, and one byte over.
        let room = (CHUNK / 2 - 1) + 4 + 3 * 2 + 1;

        let read_and_written = copy
            .run(TranscodeOp::Utf16ToUtf8, &[0, units, 0, room, 0], 2)
            .expect("utf16 to utf8");
        assert_eq!(read_and_written, [CHUNK / 2 - 1 + 2 + 3, room - 1]);
        assert_eq!(copy.take(0, room - 1), &text.as_bytes()[..room - 1]);

        let first_pass = copy
            .run(TranscodeOp::Utf16ToUtf8, &[0, units, 0, units * 3, 1], 2)
            .expect("first pass");
        assert_eq!(
            first_pass,
            [CHUNK / 2 - 1, CHUNK / 2 - 1],
            "the first pass stops at the first character past ASCII"
        );
    }

    #[wcmp_macros::test]
    fn it_deflates_and_inflates_latin1_of_several_chunks() {
        let text: String = (0..2 * CHUNK + 7)
            .map(|i| char::from((i % 0xFF) as u8 + 1))
            .collect();
        let latin1: Vec<u8> = text.chars().map(|c| c as u8).collect();
        let len = latin1.len();
        let mut copy = TwoMemories::new();

        copy.put(0, &utf16_le(&text));
        let [deflated] = copy
            .run(TranscodeOp::Utf16ToCompactProbablyUtf16, &[0, len, 0], 1)
            .expect("utf16 to compact")[..]
        else {
            panic!("one result");
        };
        assert_eq!(deflated, len, "every code point fits Latin-1");
        assert_eq!(copy.take(0, len), latin1);

        let read_and_written = copy
            .run(TranscodeOp::Utf16ToLatin1, &[0, len, 0], 2)
            .expect("utf16 to latin1");
        assert_eq!(read_and_written, [len, len]);
        assert_eq!(copy.take(0, len), latin1);

        copy.put(0, &latin1);
        copy.run(TranscodeOp::Latin1ToUtf16, &[0, len, 0], 0)
            .expect("latin1 to utf16");
        assert_eq!(copy.take(0, len * 2), utf16_le(&text));

        let read_and_written = copy
            .run(TranscodeOp::Latin1ToUtf8, &[0, len, 0, len * 2, 0], 2)
            .expect("latin1 to utf8");
        assert_eq!(read_and_written, [len, text.len()]);
        assert_eq!(copy.take(0, text.len()), text.as_bytes());
    }

    #[wcmp_macros::test]
    fn it_finishes_a_compact_utf16_string_whose_latin1_prefix_spans_chunks() {
        let prefix: Vec<u8> = (0..CHUNK + 9).map(|i| (i % 0xFF) as u8 + 1).collect();
        let rest = split_utf8();
        let mut copy = TwoMemories::new();
        copy.put_destination(0, &prefix);
        copy.put(0, rest.as_bytes());

        let [units] = copy
            .run(
                TranscodeOp::Utf8ToCompactUtf16,
                &[0, rest.len(), 0, 0, prefix.len()],
                1,
            )
            .expect("utf8 to compact utf16")[..]
        else {
            panic!("one result");
        };
        let mut expected: Vec<u8> = prefix.iter().flat_map(|b| [*b, 0]).collect();
        expected.extend(utf16_le(&rest));
        assert_eq!(units * 2, expected.len());
        assert_eq!(copy.take(0, expected.len()), expected);

        let rest = split_utf16();
        copy.put_destination(0, &prefix);
        copy.put(0, &utf16_le(&rest));
        let rest_units = rest.encode_utf16().count();
        let [units] = copy
            .run(
                TranscodeOp::Utf16ToCompactUtf16,
                &[0, rest_units, 0, 0, prefix.len()],
                1,
            )
            .expect("utf16 to compact utf16")[..]
        else {
            panic!("one result");
        };
        let mut expected: Vec<u8> = prefix.iter().flat_map(|b| [*b, 0]).collect();
        expected.extend(utf16_le(&rest));
        assert_eq!(units, prefix.len() + rest_units);
        assert_eq!(copy.take(0, expected.len()), expected);
    }

    #[wcmp_macros::test]
    fn it_deflates_utf8_to_latin1_up_to_the_first_wider_character_past_a_chunk() {
        let mut text = "a".repeat(CHUNK - 1);
        text.push('é');
        text.push_str("xyz");
        let latin1_len = text.len();
        text.push('日');
        text.push_str("tail");
        let mut copy = TwoMemories::new();
        copy.put(0, text.as_bytes());

        let read_and_written = copy
            .run(TranscodeOp::Utf8ToLatin1, &[0, text.len(), 0], 2)
            .expect("utf8 to latin1");
        assert_eq!(read_and_written, [latin1_len, CHUNK + 3]);
        let mut expected = vec![b'a'; CHUNK - 1];
        expected.extend([0xE9, b'x', b'y', b'z']);
        assert_eq!(copy.take(0, expected.len()), expected);
    }

    #[wcmp_macros::test]
    fn it_rejects_utf8_that_is_invalid_at_a_chunk_end_or_the_string_end() {
        let mut copy = TwoMemories::new();
        // A lead byte at the end of the first chunk that no
        // continuation byte follows.
        let mut bytes = vec![b'a'; CHUNK - 1];
        bytes.extend([0xC3, b'a', b'b']);
        copy.put(0, &bytes);
        let Err(error) = copy.run(TranscodeOp::CopyUtf8, &[0, bytes.len(), 0], 0) else {
            panic!("a lead byte with no continuation is not UTF-8");
        };
        assert!(
            error.to_string().contains("invalid utf8 encoding"),
            "{error}"
        );

        // A character the end of the string cuts short.
        let mut bytes = vec![b'a'; CHUNK + 5];
        bytes.extend([0xF0, 0x9F, 0x98]);
        copy.put(0, &bytes);
        let Err(error) = copy.run(TranscodeOp::Utf8ToUtf16, &[0, bytes.len(), 0], 1) else {
            panic!("a character cut short is not UTF-8");
        };
        assert!(
            error.to_string().contains("invalid utf8 encoding"),
            "{error}"
        );
        assert_eq!(
            copy.take(0, 4),
            [0, 0, 0, 0],
            "the whole source is checked before anything is written"
        );
    }

    #[wcmp_macros::test]
    fn it_rejects_a_high_surrogate_that_ends_the_string() {
        let mut copy = TwoMemories::new();
        let mut bytes = utf16_le(&"a".repeat(CHUNK / 2 + 3));
        bytes.extend(0xD83Du16.to_le_bytes());
        copy.put(0, &bytes);
        let Err(error) = copy.run(TranscodeOp::CopyUtf16, &[0, bytes.len() / 2, 0], 0) else {
            panic!("a lone high surrogate is not UTF-16");
        };
        assert!(
            error.to_string().contains("invalid utf16 encoding"),
            "{error}"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_source_out_of_bounds_before_it_writes_anything() {
        let mut copy = TwoMemories::new();
        let size = PAGES as usize * 65536;
        copy.put(size - CHUNK, &vec![b'a'; CHUNK]);
        let Err(error) = copy.run(TranscodeOp::Latin1ToUtf16, &[size - CHUNK, CHUNK + 1, 0], 0)
        else {
            panic!("the source runs past its memory");
        };
        assert!(
            error.to_string().contains("out-of-bounds string read"),
            "{error}"
        );
        assert_eq!(copy.take(0, 2), [0, 0]);
    }

    #[wcmp_macros::test]
    fn it_reads_the_source_a_chunk_at_a_time_and_copies_it_guest_to_guest() {
        let text = "a".repeat(4 * CHUNK);
        let mut copy = TwoMemories::new();
        copy.put(0, text.as_bytes());

        let (reads, writes) = memory_accesses();
        copy.run(TranscodeOp::CopyUtf8, &[0, text.len(), 0], 0)
            .expect("copy utf8");
        let (after_reads, after_writes) = memory_accesses();
        assert_eq!(
            (after_reads - reads, after_writes - writes),
            (4 + 1, 1),
            "four chunks lent to the check, then one copy between the memories"
        );
        assert_eq!(copy.take(0, text.len()), text.as_bytes());

        let (reads, writes) = memory_accesses();
        copy.run(TranscodeOp::Latin1ToUtf16, &[0, text.len(), 0], 0)
            .expect("latin1 to utf16");
        let (after_reads, after_writes) = memory_accesses();
        assert_eq!(
            (after_reads - reads, after_writes - writes),
            (4, 4),
            "each chunk lent once and written once"
        );
    }
}

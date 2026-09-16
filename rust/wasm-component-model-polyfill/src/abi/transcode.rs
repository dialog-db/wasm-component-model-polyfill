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

use wasm_runtime_layer::Val as RuntimeVal;

use crate::abi::context::BoundaryContext;
use crate::abi::layout::FlatType;
use crate::error::{Error, Result};
use crate::executor::ir::TranscodeOp;

/// The tag a "compact UTF-16" length carries when the string was
/// left as UTF-16 rather than deflated to Latin-1.
const UTF16_TAG: u32 = 1 << 31;

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
            let bytes = read(ctx, src, len)?;
            core::str::from_utf8(&bytes).map_err(|_| invalid("invalid utf8 encoding"))?;
            write(ctx, dst, &bytes)
        }
        TranscodeOp::CopyUtf16 => {
            let (src, len, dst) = three(args)?;
            let units = read_utf16(ctx, src, len)?;
            decode_utf16(&units)?;
            write_utf16(ctx, dst, &units)
        }
        TranscodeOp::CopyLatin1 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(ctx, src, len)?;
            write(ctx, dst, &bytes)
        }
        TranscodeOp::Latin1ToUtf16 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(ctx, src, len)?;
            let units: Vec<u16> = bytes.iter().map(|b| u16::from(*b)).collect();
            write_utf16(ctx, dst, &units)
        }
        TranscodeOp::Utf8ToUtf16 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(ctx, src, len)?;
            let text =
                core::str::from_utf8(&bytes).map_err(|_| invalid("invalid utf8 encoding"))?;
            let units: Vec<u16> = text.encode_utf16().collect();
            write_utf16(ctx, dst, &units)?;
            set_results(results, &[units.len()])
        }
        TranscodeOp::Utf16ToUtf8 => {
            let (src, src_len, dst, dst_len, first_pass) = five(args)?;
            let units = read_utf16(ctx, src, src_len)?;
            let mut out: Vec<u8> = Vec::with_capacity(dst_len);
            let mut src_read = 0usize;
            let mut consumed = 0usize;
            for ch in char::decode_utf16(units.iter().copied()) {
                let ch = ch.map_err(|_| invalid("invalid utf16 encoding"))?;
                consumed += ch.len_utf16();
                if first_pass != 0 && u32::from(ch) >= 0x80 {
                    break;
                }
                let remaining = dst_len - out.len();
                if remaining < 4 && remaining < ch.len_utf8() {
                    break;
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                src_read = consumed;
            }
            write(ctx, dst, &out)?;
            set_results(results, &[src_read, out.len()])
        }
        TranscodeOp::Latin1ToUtf8 => {
            let (src, src_len, dst, dst_len, first_pass) = five(args)?;
            let bytes = read(ctx, src, src_len)?;
            let stop = if first_pass != 0 {
                bytes.iter().position(|b| *b >= 0x80).unwrap_or(bytes.len())
            } else {
                bytes.len()
            };
            let mut out: Vec<u8> = Vec::with_capacity(dst_len);
            let mut read_count = 0usize;
            for b in &bytes[..stop] {
                let ch = char::from(*b);
                if out.len() + ch.len_utf8() > dst_len {
                    break;
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                read_count += 1;
            }
            write(ctx, dst, &out)?;
            set_results(results, &[read_count, out.len()])
        }
        TranscodeOp::Utf16ToCompactProbablyUtf16 => {
            let (src, len, dst) = three(args)?;
            let units = read_utf16(ctx, src, len)?;
            decode_utf16(&units)?;
            if units.iter().all(|u| *u <= 0xFF) {
                let bytes: Vec<u8> = units.iter().map(|u| *u as u8).collect();
                write(ctx, dst, &bytes)?;
                set_results(results, &[len])
            } else {
                write_utf16(ctx, dst, &units)?;
                set_results(results, &[len | UTF16_TAG as usize])
            }
        }
        TranscodeOp::Utf8ToLatin1 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(ctx, src, len)?;
            let text =
                core::str::from_utf8(&bytes).map_err(|_| invalid("invalid utf8 encoding"))?;
            let mut out: Vec<u8> = Vec::with_capacity(len);
            let mut read_count = 0usize;
            for ch in text.chars() {
                match u8::try_from(u32::from(ch)) {
                    Ok(b) => out.push(b),
                    Err(_) => break,
                }
                read_count += ch.len_utf8();
            }
            write(ctx, dst, &out)?;
            set_results(results, &[read_count, out.len()])
        }
        TranscodeOp::Utf16ToLatin1 => {
            let (src, len, dst) = three(args)?;
            let units = read_utf16(ctx, src, len)?;
            let mut out: Vec<u8> = Vec::with_capacity(len);
            for u in &units {
                match u8::try_from(*u) {
                    Ok(b) => out.push(b),
                    Err(_) => break,
                }
            }
            write(ctx, dst, &out)?;
            set_results(results, &[out.len(), out.len()])
        }
        TranscodeOp::Utf8ToCompactUtf16 => {
            let (src, src_len, dst, _dst_len, latin1_so_far) = five(args)?;
            inflate_latin1(ctx, dst, latin1_so_far)?;
            let bytes = read(ctx, src, src_len)?;
            let text =
                core::str::from_utf8(&bytes).map_err(|_| invalid("invalid utf8 encoding"))?;
            let units: Vec<u16> = text.encode_utf16().collect();
            write_utf16(ctx, dst + latin1_so_far * 2, &units)?;
            set_results(results, &[units.len() + latin1_so_far])
        }
        TranscodeOp::Utf16ToCompactUtf16 => {
            let (src, src_len, dst, _dst_len, latin1_so_far) = five(args)?;
            inflate_latin1(ctx, dst, latin1_so_far)?;
            let units = read_utf16(ctx, src, src_len)?;
            decode_utf16(&units)?;
            write_utf16(ctx, dst + latin1_so_far * 2, &units)?;
            set_results(results, &[src_len + latin1_so_far])
        }
    }
}

/// Inflate the first `count` Latin-1 bytes at `dst` into UTF-16
/// code units in place, from the end so nothing is overwritten
/// before it is read. Both halves address the side the copy writes
/// to, not the side it reads from.
fn inflate_latin1<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    dst: usize,
    count: usize,
) -> Result<()> {
    if count == 0 {
        return Ok(());
    }
    let bytes = ctx
        .read_own_bytes(dst, count)
        .map_err(|_| invalid("out-of-bounds string read in adapter"))?;
    let units: Vec<u16> = bytes.iter().map(|b| u16::from(*b)).collect();
    write_utf16(ctx, dst, &units)
}

fn read<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    length: usize,
) -> Result<Vec<u8>> {
    ctx.read_source_bytes(offset, length)
        .map_err(|_| invalid("out-of-bounds string read in adapter"))
}

fn write<T: 'static>(ctx: &mut BoundaryContext<'_, T>, offset: usize, bytes: &[u8]) -> Result<()> {
    ctx.write_own_bytes(offset, bytes)
        .map_err(|_| invalid("out-of-bounds string write in adapter"))
}

fn read_utf16<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    units: usize,
) -> Result<Vec<u16>> {
    let bytes = read(ctx, offset, units * 2)?;
    Ok(bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect())
}

fn write_utf16<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    units: &[u16],
) -> Result<()> {
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for u in units {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    write(ctx, offset, &bytes)
}

fn decode_utf16(units: &[u16]) -> Result<()> {
    for ch in char::decode_utf16(units.iter().copied()) {
        ch.map_err(|_| invalid("invalid utf16 encoding"))?;
    }
    Ok(())
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

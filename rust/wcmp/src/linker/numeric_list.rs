// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A Rust vector of numbers as the bytes of a canonical-ABI list.
//!
//! A `list<T>` of a numeric `T` sits in guest memory as its elements'
//! little-endian bytes, one after another, with no pointer to follow.
//! Such a list crosses as one block of bytes rather than one value per
//! element, and these two functions are the conversion at either end.
//!
//! `Vec<T>` implements the typed value trait for any element type, so
//! which conversion applies is decided by the element's concrete type
//! at run time, through [`Any`]. The decision is a type comparison and
//! costs nothing per element. A `Vec<u8>` is its own bytes: it moves
//! across without a copy, so the list reaches the guest in the one
//! copy the write makes and comes back in the one copy the read makes.

use core::any::Any;
use core::mem;

/// The little-endian bytes of `items` when `T` is a numeric
/// primitive's Rust type, or `items` back, untouched, when it is not.
///
/// A `Vec<u8>` is moved rather than copied, and a `Vec<i8>` or a
/// `Vec<bool>` is rewritten in place. Every wider number is encoded
/// into a fresh buffer of exactly the list's size.
pub fn encode<T: 'static>(mut items: Vec<T>) -> Result<Vec<u8>, Vec<T>> {
    let any = &mut items as &mut dyn Any;
    if let Some(bytes) = any.downcast_mut::<Vec<u8>>() {
        return Ok(mem::take(bytes));
    }
    if let Some(values) = any.downcast_mut::<Vec<i8>>() {
        return Ok(mem::take(values).into_iter().map(|v| v as u8).collect());
    }
    if let Some(values) = any.downcast_mut::<Vec<bool>>() {
        return Ok(mem::take(values).into_iter().map(u8::from).collect());
    }
    if let Some(values) = any.downcast_mut::<Vec<char>>() {
        return Ok(little_endian(values, |c| (*c as u32).to_le_bytes()));
    }
    macro_rules! wide {
        ($($rust:ty),+) => {$(
            if let Some(values) = any.downcast_mut::<Vec<$rust>>() {
                return Ok(little_endian(values, |v| v.to_le_bytes()));
            }
        )+};
    }
    wide!(u16, i16, u32, i32, u64, i64, f32, f64);
    Err(items)
}

/// Whether [`encode`] and [`decode`] convert a vector of `T`.
pub fn is_numeric<T: 'static>() -> bool {
    encode::<T>(Vec::new()).is_ok()
}

/// The vector of `T` whose little-endian bytes are `bytes`, when `T`
/// is a numeric primitive's Rust type. `None` when it is not, and
/// `Some(Err(_))` when a `char` element is not a Unicode scalar.
///
/// `bytes` must hold a whole number of elements. A `Vec<u8>` is
/// `bytes` itself; every other element type is decoded into a vector
/// of exactly the list's length.
pub fn decode<T: 'static>(bytes: Vec<u8>) -> Option<Result<Vec<T>, &'static str>> {
    let mut out: Option<Vec<T>> = None;
    let slot = &mut out as &mut dyn Any;
    if let Some(slot) = slot.downcast_mut::<Option<Vec<u8>>>() {
        *slot = Some(bytes);
    } else if let Some(slot) = slot.downcast_mut::<Option<Vec<i8>>>() {
        *slot = Some(bytes.into_iter().map(|b| b as i8).collect());
    } else if let Some(slot) = slot.downcast_mut::<Option<Vec<bool>>>() {
        *slot = Some(bytes.into_iter().map(|b| b != 0).collect());
    } else if let Some(slot) = slot.downcast_mut::<Option<Vec<char>>>() {
        let chars: Option<Vec<char>> = bytes
            .chunks_exact(4)
            .map(|chunk| char::from_u32(u32::from_le_bytes(array(chunk))))
            .collect();
        match chars {
            Some(chars) => *slot = Some(chars),
            None => return Some(Err("char value is not a valid Unicode scalar")),
        }
    } else {
        macro_rules! wide {
            ($($rust:ty),+) => {
                $(
                    if let Some(slot) = slot.downcast_mut::<Option<Vec<$rust>>>() {
                        *slot = Some(
                            bytes
                                .chunks_exact(mem::size_of::<$rust>())
                                .map(|chunk| <$rust>::from_le_bytes(array(chunk)))
                                .collect(),
                        );
                        return out.map(Ok);
                    }
                )+
                return None;
            };
        }
        wide!(u16, i16, u32, i32, u64, i64, f32, f64);
    }
    out.map(Ok)
}

/// Encode each of `values` with `to_bytes` into one buffer of the
/// list's exact size.
fn little_endian<V, const N: usize>(values: &[V], to_bytes: impl Fn(&V) -> [u8; N]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * N);
    for value in values {
        bytes.extend_from_slice(&to_bytes(value));
    }
    bytes
}

/// The `N` bytes of `chunk` as an array. `chunk` is always one of
/// `chunks_exact(N)`, so the lengths agree.
fn array<const N: usize>(chunk: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(chunk);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_moves_a_byte_vector_without_copying_it() {
        let bytes = vec![1u8, 2, 3];
        let address = bytes.as_ptr();
        let encoded = encode(bytes).expect("a byte vector is numeric");
        assert_eq!(encoded.as_ptr(), address);
        let decoded: Vec<u8> = decode(encoded).expect("numeric").expect("valid");
        assert_eq!(decoded.as_ptr(), address);
    }

    #[wcmp_macros::test]
    fn it_round_trips_every_numeric_element_type() {
        fn round_trip<T: 'static + Clone + PartialEq + core::fmt::Debug>(values: Vec<T>) {
            let bytes = encode(values.clone()).expect("numeric");
            assert_eq!(decode::<T>(bytes).expect("numeric").expect("valid"), values);
        }
        round_trip(vec![true, false]);
        round_trip(vec![-1i8, 7]);
        round_trip(vec![0xBEEFu16]);
        round_trip(vec![-2i16]);
        round_trip(vec![0xDEAD_BEEFu32]);
        round_trip(vec![-3i32]);
        round_trip(vec![u64::MAX]);
        round_trip(vec![i64::MIN]);
        round_trip(vec![1.5f32]);
        round_trip(vec![-2.25f64]);
        round_trip(vec!['a', '🍰']);
    }

    #[wcmp_macros::test]
    fn it_encodes_numbers_little_endian() {
        assert_eq!(encode(vec![0x0102_0304u32]).unwrap(), [4, 3, 2, 1]);
    }

    #[wcmp_macros::test]
    fn it_refuses_a_char_that_is_not_a_scalar() {
        let surrogate = 0xD800u32.to_le_bytes().to_vec();
        assert!(decode::<char>(surrogate).expect("numeric").is_err());
    }

    #[wcmp_macros::test]
    fn it_leaves_a_vector_of_anything_else_untouched() {
        let strings = vec!["a".to_owned()];
        assert_eq!(encode(strings.clone()), Err(strings));
        assert!(decode::<String>(Vec::new()).is_none());
        assert!(!is_numeric::<String>());
        assert!(is_numeric::<f64>());
    }
}

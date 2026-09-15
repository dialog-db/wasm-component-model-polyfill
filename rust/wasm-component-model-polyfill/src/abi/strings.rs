//! The three canonical string encodings, shared by the memory and
//! flat-slot lift and lower paths.
//!
//! `latin1+utf16` is one encoding with two representations: a length
//! word whose high bit is clear counts Latin-1 bytes, and one whose
//! high bit is set counts UTF-16 code units. A lower picks Latin-1
//! when every scalar fits in one byte and UTF-16 otherwise.

use crate::executor::ir::StringEncoding;

/// The high bit of a `latin1+utf16` length word: set for UTF-16.
pub const UTF16_TAG: u32 = 1 << 31;

/// The alignment a string's pointer must have under `encoding`.
pub fn alignment(encoding: StringEncoding) -> usize {
    match encoding {
        StringEncoding::Utf8 => 1,
        StringEncoding::Utf16 | StringEncoding::CompactUtf16 => 2,
    }
}

/// The number of bytes a string of `units` occupies under `encoding`,
/// or `None` when the count overflows.
pub fn byte_length(encoding: StringEncoding, units: u32) -> Option<usize> {
    match encoding {
        StringEncoding::Utf8 => Some(units as usize),
        StringEncoding::Utf16 => (units as usize).checked_mul(2),
        StringEncoding::CompactUtf16 => {
            if units & UTF16_TAG != 0 {
                ((units & !UTF16_TAG) as usize).checked_mul(2)
            } else {
                Some(units as usize)
            }
        }
    }
}

/// Decode the bytes of a string of `units` under `encoding`. The
/// error names what was wrong with the bytes.
pub fn decode(encoding: StringEncoding, units: u32, raw: &[u8]) -> Result<String, &'static str> {
    match encoding {
        StringEncoding::Utf8 => String::from_utf8(raw.to_vec()).map_err(|_| "invalid UTF-8 string"),
        StringEncoding::Utf16 => decode_utf16(raw),
        StringEncoding::CompactUtf16 => {
            if units & UTF16_TAG != 0 {
                decode_utf16(raw)
            } else {
                Ok(raw.iter().map(|byte| char::from(*byte)).collect())
            }
        }
    }
}

fn decode_utf16(raw: &[u8]) -> Result<String, &'static str> {
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| "invalid UTF-16 string")
}

/// Encode `s` under `encoding`: the bytes to write and the length
/// word to store next to the pointer.
pub fn encode(encoding: StringEncoding, s: &str) -> (Vec<u8>, u32) {
    match encoding {
        StringEncoding::Utf8 => (s.as_bytes().to_vec(), s.len() as u32),
        StringEncoding::Utf16 => {
            let (bytes, units) = encode_utf16(s);
            (bytes, units)
        }
        StringEncoding::CompactUtf16 => {
            if s.chars().all(|c| (c as u32) <= 0xFF) {
                let bytes: Vec<u8> = s.chars().map(|c| c as u32 as u8).collect();
                let units = bytes.len() as u32;
                (bytes, units)
            } else {
                let (bytes, units) = encode_utf16(s);
                (bytes, units | UTF16_TAG)
            }
        }
    }
}

fn encode_utf16(s: &str) -> (Vec<u8>, u32) {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for unit in &units {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    (bytes, units.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_picks_latin1_when_every_scalar_fits_in_a_byte() {
        let (bytes, units) = encode(StringEncoding::CompactUtf16, "héllo");
        assert_eq!(bytes, b"h\xe9llo");
        assert_eq!(units, 5);
        assert_eq!(
            decode(StringEncoding::CompactUtf16, units, &bytes).unwrap(),
            "héllo"
        );
    }

    #[test]
    fn it_falls_back_to_utf16_with_the_tag_bit() {
        let (bytes, units) = encode(StringEncoding::CompactUtf16, "cake 🍰");
        assert_eq!(units & UTF16_TAG, UTF16_TAG);
        assert_eq!((units & !UTF16_TAG) as usize * 2, bytes.len());
        assert_eq!(
            decode(StringEncoding::CompactUtf16, units, &bytes).unwrap(),
            "cake 🍰"
        );
    }

    #[test]
    fn it_sizes_each_representation() {
        assert_eq!(byte_length(StringEncoding::CompactUtf16, 5), Some(5));
        assert_eq!(
            byte_length(StringEncoding::CompactUtf16, 5 | UTF16_TAG),
            Some(10)
        );
        assert_eq!(byte_length(StringEncoding::Utf16, 3), Some(6));
    }
}

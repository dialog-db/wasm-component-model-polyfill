//! An unboxed 31-bit integer.

/// The integer of an `i31ref`: 31 bits, read as signed or unsigned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct I31 {
    bits: u32,
}

impl I31 {
    /// The greatest unsigned value.
    const MASK: u32 = (1 << 31) - 1;

    /// `value`, where it fits in 31 unsigned bits.
    pub const fn new_u32(value: u32) -> Option<Self> {
        if value <= Self::MASK {
            Some(Self { bits: value })
        } else {
            None
        }
    }

    /// `value`, where it fits in 31 signed bits.
    pub const fn new_i32(value: i32) -> Option<Self> {
        if value >= -(1 << 30) && value < (1 << 30) {
            Some(Self::wrapping_i32(value))
        } else {
            None
        }
    }

    /// The low 31 bits of `value`.
    pub const fn wrapping_u32(value: u32) -> Self {
        Self {
            bits: value & Self::MASK,
        }
    }

    /// The low 31 bits of `value`.
    pub const fn wrapping_i32(value: i32) -> Self {
        Self::wrapping_u32(value.cast_unsigned())
    }

    /// The integer, read as unsigned.
    pub const fn get_u32(self) -> u32 {
        self.bits
    }

    /// The integer, read as signed: its top bit extends.
    pub const fn get_i32(self) -> i32 {
        (self.bits << 1).cast_signed() >> 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_the_same_bits_as_signed_and_unsigned() {
        let minus_one = I31::wrapping_i32(-1);
        assert_eq!(minus_one.get_i32(), -1);
        assert_eq!(minus_one.get_u32(), (1 << 31) - 1);
        assert_eq!(I31::new_i32(-(1 << 30)).map(I31::get_i32), Some(-(1 << 30)));
        assert_eq!(I31::new_i32(1 << 30), None);
        assert_eq!(I31::new_u32(1 << 31), None);
        assert_eq!(I31::wrapping_u32(u32::MAX).get_u32(), (1 << 31) - 1);
    }
}

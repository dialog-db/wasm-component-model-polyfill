// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How one copy on a stream or future end ended.

/// How one copy on a stream or future end ended. The results and
/// their numbers are the reference's `CopyResult`.
///
/// The word a finished copy reports packs the result into its low
/// four bits and the count of values the copy moved into the bits
/// above, which is the reference's `result | (progress << 4)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum CopyResult {
    /// The copy moved what it could and the stream stays open.
    Completed = 0,
    /// The other end was dropped. The end can make no further copy.
    Dropped = 1,
    /// The copy was cancelled before it moved everything it asked
    /// for.
    Cancelled = 2,
}

impl CopyResult {
    /// The result a packed word carries in its low four bits. `None`
    /// for bits that name no result.
    pub fn from_packed(packed: u32) -> Option<Self> {
        match packed & 0xf {
            0 => Some(Self::Completed),
            1 => Some(Self::Dropped),
            2 => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// The word a finished copy reports: this result in the low four
    /// bits and `progress`, the count of values the copy moved, in
    /// the bits above. A count never reaches 2^28, so the word never
    /// equals the blocked sentinel `0xffffffff`.
    pub fn pack(self, progress: u32) -> u32 {
        self as u32 | (progress << 4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_packs_the_result_low_and_the_count_above_it() {
        assert_eq!(CopyResult::Completed.pack(0), 0);
        assert_eq!(CopyResult::Dropped.pack(0), 1);
        assert_eq!(CopyResult::Cancelled.pack(0), 2);
        assert_eq!(CopyResult::Completed.pack(3), 0x30);
        assert_eq!(CopyResult::Dropped.pack(2), 0x21);
        assert_eq!(
            CopyResult::from_packed(CopyResult::Dropped.pack(7)),
            Some(CopyResult::Dropped)
        );
        assert_ne!(
            CopyResult::Cancelled.pack((1 << 28) - 1),
            u32::MAX,
            "no packed word equals the blocked sentinel"
        );
    }
}

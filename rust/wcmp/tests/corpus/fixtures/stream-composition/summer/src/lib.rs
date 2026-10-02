// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The reading half of the `stream-composition` fixture, built by
//! `cargo` and wit-bindgen's async support.
//!
//! `total` calls the imported `count-up`, which `wac plug` wires to
//! the other component, and reads the `stream<u32>` it answers with.
//! The host sees only the sum.

wit_bindgen::generate!({
    path: "../wit",
    world: "summer",
});

use wcmp::stream_composition::numbers::count_up;

struct Summer;

impl Guest for Summer {
    /// Sum the numbers from 1 to `count` as the stream delivers them.
    /// A number out of order, or a stream that ends early, traps.
    async fn total(count: u32) -> u64 {
        let numbers = count_up(count).await.collect().await;
        let mut expected = 1;
        let mut sum = 0u64;
        for number in numbers {
            assert!(number == expected, "`count-up` skipped or repeated a number");
            sum += u64::from(number);
            expected += 1;
        }
        assert!(expected == count + 1, "`count-up` ended the stream early");
        sum
    }
}

export!(Summer);

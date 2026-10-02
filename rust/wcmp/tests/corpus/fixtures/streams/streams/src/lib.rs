// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A guest that streams to its host and reads a stream from it, built
//! by `cargo` and wit-bindgen's async support.
//!
//! Both exports are `async func`s, which wit-bindgen lifts in the
//! callback form. Each answers with the readable end of a stream or a
//! future it created and leaves the writing to a task it spawns, so
//! the writes happen after the export has returned, in the turns the
//! host drives.

wit_bindgen::generate!({
    path: "../wit",
    world: "streams",
});

use wit_bindgen::{FutureReader, StreamReader};

struct Streams;

impl Guest for Streams {
    /// Write each word of `sentence` as a write of its own, so the
    /// host's consumer takes the words over several copies. A write
    /// the reader answered by dropping its end stops the task.
    async fn words(sentence: String) -> StreamReader<String> {
        let (mut writer, reader) = wit_stream::new::<String>();
        wit_bindgen::spawn_local(async move {
            for word in sentence.split_whitespace() {
                if !writer.write_all(vec![word.to_owned()]).await.is_empty() {
                    return;
                }
            }
        });
        reader
    }

    /// Read `numbers` to its end and resolve the future with the sum
    /// of each number times its one-based position, so a number out
    /// of order, missing, or repeated changes the result where a plain
    /// sum could hide it. The future's default, used if the task drops
    /// the writer unwritten, is `u64::MAX`, which no checksum of the
    /// numbers the host sends reaches.
    async fn checksum(numbers: StreamReader<u32>) -> FutureReader<u64> {
        let (total, result) = wit_future::new::<u64>(|| u64::MAX);
        wit_bindgen::spawn_local(async move {
            let numbers = numbers.collect().await;
            let checksum = (1..)
                .zip(numbers)
                .map(|(position, number)| position * u64::from(number))
                .sum();
            let _ = total.write(checksum).await;
        });
        result
    }
}

export!(Streams);

//! The writing half of the `stream-composition` fixture, built by
//! `cargo` and wit-bindgen's async support.
//!
//! `count-up` answers with the readable end of a `stream<u32>` it
//! created and writes the numbers from a task it spawns, so the
//! writes happen after the export has returned. They go out in
//! batches, so the reader in the other component takes them over
//! several copies.

wit_bindgen::generate!({
    path: "../wit",
    world: "counter",
});

use exports::wcmp::stream_composition::numbers::Guest;
use wit_bindgen::StreamReader;

/// How many numbers one write offers.
const BATCH: u32 = 64;

struct Counter;

impl Guest for Counter {
    async fn count_up(count: u32) -> StreamReader<u32> {
        let (mut writer, reader) = wit_stream::new::<u32>();
        wit_bindgen::spawn_local(async move {
            let mut next = 1;
            while next <= count {
                let last = count.min(next + BATCH - 1);
                if !writer.write_all((next..=last).collect()).await.is_empty() {
                    return;
                }
                next = last + 1;
            }
        });
        reader
    }
}

export!(Counter);

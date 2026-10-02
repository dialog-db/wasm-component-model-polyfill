// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A WASI 0.3 HTTP handler, built by `cargo` and wit-bindgen against
//! the `wasi:http@0.3.0` packages vendored under `../wit/deps/`.
//!
//! The handler is an `async func`: it takes the request's body as a
//! `stream<u8>` and its trailers as a
//! `future<result<option<trailers>, error-code>>`, and hands both
//! straight to the response it answers with. `drain` is the same
//! machinery in a signature a `.wast` directive can call.

wit_bindgen::generate!({
    path: "../wit",
    world: "handler",
    generate_all,
});

use wasi::http::types::{ErrorCode, Fields, Request, Response};
use wit_bindgen::FutureReader;

struct Handler;

impl Guest for Handler {
    /// Write `bytes` into a `stream<u8>`, read them back out, and
    /// resolve a `future` with the count written beside them, so the
    /// call needs the async lift, the stream and future built-ins, and
    /// the task built-ins and nothing else. The future carries a `u32`
    /// because this one instance holds both of its ends, and the
    /// Component Model traps such a copy when the payload is not a
    /// number. A count other than the bytes read back traps, and so
    /// does a future dropped unwritten, whose default is no count a
    /// `list<u8>` can have, so neither passes for an empty input.
    async fn drain(bytes: Vec<u8>) -> Vec<u8> {
        let (mut writer, reader) = wit_stream::new::<u8>();
        let (done, finished) = wit_future::new::<u32>(|| u32::MAX);
        wit_bindgen::spawn_local(async move {
            let total = bytes.len();
            let undelivered = writer.write_all(bytes).await;
            drop(writer);
            let _ = done.write((total - undelivered.len()) as u32).await;
        });
        let collected = reader.collect().await;
        assert!(
            finished.await as usize == collected.len(),
            "`drain` read back a count other than it wrote"
        );
        collected
    }

    /// Drop the future: the export is there to name its type.
    fn count(written: FutureReader<u32>) {
        drop(written);
    }
}

impl exports::wasi::http::handler::Guest for Handler {
    /// Answer with the request's own body and trailers: the stream
    /// and the future cross from the request into the response
    /// without the handler ever holding the bytes.
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let (done, finished) = wit_future::new::<Result<(), ErrorCode>>(|| Ok(()));
        let (body, trailers) = Request::consume_body(request, finished);
        wit_bindgen::spawn_local(async move {
            let _ = done.write(Ok(())).await;
        });
        let (response, _sent) = Response::new(Fields::new(), Some(body), trailers);
        Ok(response)
    }
}

export!(Handler);

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

struct Handler;

impl Guest for Handler {
    /// Write `bytes` into a `stream<u8>`, read them back out, and
    /// resolve a `future` beside them, so the call needs the async
    /// lift, the stream and future built-ins, and the task built-ins
    /// and nothing else.
    async fn drain(bytes: Vec<u8>) -> Vec<u8> {
        let (mut writer, reader) = wit_stream::new::<u8>();
        let (done, finished) = wit_future::new::<Result<(), ErrorCode>>(|| Ok(()));
        wit_bindgen::spawn_local(async move {
            let undelivered = writer.write_all(bytes).await;
            drop(writer);
            let _ = done
                .write(if undelivered.is_empty() {
                    Ok(())
                } else {
                    Err(ErrorCode::InternalError(None))
                })
                .await;
        });
        let collected = reader.collect().await;
        match finished.await {
            Ok(()) => collected,
            Err(_) => Vec::new(),
        }
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

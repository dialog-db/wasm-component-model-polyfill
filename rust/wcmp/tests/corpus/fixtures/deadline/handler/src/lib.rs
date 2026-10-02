// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An HTTP-style handler with a timeout, built by `cargo` and
//! wit-bindgen's async support.
//!
//! `handle` races the host's `fetch` against the host's `sleep`. When
//! the timer wins, the handler drops the pending `fetch`. wit-bindgen
//! cancels an import call whose future is dropped before it resolved:
//! its runtime calls `subtask.cancel`, reads the state the subtask
//! resolved to, and releases what the call was lent. The handler then
//! drops the request it lent, which traps unless the borrow came back.

wit_bindgen::generate!({
    path: "../wit",
    world: "deadline",
});

use core::future::{Future, poll_fn};
use core::pin::pin;
use core::task::Poll;

use wcmp::deadline::upstream::{Request, fetch, sleep};

struct Handler;

impl exports::wcmp::deadline::handler::Guest for Handler {
    async fn handle(req: Request, deadline_millis: u32) -> String {
        let body = {
            let mut response = pin!(fetch(&req));
            let mut deadline = pin!(sleep(deadline_millis));
            poll_fn(|cx| {
                if let Poll::Ready(body) = response.as_mut().poll(cx) {
                    return Poll::Ready(Some(body));
                }
                if deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(None);
                }
                Poll::Pending
            })
            .await
            // Both calls drop here. A `fetch` still pending is
            // cancelled, and the borrow of `req` it held comes back.
        };
        drop(req);
        body.unwrap_or_else(|| "timeout".to_owned())
    }
}

export!(Handler);

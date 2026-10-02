// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! IndexedDB, as the demo uses it: one database with one object store of
//! keys and values, which the page and the service worker share because
//! they share an origin.
//!
//! It holds the todo list, under [`TODOS`], and each edit a person saved
//! in the drawer, under [`edit_key`].

use std::cell::RefCell;
use std::rc::Rc;

use futures::channel::oneshot;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{EventTarget, IdbRequest, IdbTransaction};

mod database;
mod idb_storage;

pub use database::Database;
pub use idb_storage::IdbStorage;

/// The database's name.
const DATABASE: &str = "wcmp-demo";

/// The object store's name.
const STORE: &str = "kv";

/// The key the todo list lives under.
pub const TODOS: &str = "todos";

/// The key the edit of a source lives under: an element's by its tag,
/// and a route's by its pattern.
pub fn edit_key(kind: &str, name: &str) -> String {
    format!("edit:{kind}:{name}")
}

/// Wait for `request` to succeed, and answer its result.
async fn done(request: &IdbRequest) -> Result<JsValue, JsValue> {
    let failed = request.clone();
    settle(request, "success", &["error"], move || {
        failed
            .error()
            .ok()
            .flatten()
            .map(JsValue::from)
            .unwrap_or_else(|| JsValue::from_str("an IndexedDB request failed"))
    })
    .await?;
    request.result()
}

/// Wait for `transaction` to complete.
async fn complete(transaction: &IdbTransaction) -> Result<(), JsValue> {
    let failed = transaction.clone();
    settle(transaction, "complete", &["error", "abort"], move || {
        failed
            .error()
            .map(JsValue::from)
            .unwrap_or_else(|| JsValue::from_str("an IndexedDB transaction failed"))
    })
    .await
}

/// Wait for the event `success` on `target`, or for one of `failures`,
/// which fails with what `error` answers. The listeners leave `target`
/// once one of them runs.
async fn settle(
    target: &EventTarget,
    success: &str,
    failures: &[&str],
    error: impl Fn() -> JsValue + 'static,
) -> Result<(), JsValue> {
    let (sender, receiver) = oneshot::channel();
    let sender = Rc::new(RefCell::new(Some(sender)));
    let succeed = sender.clone();
    let on_success = Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| {
        if let Some(sender) = succeed.borrow_mut().take() {
            let _ = sender.send(Ok(()));
        }
    });
    let on_failure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| {
        if let Some(sender) = sender.borrow_mut().take() {
            let _ = sender.send(Err(error()));
        }
    });
    target.add_event_listener_with_callback(success, on_success.as_ref().unchecked_ref())?;
    for failure in failures {
        target.add_event_listener_with_callback(failure, on_failure.as_ref().unchecked_ref())?;
    }
    let answer = receiver
        .await
        .unwrap_or_else(|_| Err(JsValue::from_str("an IndexedDB event never came")));
    let _ =
        target.remove_event_listener_with_callback(success, on_success.as_ref().unchecked_ref());
    for failure in failures {
        let _ = target
            .remove_event_listener_with_callback(failure, on_failure.as_ref().unchecked_ref());
    }
    answer
}

/// A JavaScript exception as text.
fn js_text(error: JsValue) -> String {
    format!("{error:?}")
}

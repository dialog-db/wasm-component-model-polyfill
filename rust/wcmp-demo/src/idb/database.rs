// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The demo's database.

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{IdbDatabase, IdbTransactionMode};

use super::{DATABASE, STORE, complete, done};

/// The demo's database.
#[derive(Clone)]
pub struct Database {
    db: IdbDatabase,
}

impl Database {
    /// Open the database, making its object store the first time.
    ///
    /// # Errors
    ///
    /// The exception of IndexedDB.
    pub async fn open() -> Result<Self, JsValue> {
        let global: JsValue = js_sys::global().into();
        let factory: web_sys::IdbFactory =
            js_sys::Reflect::get(&global, &"indexedDB".into())?.dyn_into()?;
        let request = factory.open_with_u32(DATABASE, 1)?;
        let upgrade = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
            let Some(target) = event.target() else {
                return;
            };
            let Ok(request) = target.dyn_into::<web_sys::IdbOpenDbRequest>() else {
                return;
            };
            if let Ok(db) = request.result().and_then(|db| db.dyn_into::<IdbDatabase>()) {
                let _ = db.create_object_store(STORE);
            }
        });
        request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        let db = done(&request).await?;
        drop(upgrade);
        Ok(Database { db: db.dyn_into()? })
    }

    /// The value under `key`, or `undefined`.
    ///
    /// # Errors
    ///
    /// The exception of IndexedDB.
    pub async fn get(&self, key: &str) -> Result<JsValue, JsValue> {
        let store = self
            .db
            .transaction_with_str_and_mode(STORE, IdbTransactionMode::Readonly)?
            .object_store(STORE)?;
        done(&store.get(&JsValue::from_str(key))?).await
    }

    /// Put `value` under `key`, and wait until the write is durable.
    ///
    /// # Errors
    ///
    /// The exception of IndexedDB.
    pub async fn put(&self, key: &str, value: &JsValue) -> Result<(), JsValue> {
        let transaction = self
            .db
            .transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)?;
        transaction
            .object_store(STORE)?
            .put_with_key(value, &JsValue::from_str(key))?;
        complete(&transaction).await
    }

    /// Remove the value under `key`, and wait until the removal is
    /// durable.
    ///
    /// # Errors
    ///
    /// The exception of IndexedDB.
    pub async fn delete(&self, key: &str) -> Result<(), JsValue> {
        let transaction = self
            .db
            .transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)?;
        transaction
            .object_store(STORE)?
            .delete(&JsValue::from_str(key))?;
        complete(&transaction).await
    }

    /// The text under `key`, if there is text there.
    ///
    /// # Errors
    ///
    /// The exception of IndexedDB.
    pub async fn text(&self, key: &str) -> Result<Option<String>, JsValue> {
        Ok(self.get(key).await?.as_string())
    }
}

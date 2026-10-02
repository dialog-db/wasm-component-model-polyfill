// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The todo model's storage in IndexedDB.

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::{Database, TODOS, js_text};
use crate::model::{Storage, Todo, TodoList};

/// The todo model's storage in IndexedDB: the list as one value under
/// [`TODOS`].
pub struct IdbStorage {
    database: Database,
}

impl IdbStorage {
    /// The storage in `database`.
    pub fn new(database: Database) -> Self {
        IdbStorage { database }
    }
}

impl Storage for IdbStorage {
    fn load(&self) -> impl Future<Output = Result<TodoList, String>> {
        let database = self.database.clone();
        async move {
            let value = database.get(TODOS).await.map_err(js_text)?;
            Ok(list_of(&value))
        }
    }

    fn save(&self, list: &TodoList) -> impl Future<Output = Result<(), String>> {
        let database = self.database.clone();
        let value = value_of(list);
        async move { database.put(TODOS, &value).await.map_err(js_text) }
    }
}

/// The list a stored value holds, or an empty one.
fn list_of(value: &JsValue) -> TodoList {
    let get =
        |object: &JsValue, key: &str| js_sys::Reflect::get(object, &JsValue::from_str(key)).ok();
    let next = get(value, "next")
        .and_then(|next| next.as_f64())
        .unwrap_or(0.0) as u64;
    let todos = get(value, "todos")
        .and_then(|todos| todos.dyn_into::<js_sys::Array>().ok())
        .map(|todos| {
            todos
                .iter()
                .map(|todo| Todo {
                    id: get(&todo, "id")
                        .and_then(|id| id.as_string())
                        .unwrap_or_default(),
                    title: get(&todo, "title")
                        .and_then(|title| title.as_string())
                        .unwrap_or_default(),
                    completed: get(&todo, "completed")
                        .and_then(|completed| completed.as_bool())
                        .unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    TodoList { todos, next }
}

/// The stored value of `list`.
fn value_of(list: &TodoList) -> JsValue {
    let object = js_sys::Object::new();
    let set = |object: &JsValue, key: &str, value: JsValue| {
        let _ = js_sys::Reflect::set(object, &JsValue::from_str(key), &value);
    };
    set(&object, "next", JsValue::from_f64(list.next as f64));
    let todos = js_sys::Array::new();
    for todo in &list.todos {
        let entry: JsValue = js_sys::Object::new().into();
        set(&entry, "id", JsValue::from_str(&todo.id));
        set(&entry, "title", JsValue::from_str(&todo.title));
        set(&entry, "completed", JsValue::from_bool(todo.completed));
        todos.push(&entry);
    }
    set(&object, "todos", todos.into());
    object.into()
}

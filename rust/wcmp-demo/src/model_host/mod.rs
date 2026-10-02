// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `demo:todo/model` on a polyfill [`Linker`]: the todo model, as the
//! host functions a route component imports.
//!
//! Each function is `async`, because the model reads and writes its
//! [`Storage`], and IndexedDB is asynchronous. A [`Model`] makes the
//! changes one at a time, so two routes that change the list at once do
//! not lose either change.

use wcmp::{Accessor, Component, Error, ExternType, InterfaceIdentifier, Linker, Val, ValField};

use crate::model::{Counts, Filter, ModelError, Storage, Todo};

mod model;
#[allow(clippy::module_inception)]
mod model_host;

pub use model::Model;
pub use model_host::ModelHost;

/// The interface this module defines.
pub const MODEL: &str = "demo:todo/model";

/// Define [`MODEL`] in `linker`, with the types `component` imports it
/// under. A component that does not import the interface leaves the
/// linker as it was.
///
/// # Errors
///
/// The polyfill's error when it refuses a definition.
pub fn define<T: ModelHost>(linker: &mut Linker<T>, component: &Component) -> Result<(), Error> {
    let Some(items) = component.imports.iter().find_map(|import| {
        match (&import.ty, import.name.to_string() == MODEL) {
            (ExternType::Instance(instance), true) => Some(instance.items.clone()),
            _ => None,
        }
    }) else {
        return Ok(());
    };
    let identifier: InterfaceIdentifier = MODEL.parse().expect("the interface's name parses");
    let mut model = linker.instance(&identifier);
    for item in items {
        let ExternType::Function(ty) = item.ty else {
            continue;
        };
        let function = item.name.clone();
        model.func_new_concurrent(
            item.name,
            ty,
            move |accessor: &Accessor<T>, args: Vec<Val>| {
                let function = function.clone();
                let model = accessor.with(|store| store.data().model());
                async move {
                    let model = model?;
                    let answer = call(&model, &function, &args)
                        .await
                        .map_err(|message| Error::Internal { message })?;
                    Ok(answer.into_iter().collect())
                }
            },
        )?;
    }
    Ok(())
}

/// Run `function` of [`MODEL`] with `args` on `model`, and answer its
/// result, if it has one.
async fn call<S: Storage>(
    model: &Model<S>,
    function: &str,
    args: &[Val],
) -> Result<Option<Val>, String> {
    let bad = || format!("`{MODEL}#{function}` was given {args:?}");
    Ok(match (function, args) {
        ("query", [filter]) => {
            let filter = filter_of(filter).ok_or_else(bad)?;
            let (todos, counts) = model.with(|list| (list.query(filter), false)).await?;
            Some(Val::Tuple(Box::new([
                Val::List(todos.iter().map(todo_val).collect()),
                counts_val(counts),
            ])))
        }
        ("add", [Val::String(title)]) => {
            let added = model
                .with(|list| {
                    let added = list.add(title);
                    let write = added.is_ok();
                    (added, write)
                })
                .await?;
            Some(result_val(added.map(|todo| Some(todo_val(&todo)))))
        }
        ("update", [Val::String(id), title, completed]) => {
            let title = text_option(title).ok_or_else(bad)?;
            let completed = flag_option(completed).ok_or_else(bad)?;
            let updated = model
                .with(|list| {
                    let updated = list.update(id, title.as_deref(), completed);
                    let write = updated.is_ok();
                    (updated, write)
                })
                .await?;
            Some(result_val(updated.map(|todo| Some(todo_val(&todo)))))
        }
        ("remove", [Val::String(id)]) => {
            let removed = model
                .with(|list| {
                    let removed = list.remove(id);
                    let write = removed.is_ok();
                    (removed, write)
                })
                .await?;
            Some(result_val(removed.map(|()| None)))
        }
        ("toggle-all", [Val::Bool(completed)]) => {
            let completed = *completed;
            model
                .with(|list| (list.toggle_all(completed), true))
                .await?;
            None
        }
        ("clear-completed", []) => {
            model.with(|list| (list.clear_completed(), true)).await?;
            None
        }
        _ => return Err(bad()),
    })
}

/// The filter an enum value names.
fn filter_of(val: &Val) -> Option<Filter> {
    match val {
        Val::Enum(case) => match case.as_str() {
            "all" => Some(Filter::All),
            "active" => Some(Filter::Active),
            "completed" => Some(Filter::Completed),
            _ => None,
        },
        _ => None,
    }
}

/// The value of an `option<string>`.
fn text_option(val: &Val) -> Option<Option<String>> {
    match val {
        Val::Option(None) => Some(None),
        Val::Option(Some(text)) => match text.as_ref() {
            Val::String(text) => Some(Some(text.clone())),
            _ => None,
        },
        _ => None,
    }
}

/// The value of an `option<bool>`.
fn flag_option(val: &Val) -> Option<Option<bool>> {
    match val {
        Val::Option(None) => Some(None),
        Val::Option(Some(flag)) => match flag.as_ref() {
            Val::Bool(flag) => Some(Some(*flag)),
            _ => None,
        },
        _ => None,
    }
}

/// A `todo` record.
fn todo_val(todo: &Todo) -> Val {
    Val::Record(Box::new([
        ValField {
            name: "id".to_string(),
            value: Val::String(todo.id.clone()),
        },
        ValField {
            name: "title".to_string(),
            value: Val::String(todo.title.clone()),
        },
        ValField {
            name: "completed".to_string(),
            value: Val::Bool(todo.completed),
        },
    ]))
}

/// A `counts` record.
fn counts_val(counts: Counts) -> Val {
    Val::Record(Box::new([
        ValField {
            name: "active".to_string(),
            value: Val::U32(counts.active),
        },
        ValField {
            name: "completed".to_string(),
            value: Val::U32(counts.completed),
        },
    ]))
}

/// A `result<_, model-error>` or `result<todo, model-error>`.
fn result_val(result: Result<Option<Val>, ModelError>) -> Val {
    Val::Result(match result {
        Ok(value) => Ok(value.map(Box::new)),
        Err(error) => Err(Some(Box::new(Val::Variant {
            discriminant: match error {
                ModelError::NotFound => "not-found",
                ModelError::EmptyTitle => "empty-title",
            }
            .to_string(),
            payload: None,
        }))),
    })
}

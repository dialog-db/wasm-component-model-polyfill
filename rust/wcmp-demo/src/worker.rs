// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The service worker.
//!
//! A service worker must add its `fetch` listener while its script is
//! first evaluated, and it cannot wait for anything there: no top-level
//! `await` and no dynamic `import()`. So `sw.js` adds the listeners and
//! starts this module without waiting, and each event waits until that
//! start completes before it calls in here.
//!
//! `sw.js` gives every request to [`fetch`], because it must answer
//! `respondWith` while the event dispatches, before it could ask this
//! module whether a route matches. A request that no route matches goes
//! to the network through the worker's own `fetch`, and waits for
//! nothing else. The first request a route matches starts the worker's
//! own copy of the polyfill and of Zena's compiler, which then compiles
//! each route on its first request.
//!
//! The worker and the page talk through messages:
//!
//! - The page sends `route-changed` with a pattern after it writes a new
//!   route source to IndexedDB, or removes an edit. The worker compiles
//!   the route at once and sends `route-compiled` to every page.
//! - The worker sends `route-compiled` after each compile of a route,
//!   and `route-trapped` after a route traps.
//! - The page sends `routes-status` on a message port, and the worker
//!   answers on the port with the last status of each route.
//! - The page sends `claim` when no worker controls it although one is
//!   active, as after a hard reload. `sw.js` answers it by claiming its
//!   clients, without this module.
//! - `define-route` and `compile-check` serve the demo's browser tests.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures::future::{FutureExt, LocalBoxFuture, Shared};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wcmp::{Linker, Val};
use web_sys::{Request, ServiceWorkerGlobalScope};

use crate::compiler::CompileRequest;
use crate::context::Context;
use crate::idb::{self, Database, IdbStorage};
use crate::model_host::Model;
use crate::routes::{HttpRequest, RouteEvent, RouteStatus, Router};
use crate::sources;

/// What the worker runs once it starts.
struct Worker {
    context: Rc<Context>,
    router: Router<IdbStorage>,
    database: Database,
}

/// The start of the worker, shared by every event that waits for it.
type Start = Shared<LocalBoxFuture<'static, Result<Rc<Worker>, String>>>;

thread_local! {
    /// The patterns of the routes, in the order of definition, known
    /// before the worker starts.
    static PATTERNS: RefCell<Vec<(String, String)>> = RefCell::new(
        sources::ROUTES
            .iter()
            .map(|(pattern, source)| (pattern.to_string(), source.to_string()))
            .collect(),
    );
    static START: RefCell<Option<Start>> = const { RefCell::new(None) };
}

/// The worker, started on the first call. A start that fails is
/// forgotten, so the next call starts the worker again.
async fn worker() -> Result<Rc<Worker>, String> {
    let start = START.with(|start| {
        start
            .borrow_mut()
            .get_or_insert_with(|| start_worker().boxed_local().shared())
            .clone()
    });
    let started = start.clone().await;
    if started.is_err() {
        START.with(|current| {
            let mut current = current.borrow_mut();
            if current
                .as_ref()
                .is_some_and(|current| current.ptr_eq(&start))
            {
                *current = None;
            }
        });
    }
    started
}

/// Start the worker: its context, the database, and the router with
/// every route, each from its edit in IndexedDB when it has one.
async fn start_worker() -> Result<Rc<Worker>, String> {
    let context = Context::start().await.map_err(js_text)?;
    let database = Database::open().await.map_err(js_text)?;
    let model = Arc::new(Model::new(IdbStorage::new(database.clone())));
    let router = Router::new(context.engine().clone(), context.compiler(), model);
    router.listen(|event| {
        wasm_bindgen_futures::spawn_local(broadcast(event));
    });
    let patterns = PATTERNS.with(|patterns| patterns.borrow().clone());
    for (pattern, shipped) in patterns {
        let edit = database
            .text(&idb::edit_key("route", &pattern))
            .await
            .map_err(js_text)?;
        router.define_route(&pattern, edit.as_deref().unwrap_or(&shipped), &shipped);
    }
    Ok(Rc::new(Worker {
        context,
        router,
        database,
    }))
}

/// Answer `request`: with the route its path matches, and otherwise
/// from the network.
///
/// # Errors
///
/// The exception of the network fetch, or of a browser API the answer
/// needed.
#[tracing::instrument(level = "debug", name = "service worker fetch", skip_all)]
pub async fn fetch(request: Request) -> Result<JsValue, JsValue> {
    let url = web_sys::Url::new(&request.url())?;
    let scope = scope()?;
    let same_origin = url.origin() == scope.location().origin();
    let path = url.pathname();
    let matched = same_origin
        && PATTERNS.with(|patterns| {
            patterns
                .borrow()
                .iter()
                .any(|(pattern, _)| crate::routes::matches(pattern, &path))
        });
    if !matched {
        return JsFuture::from(scope.fetch_with_request(&request)).await;
    }
    let worker = worker().await.map_err(|error| JsValue::from_str(&error))?;
    let body = JsFuture::from(request.array_buffer()?).await?;
    let mut headers = Vec::new();
    for entry in js_sys::try_iter(&request.headers())?.ok_or("headers are not iterable")? {
        let entry: js_sys::Array = entry?.dyn_into()?;
        headers.push((
            entry.get(0).as_string().unwrap_or_default(),
            entry.get(1).as_string().unwrap_or_default(),
        ));
    }
    let answer = worker
        .router
        .handle(HttpRequest {
            method: request.method(),
            path_with_query: format!("{path}{}", url.search()),
            headers,
            body: js_sys::Uint8Array::new(&body).to_vec(),
        })
        .await;
    let Some(answer) = answer else {
        return JsFuture::from(scope.fetch_with_request(&request)).await;
    };
    let init = web_sys::ResponseInit::new();
    init.set_status(answer.status);
    let response_headers = web_sys::Headers::new()?;
    for (name, value) in &answer.headers {
        response_headers.append(name, value)?;
    }
    init.set_headers(&response_headers);
    // A status with no body, such as 204, takes no body at all.
    let mut body = answer.body;
    let response = if body.is_empty() {
        web_sys::Response::new_with_opt_str_and_init(None, &init)?
    } else {
        web_sys::Response::new_with_opt_u8_array_and_init(Some(&mut body), &init)?
    };
    Ok(response.into())
}

/// Handle a message from a page, answering on `port` when the message
/// carries one: with the answer, or with an object whose `failure` says
/// why the message failed, so a page never waits on a failure.
///
/// # Errors
///
/// The exception of posting the answer.
pub async fn message(data: JsValue, port: JsValue) -> Result<(), JsValue> {
    let answer = match answer(data).await {
        Ok(answer) => answer,
        Err(error) => {
            let failure = js_sys::Object::new();
            let text = error.as_string().unwrap_or_else(|| js_text(error));
            js_sys::Reflect::set(&failure, &"failure".into(), &JsValue::from_str(&text))?;
            failure.into()
        }
    };
    if let Ok(port) = port.dyn_into::<web_sys::MessagePort>() {
        port.post_message(&answer)?;
    }
    Ok(())
}

/// The answer to the message `data`.
async fn answer(data: JsValue) -> Result<JsValue, JsValue> {
    let kind = string(&data, "kind");
    let pattern = string(&data, "pattern");
    let answer: JsValue = match kind.as_str() {
        "route-changed" => {
            let worker = worker().await.map_err(|error| JsValue::from_str(&error))?;
            let shipped = PATTERNS.with(|patterns| {
                patterns
                    .borrow()
                    .iter()
                    .find(|(defined, _)| *defined == pattern)
                    .map(|(_, shipped)| shipped.clone())
            });
            let shipped = shipped.ok_or_else(|| format!("no route has the pattern {pattern}"))?;
            let edit = worker
                .database
                .text(&idb::edit_key("route", &pattern))
                .await?;
            let status = worker
                .router
                .replace(&pattern, edit.as_deref().unwrap_or(&shipped))
                .await
                .map_err(|error| JsValue::from_str(&error))?;
            status_value(&status)
        }
        "routes-status" => {
            let worker = worker().await.map_err(|error| JsValue::from_str(&error))?;
            let statuses = worker.router.statuses().await;
            statuses
                .iter()
                .map(status_value)
                .collect::<js_sys::Array>()
                .into()
        }
        "define-route" => {
            let source = string(&data, "source");
            PATTERNS.with(|patterns| {
                let mut patterns = patterns.borrow_mut();
                patterns.retain(|(defined, _)| *defined != pattern);
                patterns.push((pattern.clone(), source.clone()));
            });
            let started = START.with(|start| start.borrow().clone());
            if let Some(started) = started
                && let Ok(worker) = started.await
            {
                worker.router.define_route(&pattern, &source, &source);
            }
            JsValue::TRUE
        }
        "compile-check" => {
            let worker = worker().await.map_err(|error| JsValue::from_str(&error))?;
            compile_check(&worker.context, &data).await
        }
        other => return Err(JsValue::from_str(&format!("`{other}` is not a message"))),
    };
    Ok(answer)
}

/// Compile the program a `compile-check` message holds, instantiate it,
/// and call its export: what the page's own check does, in the worker.
pub async fn compile_check(context: &Context, data: &JsValue) -> JsValue {
    let source = string(data, "source");
    let entry = string(data, "entry");
    let export = string(data, "export");
    let mut files = Vec::new();
    if let Ok(extra) = js_sys::Reflect::get(data, &"files".into())
        && let Ok(entries) =
            js_sys::Object::entries(&extra.unchecked_into()).dyn_into::<js_sys::Array>()
    {
        for entry in entries.iter() {
            let entry: js_sys::Array = entry.unchecked_into();
            files.push((
                entry.get(0).as_string().unwrap_or_default(),
                entry.get(1).as_string().unwrap_or_default(),
            ));
        }
    }
    let answer = js_sys::Object::new();
    let set = |key: &str, value: JsValue| {
        let _ = js_sys::Reflect::set(&answer, &JsValue::from_str(key), &value);
    };
    let compiled = context
        .compile(&CompileRequest {
            entry_path: &entry,
            source: &source,
            files: &files,
            wit: "",
            world: "",
        })
        .await;
    let compiled = match compiled {
        Ok(compiled) => compiled,
        Err(error) => {
            set("diagnostics", JsValue::from_str(&error.to_string()));
            return answer.into();
        }
    };
    set("compileMs", JsValue::from_f64(compiled.millis));
    set("bytes", JsValue::from_f64(compiled.bytes.len() as f64));
    let outcome =
        async {
            let (component, parse_ms) = context.parse(&compiled.bytes).await?;
            let mut linker: Linker<()> = Linker::new(context.engine());
            crate::wasi::define(&mut linker, &entry)?;
            let mut instantiated = context
                .instantiate(&component, parse_ms, &linker, ())
                .await?;
            let func = instantiated.instance.get_func(&export).ok_or_else(|| {
                wcmp::Error::Unsupported {
                    feature: format!("a program with no export `{export}`"),
                }
            })?;
            let results = func.call(&mut instantiated.store, &[]).await?;
            Ok::<_, wcmp::Error>((instantiated.millis, results))
        }
        .await;
    match outcome {
        Ok((millis, results)) => {
            set("instantiateMs", JsValue::from_f64(millis));
            let result = match results.first() {
                Some(Val::S32(value)) => JsValue::from_f64(f64::from(*value)),
                Some(Val::String(value)) => JsValue::from_str(value),
                other => JsValue::from_str(&format!("{other:?}")),
            };
            set("result", result);
        }
        Err(error) => set("error", JsValue::from_str(&error.to_string())),
    }
    answer.into()
}

/// Send `event` to every page the worker controls.
async fn broadcast(event: RouteEvent) {
    let (kind, status) = match &event {
        RouteEvent::Compiled(status) => ("route-compiled", status),
        RouteEvent::Trapped(status) => ("route-trapped", status),
    };
    let message = status_value(status);
    let _ = js_sys::Reflect::set(&message, &"kind".into(), &JsValue::from_str(kind));
    let Ok(scope) = scope() else {
        return;
    };
    let Ok(clients) = JsFuture::from(scope.clients().match_all()).await else {
        return;
    };
    for client in js_sys::Array::from(&clients).iter() {
        if let Ok(client) = client.dyn_into::<web_sys::Client>() {
            let _ = client.post_message(&message);
        }
    }
}

/// A route's status as a message: its pattern, sources, times,
/// diagnostics, trap, and the number of compiles.
fn status_value(status: &RouteStatus) -> JsValue {
    let object = js_sys::Object::new();
    let set = |key: &str, value: JsValue| {
        let _ = js_sys::Reflect::set(&object, &JsValue::from_str(key), &value);
    };
    let optional = |value: &Option<String>| match value {
        Some(text) => JsValue::from_str(text),
        None => JsValue::NULL,
    };
    let millis = |value: Option<f64>| value.map_or(JsValue::NULL, JsValue::from_f64);
    set("pattern", JsValue::from_str(&status.pattern));
    set("source", JsValue::from_str(&status.source));
    set("shipped", JsValue::from_str(&status.shipped));
    set("compileMs", millis(status.compile_ms));
    set("instantiateMs", millis(status.instantiate_ms));
    set("diagnostics", optional(&status.diagnostics));
    set("trapped", optional(&status.trapped));
    set("compiles", JsValue::from_f64(f64::from(status.compiles)));
    set("compiledAt", millis(status.compiled_at));
    object.into()
}

/// The text property `key` of `object`, or empty.
fn string(object: &JsValue, key: &str) -> String {
    js_sys::Reflect::get(object, &JsValue::from_str(key))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default()
}

/// The global scope of the service worker.
fn scope() -> Result<ServiceWorkerGlobalScope, JsValue> {
    js_sys::global()
        .dyn_into::<ServiceWorkerGlobalScope>()
        .map_err(|_| JsValue::from_str("the worker module runs outside a service worker"))
}

/// A JavaScript exception as text.
fn js_text(error: JsValue) -> String {
    format!("{error:?}")
}

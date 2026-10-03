// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The page: the main thread of the browser tab.
//!
//! The page starts in this order:
//!
//! 1. It registers the demo's service worker, `sw.js`, which calls
//!    `skipWaiting` and `clients.claim`. Trunk registers no worker.
//! 2. It fetches the compiler component and the source bundle, and
//!    instantiates the compiler.
//! 3. It reads the element sources a person edited from IndexedDB.
//! 4. It compiles and defines each element. An edit that does not
//!    compile falls back to the shipped source, and the shelf keeps the
//!    edit and its diagnostics.
//! 5. It waits until the service worker controls it.
//! 6. It replaces the skeleton with `<todo-app>`, whose first request
//!    goes to the service worker, and sets its `filter` from the URL
//!    hash, then and at each `hashchange`.
//!
//! The page records each step on the document element's `data-boot`
//! attribute, which the browser tests read, and exposes `window.demo`,
//! the hooks those tests drive the host framework through.

use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, future_to_promise};
use web_sys::{ServiceWorkerContainer, Window};

use crate::context::Context;
use crate::elements;
use crate::idb::{self, Database};
use crate::shelf;
use crate::sources;
use crate::telemetry;

/// The script of the demo's service worker, beside the page.
const SERVICE_WORKER: &str = "./sw.js";

/// Start the page.
///
/// # Errors
///
/// The exception of the browser API that failed, such as a refused
/// registration of the service worker, or the reason a step failed.
pub async fn start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("the page has no window")?;
    boot_step(&window, "registering");
    let container = window.navigator().service_worker();
    JsFuture::from(container.register(&service_worker_url())).await?;

    boot_step(&window, "compiler");
    let context = Context::start().await?;
    elements::set_context(context.clone());
    install_hooks(&window, context.clone())?;

    boot_step(&window, "elements");
    let database = Database::open().await?;
    for (tag, shipped) in sources::ELEMENTS {
        let edit = database.text(&idb::edit_key("element", tag)).await?;
        elements::define_with_edit(tag, edit.as_deref(), shipped)
            .await
            .map_err(|error| JsValue::from_str(&error))?;
    }

    boot_step(&window, "waiting-for-worker");
    controlled(&container).await?;

    boot_step(&window, "mounting");
    mount(&window)?;
    shelf::mount(database).map_err(|error| JsValue::from_str(&error))?;
    boot_step(&window, "ready");
    Ok(())
}

/// The URL of the service worker's script, with the page's `trace`
/// parameter when it has one, so that the worker records its spans at
/// the page's level.
fn service_worker_url() -> String {
    let search = telemetry::search();
    match telemetry::parameter(&search) {
        Some(level) => format!("{SERVICE_WORKER}?{}={level}", telemetry::PARAMETER),
        None => SERVICE_WORKER.to_string(),
    }
}

/// Replace the skeleton with `<todo-app>`, and keep its `filter` in step
/// with the URL hash.
fn mount(window: &Window) -> Result<(), JsValue> {
    let document = window.document().ok_or("the page has no document")?;
    let main = document
        .get_element_by_id("app")
        .ok_or("the page has no #app")?;
    let app = document.create_element("todo-app")?;
    app.set_attribute("filter", &filter_of(&window.location().hash()?))?;
    let children = js_sys::Array::of1(&app);
    main.replace_children_with_node(&children);
    let hashchange = Closure::<dyn Fn()>::new(move || {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(hash) = window.location().hash() else {
            return;
        };
        let _ = app.set_attribute("filter", &filter_of(&hash));
    });
    window.add_event_listener_with_callback("hashchange", hashchange.as_ref().unchecked_ref())?;
    hashchange.forget();
    Ok(())
}

/// The filter a URL hash names: `#/active`, `#/completed`, or all.
fn filter_of(hash: &str) -> String {
    match hash {
        "#/active" => "active",
        "#/completed" => "completed",
        _ => "all",
    }
    .to_string()
}

/// Wait until the service worker controls the page: at once when it
/// already does, and otherwise at the next `controllerchange`, which the
/// worker's `clients.claim` fires.
async fn controlled(container: &ServiceWorkerContainer) -> Result<(), JsValue> {
    if container.controller().is_some() {
        return Ok(());
    }
    let changed = js_sys::Promise::new(&mut |resolve, _reject| {
        let options = web_sys::AddEventListenerOptions::new();
        options.set_once(true);
        let _ = container.add_event_listener_with_callback_and_add_event_listener_options(
            "controllerchange",
            &resolve,
            &options,
        );
    });
    // The worker can have claimed the page between the check and the
    // listener, in which case no event comes.
    if container.controller().is_some() {
        return Ok(());
    }
    // A worker that was already active claimed its clients when it
    // activated, and a hard reload leaves the page outside them: ask it
    // to claim again.
    let registration: web_sys::ServiceWorkerRegistration =
        JsFuture::from(container.ready()?).await?.dyn_into()?;
    if let Some(active) = registration.active() {
        let message = js_sys::Object::new();
        js_sys::Reflect::set(&message, &"kind".into(), &"claim".into())?;
        active.post_message(&message)?;
    }
    JsFuture::from(changed).await?;
    Ok(())
}

/// Record that the page reached `step` of its start.
fn boot_step(window: &Window, step: &str) {
    if let Some(root) = window
        .document()
        .and_then(|document| document.document_element())
    {
        let _ = root.set_attribute("data-boot", step);
    }
}

/// How long the page waits for the service worker to answer a message.
/// A route's compile takes seconds; a minute means the worker is gone.
const ANSWER_TIMEOUT_MS: i32 = 60_000;

/// Send `message` to the service worker and answer what it answers on
/// a message port.
///
/// # Errors
///
/// The exception of the browser API, a message when no worker controls
/// the page, the `failure` the worker answered, or a timeout when the
/// worker never answers.
pub async fn ask_worker(message: &JsValue) -> Result<JsValue, JsValue> {
    let window = web_sys::window().ok_or("the page has no window")?;
    let worker = window
        .navigator()
        .service_worker()
        .controller()
        .ok_or("no service worker controls the page")?;
    let channel = web_sys::MessageChannel::new()?;
    let answer = js_sys::Promise::new(&mut |resolve, reject| {
        let on_message = Closure::once_into_js(move |event: web_sys::MessageEvent| {
            let _ = resolve.call1(&JsValue::NULL, &event.data());
        });
        channel
            .port1()
            .set_onmessage(Some(on_message.unchecked_ref()));
        let on_timeout = Closure::once_into_js(move || {
            let error = JsValue::from_str("the service worker did not answer");
            let _ = reject.call1(&JsValue::NULL, &error);
        });
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            on_timeout.unchecked_ref(),
            ANSWER_TIMEOUT_MS,
        );
    });
    let transfer = js_sys::Array::of1(&channel.port2());
    worker.post_message_with_transferable(message, &transfer)?;
    let answer = JsFuture::from(answer).await;
    channel.port1().set_onmessage(None);
    let answer = answer?;
    if answer.is_object() && !js_sys::Array::is_array(&answer) {
        let error = js_sys::Reflect::get(&answer, &"failure".into())?;
        if !error.is_undefined() {
            return Err(error);
        }
    }
    Ok(answer)
}

/// `window.demo`: the hooks the demo's browser tests drive the host
/// framework through.
///
/// - `compileCheck(request)` compiles a program on the page, against
///   the world Zena derives, instantiates it, and calls an export with
///   no arguments. `workerCompileCheck(request)` does the same in the
///   service worker. A request holds `source`, `entry`, `export`, and
///   optionally `files`, an object of paths to sources beside the entry.
/// - `define(tag, source)` defines an element.
/// - `defineRoute(pattern, source)` defines a route in the worker.
/// - `elements()` answers the status of each element, and `routes()`
///   the status of each route.
/// - `restart(tag)` compiles the element again in a new store.
fn install_hooks(window: &Window, context: Rc<Context>) -> Result<(), JsValue> {
    let hooks = js_sys::Object::new();
    let set = |name: &str, function: &JsValue| {
        js_sys::Reflect::set(&hooks, &JsValue::from_str(name), function)
    };

    let page_context = context.clone();
    let compile_check =
        Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(move |request: JsValue| {
            let context = page_context.clone();
            future_to_promise(
                async move { Ok(crate::worker::compile_check(&context, &request).await) },
            )
        });
    set("compileCheck", compile_check.as_ref())?;
    compile_check.forget();

    let worker_check = Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(|request: JsValue| {
        future_to_promise(async move {
            let message = js_sys::Object::assign(&js_sys::Object::new(), &request.unchecked_into());
            js_sys::Reflect::set(&message, &"kind".into(), &"compile-check".into())?;
            ask_worker(&message).await
        })
    });
    set("workerCompileCheck", worker_check.as_ref())?;
    worker_check.forget();

    let define =
        Closure::<dyn Fn(String, String) -> js_sys::Promise>::new(|tag: String, source: String| {
            future_to_promise(async move {
                elements::define_element(&tag, &source)
                    .await
                    .map(|()| JsValue::TRUE)
                    .map_err(|error| JsValue::from_str(&error))
            })
        });
    set("define", define.as_ref())?;
    define.forget();

    let define_route = Closure::<dyn Fn(String, String) -> js_sys::Promise>::new(
        |pattern: String, source: String| {
            future_to_promise(async move {
                let message = js_sys::Object::new();
                js_sys::Reflect::set(&message, &"kind".into(), &"define-route".into())?;
                js_sys::Reflect::set(&message, &"pattern".into(), &pattern.into())?;
                js_sys::Reflect::set(&message, &"source".into(), &source.into())?;
                ask_worker(&message).await
            })
        },
    );
    set("defineRoute", define_route.as_ref())?;
    define_route.forget();

    let statuses = Closure::<dyn Fn() -> JsValue>::new(|| {
        elements::statuses()
            .iter()
            .map(shelf::element_status_value)
            .collect::<js_sys::Array>()
            .into()
    });
    set("elements", statuses.as_ref())?;
    statuses.forget();

    let routes = Closure::<dyn Fn() -> js_sys::Promise>::new(|| {
        future_to_promise(async move {
            let message = js_sys::Object::new();
            js_sys::Reflect::set(&message, &"kind".into(), &"routes-status".into())?;
            ask_worker(&message).await
        })
    });
    set("routes", routes.as_ref())?;
    routes.forget();

    let restart = Closure::<dyn Fn(String) -> js_sys::Promise>::new(|tag: String| {
        future_to_promise(async move {
            elements::restart(&tag)
                .await
                .map(|()| JsValue::TRUE)
                .map_err(|error| JsValue::from_str(&error))
        })
    });
    set("restart", restart.as_ref())?;
    restart.forget();

    js_sys::Reflect::set(window, &"demo".into(), &hooks)?;
    Ok(())
}

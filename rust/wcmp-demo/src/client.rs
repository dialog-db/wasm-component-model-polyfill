// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `wasi:http/client@0.3.0` on the page: `send` with `fetch`.
//!
//! An element builds a `wasi:http` request with the subset of
//! [`http_types`], and `send` turns it into a browser `Request` and
//! fetches it. The service worker intercepts that fetch and gives it to
//! a route component, so each request is a `wasi:http` request when it
//! leaves a component and when it reaches one. The browser's response
//! comes back as a `wasi:http` response whose body the element reads as
//! a stream.

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use wcmp::{Accessor, Component, Error, ExternType, InterfaceIdentifier, Linker, Val};

use crate::http_types::{self, Body, Fields, HttpHost, Request, Response};

/// The interface this module defines.
pub const CLIENT: &str = "wasi:http/client@0.3.0";

/// Define [`CLIENT`] in `linker`, with the type `component` imports it
/// under. A component that does not import the interface leaves the
/// linker as it was.
///
/// # Errors
///
/// The polyfill's error when it refuses the definition.
pub fn define<T: HttpHost>(linker: &mut Linker<T>, component: &Component) -> Result<(), Error> {
    let Some(ty) = component.imports.iter().find_map(|import| {
        let ExternType::Instance(instance) = &import.ty else {
            return None;
        };
        if import.name.to_string() != CLIENT {
            return None;
        }
        instance.items.iter().find_map(|item| match &item.ty {
            ExternType::Function(ty) if item.name == "send" => Some(ty.clone()),
            _ => None,
        })
    }) else {
        return Ok(());
    };
    let identifier: InterfaceIdentifier = CLIENT.parse().expect("the interface's name parses");
    linker.instance(&identifier).func_new_concurrent(
        "send",
        ty,
        |accessor: &Accessor<T>, args: Vec<Val>| {
            let accessor = accessor.clone();
            async move {
                let answer = match send(&accessor, args).await {
                    Ok(rep) => {
                        let handle =
                            accessor.with(|store| http_types::response_handle(store, rep))??;
                        Val::Result(Ok(Some(Box::new(Val::Own(handle)))))
                    }
                    Err(reason) => Val::Result(Err(Some(Box::new(internal_error(&reason))))),
                };
                Ok(vec![answer])
            }
        },
    )
}

/// Send the request `args` holds with `fetch`, and answer the
/// representation of the response the host keeps, or why there is
/// none.
async fn send<T: HttpHost>(accessor: &Accessor<T>, args: Vec<Val>) -> Result<u32, String> {
    let rep = match args.first() {
        Some(Val::Own(handle)) => handle.rep(),
        other => return Err(format!("`send` was given {other:?}")),
    };
    let (request, body) = accessor
        .with(|store| {
            let request =
                store.data_mut().http().take_request(rep).ok_or_else(|| {
                    "`send` was given a request the host does not hold".to_string()
                })?;
            let Request {
                method,
                path_with_query,
                headers,
                body,
                ..
            } = request;
            let body = http_types::read_body(store, body).map_err(|error| error.to_string())?;
            Ok::<_, String>(((method, path_with_query, headers), body))
        })
        .map_err(|error| error.to_string())??;
    let body = body.await;
    let (method, path, headers) = request;
    let response = fetch(&method, path.as_deref().unwrap_or("/"), &headers, body)
        .await
        .map_err(|error| format!("{error:?}"))?;
    accessor
        .with(|store| store.data_mut().http().insert_response(response))
        .map_err(|error| error.to_string())
}

/// Fetch `path` with `method`, `headers`, and `body`, and read the whole
/// response.
async fn fetch(
    method: &str,
    path: &str,
    headers: &Fields,
    body: Vec<u8>,
) -> Result<Response, JsValue> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    let browser_headers = web_sys::Headers::new()?;
    for (name, value) in &headers.entries {
        browser_headers.append(name, &String::from_utf8_lossy(value))?;
    }
    init.set_headers(&browser_headers);
    if !body.is_empty() {
        init.set_body(&js_sys::Uint8Array::from(body.as_slice()));
    }
    let request = web_sys::Request::new_with_str_and_init(path, &init)?;
    let global: JsValue = js_sys::global().into();
    let fetch: js_sys::Function = js_sys::Reflect::get(&global, &"fetch".into())?.dyn_into()?;
    let response: web_sys::Response =
        JsFuture::from(js_sys::Promise::from(fetch.call1(&global, &request)?))
            .await?
            .dyn_into()?;
    let mut fields = Fields::default();
    let entries = js_sys::try_iter(&response.headers())?.ok_or("headers are not iterable")?;
    for entry in entries {
        let entry: js_sys::Array = entry?.dyn_into()?;
        let name = entry.get(0).as_string().unwrap_or_default();
        let value = entry.get(1).as_string().unwrap_or_default();
        fields.entries.push((name, value.into_bytes()));
    }
    fields.immutable = true;
    let buffer = JsFuture::from(response.array_buffer()?).await?;
    Ok(Response {
        status: response.status(),
        headers: fields,
        body: Body::Bytes(js_sys::Uint8Array::new(&buffer).to_vec()),
    })
}

/// `error-code.internal-error` with `reason`.
fn internal_error(reason: &str) -> Val {
    Val::Variant {
        discriminant: "internal-error".to_string(),
        payload: Some(Box::new(Val::Option(Some(Box::new(Val::String(
            reason.to_string(),
        )))))),
    }
}

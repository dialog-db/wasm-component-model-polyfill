// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A subset of `wasi:http/types@0.3.0`, defined on a polyfill
//! [`Linker`], that serves both contexts: the page, whose elements send
//! requests through `wasi:http/client`, and the service worker, whose
//! routes answer them through `wasi:http/handler`.
//!
//! The subset is what the demo's authoring library and Zena's standard
//! library call: fields, requests, and responses, and their bodies as
//! streams. Every other function of the interface is in the linker, so a
//! component that imports the whole interface links, and it returns an
//! error when it is called. So a change in Zena's output shows as a
//! failure, not a silent wrong answer.
//!
//! The host keeps what each resource holds in an [`HttpTable`] in the
//! store's data, by the resource's representation. A body that crosses
//! the host is whole bytes: the host reads a guest's body stream to its
//! end, and writes a body to a guest as one stream that ends.
//!
//! The function types come from the component that imports the
//! interface, so the definitions here never restate the WIT; the host
//! checks each value it is given against the shape it reads.

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use wcmp::{
    Component, ComponentValue, Error, ExternType, FunctionType, FutureConsumer, FutureReader,
    HostCall, HostResource, InterfaceIdentifier, Linker, ResourceHandle, Source, StoreContext,
    StreamConsumer, StreamReader, StreamResult, Val, ValueType,
};

mod body;
mod fields;
mod http_host;
mod http_table;
mod request;
mod response;
mod types;

pub use body::Body;
pub use fields::Fields;
pub use http_host::HttpHost;
pub use http_table::HttpTable;
pub use request::Request;
pub use response::Response;
pub use types::Types;

/// The interface this module defines.
pub const TYPES: &str = "wasi:http/types@0.3.0";

/// A handle the host makes for the response of `rep`, which it answers a
/// guest's `wasi:http/client.send` with.
///
/// # Errors
///
/// The polyfill's error when it cannot make the handle.
pub fn response_handle<T: HttpHost>(
    store: &mut StoreContext<'_, T>,
    rep: u32,
) -> Result<ResourceHandle, Error> {
    let types = store.data_mut().http().types()?;
    store.resource_new(types.response, rep)
}

/// Define [`TYPES`] in `linker`, with the types `component` imports it
/// under, and answer the resource types for the store's
/// [`HttpTable`]. A component that does not import the interface leaves
/// the linker as it was, and has no resource types.
///
/// # Errors
///
/// The polyfill's error when it refuses a definition.
pub fn define<T: HttpHost>(
    linker: &mut Linker<T>,
    component: &Component,
) -> Result<Option<Types>, Error> {
    let Some(items) = component.imports.iter().find_map(|import| {
        match (&import.ty, import.name.to_string() == TYPES) {
            (ExternType::Instance(instance), true) => Some(instance.items.clone()),
            _ => None,
        }
    }) else {
        return Ok(None);
    };
    let identifier: InterfaceIdentifier = TYPES.parse().expect("the interface's name parses");
    let mut types = linker.instance(&identifier);
    let mut resources = HashMap::new();
    let mut hosts = HashMap::new();
    for name in RESOURCES {
        let label = name.to_string();
        let host = HostResource::new(move |data: &mut T, rep| {
            let table = data.http();
            match label.as_str() {
                "fields" => drop(table.fields.remove(&rep)),
                "request" => drop(table.requests.remove(&rep)),
                "response" => drop(table.responses.remove(&rep)),
                _ => {}
            }
            Ok(())
        });
        resources.insert(name, types.resource_with(name, host.clone())?);
        hosts.insert(name, host);
    }
    let registered = Types {
        fields: resources["fields"],
        request: resources["request"],
        response: resources["response"],
    };
    for item in &items {
        let ExternType::Function(ty) = &item.ty else {
            continue;
        };
        let name = item.name.clone();
        let ty: FunctionType = ty.clone();
        let function = name.clone();
        types.func_new(name, ty, move |mut call, args, results| {
            let answer = call_types(&mut call, &function, args)?;
            if let (Some(answer), Some(slot)) = (answer, results.first_mut()) {
                *slot = answer;
            }
            Ok(())
        })?;
    }
    // Another interface of `wasi:http`, such as `client` or `handler`,
    // names the resources it uses from `types`. Each such name is the
    // same resource type, so it is registered with the same host
    // resource.
    for import in &component.imports {
        let ExternType::Instance(instance) = &import.ty else {
            continue;
        };
        let name = import.name.to_string();
        if name == TYPES {
            continue;
        }
        let Ok(identifier) = name.parse::<InterfaceIdentifier>() else {
            continue;
        };
        for item in &instance.items {
            let (ExternType::Resource(_) | ExternType::ResourceEquals(_)) = &item.ty else {
                continue;
            };
            if let Some(host) = hosts.get(item.name.as_str()) {
                linker
                    .instance(&identifier)
                    .resource_with(item.name.clone(), host.clone())?;
            }
        }
    }
    Ok(Some(registered))
}

/// The resources of [`TYPES`].
const RESOURCES: [&str; 4] = ["fields", "request", "response", "request-options"];

/// What a function of [`TYPES`] that the subset leaves out answers.
fn unsupported(function: &str) -> Error {
    Error::Unsupported {
        feature: format!("`{TYPES}#{function}` in the demo's subset of `wasi:http`"),
    }
}

/// The error for an argument of the wrong shape.
fn malformed(function: &str, args: &[Val]) -> Error {
    Error::Internal {
        message: format!("`{TYPES}#{function}` was given {args:?}"),
    }
}

/// The representation of a handle argument.
fn rep_of(val: &Val) -> Option<u32> {
    match val {
        Val::Own(handle) | Val::Borrow(handle) => Some(handle.rep()),
        _ => None,
    }
}

/// `result<_, E>` that is `ok`.
fn ok_unit() -> Val {
    Val::Result(Ok(None))
}

/// Run the function `function` of [`TYPES`] with `args`, and answer its
/// one result, if it has one.
fn call_types<T: HttpHost>(
    call: &mut HostCall<'_, T>,
    function: &str,
    args: &[Val],
) -> Result<Option<Val>, Error> {
    let bad = || malformed(function, args);
    Ok(Some(match function {
        "[constructor]fields" => {
            let table = call.data_mut().http();
            let rep = table.rep();
            table.fields.insert(rep, Fields::default());
            let types = table.types()?;
            Val::Own(call.resource_new(types.fields, rep)?)
        }
        "[static]fields.from-list" => {
            let [Val::List(entries)] = args else {
                return Err(bad());
            };
            let mut fields = Fields::default();
            for entry in entries.iter() {
                let Val::Tuple(pair) = entry else {
                    return Err(bad());
                };
                let [Val::String(name), value] = &pair[..] else {
                    return Err(bad());
                };
                fields
                    .entries
                    .push((name.clone(), bytes_of(value).ok_or_else(bad)?));
            }
            let table = call.data_mut().http();
            let rep = table.rep();
            table.fields.insert(rep, fields);
            let types = table.types()?;
            Val::Result(Ok(Some(Box::new(Val::Own(
                call.resource_new(types.fields, rep)?,
            )))))
        }
        "[method]fields.get" => {
            let (fields, name) = fields_and_name(call, args).ok_or_else(bad)?;
            Val::List(
                fields
                    .entries
                    .iter()
                    .filter(|(field, _)| field.eq_ignore_ascii_case(&name))
                    .map(|(_, value)| bytes_val(value))
                    .collect(),
            )
        }
        "[method]fields.has" => {
            let (fields, name) = fields_and_name(call, args).ok_or_else(bad)?;
            Val::Bool(
                fields
                    .entries
                    .iter()
                    .any(|(field, _)| field.eq_ignore_ascii_case(&name)),
            )
        }
        "[method]fields.copy-all" => {
            let rep = args.first().and_then(rep_of).ok_or_else(bad)?;
            let fields = call
                .data_mut()
                .http()
                .fields
                .get(&rep)
                .cloned()
                .ok_or_else(bad)?;
            Val::List(
                fields
                    .entries
                    .iter()
                    .map(|(name, value)| {
                        Val::Tuple(Box::new([Val::String(name.clone()), bytes_val(value)]))
                    })
                    .collect(),
            )
        }
        "[method]fields.append" | "[method]fields.set" | "[method]fields.delete" => {
            let rep = args.first().and_then(rep_of).ok_or_else(bad)?;
            let table = call.data_mut().http();
            let fields = table.fields.get_mut(&rep).ok_or_else(bad)?;
            if fields.immutable {
                return Ok(Some(header_error("immutable")));
            }
            let Some(Val::String(name)) = args.get(1) else {
                return Err(bad());
            };
            let name = name.to_ascii_lowercase();
            match function {
                "[method]fields.append" => {
                    let value = args.get(2).and_then(bytes_of).ok_or_else(bad)?;
                    fields.entries.push((name, value));
                }
                "[method]fields.set" => {
                    let Some(Val::List(values)) = args.get(2) else {
                        return Err(bad());
                    };
                    fields
                        .entries
                        .retain(|(field, _)| !field.eq_ignore_ascii_case(&name));
                    for value in values.iter() {
                        fields
                            .entries
                            .push((name.clone(), bytes_of(value).ok_or_else(bad)?));
                    }
                }
                _ => fields
                    .entries
                    .retain(|(field, _)| !field.eq_ignore_ascii_case(&name)),
            }
            ok_unit()
        }
        "[method]fields.clone" => {
            let rep = args.first().and_then(rep_of).ok_or_else(bad)?;
            let table = call.data_mut().http();
            let mut fields = table.fields.get(&rep).cloned().ok_or_else(bad)?;
            fields.immutable = false;
            let clone = table.rep();
            table.fields.insert(clone, fields);
            let types = table.types()?;
            Val::Own(call.resource_new(types.fields, clone)?)
        }
        "[static]request.new" => {
            let [headers, contents, trailers, _options] = args else {
                return Err(bad());
            };
            discard::<TrailersOutcome, T>(call.store(), trailers)?;
            let headers_rep = rep_of(headers).ok_or_else(bad)?;
            let body = match contents {
                Val::Option(None) => Body::Empty,
                Val::Option(Some(stream)) => Body::Stream(stream.as_ref().clone()),
                _ => return Err(bad()),
            };
            let table = call.data_mut().http();
            let mut headers = table.fields.remove(&headers_rep).ok_or_else(bad)?;
            headers.immutable = true;
            let rep = table.insert_request(Request {
                method: "GET".to_string(),
                scheme: None,
                authority: None,
                path_with_query: None,
                headers,
                body,
            });
            let types = table.types()?;
            let request = Val::Own(call.resource_new(types.request, rep)?);
            let transmitted = FutureReader::new(call.store(), async {
                Ok::<_, Error>(ErrorCodeOutcome::ok())
            })?;
            Val::Tuple(Box::new([request, transmitted.to_val()]))
        }
        "[method]request.get-method" => {
            let request = request(call, args).ok_or_else(bad)?;
            method_val(&request.method)
        }
        "[method]request.set-method" => {
            let method = args.get(1).and_then(method_of).ok_or_else(bad)?;
            request_mut(call, args).ok_or_else(bad)?.method = method;
            ok_unit()
        }
        "[method]request.get-path-with-query" => {
            option_string(request(call, args).ok_or_else(bad)?.path_with_query.clone())
        }
        "[method]request.set-path-with-query" => {
            let value = args.get(1).and_then(string_option).ok_or_else(bad)?;
            request_mut(call, args).ok_or_else(bad)?.path_with_query = value;
            ok_unit()
        }
        "[method]request.get-authority" => {
            option_string(request(call, args).ok_or_else(bad)?.authority.clone())
        }
        "[method]request.set-authority" => {
            let value = args.get(1).and_then(string_option).ok_or_else(bad)?;
            request_mut(call, args).ok_or_else(bad)?.authority = value;
            ok_unit()
        }
        "[method]request.get-scheme" => {
            let scheme = request(call, args).ok_or_else(bad)?.scheme.clone();
            Val::Option(scheme.map(|scheme| Box::new(scheme_val(&scheme))))
        }
        "[method]request.set-scheme" => {
            let value = match args.get(1) {
                Some(Val::Option(None)) => None,
                Some(Val::Option(Some(scheme))) => Some(scheme_of(scheme).ok_or_else(bad)?),
                _ => return Err(bad()),
            };
            request_mut(call, args).ok_or_else(bad)?.scheme = value;
            ok_unit()
        }
        "[method]request.get-headers" => {
            let headers = request(call, args).ok_or_else(bad)?.headers.clone();
            new_fields(call, headers)?
        }
        "[method]response.get-headers" => {
            let headers = response(call, args).ok_or_else(bad)?.headers.clone();
            new_fields(call, headers)?
        }
        "[method]request.get-options" => Val::Option(None),
        "[static]response.new" => {
            let [headers, contents, trailers] = args else {
                return Err(bad());
            };
            discard::<TrailersOutcome, T>(call.store(), trailers)?;
            let headers_rep = rep_of(headers).ok_or_else(bad)?;
            let body = match contents {
                Val::Option(None) => Body::Empty,
                Val::Option(Some(stream)) => Body::Stream(stream.as_ref().clone()),
                _ => return Err(bad()),
            };
            let table = call.data_mut().http();
            let mut headers = table.fields.remove(&headers_rep).ok_or_else(bad)?;
            headers.immutable = true;
            let rep = table.insert_response(Response {
                status: 200,
                headers,
                body,
            });
            let types = table.types()?;
            let response = Val::Own(call.resource_new(types.response, rep)?);
            let transmitted = FutureReader::new(call.store(), async {
                Ok::<_, Error>(ErrorCodeOutcome::ok())
            })?;
            Val::Tuple(Box::new([response, transmitted.to_val()]))
        }
        "[method]response.get-status-code" => {
            Val::U16(response(call, args).ok_or_else(bad)?.status)
        }
        "[method]response.set-status-code" => {
            let Some(Val::U16(status)) = args.get(1) else {
                return Err(bad());
            };
            let status = *status;
            let rep = args.first().and_then(rep_of).ok_or_else(bad)?;
            call.data_mut()
                .http()
                .responses
                .get_mut(&rep)
                .ok_or_else(bad)?
                .status = status;
            ok_unit()
        }
        "[static]request.consume-body" | "[static]response.consume-body" => {
            let rep = args.first().and_then(rep_of).ok_or_else(bad)?;
            // How the guest's receipt of the body went: read, and kept
            // nowhere.
            discard::<ErrorCodeOutcome, T>(call.store(), args.get(1).ok_or_else(bad)?)?;
            let table = call.data_mut().http();
            let body = if function == "[static]request.consume-body" {
                table.requests.remove(&rep).map(|request| request.body)
            } else {
                table.responses.remove(&rep).map(|response| response.body)
            }
            .ok_or_else(bad)?;
            let stream = match body {
                Body::Stream(stream) => stream,
                // A vector delivers its bytes on its first read and ends
                // the stream.
                Body::Empty => StreamReader::new(call.store(), Vec::<u8>::new())?.to_val(),
                Body::Bytes(bytes) => StreamReader::new(call.store(), bytes)?.to_val(),
            };
            let trailers = FutureReader::new(call.store(), async {
                Ok::<_, Error>(TrailersOutcome::none())
            })?;
            Val::Tuple(Box::new([stream, trailers.to_val()]))
        }
        _ => return Err(unsupported(function)),
    }))
}

/// The fields of the handle in `args[0]` and the field name in
/// `args[1]`, lower case.
fn fields_and_name<T: HttpHost>(
    call: &mut HostCall<'_, T>,
    args: &[Val],
) -> Option<(Fields, String)> {
    let rep = args.first().and_then(rep_of)?;
    let Some(Val::String(name)) = args.get(1) else {
        return None;
    };
    let fields = call.data_mut().http().fields.get(&rep)?.clone();
    Some((fields, name.to_ascii_lowercase()))
}

/// The request of the handle in `args[0]`.
fn request<'c, T: HttpHost>(call: &'c mut HostCall<'_, T>, args: &[Val]) -> Option<&'c Request> {
    let rep = args.first().and_then(rep_of)?;
    call.data_mut().http().requests.get(&rep)
}

/// The request of the handle in `args[0]`, mutably.
fn request_mut<'c, T: HttpHost>(
    call: &'c mut HostCall<'_, T>,
    args: &[Val],
) -> Option<&'c mut Request> {
    let rep = args.first().and_then(rep_of)?;
    call.data_mut().http().requests.get_mut(&rep)
}

/// The response of the handle in `args[0]`.
fn response<'c, T: HttpHost>(call: &'c mut HostCall<'_, T>, args: &[Val]) -> Option<&'c Response> {
    let rep = args.first().and_then(rep_of)?;
    call.data_mut().http().responses.get(&rep)
}

/// A new immutable `fields` resource that holds `fields`.
fn new_fields<T: HttpHost>(call: &mut HostCall<'_, T>, mut fields: Fields) -> Result<Val, Error> {
    fields.immutable = true;
    let table = call.data_mut().http();
    let rep = table.rep();
    table.fields.insert(rep, fields);
    let types = table.types()?;
    Ok(Val::Own(call.resource_new(types.fields, rep)?))
}

/// `err` with the `header-error` case `case`.
fn header_error(case: &str) -> Val {
    Val::Result(Err(Some(Box::new(Val::Variant {
        discriminant: case.to_string(),
        payload: None,
    }))))
}

/// The bytes of a `list<u8>`.
fn bytes_of(val: &Val) -> Option<Vec<u8>> {
    let Val::List(items) = val else {
        return None;
    };
    items
        .iter()
        .map(|item| match item {
            Val::U8(byte) => Some(*byte),
            _ => None,
        })
        .collect()
}

/// A `list<u8>` of `bytes`.
fn bytes_val(bytes: &[u8]) -> Val {
    Val::List(bytes.iter().map(|byte| Val::U8(*byte)).collect())
}

/// `option<string>`.
fn option_string(value: Option<String>) -> Val {
    Val::Option(value.map(|value| Box::new(Val::String(value))))
}

/// The value of an `option<string>`.
fn string_option(val: &Val) -> Option<Option<String>> {
    match val {
        Val::Option(None) => Some(None),
        Val::Option(Some(text)) => match text.as_ref() {
            Val::String(text) => Some(Some(text.clone())),
            _ => None,
        },
        _ => None,
    }
}

/// The cases of `method`, in the order WIT declares them, and the
/// method each names.
const METHODS: [(&str, &str); 9] = [
    ("get", "GET"),
    ("head", "HEAD"),
    ("post", "POST"),
    ("put", "PUT"),
    ("delete", "DELETE"),
    ("connect", "CONNECT"),
    ("options", "OPTIONS"),
    ("trace", "TRACE"),
    ("patch", "PATCH"),
];

/// The `method` variant of `method`.
fn method_val(method: &str) -> Val {
    match METHODS.iter().find(|(_, name)| *name == method) {
        Some((case, _)) => Val::Variant {
            discriminant: (*case).to_string(),
            payload: None,
        },
        None => Val::Variant {
            discriminant: "other".to_string(),
            payload: Some(Box::new(Val::String(method.to_string()))),
        },
    }
}

/// The method a `method` variant names.
fn method_of(val: &Val) -> Option<String> {
    let Val::Variant {
        discriminant,
        payload,
    } = val
    else {
        return None;
    };
    if discriminant == "other" {
        return match payload.as_deref() {
            Some(Val::String(name)) => Some(name.clone()),
            _ => None,
        };
    }
    METHODS
        .iter()
        .find(|(case, _)| case == discriminant)
        .map(|(_, name)| (*name).to_string())
}

/// The `scheme` variant of `scheme`.
fn scheme_val(scheme: &str) -> Val {
    match scheme {
        "http" => Val::Variant {
            discriminant: "HTTP".to_string(),
            payload: None,
        },
        "https" => Val::Variant {
            discriminant: "HTTPS".to_string(),
            payload: None,
        },
        other => Val::Variant {
            discriminant: "other".to_string(),
            payload: Some(Box::new(Val::String(other.to_string()))),
        },
    }
}

/// The scheme a `scheme` variant names.
fn scheme_of(val: &Val) -> Option<String> {
    let Val::Variant {
        discriminant,
        payload,
    } = val
    else {
        return None;
    };
    match (discriminant.as_str(), payload.as_deref()) {
        ("HTTP", None) => Some("http".to_string()),
        ("HTTPS", None) => Some("https".to_string()),
        ("other", Some(Val::String(scheme))) => Some(scheme.clone()),
        _ => None,
    }
}

/// The bytes a guest wrote to a body stream, and whether the stream has
/// ended.
#[derive(Default)]
struct Collected {
    bytes: Vec<u8>,
    ended: bool,
    waker: Option<Waker>,
}

/// A consumer that collects every byte of a body stream.
struct Collects(Arc<Mutex<Collected>>);

impl<T: 'static> StreamConsumer<T> for Collects {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, T>,
        mut source: Source<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let mut bytes = Vec::new();
        let count = source.remaining();
        source.read(store, &mut bytes, count)?;
        lock(&self.0).bytes.extend(bytes);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for Collects {
    /// The store drops the consumer once the guest drops the stream's
    /// writable end: the body is whole.
    fn drop(&mut self) {
        let waker = {
            let mut collected = lock(&self.0);
            collected.ended = true;
            collected.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Lock `shared`, whatever a panic elsewhere left in it.
fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Start reading `body` in `store`, and answer a future of its bytes,
/// which resolves once the guest has written all of them. A body the
/// host holds resolves at once.
///
/// The future resolves only while a driver runs turns of the store,
/// such as the `run_concurrent` around the call that made the body.
///
/// # Errors
///
/// The polyfill's error when the stream cannot be read.
pub fn read_body<T: 'static>(
    store: &mut StoreContext<'_, T>,
    body: Body,
) -> Result<impl core::future::Future<Output = Vec<u8>> + use<T>, Error> {
    let collected = Arc::new(Mutex::new(Collected::default()));
    match body {
        Body::Empty => lock(&collected).ended = true,
        Body::Bytes(bytes) => {
            let mut collected = lock(&collected);
            collected.bytes = bytes;
            collected.ended = true;
        }
        Body::Stream(stream) => {
            StreamReader::<u8>::from_val(&stream)?.pipe(store, Collects(collected.clone()))?;
        }
    }
    Ok(core::future::poll_fn(move |cx| {
        let mut collected = lock(&collected);
        if collected.ended {
            Poll::Ready(core::mem::take(&mut collected.bytes))
        } else {
            collected.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }))
}

/// A consumer that reads the one value of a future a guest handed the
/// host, and keeps none of it.
///
/// A guest that hands the host the readable end of a future, such as a
/// request's trailers, writes the value and waits for that write to
/// complete before its task can end. A host that only dropped the end
/// would leave the write, and with it the task, waiting forever. So the
/// host reads each such future, even where the subset has no use for
/// its value.
struct Discards<T>(core::marker::PhantomData<fn() -> T>);

impl<T: ComponentValue + 'static, D: 'static> FutureConsumer<D> for Discards<T> {
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        mut source: Source<'_, T>,
        _finish: bool,
    ) -> Poll<Result<(), Error>> {
        let mut values = Vec::new();
        source.read(store, &mut values, 1)?;
        Poll::Ready(Ok(()))
    }
}

/// Read and discard the future `val` a guest handed the host, whose
/// payload is `T`.
fn discard<T: ComponentValue + 'static, D: 'static>(
    store: &mut StoreContext<'_, D>,
    val: &Val,
) -> Result<(), Error> {
    FutureReader::<T>::from_val(val)?.pipe(store, Discards::<T>(core::marker::PhantomData))
}

/// The payload of a future `result<_, error-code>` the host resolves:
/// only ever `ok`.
struct ErrorCodeOutcome;

impl ErrorCodeOutcome {
    fn ok() -> Self {
        ErrorCodeOutcome
    }
}

impl ComponentValue for ErrorCodeOutcome {
    fn value_type() -> ValueType {
        ValueType::Result(wcmp::ResultType::new(None, Some(error_code())))
    }

    /// Any outcome a guest writes, which the host reads only to let the
    /// guest's write complete.
    fn from_val(val: &Val) -> Result<Self, Error> {
        match val {
            Val::Result(_) => Ok(ErrorCodeOutcome),
            other => Err(Error::Internal {
                message: format!("not an outcome: {other:?}"),
            }),
        }
    }

    fn to_val(self) -> Val {
        Val::Result(Ok(None))
    }
}

/// The payload of a trailers future the host resolves:
/// `result<option<trailers>, error-code>`, only ever `ok(none)`.
struct TrailersOutcome;

impl TrailersOutcome {
    fn none() -> Self {
        TrailersOutcome
    }
}

impl ComponentValue for TrailersOutcome {
    fn value_type() -> ValueType {
        let fields = ValueType::Own(wcmp::ResourceType::new("fields"));
        ValueType::Result(wcmp::ResultType::new(
            Some(ValueType::Option(wcmp::OptionType::new(fields))),
            Some(error_code()),
        ))
    }

    /// Any trailers a guest writes, which the host reads only to let the
    /// guest's write complete. The subset carries no trailers.
    fn from_val(val: &Val) -> Result<Self, Error> {
        match val {
            Val::Result(_) => Ok(TrailersOutcome),
            other => Err(Error::Internal {
                message: format!("not a trailers outcome: {other:?}"),
            }),
        }
    }

    fn to_val(self) -> Val {
        Val::Result(Ok(Some(Box::new(Val::Option(None)))))
    }
}

/// `wasi:http/types.error-code`, as `wasi:http@0.3.0` declares it.
fn error_code() -> ValueType {
    use wcmp::{OptionType, PrimitiveType, RecordField, RecordType, VariantCase, VariantType};
    let option = |ty: ValueType| ValueType::Option(OptionType::new(ty));
    let primitive = ValueType::Primitive;
    let record = |fields: [(&str, PrimitiveType); 2]| {
        ValueType::Record(RecordType::new(
            fields
                .into_iter()
                .map(|(name, ty)| RecordField::new(name, option(primitive(ty)))),
        ))
    };
    let dns_error = record([
        ("rcode", PrimitiveType::String),
        ("info-code", PrimitiveType::U16),
    ]);
    let tls_alert = record([
        ("alert-id", PrimitiveType::U8),
        ("alert-message", PrimitiveType::String),
    ]);
    let field_size = record([
        ("field-name", PrimitiveType::String),
        ("field-size", PrimitiveType::U32),
    ]);
    let u32_size = || Some(option(primitive(PrimitiveType::U32)));
    let u64_size = || Some(option(primitive(PrimitiveType::U64)));
    let text = || Some(option(primitive(PrimitiveType::String)));
    let cases: [(&str, Option<ValueType>); 39] = [
        ("DNS-timeout", None),
        ("DNS-error", Some(dns_error)),
        ("destination-not-found", None),
        ("destination-unavailable", None),
        ("destination-IP-prohibited", None),
        ("destination-IP-unroutable", None),
        ("connection-refused", None),
        ("connection-terminated", None),
        ("connection-timeout", None),
        ("connection-read-timeout", None),
        ("connection-write-timeout", None),
        ("connection-limit-reached", None),
        ("TLS-protocol-error", None),
        ("TLS-certificate-error", None),
        ("TLS-alert-received", Some(tls_alert)),
        ("HTTP-request-denied", None),
        ("HTTP-request-length-required", None),
        ("HTTP-request-body-size", u64_size()),
        ("HTTP-request-method-invalid", None),
        ("HTTP-request-URI-invalid", None),
        ("HTTP-request-URI-too-long", None),
        ("HTTP-request-header-section-size", u32_size()),
        ("HTTP-request-header-size", Some(option(field_size.clone()))),
        ("HTTP-request-trailer-section-size", u32_size()),
        ("HTTP-request-trailer-size", Some(field_size.clone())),
        ("HTTP-response-incomplete", None),
        ("HTTP-response-header-section-size", u32_size()),
        ("HTTP-response-header-size", Some(field_size.clone())),
        ("HTTP-response-body-size", u64_size()),
        ("HTTP-response-trailer-section-size", u32_size()),
        ("HTTP-response-trailer-size", Some(field_size)),
        ("HTTP-response-transfer-coding", text()),
        ("HTTP-response-content-coding", text()),
        ("HTTP-response-timeout", None),
        ("HTTP-upgrade-failed", None),
        ("HTTP-protocol-error", None),
        ("loop-detected", None),
        ("configuration-error", None),
        ("internal-error", text()),
    ];
    ValueType::Variant(VariantType::new(
        cases
            .into_iter()
            .map(|(name, payload)| VariantCase::new(name, payload)),
    ))
}

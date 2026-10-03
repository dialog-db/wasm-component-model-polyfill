// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The routes of the service worker.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures::lock::Mutex;
use wcmp::{Accessor, Engine, Func, Linker, Store, Val};

use crate::compiler::{CompileError, Compiler};
use crate::glue::Compile;
use crate::http_types::{self, Body, Fields, HttpHost, HttpTable, Request, Types};
use crate::model::Storage;
use crate::model_host::{self, Model};
use crate::platform;
use crate::wasi;

use super::{HttpRequest, HttpResponse, RouteEvent, RouteHost, RouteStatus, matches};

/// The interface a route exports.
const HANDLER: &str = "wasi:http/handler@0.3.0";

/// A route's running instance.
struct Instance<S: Storage + 'static> {
    store: Store<RouteHost<S>>,
    handle: Func,
    types: Types,
}

/// One route.
struct Route<S: Storage + 'static> {
    status: RouteStatus,
    instance: Option<Instance<S>>,
    /// The source the route last started from: what a compile after a
    /// trap starts from again.
    running: Option<String>,
}

/// A route behind the lock that keeps one request at a time on it.
type Shared<S> = Rc<Mutex<Route<S>>>;

/// What the router tells of each compile and trap.
type Listener = Box<dyn Fn(RouteEvent)>;

/// The routes of the service worker.
pub struct Router<S: Storage + 'static> {
    engine: Engine,
    compiler: Rc<Mutex<Compiler>>,
    model: Arc<Model<S>>,
    routes: RefCell<Vec<(String, Shared<S>)>>,
    listener: RefCell<Option<Listener>>,
}

impl<S: Storage + 'static> Router<S> {
    /// A router that compiles with `compiler`, which it can share with
    /// the rest of its context, on `engine`, and whose routes reach
    /// `model`.
    pub fn new(engine: Engine, compiler: Rc<Mutex<Compiler>>, model: Arc<Model<S>>) -> Self {
        Router {
            engine,
            compiler,
            model,
            routes: RefCell::new(Vec::new()),
            listener: RefCell::new(None),
        }
    }

    /// Hear about each compile and each trap.
    pub fn listen(&self, listener: impl Fn(RouteEvent) + 'static) {
        *self.listener.borrow_mut() = Some(Box::new(listener));
    }

    /// Tell the listener about `event`.
    fn tell(&self, event: RouteEvent) {
        if let Some(listener) = &*self.listener.borrow() {
            listener(event);
        }
    }

    /// Define the route `pattern` from Zena `source`, which the demo
    /// ships as `shipped`, in place of any route with that pattern. The
    /// router compiles it on its first request.
    pub fn define_route(&self, pattern: &str, source: &str, shipped: &str) {
        let route = Route {
            status: RouteStatus {
                pattern: pattern.to_string(),
                source: source.to_string(),
                shipped: shipped.to_string(),
                ..RouteStatus::default()
            },
            instance: None,
            running: None,
        };
        let mut routes = self.routes.borrow_mut();
        routes.retain(|(defined, _)| defined != pattern);
        routes.push((pattern.to_string(), Rc::new(Mutex::new(route))));
    }

    /// The route `pattern`.
    fn route(&self, pattern: &str) -> Option<Rc<Mutex<Route<S>>>> {
        self.routes
            .borrow()
            .iter()
            .find(|(defined, _)| defined == pattern)
            .map(|(_, route)| route.clone())
    }

    /// The status of each route, in the order of definition.
    pub async fn statuses(&self) -> Vec<RouteStatus> {
        let routes: Vec<_> = self
            .routes
            .borrow()
            .iter()
            .map(|(_, route)| route.clone())
            .collect();
        let mut statuses = Vec::with_capacity(routes.len());
        for route in routes {
            statuses.push(route.lock().await.status.clone());
        }
        statuses
    }

    /// Compile the route `pattern` from `source` now, and run that from
    /// its next request on. When the compile fails, the instance that
    /// runs keeps serving, and the status carries the diagnostics.
    ///
    /// # Errors
    ///
    /// A message when no route has the pattern.
    pub async fn replace(&self, pattern: &str, source: &str) -> Result<RouteStatus, String> {
        let route = self
            .route(pattern)
            .ok_or_else(|| format!("no route has the pattern {pattern}"))?;
        let mut route = route.lock().await;
        route.status.source = source.to_string();
        // On a failure the status carries the diagnostics, and the
        // instance that runs keeps serving.
        if let Ok(instance) = self.start_from(&mut route.status, source).await {
            route.instance = Some(instance);
            route.running = Some(source.to_string());
        }
        self.tell(RouteEvent::Compiled(route.status.clone()));
        Ok(route.status.clone())
    }

    /// The pattern of the first route `path` matches.
    pub fn matching(&self, path: &str) -> Option<String> {
        self.routes
            .borrow()
            .iter()
            .find(|(pattern, _)| matches(pattern, path))
            .map(|(pattern, _)| pattern.clone())
    }

    /// Answer `request` with the first route that matches its path, or
    /// `None` when no route matches.
    #[tracing::instrument(level = "debug", name = "route request", skip_all, fields(method = %request.method, path = %request.path_with_query))]
    pub async fn handle(&self, request: HttpRequest) -> Option<HttpResponse> {
        let path = request
            .path_with_query
            .split('?')
            .next()
            .unwrap_or_default()
            .to_string();
        let pattern = self.matching(&path)?;
        let route = self.route(&pattern)?;
        let mut route = route.lock().await;
        if route.instance.is_none() {
            // After a trap the route starts again from the source that
            // ran; at first, from its source.
            let first = route
                .running
                .clone()
                .unwrap_or_else(|| route.status.source.clone());
            let started = match self.start_from(&mut route.status, &first).await {
                Ok(instance) => Ok((instance, first)),
                Err(reason) if first == route.status.shipped => Err(reason),
                Err(_) => {
                    // An edit that does not compile falls back to the
                    // shipped source. The status keeps the edit's
                    // diagnostics, for the shelf to show.
                    let diagnostics = route.status.diagnostics.clone();
                    let shipped = route.status.shipped.clone();
                    let fallback = self.start_from(&mut route.status, &shipped).await;
                    route.status.diagnostics = diagnostics;
                    fallback.map(|instance| (instance, shipped))
                }
            };
            self.tell(RouteEvent::Compiled(route.status.clone()));
            match started {
                Ok((instance, source)) => {
                    route.instance = Some(instance);
                    route.running = Some(source);
                }
                Err(reason) => return Some(HttpResponse::text(500, &reason)),
            }
        }
        let instance = route.instance.as_mut().expect("the route has an instance");
        match serve(instance, request).await {
            Ok(response) => Some(response),
            Err(trap) => {
                route.instance = None;
                route.status.trapped = Some(trap.clone());
                self.tell(RouteEvent::Trapped(route.status.clone()));
                Some(HttpResponse::text(
                    500,
                    &format!("the route {pattern} trapped: {trap}"),
                ))
            }
        }
    }

    /// Compile and instantiate the route `status` names from `source`,
    /// recording the times or the diagnostics in `status`.
    #[tracing::instrument(level = "debug", name = "route start", skip_all, fields(pattern = %status.pattern))]
    async fn start_from(
        &self,
        status: &mut RouteStatus,
        source: &str,
    ) -> Result<Instance<S>, String> {
        let compile = Compile::route(&status.pattern, source);
        status.compiles += 1;
        status.compiled_at = Some(platform::wall_millis());
        let compiled = match self.compiler.lock().await.compile(&compile.request()).await {
            Ok(compiled) => compiled,
            Err(error) => {
                if let CompileError::Diagnostics { millis, .. } = &error {
                    status.compile_ms = Some(*millis);
                }
                let text = error.to_string();
                status.diagnostics = Some(text.clone());
                return Err(text);
            }
        };
        status.compile_ms = Some(compiled.millis);
        status.diagnostics = None;
        status.trapped = None;
        let started = platform::now_millis();
        let component = wcmp::Component::new(&self.engine, &compiled.bytes)
            .await
            .map_err(|error| error.to_string())?;
        status.wasm_compile_ms = Some(platform::now_millis() - started);
        let started = platform::now_millis();
        let mut linker = Linker::new(&self.engine);
        wasi::define(&mut linker, &status.pattern).map_err(|error| error.to_string())?;
        let types = http_types::define(&mut linker, &component)
            .map_err(|error| error.to_string())?
            .ok_or("the route does not import wasi:http/types")?;
        model_host::define(&mut linker, &component).map_err(|error| error.to_string())?;
        let mut http = HttpTable::default();
        http.set_types(Some(types));
        let mut store = Store::new(
            &self.engine,
            RouteHost {
                http,
                model: self.model.clone(),
            },
        )
        .map_err(|error| error.to_string())?;
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .map_err(|error| error.to_string())?;
        let handle = instance
            .exports()
            .instance(HANDLER)
            .and_then(|handler| handler.func("handle"))
            .ok_or("the route exports no wasi:http/handler")?;
        status.instantiate_ms = Some(platform::now_millis() - started);
        Ok(Instance {
            store,
            handle,
            types,
        })
    }
}

/// Hand `request` to the route's `handle`, and read back the response
/// it answers, its body included.
///
/// # Errors
///
/// The text of the trap, or of the error, that stopped the call.
#[tracing::instrument(level = "debug", name = "route handle", skip_all)]
async fn serve<S: Storage + 'static>(
    instance: &mut Instance<S>,
    request: HttpRequest,
) -> Result<HttpResponse, String> {
    let rep = instance.store.data_mut().http().insert_request(Request {
        method: request.method,
        scheme: None,
        authority: None,
        path_with_query: Some(request.path_with_query),
        headers: Fields {
            entries: request
                .headers
                .into_iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.into_bytes()))
                .collect(),
            immutable: true,
        },
        body: Body::Bytes(request.body),
    });
    let handle_request = instance
        .store
        .resource_new(instance.types.request(), rep)
        .map_err(|error| error.to_string())?;
    let handle = &instance.handle;
    instance
        .store
        .run_concurrent(async |accessor: &Accessor<RouteHost<S>>| {
            let returned = handle
                .call_concurrent(accessor, &[Val::Own(handle_request)])
                .await
                .map_err(|error| platform::describe(&error))?;
            let response = match returned.first() {
                Some(Val::Result(Ok(Some(response)))) => match response.as_ref() {
                    Val::Own(response) => response.rep(),
                    other => return Err(format!("`handle` answered {other:?}")),
                },
                Some(Val::Result(Err(Some(code)))) => {
                    return Err(format!("`handle` answered the error {code:?}"));
                }
                other => return Err(format!("`handle` answered {other:?}")),
            };
            let (status, headers, body) = accessor
                .with(|store| {
                    let response = store
                        .data_mut()
                        .http()
                        .take_response(response)
                        .ok_or("`handle` answered a response the host does not hold")?;
                    let body = http_types::read_body(store, response.body)
                        .map_err(|error| error.to_string())?;
                    Ok::<_, String>((response.status, response.headers, body))
                })
                .map_err(|error| error.to_string())??;
            let body = body.await;
            Ok::<_, String>(HttpResponse {
                status,
                headers: headers
                    .entries
                    .into_iter()
                    .map(|(name, value)| (name, String::from_utf8_lossy(&value).into_owned()))
                    .collect(),
                body,
            })
        })
        .await
        .map_err(|error| platform::describe(&error))?
}

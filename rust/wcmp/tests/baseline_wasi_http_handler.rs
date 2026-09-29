//! Baseline tests for the WASI 0.3 HTTP handler fixture.
//!
//! `tests/corpus/fixtures/wasi-http/handler.wasm` is a component
//! wit-bindgen built against the `wasi:http@0.3.0` packages. It imports
//! `wasi:http/types@0.3.0` and exports `wasi:http/handler@0.3.0`, whose
//! `handle` is an `async func` that takes a `request` and answers with a
//! `response`. The handler consumes the request's body, a `stream<u8>`,
//! and its trailers, a `future<result<option<trailers>, error-code>>`,
//! and hands both straight to the response it builds. Beside it the
//! component exports `drain`, an `async func` that writes its bytes into
//! a `stream<u8>` of its own, reads them back, and resolves a
//! `future<u32>` of its own with the count beside them.
//!
//! The host supplies `wasi:http/types@0.3.0` through the linker: the
//! `request`, `fields`, and `response` resources with destructors, the
//! `fields` constructor, `request.consume-body`, and `response.new`.
//! Those are what the handler imports. It imports no `request.new`,
//! because a handler receives its request; the host builds the request
//! the way `request.new` would, from a body stream over a producer and
//! a trailers future that resolves only once that producer has ended,
//! so the trailers follow the last chunk of the body.
//!
//! `tests/corpus/fixtures/wasi-http-same-instance/handler.wasm` is the
//! same handler as first written, whose `drain` resolves a
//! `future<result<_, error-code>>` whose two ends it holds itself. The
//! Component Model traps such a copy, as a temporary rule, when the
//! payload is not a number type, and a test here holds the polyfill to
//! that trap until the rule is lifted.
//!
//! wit-bindgen's runtime links `task.cancel` in every `async` export and
//! `subtask.cancel` in every awaited import. The polyfill fails either
//! at the call, so a call that returns proves the handler's
//! cancellation paths were never reached.
//!
//! In the browser the body's producer awaits a JavaScript promise for
//! each chunk, which is not `Send`.

#![cfg(test)]

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use wcmp::{
    Accessor, Component, ComponentValue, Destination, Engine, Error, FunctionParameter,
    FunctionType, FutureConsumer, FutureReader, FutureType, HostResource, Instance,
    InterfaceIdentifier, Linker, OptionType, PrimitiveType, RecordField, RecordType,
    ResourceHandle, ResourceType, ResourceTypeId, ResultType, Source, Store, StoreContext,
    StreamConsumer, StreamProducer, StreamReader, StreamResult, StreamType, TupleType, Val,
    ValueType, VariantCase, VariantType,
};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The WASI 0.3 HTTP handler fixture.
const HANDLER: &[u8] = include_bytes!("corpus/fixtures/wasi-http/handler.wasm");

/// The handler as first written, whose `drain` copies a non-number
/// payload between two ends one instance holds.
const SAME_INSTANCE_HANDLER: &[u8] =
    include_bytes!("corpus/fixtures/wasi-http-same-instance/handler.wasm");

/// The interface the handler imports its types from.
const TYPES: &str = "wasi:http/types@0.3.0";

/// The interface the handler exports `handle` from.
const HANDLER_INTERFACE: &str = "wasi:http/handler@0.3.0";

/// The representation the host gives the request it hands `handle`.
const REQUEST_REP: u32 = 1;

/// The representation the host gives the trailers its request
/// carries.
const TRAILERS_REP: u32 = 2;

/// The first representation the host gives a resource the guest asks
/// it to create.
const FIRST_GUEST_REP: u32 = 100;

/// The chunks of the request's body, which the producer delivers one
/// at a time.
const BODY: [&str; 3] = ["hello, ", "wasi:http ", "0.3"];

/// The WIT type `option<T>`.
fn option(payload: ValueType) -> ValueType {
    ValueType::Option(OptionType::new(payload))
}

/// A primitive WIT type.
fn primitive(ty: PrimitiveType) -> ValueType {
    ValueType::Primitive(ty)
}

/// A WIT record whose fields all take the type `option<T>`.
fn record_of_options(fields: [(&str, PrimitiveType); 2]) -> ValueType {
    ValueType::Record(RecordType::new(
        fields
            .into_iter()
            .map(|(name, ty)| RecordField::new(name, option(primitive(ty)))),
    ))
}

/// `own<fields>`: `headers` and `trailers` are both the `fields`
/// resource.
fn fields() -> ValueType {
    ValueType::Own(ResourceType::new("fields"))
}

/// `wasi:http/types.error-code`, as `wasi:http@0.3.0` declares it.
fn error_code() -> ValueType {
    let dns_error = record_of_options([
        ("rcode", PrimitiveType::String),
        ("info-code", PrimitiveType::U16),
    ]);
    let tls_alert = record_of_options([
        ("alert-id", PrimitiveType::U8),
        ("alert-message", PrimitiveType::String),
    ]);
    let field_size = record_of_options([
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

/// A parameter of a host function.
fn parameter(name: &str, ty: ValueType) -> FunctionParameter {
    FunctionParameter {
        name: name.to_owned(),
        ty,
    }
}

/// The payload of a trailers future:
/// `result<option<trailers>, error-code>`. The error code stays a
/// [`Val`], because the handler never sends one.
#[derive(Debug, PartialEq)]
struct Trailers(Result<Option<ResourceHandle>, Val>);

impl ComponentValue for Trailers {
    fn value_type() -> ValueType {
        ValueType::Result(ResultType::new(Some(option(fields())), Some(error_code())))
    }

    fn from_val(val: &Val) -> Result<Self, Error> {
        match val {
            Val::Result(Ok(Some(some))) => match some.as_ref() {
                Val::Option(None) => Ok(Self(Ok(None))),
                Val::Option(Some(handle)) => match handle.as_ref() {
                    Val::Own(handle) => Ok(Self(Ok(Some(*handle)))),
                    other => Err(not_a_payload(other)),
                },
                other => Err(not_a_payload(other)),
            },
            Val::Result(Err(Some(code))) => Ok(Self(Err(code.as_ref().clone()))),
            other => Err(not_a_payload(other)),
        }
    }

    fn to_val(self) -> Val {
        Val::Result(match self.0 {
            Ok(handle) => Ok(Some(Box::new(Val::Option(
                handle.map(|handle| Box::new(Val::Own(handle))),
            )))),
            Err(code) => Err(Some(Box::new(code))),
        })
    }
}

/// The payload of the future that reports how a body was handled:
/// `result<_, error-code>`.
#[derive(Debug, PartialEq)]
struct Outcome(Result<(), Val>);

impl ComponentValue for Outcome {
    fn value_type() -> ValueType {
        ValueType::Result(ResultType::new(None, Some(error_code())))
    }

    fn from_val(val: &Val) -> Result<Self, Error> {
        match val {
            Val::Result(Ok(None)) => Ok(Self(Ok(()))),
            Val::Result(Err(Some(code))) => Ok(Self(Err(code.as_ref().clone()))),
            other => Err(not_a_payload(other)),
        }
    }

    fn to_val(self) -> Val {
        Val::Result(match self.0 {
            Ok(()) => Ok(None),
            Err(code) => Err(Some(Box::new(code))),
        })
    }
}

/// The error a typed payload answers for a value of another shape.
fn not_a_payload(val: &Val) -> Error {
    Error::Internal {
        message: format!("not a `wasi:http` future payload: {val:?}"),
    }
}

/// A request the host holds until the guest consumes its body.
struct Request {
    body: StreamReader<u8>,
    trailers: FutureReader<Trailers>,
}

/// A response the guest built through `response.new`.
struct Response {
    headers: ResourceHandle,
    body: Option<Val>,
    trailers: Val,
}

/// What the host's `wasi:http/types` keeps, shared by the linker's
/// functions and the test.
#[derive(Default)]
struct Host {
    /// The requests the host made, by representation.
    requests: HashMap<u32, Request>,
    /// The responses the guest made, by representation.
    responses: HashMap<u32, Response>,
    /// The handles the guest gave the host with a request it consumed.
    consumed: Vec<ResourceHandle>,
    /// The next representation for a resource the guest creates.
    next_rep: u32,
    /// Every destructor that ran: the resource and its representation.
    dropped: Vec<(&'static str, u32)>,
    /// What each future the guest reports a body's outcome through
    /// delivered.
    outcomes: Arc<Mutex<Vec<Outcome>>>,
    /// The waker of the test while it runs turns, which each consumer
    /// wakes when it makes progress.
    signal: Signal,
}

/// A waker to wake when a consumer makes progress.
type Signal = Arc<Mutex<Option<Waker>>>;

/// Wake the waker `signal` holds, if it holds one.
fn wake(signal: &Signal) {
    let waker = signal.lock().ok().and_then(|mut waker| waker.take());
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// The host's `wasi:http/types`, shared.
type Shared = Arc<Mutex<Host>>;

/// Lock `host`.
fn lock(host: &Shared) -> std::sync::MutexGuard<'_, Host> {
    host.lock().expect("the host's tables")
}

/// A fresh representation for a resource the guest asks for.
fn next_rep(host: &Shared) -> u32 {
    let mut host = lock(host);
    host.next_rep += 1;
    FIRST_GUEST_REP + host.next_rep - 1
}

/// The resource types the host registered, by name.
#[derive(Clone, Copy)]
struct Resources {
    request: ResourceTypeId,
    fields: ResourceTypeId,
    response: ResourceTypeId,
}

/// A destructor that records `name` and the representation it drops.
fn records_drops(host: &Shared, name: &'static str) -> HostResource<()> {
    let host = host.clone();
    HostResource::new(move |_, rep| {
        lock(&host).dropped.push((name, rep));
        Ok(())
    })
}

/// Register the host's `wasi:http/types@0.3.0` with `linker`.
fn define_types(linker: &mut Linker<()>, host: &Shared) -> Resources {
    let id: InterfaceIdentifier = TYPES.parse().expect("the identifier parses");
    let mut types = linker.instance(&id);
    let resources = Resources {
        request: types
            .resource_with("request", records_drops(host, "request"))
            .expect("the registration"),
        fields: types
            .resource_with("fields", records_drops(host, "fields"))
            .expect("the registration"),
        response: types
            .resource_with("response", records_drops(host, "response"))
            .expect("the registration"),
    };

    let fields_host = host.clone();
    types
        .func_new(
            "[constructor]fields",
            FunctionType {
                parameters: Vec::new(),
                result: Some(fields()),
                async_: false,
            },
            move |call, _args, results| {
                let rep = next_rep(&fields_host);
                results[0] = Val::Own(call.resource_new(resources.fields, rep)?);
                Ok(())
            },
        )
        .expect("the registration");

    let consume_host = host.clone();
    types
        .func_new(
            "[static]request.consume-body",
            FunctionType {
                parameters: vec![
                    parameter("this", ValueType::Own(ResourceType::new("request"))),
                    parameter("res", ValueType::Future(future_of::<Outcome>())),
                ],
                result: Some(ValueType::Tuple(TupleType::new([
                    ValueType::Stream(StreamType::new(Some(primitive(PrimitiveType::U8)))),
                    ValueType::Future(future_of::<Trailers>()),
                ]))),
                async_: false,
            },
            move |mut call, args, results| {
                let [Val::Own(request), Val::Future(outcome)] = args else {
                    panic!("`request.consume-body` was given {args:?}");
                };
                let Request { body, trailers } = {
                    let mut host = lock(&consume_host);
                    host.consumed.push(*request);
                    host.requests
                        .remove(&request.rep())
                        .expect("the guest consumes a request the host made")
                };
                let keeps = {
                    let host = lock(&consume_host);
                    Keeps {
                        values: host.outcomes.clone(),
                        signal: host.signal.clone(),
                    }
                };
                FutureReader::<Outcome>::try_from_future_any(outcome.clone())?
                    .pipe(call.store(), keeps)?;
                results[0] = Val::Tuple(Box::new([body.to_val(), trailers.to_val()]));
                Ok(())
            },
        )
        .expect("the registration");

    let response_host = host.clone();
    types
        .func_new(
            "[static]response.new",
            FunctionType {
                parameters: vec![
                    parameter("headers", fields()),
                    parameter(
                        "contents",
                        option(ValueType::Stream(StreamType::new(Some(primitive(
                            PrimitiveType::U8,
                        ))))),
                    ),
                    parameter("trailers", ValueType::Future(future_of::<Trailers>())),
                ],
                result: Some(ValueType::Tuple(TupleType::new([
                    ValueType::Own(ResourceType::new("response")),
                    ValueType::Future(future_of::<Outcome>()),
                ]))),
                async_: false,
            },
            move |mut call, args, results| {
                let [Val::Own(headers), Val::Option(body), trailers] = args else {
                    panic!("`response.new` was given {args:?}");
                };
                let rep = next_rep(&response_host);
                lock(&response_host).responses.insert(
                    rep,
                    Response {
                        headers: *headers,
                        body: body.as_deref().cloned(),
                        trailers: trailers.clone(),
                    },
                );
                let sent =
                    FutureReader::new(call.store(), async { Ok::<_, Error>(Outcome(Ok(()))) })?;
                results[0] = Val::Tuple(Box::new([
                    Val::Own(call.resource_new(resources.response, rep)?),
                    sent.to_val(),
                ]));
                Ok(())
            },
        )
        .expect("the registration");

    resources
}

/// The future type whose payload is `T`.
fn future_of<T: ComponentValue>() -> FutureType {
    FutureType::new(Some(T::value_type()))
}

/// Instantiate the handler `bytes` with the host's `wasi:http/types`.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance, Shared, Resources) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("the handler translates");
    let host = Shared::default();
    let mut linker: Linker<()> = Linker::new(&engine);
    let resources = define_types(&mut linker, &host);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the handler links against the host's `wasi:http/types`");
    (store, instance, host, resources)
}

/// A future consumer that keeps each value it is handed.
struct Keeps<T> {
    values: Arc<Mutex<Vec<T>>>,
    signal: Signal,
}

impl<T: ComponentValue> FutureConsumer<()> for Keeps<T> {
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        mut source: Source<'_, T>,
        _finish: bool,
    ) -> Poll<Result<(), Error>> {
        let mut value = Vec::new();
        source.read(store, &mut value, 1)?;
        self.values.lock().expect("the kept values").extend(value);
        wake(&self.signal);
        Poll::Ready(Ok(()))
    }
}

/// Trailers a consumer was handed, with the bytes the body's consumer
/// held when they arrived.
struct Arrival {
    trailers: Trailers,
    body: Vec<u8>,
}

/// A future consumer that keeps each [`Arrival`] of trailers.
struct KeepsTrailers {
    values: Arc<Mutex<Vec<Arrival>>>,
    collected: Arc<Mutex<Collected>>,
    signal: Signal,
}

impl FutureConsumer<()> for KeepsTrailers {
    type Item = Trailers;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        mut source: Source<'_, Trailers>,
        _finish: bool,
    ) -> Poll<Result<(), Error>> {
        let mut value = Vec::new();
        source.read(store, &mut value, 1)?;
        let body = self.collected.lock().expect("the body").bytes.clone();
        self.values
            .lock()
            .expect("the kept trailers")
            .extend(value.into_iter().map(|trailers| Arrival {
                trailers,
                body: body.clone(),
            }));
        wake(&self.signal);
        Poll::Ready(Ok(()))
    }
}

/// Whether the body's producer has ended, and the waker of the future
/// waiting for that.
#[derive(Clone, Default)]
struct BodyEnd(Arc<Mutex<(bool, Option<Waker>)>>);

impl BodyEnd {
    /// Record that the producer has ended, and wake the future waiting
    /// for it.
    fn end(&self) {
        let waker = {
            let mut state = self.0.lock().expect("the body's end");
            state.0 = true;
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Resolve once the producer has ended.
    async fn ended(&self) {
        core::future::poll_fn(|cx| {
            let mut state = self.0.lock().expect("the body's end");
            if state.0 {
                Poll::Ready(())
            } else {
                state.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }
}

/// What a body consumer took, and whether the pipe is over.
#[derive(Default)]
struct Collected {
    bytes: Vec<u8>,
    over: bool,
}

/// A stream consumer that takes every byte offered. The pipe drops it
/// once the stream ends.
struct Collects {
    collected: Arc<Mutex<Collected>>,
    signal: Signal,
}

impl StreamConsumer<()> for Collects {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        mut source: Source<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let mut bytes = Vec::new();
        let count = source.remaining();
        source.read(store, &mut bytes, count)?;
        self.collected.lock().expect("the body").bytes.extend(bytes);
        wake(&self.signal);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for Collects {
    fn drop(&mut self) {
        if let Ok(mut collected) = self.collected.lock() {
            collected.over = true;
        }
        wake(&self.signal);
    }
}

/// A producer that delivers `chunks` one per poll, pending once
/// before each, so the body reaches its consumer over several turns.
#[cfg(not(target_arch = "wasm32"))]
struct Chunks {
    chunks: VecDeque<&'static str>,
    parked: bool,
    /// How many polls answered pending.
    pended: Arc<AtomicUsize>,
    /// Ended once the last chunk has been delivered.
    end: BodyEnd,
}

#[cfg(not(target_arch = "wasm32"))]
impl StreamProducer<()> for Chunks {
    type Item = u8;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        mut destination: Destination<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let this = self.get_mut();
        if !this.parked {
            this.parked = true;
            this.pended.fetch_add(1, Ordering::Relaxed);
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.parked = false;
        let Some(chunk) = this.chunks.pop_front() else {
            this.end.end();
            return Poll::Ready(Ok(StreamResult::Dropped));
        };
        destination.set_buffer(chunk.as_bytes().to_vec());
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// The body's producer natively: the chunks of [`BODY`], a turn
/// apart. It counts its pending polls in `pended`, and ends `end` once
/// it has delivered the last chunk.
#[cfg(not(target_arch = "wasm32"))]
fn body(pended: Arc<AtomicUsize>, end: BodyEnd) -> impl StreamProducer<(), Item = u8> {
    Chunks {
        chunks: VecDeque::from(BODY),
        parked: false,
        pended,
        end,
    }
}

/// A producer that awaits a JavaScript promise for each chunk of
/// `chunks`, and delivers the chunk the promise resolves with.
#[cfg(target_arch = "wasm32")]
struct AwaitsPromises {
    chunks: VecDeque<&'static str>,
    pending: Option<wasm_bindgen_futures::JsFuture>,
    /// How many polls found the promise not yet resolved.
    pended: Arc<AtomicUsize>,
    /// Ended once the last chunk has been delivered.
    end: BodyEnd,
}

#[cfg(target_arch = "wasm32")]
impl StreamProducer<()> for AwaitsPromises {
    type Item = u8;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        mut destination: Destination<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let this = self.get_mut();
        if this.pending.is_none() {
            let Some(chunk) = this.chunks.pop_front() else {
                this.end.end();
                return Poll::Ready(Ok(StreamResult::Dropped));
            };
            let promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::from_str(chunk));
            this.pending = Some(wasm_bindgen_futures::JsFuture::from(promise));
        }
        let pending = this.pending.as_mut().expect("the promise being awaited");
        let Poll::Ready(resolved) = core::future::Future::poll(Pin::new(pending), cx) else {
            this.pended.fetch_add(1, Ordering::Relaxed);
            return Poll::Pending;
        };
        this.pending = None;
        let chunk = resolved
            .ok()
            .and_then(|value| value.as_string())
            .expect("the promise resolves with a string");
        destination.set_buffer(chunk.into_bytes());
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// The body's producer in the browser: each chunk of [`BODY`] arrives
/// through a JavaScript promise. It counts its pending polls in
/// `pended`, and ends `end` once it has delivered the last chunk.
#[cfg(target_arch = "wasm32")]
fn body(pended: Arc<AtomicUsize>, end: BodyEnd) -> impl StreamProducer<(), Item = u8> {
    AwaitsPromises {
        chunks: VecDeque::from(BODY),
        pending: None,
        pended,
        end,
    }
}

/// Call `handle` with `request` and answer the value it returned.
async fn handle(store: &mut Store<()>, instance: &Instance, request: ResourceHandle) -> Val {
    let handle = instance
        .exports()
        .instance(HANDLER_INTERFACE)
        .expect("the handler exports `wasi:http/handler`")
        .func("handle")
        .expect("`wasi:http/handler` exports `handle`");
    let returned = store
        .run_concurrent(async |accessor: &Accessor<()>| {
            handle.call_concurrent(accessor, &[Val::Own(request)]).await
        })
        .await
        .expect("the turns run")
        .expect("`handle` returns");
    let [returned] = &*returned else {
        panic!("`handle` returned {returned:?}");
    };
    returned.clone()
}

/// Run turns of `store` until `done` holds, parking on `signal`
/// between checks.
async fn run_until(store: &mut Store<()>, signal: &Signal, mut done: impl FnMut() -> bool) {
    store
        .run_concurrent(async |_accessor| {
            core::future::poll_fn(|cx| {
                *signal.lock().expect("the signal") = Some(cx.waker().clone());
                if done() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        })
        .await
        .expect("the turns run");
}

#[wcmp_macros::test]
async fn it_answers_a_request_with_its_body_and_trailers() {
    let (mut store, instance, host, resources) = instantiate(HANDLER).await;
    let pended = Arc::new(AtomicUsize::new(0));
    let end = BodyEnd::default();
    let body = StreamReader::new(
        &mut store.as_context_mut(),
        body(pended.clone(), end.clone()),
    )
    .expect("a body stream");
    let sent_trailers = store
        .resource_new(resources.fields, TRAILERS_REP)
        .expect("the trailers");
    let trailers = FutureReader::new(&mut store.as_context_mut(), async move {
        end.ended().await;
        Ok::<_, Error>(Trailers(Ok(Some(sent_trailers))))
    })
    .expect("a trailers future");
    lock(&host)
        .requests
        .insert(REQUEST_REP, Request { body, trailers });
    let request = store
        .resource_new(resources.request, REQUEST_REP)
        .expect("the request");

    let returned = handle(&mut store, &instance, request).await;

    let Val::Result(Ok(Some(response))) = &returned else {
        panic!("`handle` answered {returned:?}");
    };
    let Val::Own(response) = **response else {
        panic!("`handle` answered {returned:?}");
    };
    assert!(
        lock(&host).requests.is_empty(),
        "the handler consumed the request's body"
    );
    let Response {
        headers,
        body,
        trailers,
    } = lock(&host)
        .responses
        .remove(&response.rep())
        .expect("the handler answered with a response it made through `response.new`");

    let collected = Arc::new(Mutex::new(Collected::default()));
    let received = Arc::new(Mutex::new(Vec::<Arrival>::new()));
    let (outcomes, signal) = {
        let host = lock(&host);
        (host.outcomes.clone(), host.signal.clone())
    };
    StreamReader::<u8>::from_val(&body.expect("the response carries a body"))
        .expect("the body is the request's `stream<u8>`")
        .pipe(
            &mut store.as_context_mut(),
            Collects {
                collected: collected.clone(),
                signal: signal.clone(),
            },
        )
        .expect("the host pipes the response's body");
    FutureReader::<Trailers>::from_val(&trailers)
        .expect("the trailers are the request's future")
        .pipe(
            &mut store.as_context_mut(),
            KeepsTrailers {
                values: received.clone(),
                collected: collected.clone(),
                signal: signal.clone(),
            },
        )
        .expect("the host pipes the response's trailers");

    run_until(&mut store, &signal, || {
        collected.lock().expect("the body").over
            && !received.lock().expect("the trailers").is_empty()
            && !outcomes.lock().expect("the outcomes").is_empty()
    })
    .await;

    assert_eq!(
        String::from_utf8(collected.lock().expect("the body").bytes.clone()).expect("UTF-8"),
        BODY.concat(),
        "the response's body carried every byte of the request's"
    );
    assert!(
        pended.load(Ordering::Relaxed) > 0,
        "the producer was pending before it delivered, so the body \
         crossed several turns"
    );
    {
        let received = received.lock().expect("the trailers");
        let [
            Arrival {
                trailers,
                body: body_before,
            },
        ] = &received[..]
        else {
            panic!("the response's trailers arrived {} times", received.len());
        };
        assert_eq!(
            *trailers,
            Trailers(Ok(Some(sent_trailers))),
            "the response's trailers were the request's"
        );
        assert_eq!(
            String::from_utf8(body_before.clone()).expect("UTF-8"),
            BODY.concat(),
            "the trailers arrived after the last chunk of the body"
        );
    }
    assert_eq!(
        *outcomes.lock().expect("the outcomes"),
        [Outcome(Ok(()))],
        "the handler reported through `consume-body`'s future that it \
         handled the body"
    );

    let consumed = lock(&host).consumed.clone();
    assert_eq!(consumed.len(), 1, "the handler consumed one request");
    for handle in [response, headers, sent_trailers, consumed[0]] {
        store
            .resource_drop(handle)
            .expect("the host releases what the guest gave it");
    }
    assert_eq!(
        lock(&host).dropped,
        [
            ("response", response.rep()),
            ("fields", headers.rep()),
            ("fields", TRAILERS_REP),
            ("request", REQUEST_REP),
        ],
        "each resource's destructor ran once, with the host's \
         representation"
    );
}

#[wcmp_macros::test]
async fn it_drains_bytes_through_the_guests_own_stream_and_future() {
    let (mut store, instance, _, _) = instantiate(HANDLER).await;
    let drain = instance
        .get_func("drain")
        .expect("the handler exports `drain`")
        .typed::<(Vec<u8>,), Vec<u8>>()
        .expect("`drain` takes and returns a `list<u8>`");

    for bytes in [b"hi".to_vec(), Vec::new(), (0..=255).collect()] {
        let returned = store
            .run_concurrent(async |accessor: &Accessor<()>| {
                drain.call_concurrent(accessor, (bytes.clone(),)).await
            })
            .await
            .expect("the turns run")
            .expect("`drain` returns");
        assert_eq!(returned, bytes, "`drain` returned the bytes it wrote");
    }
}

/// Every message in the chain of `error`, outermost first.
fn messages(error: &Error) -> String {
    let mut messages = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        messages.push_str(": ");
        messages.push_str(&cause.to_string());
        source = cause.source();
    }
    messages
}

#[wcmp_macros::test]
async fn it_traps_the_same_instance_drain_on_a_non_number_payload() {
    let (mut store, instance, _, _) = instantiate(SAME_INSTANCE_HANDLER).await;
    let drain = instance
        .get_func("drain")
        .expect("the handler exports `drain`")
        .typed::<(Vec<u8>,), Vec<u8>>()
        .expect("`drain` takes and returns a `list<u8>`");

    let error = store
        .run_concurrent(async |accessor: &Accessor<()>| {
            drain.call_concurrent(accessor, (b"hi".to_vec(),)).await
        })
        .await
        .and_then(|returned| returned)
        .expect_err(
            "`drain` copied a `result<_, error-code>` between two ends its own \
             instance holds; if the spec has lifted its temporary rule against \
             that copy, reword the hand notes on this fixture's lines of the \
             expected-failure list, which still cascade from the link failure, \
             and turn this test around",
        );
    assert!(
        messages(&error).contains("cannot read from and write to intra-component future/stream"),
        "`drain` trapped on the same-instance rule, not on {error:?}"
    );
}

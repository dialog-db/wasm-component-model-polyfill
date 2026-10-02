// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The test host functions a scenario imports from the polyfill.
//!
//! They are not a WASI host. They cover only what the scenarios call,
//! at the interface versions the pinned toolchain emits, through the
//! polyfill's public `Linker`:
//!
//! - `wasi:cli/stdout@0.3.0` and `wasi:cli/stderr@0.3.0`, whose
//!   `write-via-stream` takes a `stream<u8>` and answers a
//!   `future<result<_, error-code>>`. Every byte the guest writes goes
//!   to a buffer in the store, one for each interface. The future
//!   resolves once the guest drops the stream's writable end. The runner
//!   compares the standard output buffer with the lines the Wasmtime
//!   run captured. It keeps standard error apart and compares none of
//!   it. `wasi:cli/types@0.3.0`, where `error-code` lives, holds no
//!   function. The pinned toolchain's console prints through these
//!   interfaces, to standard error only for `console.error`, and imports
//!   no Preview 2 stdio, so none is defined here.
//! - `wasi:clocks/monotonic-clock@0.3.0`, whose `wait-for` is an async
//!   host function: a `setTimeout` timer backs it in the browser, and
//!   the test runtime's timer natively. Its `now` reads the target's
//!   monotonic clock, which the toolchain's timer queue reads to place
//!   each deadline. `wasi:clocks/types@0.3.0`, where `duration` lives,
//!   holds no function.
//! - The fixed test interface, whose one function takes a string and
//!   returns it, as the Wasmtime run defines it on Wasmtime's `Linker`.
//! - The compiler component's `read-source`, which answers a path with
//!   the text of that file from the toolchain's source bundle, or with
//!   none when the bundle has no such file, as the Wasmtime run answers
//!   it.
//!
//! Every function of an interface defined here is in the linker, so a
//! component that imports the whole interface links. A function the
//! scenarios do not call, `get-resolution` or `wait-until`, returns an
//! error when it is called, so a toolchain change that starts calling
//! one shows as a stage, not a silent pass. An interface these do not
//! define is missing from the linker, so a scenario that imports one
//! stops at `link`.

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::{Arc, Mutex, MutexGuard};

use wcmp::{
    Accessor, ComponentValue, EnumType, Error, FunctionParameter, FunctionType, FutureProducer,
    FutureReader, FutureType, HostCall, InterfaceIdentifier, Linker, ResultType, Source,
    StoreContext, StreamConsumer, StreamReader, StreamResult, Val, ValueType,
};
use wcmp_scenario::SourceBundle;

/// The interface `wasi:cli/stdout` and `wasi:cli/stderr` import their
/// `error-code` from.
const CLI_TYPES: &str = "wasi:cli/types@0.3.0";

/// The interface a scenario prints through.
const STDOUT: &str = "wasi:cli/stdout@0.3.0";

/// The interface a scenario writes its error lines through.
const STDERR: &str = "wasi:cli/stderr@0.3.0";

/// The interface `wasi:clocks/monotonic-clock` imports its `duration`
/// from.
const CLOCK_TYPES: &str = "wasi:clocks/types@0.3.0";

/// The interface a scenario reads the time and sleeps through.
const MONOTONIC_CLOCK: &str = "wasi:clocks/monotonic-clock@0.3.0";

/// The cases of `wasi:cli/types.error-code`, in order.
const ERROR_CODES: [&str; 3] = ["io", "illegal-byte-sequence", "pipe"];

/// What a scenario's store holds: everything the scenario printed, to
/// standard output and, apart, to standard error.
#[derive(Default)]
pub struct Host {
    stdout: Output,
    stderr: Output,
}

impl Host {
    /// Every line the scenario printed to standard output, in order.
    pub fn lines(&self) -> Vec<String> {
        lines(&self.stdout)
    }

    /// Every line the scenario wrote to standard error, in order.
    pub fn error_lines(&self) -> Vec<String> {
        lines(&self.stderr)
    }
}

/// Define the test host functions in `linker`. `read-source` answers
/// from `sources`.
pub fn define(linker: &mut Linker<Host>, sources: Arc<SourceBundle>) -> Result<(), Error> {
    linker.instance(&identifier(CLI_TYPES));
    define_output(linker, STDOUT, |host| &host.stdout)?;
    define_output(linker, STDERR, |host| &host.stderr)?;
    linker.instance(&identifier(CLOCK_TYPES));
    let mut monotonic_clock = linker.instance(&identifier(MONOTONIC_CLOCK));
    monotonic_clock.func_wrap("now", |_, (): ()| Ok(clock::now()))?;
    monotonic_clock.func_wrap("get-resolution", |_, (): ()| {
        Err::<u64, _>(unimplemented(MONOTONIC_CLOCK, "get-resolution"))
    })?;
    monotonic_clock
        .func_wrap_concurrent("wait-for", |_: &Accessor<Host>, (how_long,): (u64,)| {
            clock::wait(how_long)
        })?;
    monotonic_clock.func_wrap_concurrent(
        "wait-until",
        |_: &Accessor<Host>, (_when,): (u64,)| {
            core::future::ready(Err::<(), _>(unimplemented(MONOTONIC_CLOCK, "wait-until")))
        },
    )?;
    let test_interface = identifier(wcmp_scenario::TEST_INTERFACE);
    linker
        .instance(&test_interface)
        .func_wrap(wcmp_scenario::TEST_FUNCTION, |_, (text,): (String,)| {
            Ok(text)
        })?;
    linker
        .instance(&identifier(wcmp_scenario::COMPILER_HOST_INTERFACE))
        .func_wrap(wcmp_scenario::READ_SOURCE, move |_, (path,): (String,)| {
            Ok(sources.read(&path).map(str::to_string))
        })
}

/// Define `write-via-stream` of the output interface `interface` in
/// `linker`. Every byte the guest writes goes to the buffer `output`
/// picks from the store.
fn define_output(
    linker: &mut Linker<Host>,
    interface: &str,
    output: fn(&Host) -> &Output,
) -> Result<(), Error> {
    linker.instance(&identifier(interface)).func_new(
        "write-via-stream",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "data".to_string(),
                ty: StreamReader::<u8>::value_type(),
            }],
            result: Some(ValueType::Future(FutureType::new(Some(
                WriteResult::value_type(),
            )))),
            async_: false,
        },
        move |mut call: HostCall<'_, Host>, args, results| {
            let [Val::Stream(data)] = args else {
                return Err(Error::Internal {
                    message: format!("`write-via-stream` was given {args:?}"),
                });
            };
            let output = output(call.data()).clone();
            let end = Arc::new(Mutex::new(End::default()));
            let consumer = Collects {
                output,
                end: end.clone(),
            };
            StreamReader::<u8>::try_from_stream_any(data.clone())?.pipe(call.store(), consumer)?;
            results[0] = FutureReader::new(call.store(), Finishes(end))?.to_val();
            Ok(())
        },
    )
}

/// What a function of `interface` the test host does not implement,
/// `method`, answers when it is called.
fn unimplemented(interface: &str, method: &str) -> Error {
    Error::Unsupported {
        feature: format!("`{interface}#{method}` in the scenario runner's test host"),
    }
}

/// The identifier of an interface this file names.
fn identifier(interface: &str) -> InterfaceIdentifier {
    interface
        .parse()
        .unwrap_or_else(|error| panic!("`{interface}` is not an interface: {error}"))
}

/// Everything a scenario printed to one output, shared by the store and
/// the streams that write it.
type Output = Arc<Mutex<Vec<u8>>>;

/// The lines of `output`, in order.
fn lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&lock(output))
        .lines()
        .map(str::to_string)
        .collect()
}

/// Lock `shared`, whatever a panic elsewhere left in it.
fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The end of one stream passed to `write-via-stream`, shared by its
/// consumer and the future that reports it.
#[derive(Default)]
struct End {
    /// Whether the guest dropped the stream's writable end.
    ended: bool,
    /// The future waiting for that end.
    waker: Option<Waker>,
}

/// The consumer of a stream passed to `write-via-stream`: it takes
/// every byte the guest writes.
struct Collects {
    output: Output,
    end: Arc<Mutex<End>>,
}

impl StreamConsumer<Host> for Collects {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, Host>,
        mut source: Source<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let mut bytes = Vec::new();
        let count = source.remaining();
        source.read(store, &mut bytes, count)?;
        lock(&self.output).extend(bytes);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for Collects {
    /// The store drops the consumer once the guest drops the stream's
    /// writable end, which is where the future of the write resolves.
    fn drop(&mut self) {
        let waker = {
            let mut end = lock(&self.end);
            end.ended = true;
            end.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The producer of the future `write-via-stream` answers: `ok` once its
/// stream has ended.
struct Finishes(Arc<Mutex<End>>);

impl FutureProducer<Host> for Finishes {
    type Item = WriteResult;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, Host>,
        finish: bool,
    ) -> Poll<Result<Option<WriteResult>, Error>> {
        let mut end = lock(&self.0);
        if end.ended {
            return Poll::Ready(Ok(Some(WriteResult)));
        }
        if finish {
            return Poll::Ready(Ok(None));
        }
        end.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// The payload of the future `write-via-stream` answers:
/// `result<_, error-code>`. The host never fails a write, so it only
/// ever holds `ok`.
struct WriteResult;

impl ComponentValue for WriteResult {
    fn value_type() -> ValueType {
        let codes = EnumType::new(ERROR_CODES.map(str::to_string));
        ValueType::Result(ResultType::new(None, Some(ValueType::Enum(codes))))
    }

    fn from_val(val: &Val) -> Result<Self, Error> {
        match val {
            Val::Result(Ok(None)) => Ok(WriteResult),
            other => Err(Error::Internal {
                message: format!("the host writes only `ok` to an output, not {other:?}"),
            }),
        }
    }

    fn to_val(self) -> Val {
        Val::Result(Ok(None))
    }
}

/// The monotonic clock of `wasi:clocks/monotonic-clock` on the target
/// the test runs on: its reading and its timer, both in nanoseconds.
#[cfg(not(target_arch = "wasm32"))]
mod clock {
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use wcmp::Error;

    /// Nanoseconds since the first reading in this process.
    pub fn now() -> u64 {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        let elapsed = ORIGIN.get_or_init(Instant::now).elapsed();
        u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
    }

    /// Wait `nanos` nanoseconds on the test runtime's timer.
    pub async fn wait(nanos: u64) -> Result<(), Error> {
        tokio::time::sleep(Duration::from_nanos(nanos)).await;
        Ok(())
    }
}

/// The monotonic clock of `wasi:clocks/monotonic-clock` on the target
/// the test runs on: its reading and its timer, both in nanoseconds.
///
/// Both go through the global rather than `window`, so they work in a
/// worker as well as on a page.
#[cfg(target_arch = "wasm32")]
mod clock {
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use wcmp::Error;

    /// Nanoseconds in a millisecond, the unit of the browser's clock
    /// and of `setTimeout`.
    const NANOS_PER_MILLI: f64 = 1_000_000.0;

    /// Nanoseconds since the time origin of the page, from
    /// `performance.now()`.
    pub fn now() -> u64 {
        let millis = method("performance", "now")
            .and_then(|(performance, now)| now.call0(&performance).ok())
            .and_then(|reading| reading.as_f64())
            .unwrap_or_else(js_sys::Date::now);
        // A float-to-integer cast saturates, which is what a reading
        // past the range of `u64` should do.
        (millis * NANOS_PER_MILLI) as u64
    }

    /// Wait `nanos` nanoseconds on a `setTimeout` timer, rounded up to
    /// the whole millisecond `setTimeout` counts in.
    pub async fn wait(nanos: u64) -> Result<(), Error> {
        let millis = (nanos as f64 / NANOS_PER_MILLI).ceil();
        let Some((global, set_timeout)) = method("", "setTimeout") else {
            return Err(Error::Unsupported {
                feature: "a timer on a global with no `setTimeout`".to_string(),
            });
        };
        let mut armed = Ok(JsValue::UNDEFINED);
        let fired = js_sys::Promise::new(&mut |resolve, _reject| {
            armed = set_timeout.call2(&global, &resolve, &JsValue::from_f64(millis));
        });
        if let Err(thrown) = armed {
            return Err(Error::Unsupported {
                feature: format!("a timer from a `setTimeout` that threw {thrown:?}"),
            });
        }
        // The promise only ever resolves.
        let _ = JsFuture::from(fired).await;
        Ok(())
    }

    /// The function `name` of the global's property `object`, or of the
    /// global itself when `object` is empty, with the value to call it
    /// on.
    fn method(object: &str, name: &str) -> Option<(JsValue, js_sys::Function)> {
        let mut this: JsValue = js_sys::global().into();
        if !object.is_empty() {
            this = js_sys::Reflect::get(&this, &JsValue::from_str(object)).ok()?;
        }
        let function = js_sys::Reflect::get(&this, &JsValue::from_str(name))
            .ok()?
            .dyn_into::<js_sys::Function>()
            .ok()?;
        Some((this, function))
    }
}

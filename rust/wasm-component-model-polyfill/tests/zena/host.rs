//! The test host functions a scenario imports from the polyfill.
//!
//! They are not a WASI host. They cover only what the scenarios call,
//! at the interface versions the pinned toolchain emits, through the
//! polyfill's public `Linker`:
//!
//! - `wasi:cli/stdout@0.3.0`, whose `write-via-stream` takes a
//!   `stream<u8>` and answers a `future<result<_, error-code>>`. Every
//!   byte the guest writes goes to a buffer in the store, which the
//!   runner compares with the lines the Wasmtime run captured. The
//!   future resolves once the guest drops the stream's writable end.
//!   `wasi:cli/types@0.3.0`, where `error-code` lives, holds no function.
//! - The fixed test interface, whose one function takes a string and
//!   returns it, as the Wasmtime run defines it on Wasmtime's `Linker`.
//!
//! A function these do not define is missing from the linker, so a
//! scenario that imports one stops at `link`, not at a silent pass.

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::{Arc, Mutex, MutexGuard};

use wasm_component_model_polyfill::{
    ComponentValue, EnumType, Error, FunctionParameter, FunctionType, FutureProducer, FutureReader,
    FutureType, HostCall, InterfaceIdentifier, Linker, ResultType, Source, StoreContext,
    StreamConsumer, StreamReader, StreamResult, Val, ValueType,
};

/// The interface `wasi:cli/stdout` imports its `error-code` from.
const CLI_TYPES: &str = "wasi:cli/types@0.3.0";

/// The interface a scenario prints through.
const STDOUT: &str = "wasi:cli/stdout@0.3.0";

/// The cases of `wasi:cli/types.error-code`, in order.
const ERROR_CODES: [&str; 3] = ["io", "illegal-byte-sequence", "pipe"];

/// What a scenario's store holds: everything the scenario printed.
#[derive(Default)]
pub struct Host {
    stdout: Stdout,
}

impl Host {
    /// Every line the scenario printed to standard output, in order.
    pub fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(&lock(&self.stdout))
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// Define the test host functions in `linker`.
pub fn define(linker: &mut Linker<Host>) -> Result<(), Error> {
    linker.instance(&identifier(CLI_TYPES));
    linker.instance(&identifier(STDOUT)).func_new(
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
        |mut call: HostCall<'_, Host>, args, results| {
            let [Val::Stream(data)] = args else {
                return Err(Error::Internal {
                    message: format!("`write-via-stream` was given {args:?}"),
                });
            };
            let stdout = call.data().stdout.clone();
            let end = Arc::new(Mutex::new(End::default()));
            let consumer = Collects {
                stdout,
                end: end.clone(),
            };
            StreamReader::<u8>::try_from_stream_any(data.clone())?.pipe(call.store(), consumer)?;
            results[0] = FutureReader::new(call.store(), Finishes(end))?.to_val();
            Ok(())
        },
    )?;
    let test_interface = identifier(wcmp_scenario::TEST_INTERFACE);
    linker
        .instance(&test_interface)
        .func_wrap(wcmp_scenario::TEST_FUNCTION, |_, (text,): (String,)| {
            Ok(text)
        })
}

/// The identifier of an interface this file names.
fn identifier(interface: &str) -> InterfaceIdentifier {
    interface
        .parse()
        .unwrap_or_else(|error| panic!("`{interface}` is not an interface: {error}"))
}

/// Everything a scenario printed, shared by the store and the streams
/// that write it.
type Stdout = Arc<Mutex<Vec<u8>>>;

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
    stdout: Stdout,
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
        lock(&self.stdout).extend(bytes);
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
                message: format!("the host writes only `ok` to standard output, not {other:?}"),
            }),
        }
    }

    fn to_val(self) -> Val {
        Val::Result(Ok(None))
    }
}

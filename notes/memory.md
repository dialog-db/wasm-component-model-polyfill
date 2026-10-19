# Memory

How each backend holds guest memory, and how the polyfill reaches it:

| Memory operation                        | Browser                                                                | Wasmi                                   | Wasmtime                                                           |
| --------------------------------------- | ---------------------------------------------------------------------- | --------------------------------------- | ------------------------------------------------------------------ |
| How guest memory is held                | The browser's engine decides                                           | A `Vec` that is zero-filled as it grows | Reserved address space, with only touched pages committed          |
| A scalar load or store                  | A call into a generated accessor module, from Wasm to Wasm             | Direct                                  | Direct                                                             |
| `with_bytes` (lend a range to the host) | Copies the range into a buffer of the polyfill once                    | Lends the memory's own bytes            | Lends the memory's own bytes, or copies a shared memory atomically |
| A bulk copy                             | One `memory.copy` with `multi_memory`. One `TypedArray.set` without it | One `memmove`                           | One `memmove`                                                      |
| A shared memory without `multi_memory`  | Eight bytes per accessor call, each byte atomic                        | Not supported                           | Not applicable                                                     |

Sources: each backend's crate docs, `rust/wcmp-wasm-core-web/src/accessor.rs`,
card `f7eb9b` for the eight-byte batching, and
`rust/wcmp/tests/corpus/README.md` for how Wasmi and Wasmtime hold memory.

## Wasmi holds every page

Measured alone in the debug build, `wasmtime/memory64.wast` peaks at 34 MiB on
Wasmtime and 4112 MiB on Wasmi, and `wasmtime/big-strings.wast` at 34 MiB and
5134 MiB (`rust/wcmp/tests/corpus/README.md`). The Wasmi lane runs these two
files one at a time.

## The browser reaches only 4 GiB

The polyfill is 32-bit code in the browser, and its Canonical ABI addresses
guest memory with `usize`. So it cannot lift or lower data that lies past 4 GiB
in a 64-bit guest memory. This costs three directives of
`wasmtime/memory64.wast` (`rust/wcmp/tests/corpus/expected-failures.web.txt`).
Without `multi_memory`, a bulk copy also works only within the first 4 GiB of a
memory, and fails with `Error::Backend` past it
(`rust/wcmp-wasm-core-web/src/store.rs`).

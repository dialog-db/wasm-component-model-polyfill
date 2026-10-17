# Wasmtime's macro, pinned

These files are copies of Wasmtime's sources at `v49.0.0-rc.1` (commit
`62ef0a5586d450d8dff436388370fe7fc736a2bb`), the version the workspace pins.
Wasmtime is licensed under Apache-2.0 WITH LLVM-exception.

- `bindgen.rs.txt` is `crates/component-macro/src/bindgen.rs`: the parser of
  the options of `wasmtime::component::bindgen!`.
- `config.rs.txt` is `crates/wit-bindgen/src/config.rs`: the flags the `imports`
  and `exports` options give a function.
- `codegen/` is `crates/component-macro/tests/codegen/`: the codegen corpus, the
  WIT files that Wasmtime's macro tests use.

The tests read them:

- The ledger test, in `rust/wcmp-bindgen/src/tests.rs`, compares the options and
  the flags these files define with the option ledger of `wcmp::bindgen!`.
- The codegen corpus test runs `wcmp::bindgen!` over each entry of `codegen/`.

The `bindgen-pin` flake check compares each file with the one the pinned
Wasmtime crates hold, so a copy cannot drift from the pin.

## Moving the pin

When the workspace moves its Wasmtime pin, the `bindgen-pin` check fails. Copy
the three sources again from the new crates, and run the tests. A new option, a
new flag, or a new corpus entry fails the ledger test or the corpus test, and
names it. Classify it in the option ledger (`rust/wcmp-bindgen/src/ledger.rs`)
or give the corpus entry its expected result, and the tests pass.

## The snapshots

`tests/codegen.rs` also compares the code `wcmp::bindgen!` writes for a few
corpus entries with the snapshots in `tests/snapshots/`, and
`tests/front_end.rs` does the same for `wit/`. A change to the generator fails
them. When the change is meant, run the test with its output shown, for example
`tests native debug -E 'test(it_keeps_the_shape_of_the_generated_code)' --no-capture`,
and write each block it prints to `tests/snapshots/<name>.txt`.

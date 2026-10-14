---
id: 0ef2ae
title: Try Chromium with wasmfx and the web suite under WebKitGTK
type: chore
blocked_by: [fbe4df]
labels: [runtime-layer, backlog-burndown-002]
created: 2026-10-13T18:44:17Z
disposition: accepted
disposition_at: 2026-10-16T06:34:16Z
---

## What to build
From the 2026-10-13 review of fbe4df. The survey names two cheap routes to more fidelity that need no new backend. Try them before choosing a fourth backend.

- **Chromium with `--js-flags=--wasm-wasmfx`.** The web lane's Chrome would then turn on V8's experimental stack switching, the one capability the browser lane lacks. Check whether the browser backend's probes see it, and what the conformance and fidelity suites do with it.
- **The web suite under WebKitGTK.** The flake already has a WebKitGTK smoke driver (`tests smoke webkit`). Run the web test suite under it, as a JavaScriptCore lane with Safari's semantics. The project weighs Safari above Firefox.

This card records findings; it does not add a permanent lane unless the owner asks.

## Acceptance criteria
- [x] For each route, a note on this card says whether it works, what it costs to run, and the conformance and fidelity results or the reason they could not be measured.
- [x] The owner has what they need to choose between V8 (full fidelity on Chrome's engine) and JavaScriptCore (Safari parity, no stack switching).

## Implementation notes

- 2026-10-13 findings. Nothing permanent was added: no lane, no flake
  change. Both runs used the debug web archive of the series tip on this
  host. The temporary tests used to read the provider and the user agent
  were reverted.

**Route 1: Chromium 154 with `--js-flags=--wasm-wasmfx`** (set by appending
it to `WBG_POOL_BROWSER_ARGS`).
- It works, in that Chromium takes the flag and the browser backend's
  `cont.new` probe passes. `tests fidelity web` reports `stack_switching:
  declared, and no script of the pinned test suite needs it`. The suite
  passes 264/264, and stack switching itself goes unmeasured, since the
  pinned testsuite has no stack-switching script.
- The engine still selects `HostSuspension`. A temporary test printed
  `engine(true).suspend_provider()` both with and without the flag. The
  reason is structural: `SwitchProbe::passes`
  (`rust/wcmp/src/concurrency/switch_probe.rs:117`) needs the probe module
  to instantiate at once (`at_once`), and the browser always instantiates
  asynchronously. So the browser cannot use the stack-switching provider
  without new code: a synchronous small-module path for the probe and the
  per-store switch module, or a host-suspension-free provider for the
  browser.
- The flag makes things worse for JSPI. `tests web debug`: 1722 of 1728
  pass. Five tests fail with V8's `trying to suspend JS frames`:
  - `baseline::call_concurrent::it_runs_a_queued_call_that_re_enters_the_import_the_block_is_inside`;
  - four `host_suspension_switch_module` tests (drop with suspended threads,
    refused resume, and both nested-resume orders).
  The sixth is `cm/async/during-sync-call-no-sibling-resume.wast:156`
  (`uncaught exception propagated out of component`).
- Cost: the same as today's web lane (about 47 s for the debug suite). It
  needs only the flag.

**Route 2: the web suite under WebKitGTK 2.54.1** (MiniBrowser through
`WebKitWebDriver` on a headless Weston, as `tests smoke webkit` drives it).
`wbg-pool` drives only Chrome, so the run used the stock
`wasm-bindgen-test-runner` (`CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER`,
`SAFARIDRIVER=WebKitWebDriver`, the smoke lane's capabilities JSON). A
temporary test read the user agent as `AppleWebKit/605.1.15 … Safari/605.1.15`,
and the provider is `HostSuspension` (JSPI on).
- It works. `tests web debug` equivalent: 1711 of 1728 pass.
  - 11 failures are memory64, off by default in JSC
    (`useWasmMemory64=false`): `conformance::it_passes_wasmtime_memory64` (one
    `unexpected:` line, `memory64.wast:24`); five `thread_start_table` tests;
    the 64-bit transcode; the 32/64-bit composition; the i64 thread start;
    `web::it_declares_what_chromium_implements`; and the 4 GiB bulk copy.
  - 6 are engine differences:
    - Trap texts the V8-only trap table does not map: `contract
      it_raises_each_core_trap_the_capabilities_permit` (JSC says
      `call_indirect to a signature that does not match`).
    - `accessor` expects V8's stack format.
    - No synchronous compile size limit (`web.rs:299`).
    - A shared memory's buffer is not a `SharedArrayBuffer` (`web.rs:1534`).
    - A refused main-thread wait reads as `Out of bounds memory access`
      (`web.rs:2142`, `web.rs:2195`).
  - Conformance otherwise matches: every other corpus file passes as it
    does on Chrome.
- Fidelity (`--profile fidelity`, the browser backend's binary): 239 of
  264 scripts pass. There are 1943 `unexpected:` directives and 3 stale ones
  (V8 entries JSC passes).
  - Nearly all of the unexpected ones are trap kinds the browser backend
    maps from V8's strings only and reads as `Other` under JSC (`table_copy`
    1181, `table_init` 564, and the `data`, `array_*`, `call_indirect`
    and `linking` files).
  - About 70 are invalid modules JSC compiles (64 in
    `unreached-invalid.wast`, plus `call_ref` and `return_call_ref`).
  - A handful are value mismatches in `array_init_elem`, `table_init` and
    `array_new_elem`.
- Cost: 48 m 38 s for the debug web suite at `-j 6`, about 6 s per test,
  since the stock runner starts a MiniBrowser and runs wasm-bindgen codegen
  for each test. Fidelity took 4 m 14 s. It needs the WebKitGTK and Weston
  the flake already has, but not the build sandbox, so it is a menu command
  (like `tests smoke webkit`), not a check. A pooled runner for WebKit, as
  wbg-pool is for Chrome, would cut most of the time.

**For the choice.**
- V8 by flag gives no stack-switching provider in the browser as the
  polyfill stands, and it breaks some JSPI paths. Full fidelity on V8
  still means a native V8 backend.
- JavaScriptCore runs today's browser backend almost as is. Its real gaps
  are a JSC trap-string table, memory64 (off, and IPInt-only when on), and
  a lax validator. That makes it a cheap Safari-parity lane once the trap
  table learns JSC's strings and a per-engine expected-failure list exists
  (card 841af1's "Safari lane readiness").
- The script below reproduces route 2. It runs inside `nix develop`, with
  nextest arguments after it (for example `-j 6 --no-fail-fast`, or
  `--profile fidelity -E 'binary_id(wcmp-wasm-core-web::fidelity)'`). The
  store paths are this host's WebKitGTK 2.54.1 and Weston 16.

```bash
#!/usr/bin/env bash
# Experiment for card 0ef2ae: replay the web debug archive's tests under
# WebKitGTK's MiniBrowser through WebKitWebDriver, with the stock
# wasm-bindgen-test-runner in place of wbg-pool. Runs inside `nix develop`.
set -u
WK=/nix/store/35fkb81rx2i8kzpdpfcs78bram5mbisi-webkitgtk-2.54.1+abi=6.0
WESTON=/nix/store/khhx4dlk2kgj9c4w8y1c8nxy25rd7b81-weston-16.0.0
runtime=$(mktemp -d /tmp/wk.XXXX)
trap 'kill "${weston:-}" 2>/dev/null || true; rm -rf "$runtime"' EXIT
XDG_RUNTIME_DIR=$runtime $WESTON/bin/weston --backend=headless --socket=wk \
  --width=1280 --height=800 --idle-time=0 >"$runtime/weston.log" 2>&1 &
weston=$!
for _ in $(seq 100); do [ -S "$runtime/wk" ] && break; sleep 0.1; done
export XDG_RUNTIME_DIR=$runtime WAYLAND_DISPLAY=wk
cat > "$runtime/caps.json" <<JSON
{"browserName":"MiniBrowser","webkitgtk:browserOptions":{"binary":"$WK/libexec/webkitgtk-6.0/MiniBrowser","args":["--automation"]}}
JSON
export WASM_BINDGEN_TEST_WEBDRIVER_JSON="$runtime/caps.json"
unset CHROMEDRIVER GECKODRIVER
export SAFARIDRIVER=$WK/bin/WebKitWebDriver
export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner
export WASM_BINDGEN_TEST_TIMEOUT=180
archive=$(nix build --no-link --print-out-paths .#tests-web-debug)
ws=$HOME/.cache/wcmp-tests/wk; rm -rf "$ws"; mkdir -p "$ws/tmp" "$ws/archive"; export TMPDIR=$ws/tmp
cargo nextest run --workspace-remap ./ --archive-file "$archive/tests-web-debug.tar.zst" \
  --extract-to "$ws/archive" --extract-overwrite "$@"
status=$?
rm -rf "$ws"
exit $status
```

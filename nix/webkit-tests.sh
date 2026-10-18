# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# `tests webkit debug`: the web test archive in WebKitGTK's MiniBrowser,
# the engine Safari is built on, on a headless Weston.
#
#   wcmp-webkit-tests ARCHIVE WORKSPACE [NEXTEST ARGUMENTS...]
#
# nextest picks the tests: `cargo nextest list` reads the archive with
# the default profile and any filter after the leaf, as the Chrome lanes
# do. The tests then run one binary at a time per browser, not one test
# at a time. The stock `wasm-bindgen-test-runner` generates the bindings
# of a binary every time it starts, which takes a dozen seconds for the
# polyfill's unit tests, so a browser per test would take hours. Each
# binary instead runs once, with every test the profile leaves out
# passed as `--skip` with `--exact`, so the lane runs the same tests as
# `tests web debug`, and the whole archive takes about two minutes rather
# than hours. The archive's host binaries (the procedural macros'
# tests) run natively in the Chrome lanes and are left out here.
#
# The tests of a binary share one page, as they do under `cargo test`,
# where the Chrome lanes give each test a page of its own. A test can
# leave the page changed for the tests after it: a Content Security
# Policy, once installed, stays. So each test that fails in the shared
# page runs again on its own, in a browser of its own, and only a test
# that fails there too fails the lane.
#
# The runner knows no WebKit driver by name, so it is handed
# `WebKitWebDriver` as its geckodriver (`GECKODRIVER`). It starts one
# driver per binary, on a port of its own, and opens a W3C session with
# the capabilities `WASM_BINDGEN_TEST_WEBDRIVER_JSON` names, which name
# MiniBrowser. WebKitGTK ignores the Firefox options the runner adds,
# and the runner's own output calls the browser Firefox for that reason.
#
# WebKitGTK draws through a Wayland compositor and has no headless mode
# of its own, so the script starts a headless Weston first. A Wayland
# socket's path must fit in 108 bytes, so the compositor's runtime
# directory goes under `/run/user/<uid>` where that exists, and under
# `/tmp` otherwise.
#
# A binary in flight holds a runner, about a gigabyte and a half while
# it generates the bindings, and a browser, so the script runs one
# binary per 3 GB of available memory, capped at the core count, unless
# `NEXTEST_TEST_THREADS` says otherwise. A binary that fails has its
# failures printed, and its whole output is kept under the cache
# directory's `wcmp-tests/webkit-logs`, which outlives the workspace the
# lane removes on exit. The script fails when a binary does, after every
# binary has run.
#
# The runner's timeout (`WASM_BINDGEN_TEST_TIMEOUT`) bounds one page, and
# a page here holds a whole binary, so the script sets it to
# `WCMP_WEBKIT_TIMEOUT` seconds, 900 unless that says otherwise. The
# polyfill's unit tests take about twenty seconds here.
#
# The flake supplies `WCMP_WESTON`, `WCMP_WEBKIT_DRIVER`, and
# `WCMP_WEBKIT_CAPABILITIES`. `cargo` and `wasm-bindgen-test-runner`
# come from the development shell.

set -euo pipefail

archive=$1
workspace=$2
shift 2

base=/run/user/$(id -u)
if [ ! -d "$base" ] || [ ! -w "$base" ]; then
  base=/tmp
fi
runtime=$(mktemp -d "$base/wcmp-webkit-XXXXXX")
logs="${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/webkit-logs"
rm -rf "$logs"
cleanup() {
  if [ -n "${weston:-}" ]; then
    kill "$weston" 2>/dev/null || true
    wait "$weston" 2>/dev/null || true
  fi
  rm -rf "$runtime"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
XDG_RUNTIME_DIR=$runtime "$WCMP_WESTON" --backend=headless \
  --socket=wcmp-webkit --width=1280 --height=800 --idle-time=0 \
  >"$runtime/weston.log" 2>&1 &
weston=$!
for _ in $(seq 100); do
  [ -S "$runtime/wcmp-webkit" ] && break
  sleep 0.1
done
if [ ! -S "$runtime/wcmp-webkit" ]; then
  cat "$runtime/weston.log" >&2
  echo "tests webkit: the headless Weston did not start" >&2
  exit 1
fi
export XDG_RUNTIME_DIR=$runtime WAYLAND_DISPLAY=wcmp-webkit
export GECKODRIVER=$WCMP_WEBKIT_DRIVER
export WASM_BINDGEN_TEST_WEBDRIVER_JSON=$WCMP_WEBKIT_CAPABILITIES
export WASM_BINDGEN_TEST_TIMEOUT=${WCMP_WEBKIT_TIMEOUT:-900}

threads=${NEXTEST_TEST_THREADS:-}
if [ -z "$threads" ]; then
  available_kb=$(awk '/MemAvailable/ { print $2 }' /proc/meminfo)
  threads=$((available_kb / 1024 / 1024 / 3))
  cores=$(nproc)
  [ "$threads" -gt "$cores" ] && threads=$cores
  [ "$threads" -lt 1 ] && threads=1
fi

cargo nextest list \
  --workspace-remap ./ \
  --archive-file "$archive" \
  --extract-to "$workspace/archive" \
  --extract-overwrite \
  --message-format json \
  "$@" >"$workspace/list.json"

# One line per binary with a test to run: the binary's identity, its
# path, the directory nextest would run it in, and each test to skip.
mkdir -p "$workspace/runs" "$workspace/logs"
jq -r '
  ."rust-suites" | to_entries[]
  | select(.value.kind != "proc-macro" and (.value."binary-path" | endswith(".wasm")))
  | select([.value.testcases[] | select(."filter-match".status == "matches")] | length > 0)
  | [.key, .value."binary-path", .value.cwd]
    + [.value.testcases | to_entries[] | select(.value."filter-match".status != "matches") | .key]
  | @json
' "$workspace/list.json" >"$workspace/binaries"
total=$(wc -l <"$workspace/binaries")
# One file per binary, which xargs hands over by name: a line of xargs
# holds about 128 KiB, and a binary's skip list could outgrow that.
split -l 1 -d -a 4 "$workspace/binaries" "$workspace/runs/binary-"
selected=$(jq '[."rust-suites"[] | select(."binary-path" | endswith(".wasm")) | .testcases[] | select(."filter-match".status == "matches")] | length' "$workspace/list.json")
echo "tests webkit: $selected tests in $total binaries, $threads binaries at a time"

run_binary() {
  local line id path cwd log
  line=$(cat "$1")
  id=$(jq -r '.[0]' <<<"$line")
  path=$(jq -r '.[1]' <<<"$line")
  cwd=$(jq -r '.[2]' <<<"$line")
  log="$workspace/logs/${id//[^A-Za-z0-9_.-]/_}.log"
  local skips=()
  while IFS= read -r name; do
    skips+=(--skip "$name")
  done < <(jq -r '.[3:][]' <<<"$line")
  local started=$SECONDS status=0
  (cd "$cwd" && wasm-bindgen-test-runner "$path" --exact "${skips[@]}") >"$log" 2>&1 || status=$?
  local summary
  summary=$(grep -E '^test result:' "$log" | tail -n 1 || true)
  if [ "$status" = 0 ]; then
    echo "  PASS [$((SECONDS - started))s] $id: ${summary:-no summary}"
    return
  fi
  if [ -z "$summary" ]; then
    echo "  FAIL [$((SECONDS - started))s] $id: the runner failed before it reported"
    echo "$id" >>"$workspace/runs/failed"
    return
  fi
  # The names under the last `failures:` heading, one per line, indented.
  local failed=() name
  while IFS= read -r name; do
    failed+=("$name")
  done < <(awk '/^failures:$/ { names = "" ; on = 1 ; next }
    /^test result:/ { on = 0 }
    on && /^    [^ ]/ { sub(/^    /, ""); names = names $0 "\n" }
    END { printf "%s", names }' "$log")
  # A run that failed and names no failing test fails as it is, rather
  # than passing for want of a test to run again.
  if [ "${#failed[@]}" = 0 ]; then
    echo "  FAIL [$((SECONDS - started))s] $id: $summary, and the runner named no failing test"
    echo "$id" >>"$workspace/runs/failed"
    return
  fi
  local alone=0 still=()
  for name in "${failed[@]}"; do
    alone=$((alone + 1))
    if ! (cd "$cwd" && wasm-bindgen-test-runner "$path" --exact "$name") >"$log.$alone" 2>&1; then
      still+=("$name")
      cat "$log.$alone" >>"$log.alone"
    fi
  done
  if [ "${#still[@]}" = 0 ]; then
    echo "  PASS [$((SECONDS - started))s] $id: $summary; each of the ${#failed[@]} passed on a page of its own"
    return
  fi
  echo "  FAIL [$((SECONDS - started))s] $id: ${#still[@]} of ${#failed[@]} that failed in the shared page failed on a page of their own too: ${still[*]}"
  echo "$id" >>"$workspace/runs/failed"
}
export -f run_binary
export workspace

started=$SECONDS
# shellcheck disable=SC2016 # the inner shell expands `$1`
find "$workspace/runs" -name 'binary-*' -print0 |
  xargs -0 -n 1 -P "$threads" "$BASH" -c 'run_binary "$1"' _

if [ -s "$workspace/runs/failed" ]; then
  mkdir -p "$logs"
  while IFS= read -r id; do
    echo
    echo "--- $id ---"
    # The runner prints each test it invokes; the failures and the
    # summary of the runs on a page of their own are what a reader needs.
    log="$workspace/logs/${id//[^A-Za-z0-9_.-]/_}.log"
    if [ -s "$log.alone" ]; then
      sed -n '/^failures:/,/^test result:/p' "$log.alone" || true
    else
      tail -n 40 "$log"
    fi
    cp "$log"* "$logs/"
  done <"$workspace/runs/failed"
  echo
  echo "tests webkit: the failed binaries' whole output is in $logs"
  echo "tests webkit: $(wc -l <"$workspace/runs/failed") of $total binaries failed after $((SECONDS - started))s"
  exit 1
fi
echo "tests webkit: $total binaries passed in $((SECONDS - started))s"

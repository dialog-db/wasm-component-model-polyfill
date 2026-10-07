# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# The WebDriver plumbing every script that drives a page headlessly
# shares: the smoke check (`rust/wcmp-smoke/web/check.sh`) and the
# benchmark runner (`rust/wcmp-bench/web/run.sh`). The flake puts this
# file in front of each script's text, so it defines functions and
# runs nothing. Each script runs with `curl`, `jq`, and
# `static-web-server` on its PATH, and with the WebDriver server it
# names.
#
# `webdriver_open PAGE URL_SUFFIX` serves the directory PAGE with
# static-web-server on a loopback port, starts the WebDriver server,
# opens a session with the capabilities `WASM_BINDGEN_TEST_WEBDRIVER_JSON`
# names, and loads the page with URL_SUFFIX after its `/`, such as a
# query string. The server is `WCMP_WEBDRIVER`, or `chromedriver` when
# that is unset; `tests smoke webkit` names WebKitGTK's
# `WebKitWebDriver`, which takes the same `--port`. An exit trap stops
# both servers.
#
# `webdriver_execute SCRIPT` runs SCRIPT in the page and prints the
# value it returns. `webdriver_close` ends the session.
#
# Every request is bounded: a browser that never starts, or a page that
# never answers, fails the script at that request instead of holding a
# build until a CI job's time limit. Starting the session, which
# launches the browser, is the slow one.

webdriver_open() {
  local page=$1 suffix=${2:-}
  # Two ports on the loopback. A build sandbox has a loopback of its
  # own, so any port is free there; outside one, a random pair keeps
  # two runs apart.
  webdriver_page_port=$((20000 + RANDOM % 20000))
  webdriver_driver_port=$((webdriver_page_port + 1))
  static-web-server --root "$page" --host 127.0.0.1 \
    --port "$webdriver_page_port" --log-level error &
  webdriver_server=$!
  "${WCMP_WEBDRIVER:-chromedriver}" --port="$webdriver_driver_port" >/dev/null 2>&1 &
  webdriver_driver=$!
  trap 'kill "$webdriver_driver" "$webdriver_server" 2>/dev/null || true' EXIT

  webdriver_wait_for "http://127.0.0.1:$webdriver_page_port/"
  webdriver_wait_for "http://127.0.0.1:$webdriver_driver_port/status"

  webdriver_base="http://127.0.0.1:$webdriver_driver_port"
  webdriver_session=$(jq --compact-output '{capabilities: {alwaysMatch: .}}' \
    "$WASM_BINDGEN_TEST_WEBDRIVER_JSON" | webdriver_post session \
    | jq -r '.value.sessionId')
  jq --null-input --compact-output \
    --arg url "http://127.0.0.1:$webdriver_page_port/$suffix" '{url: $url}' \
    | webdriver_post "session/$webdriver_session/url" >/dev/null
}

webdriver_wait_for() {
  for _ in $(seq 100); do
    if curl -fs --max-time 2 -o /dev/null "$1"; then
      return 0
    fi
    sleep 0.1
  done
  echo "nothing answers at $1" >&2
  exit 1
}

webdriver_post() {
  curl -fs --max-time 120 -X POST -H 'Content-Type: application/json' \
    --data-binary @- "$webdriver_base/$1"
}

webdriver_execute() {
  jq --null-input --compact-output --arg script "$1" \
    '{script: $script, args: []}' \
    | webdriver_post "session/$webdriver_session/execute/sync" | jq -r '.value'
}

webdriver_close() {
  curl -fs --max-time 30 -X DELETE \
    "$webdriver_base/session/$webdriver_session" >/dev/null
}

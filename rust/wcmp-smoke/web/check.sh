# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Drive the smoke test page headlessly and compare its report.
#
# The flake's `smoke-web` check runs this script inside the build
# sandbox (`tests smoke check` builds that check and prints the
# report): it serves the built page with static-web-server on a
# loopback port, opens it in the flake's Chromium through chromedriver
# (the same WebDriver plumbing the browser tests use, configured by the
# same `webdriver.json`), waits for the page's transcript to end in the
# report's summary line, and fails unless that line reports no failure
# and no skip and equals the line the native smoke binary printed.
#
# The page declares `script-src 'self' 'wasm-unsafe-eval'`, the policy a
# hardened site sets, so the whole run happens under it. `boot.js`
# records on the root element whether the browser is enforcing that
# policy, and this script fails if it is not: a run that passed because
# the policy was lost would prove nothing.
#
# Arguments: the page directory, the native summary line, the report's
# destination path. Environment: `WASM_BINDGEN_TEST_WEBDRIVER_JSON` for
# the browser capabilities, and `WCMP_SMOKE_WEBDRIVER` for the WebDriver
# server, `chromedriver` unless it names another: `tests smoke webkit`
# names WebKitGTK's `WebKitWebDriver`, which takes the same `--port`.
# The flake wraps this script with `curl`, `jq`, `chromedriver`, and
# `static-web-server` on its PATH.

page=$1
native=$2
destination=$3
budget=180

# Two ports on the loopback. The build sandbox has a loopback of its
# own, so any port is free there; outside one, a random pair keeps two
# runs apart.
page_port=$((20000 + RANDOM % 20000))
driver_port=$((page_port + 1))

static-web-server --root "$page" --host 127.0.0.1 --port "$page_port" \
  --log-level error &
server=$!
"${WCMP_SMOKE_WEBDRIVER:-chromedriver}" --port="$driver_port" >/dev/null 2>&1 &
driver=$!
trap 'kill "$driver" "$server" 2>/dev/null || true' EXIT

wait_for() {
  for _ in $(seq 100); do
    if curl -fs --max-time 2 -o /dev/null "$1"; then
      return 0
    fi
    sleep 0.1
  done
  echo "nothing answers at $1" >&2
  exit 1
}
wait_for "http://127.0.0.1:$page_port/"
wait_for "http://127.0.0.1:$driver_port/status"

base="http://127.0.0.1:$driver_port"
# Every WebDriver request is bounded: a browser that never starts, or a
# page that never answers, fails the check at that request instead of
# holding the build until a CI job's time limit. Starting the session,
# which launches Chrome, is the slow one.
post() {
  curl -fs --max-time 120 -X POST -H 'Content-Type: application/json' \
    --data-binary @- "$base/$1"
}
session=$(jq --compact-output '{capabilities: {alwaysMatch: .}}' \
  "$WASM_BINDGEN_TEST_WEBDRIVER_JSON" | post session | jq -r '.value.sessionId')
execute() {
  jq --null-input --compact-output --arg script "$1" \
    '{script: $script, args: []}' \
    | post "session/$session/execute/sync" | jq -r '.value'
}

jq --null-input --compact-output --arg url "http://127.0.0.1:$page_port/" \
  '{url: $url}' | post "session/$session/url" >/dev/null

deadline=$((SECONDS + budget))
text=""
summary=""
while [ "$SECONDS" -lt "$deadline" ]; do
  text=$(execute "return document.getElementById('report').textContent")
  summary=$(printf '%s\n' "$text" | tail -n 1)
  case $summary in
    "smoke: "*) break ;;
  esac
  sleep 0.5
done
csp=$(execute "return document.documentElement.dataset.csp || 'absent'")
curl -fs --max-time 30 -X DELETE "$base/session/$session" >/dev/null

printf '%s\n' "$text" | tee "$destination"

case $summary in
  "smoke: "*) ;;
  *)
    echo "the page did not finish within ${budget}s" >&2
    exit 1
    ;;
esac
if [ "$csp" != "enforced" ]; then
  echo "the browser did not enforce the page's content-security policy" \
    "(\`document.documentElement.dataset.csp\` read \`$csp\`), so the run" \
    "says nothing about instantiating without 'unsafe-eval'" >&2
  exit 1
fi
if [ "$summary" != "$native" ]; then
  echo "the page reported \`$summary\`, the native run \`$native\`" >&2
  exit 1
fi
case $summary in
  *" 0 failed, 0 skipped") ;;
  *)
    echo "the page reported a failure or a skip: \`$summary\`" >&2
    exit 1
    ;;
esac

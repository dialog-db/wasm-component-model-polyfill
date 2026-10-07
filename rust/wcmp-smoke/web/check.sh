# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Drive the smoke test page headlessly and compare its report.
#
# The flake's `smoke-web` check runs this script inside the build
# sandbox (`tests smoke check` builds that check and prints the
# report): it serves the built page and opens it in the flake's
# Chromium through `nix/webdriver.sh`, which the flake puts in front of
# this text (the same capabilities the browser tests use, from the same
# `webdriver.json`), waits for the page's transcript to end in the
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
# destination path. Environment: as `nix/webdriver.sh` states. The flake
# wraps this script with `curl`, `jq`, `chromedriver`, and
# `static-web-server` on its PATH.

page=$1
native=$2
destination=$3
budget=180

webdriver_open "$page"

deadline=$((SECONDS + budget))
text=""
summary=""
while [ "$SECONDS" -lt "$deadline" ]; do
  text=$(webdriver_execute "return document.getElementById('report').textContent")
  summary=$(printf '%s\n' "$text" | tail -n 1)
  case $summary in
    "smoke: "*) break ;;
  esac
  sleep 0.5
done
csp=$(webdriver_execute "return document.documentElement.dataset.csp || 'absent'")
webdriver_close

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

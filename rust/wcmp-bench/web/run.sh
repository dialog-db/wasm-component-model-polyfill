# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Run the benchmark suite in headless Chrome and keep its report.
#
# The `bench web` menu command runs this script: it serves the built
# page and opens it in the flake's Chromium through `nix/webdriver.sh`,
# which the flake puts in front of this text (the same capabilities the
# browser tests use, from the same `webdriver.json`), waits for the page
# to publish its report, writes the JSON where it was asked to, and
# prints the table.
#
# The measurement deliberately happens here rather than inside a Nix
# build: a derivation's output is cached, and a cached benchmark result
# is a stale one. Nix builds the page; this script measures it, every
# time it is asked.
#
# Arguments: the page directory, the JSON report's destination, then any
# `key=value` run controls, which reach the page as its query string.
# Environment: as `nix/webdriver.sh` states, and
# `WCMP_BENCH_BUDGET_SECONDS` for how long the whole run may take (900
# by default).

page=$1
destination=$2
shift 2
budget=${WCMP_BENCH_BUDGET_SECONDS:-900}

query=$(jq --null-input --raw-output \
  '$ARGS.positional | map(@uri | gsub("%3D"; "=")) | join("&")' --args "$@")
webdriver_open "$page" "${query:+?$query}"

deadline=$((SECONDS + budget))
report=null
while [ "$SECONDS" -lt "$deadline" ]; do
  report=$(webdriver_execute "return window.wcmpBenchReport || null")
  if [ "$report" != null ]; then
    break
  fi
  sleep 0.5
done
table=$(webdriver_execute "return document.getElementById('out').textContent")
webdriver_close

if [ "$report" = null ]; then
  echo "the page did not finish within ${budget}s" \
    "(raise WCMP_BENCH_BUDGET_SECONDS, or ask for fewer samples)" >&2
  exit 1
fi
printf '%s\n' "$table"

# The report is kept only when it is one: a page that died says so in
# JSON of its own, and overwriting the last good report with it would
# lose the numbers it did not replace.
if ! printf '%s' "$report" | jq --exit-status 'type == "object" and has("benchmarks")' \
  >/dev/null 2>&1; then
  echo "the page reported no benchmarks: $report" >&2
  exit 1
fi
printf '%s\n' "$report" >"$destination"
echo "report written to $destination"

failed=$(printf '%s' "$report" \
  | jq --raw-output '[.benchmarks[] | select(.error != null) | .name] | join(", ")')
if [ -n "$failed" ]; then
  echo "benchmarks failed: $failed" >&2
  exit 1
fi

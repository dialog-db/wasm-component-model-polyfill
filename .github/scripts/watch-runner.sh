#!/usr/bin/env bash
# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Run a command while watching the runner's free disk and memory.
#
# A runner whose root filesystem fills up stalls instead of failing,
# and once it stalls nothing after it runs, so a report written to a
# file or printed by a later step is lost with it. This script reports
# while the command runs instead: every interval it prints one line of
# free space and free memory to the step's output, which GitHub streams
# off the runner as it is written. When free space on the root
# filesystem or on /nix falls below the floor, it stops the command,
# prints the largest directories on that filesystem, and fails the step
# while the runner can still report why.
#
# Usage: watch-runner.sh <command> [argument...]
# Environment: WATCH_INTERVAL (seconds, default 120) and WATCH_FLOOR_MB
# (default 3072).

set -uo pipefail

interval=${WATCH_INTERVAL:-120}
floor_mb=${WATCH_FLOOR_MB:-3072}
tripped=$(mktemp)

free_mb() {
  df --output=avail -m "$1" 2>/dev/null | tail -n 1 | tr -d ' '
}

report() {
  local line
  line="runner $(date -u +%H:%M:%S)"
  for fs in / /nix /mnt; do
    [ -d "$fs" ] && line+=" $fs=$(free_mb "$fs")MB"
  done
  line+=" $(free -m | awk '/^Mem:/ { printf "mem=%sMB", $7 } /^Swap:/ { printf " swap-used=%sMB", $3 }')"
  echo "$line"
}

largest() {
  echo "Largest directories on the filesystem of $1:"
  du -xh --max-depth=3 "$1" 2>/dev/null | sort -h | tail -n 25
}

setsid "$@" &
child=$!

(
  while kill -0 "$child" 2>/dev/null; do
    report
    for fs in / /nix; do
      available=$(free_mb "$fs")
      if [ -n "$available" ] && [ "$available" -lt "$floor_mb" ]; then
        echo "::error title=Runner out of disk::free space on $fs fell to ${available}MB (floor ${floor_mb}MB); stopping the step"
        echo "$fs" > "$tripped"
        # Stop the writing first, then measure: listing the largest
        # directories takes a while on a full disk.
        kill -TERM -- "-$child" 2>/dev/null
        largest "$fs"
        kill -KILL -- "-$child" 2>/dev/null
        exit 0
      fi
    done
    sleep "$interval"
  done
) &
watcher=$!

wait "$child"
status=$?
kill "$watcher" 2>/dev/null
wait "$watcher" 2>/dev/null
report

if [ -s "$tripped" ]; then
  exit 1
fi
exit "$status"

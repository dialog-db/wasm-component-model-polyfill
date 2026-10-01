#!/usr/bin/env bash
# Run a command, and run it again when it failed because Nix could not
# download something.
#
# Nix retries a dropped download by resuming it, but a resume the server
# refuses (HTTP 416, as cache.nixos.org sometimes answers) ends the
# whole build with "no substituter can build it". A second run starts
# the download over and usually succeeds. Only a failure whose output
# carries Nix's download errors is retried: a failed build or test
# never matches them, so it fails at once.
#
# Usage: retry-downloads.sh <command> [argument...]
# Environment: RETRY_ATTEMPTS (default 3) and RETRY_PAUSE (seconds,
# default 30).

set -uo pipefail

attempts=${RETRY_ATTEMPTS:-3}
pause=${RETRY_PAUSE:-30}
# The transcript of a `lint -L` run is large; keep it off the root
# filesystem, beside the rest of a job's scratch space.
transcript=$(mktemp -p "${XDG_CACHE_HOME:-/tmp}")
trap 'rm -f "$transcript"' EXIT

# Nix's final errors only: it also prints `warning: unable to download
# ...; retrying` for a download it then recovers, and a run that saw one
# of those and failed a test must not be retried.
download_failure='^[[:space:]]*error: (unable to download|.*there is no substituter that can build it|some substitutes for the outputs of derivation .* failed)'

for ((attempt = 1; ; attempt++)); do
  "$@" 2>&1 | tee "$transcript"
  status=${PIPESTATUS[0]}
  if [ "$status" -eq 0 ]; then
    exit 0
  fi
  if ! grep -q -E "$download_failure" "$transcript"; then
    exit "$status"
  fi
  if [ "$attempt" -ge "$attempts" ]; then
    echo "::error title=Nix downloads failed::still failing to download after $attempt attempts"
    exit "$status"
  fi
  echo "::warning title=Nix downloads failed::attempt $attempt of $attempts failed to download; retrying in ${pause}s"
  sleep "$pause"
done

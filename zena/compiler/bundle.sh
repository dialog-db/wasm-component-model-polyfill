#!/usr/bin/env bash
# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Packs the files a compile with the compiler component can read through
# its `read-source` import into one source bundle.
#
# Arguments: the file to write, then one or more pairs of a directory and
# the path its files take in the bundle. For the toolchain bundle the
# pair is Zena's standard library (`packages/stdlib/zena`) and `/stdlib`,
# the root the compiler component asks for it under. A prefix of `.`
# keeps the files' paths relative, as the compiler asks for its package
# manifest, `zena-packages.json`, and the packages the manifest names.
#
# The bundle holds every `.zena`, `.wit`, and `.json` file under each
# directory, at its path under the pair's prefix. It also holds every
# directory that has a `.wit` file directly in it, at the directory's
# path: its text is the text Zena's `readWitSource` makes of a
# directory, every `.wit` file in it in name order, each followed by a
# newline. So the compiler reads a WIT directory, such as the standard
# library's `wit`, with one request.
#
# The format is the one the Zena scenario bundle uses: each entry is a
# header line `file <path> <length>`, then its `<length>` bytes, then a
# newline. A header holds no quoting, so the script refuses a path with
# white space in it. Each pair's files come in path order and then its
# WIT directories in path order, so the same inputs make the same bytes.
set -euo pipefail
export LC_ALL=C

out=$1
shift
if [ $(($# % 2)) -ne 0 ] || [ $# -eq 0 ]; then
  echo "bundle.sh: expected pairs of a directory and a prefix after the output" >&2
  exit 1
fi

: >"$out"
add() {
  # $1 is the bundle path, and the rest are the files whose text it
  # holds, one after the other.
  local path=$1
  shift
  if [[ $path =~ [[:space:]] ]]; then
    echo "bundle.sh: a bundle path cannot hold white space: $path" >&2
    exit 1
  fi
  local length=0 file
  for file in "$@"; do
    length=$((length + $(stat -c %s "$file")))
  done
  {
    printf 'file %s %s\n' "$path" "$length"
    cat "$@"
    printf '\n'
  } >>"$out"
}

under() {
  # The bundle path of `$1`, a path relative to a pair's directory.
  if [ "$prefix" = . ]; then
    printf '%s' "${1:-.}"
  elif [ -z "$1" ]; then
    printf '%s' "$prefix"
  else
    printf '%s/%s' "$prefix" "$1"
  fi
}

while [ $# -gt 0 ]; do
  root=$1
  prefix=${2%/}
  shift 2
  (
    cd "$root"
    find . -type f \( -name '*.zena' -o -name '*.wit' -o -name '*.json' \) |
      sed 's|^\./||' | sort |
      while read -r file; do
        add "$(under "$file")" "$file"
      done
    # A WIT directory: each `.wit` file followed by a newline, which
    # is what `readWitSource` puts after each.
    find . -type f -name '*.wit' -printf '%h\n' | sed 's|^\./\?||' | sort -u |
      while read -r directory; do
        path=$(under "$directory")
        joined=$(mktemp)
        find "${directory:-.}" -maxdepth 1 -type f -name '*.wit' | sort |
          while read -r file; do
            cat "$file"
            printf '\n'
          done >"$joined"
        add "$path" "$joined"
        rm "$joined"
      done
  )
done

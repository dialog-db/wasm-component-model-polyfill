#!/usr/bin/env bash
# Packs what the polyfill subjects of the Zena scenarios read into one
# file. The flake's test archives embed that file in the `zena` test
# with `include_bytes!`, so the compiled scenarios and the Wasmtime
# observations reach the browser lane, which has no file system, as
# well as the native lane.
#
# Arguments: the directory of scenario sources, the output of
# `build.sh` for it, the output of the Wasmtime run for it, and the
# file to write.
#
# The bundle holds, for each scenario the build compiled:
#
# - `<scenario>/expectations.txt`, from the sources.
# - `<scenario>/observations.txt`, from the Wasmtime run.
# - `<scenario>/<program>.status`, `.log`, and `.wasm` for each
#   program, as `build.sh` left them.
#
# and `zena-revision`, the toolchain's revision. Each file is a header
# line `file <path> <length>`, then its `<length>` bytes, then a
# newline. The test reads the bundle back in `tests/zena/bundle.rs`.
set -euo pipefail
shopt -s nullglob

scenarios=$1
compiled=$2
observed=$3
out=$4

: >"$out"
add() {
  {
    printf 'file %s %s\n' "$1" "$(stat -c %s "$2")"
    cat "$2"
    printf '\n'
  } >>"$out"
}

cd "$compiled"
add zena-revision zena-revision
for directory in */; do
  name=${directory%/}
  add "$name/expectations.txt" "$scenarios/$name/expectations.txt"
  add "$name/observations.txt" "$observed/$name/observations.txt"
  for file in "$name"/*; do
    add "$file" "$file"
  done
done

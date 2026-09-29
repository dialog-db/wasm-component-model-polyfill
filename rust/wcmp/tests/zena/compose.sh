#!/usr/bin/env bash
# Composes the components of each Zena scenario whose wiring asks for
# a composition. The flake runs this script inside the build sandbox,
# with `wac` on the PATH, after the Zena programs and the Rust partners
# of every scenario are compiled side by side.
#
# Arguments: the directory of scenario sources, and the directory of
# compiled scenarios, which the script changes in place.
#
# A scenario's `wiring.txt` holds one link per line, `<linking>
# <importer> <import> <exporter>`, and a line that starts with `#` is a
# comment. Each `composition` link plugs the exporter into the importer: every
# exporter of one importer goes into one `wac plug`, which satisfies
# the importer's imports with the exporters' exports of the same name.
# The composition takes the importer's name, so the subjects run it
# like a scenario with one component, and the calls of the
# expectations name it as they name the importer.
#
# `wac` refusing the components does not fail the build: that outcome
# is the scenario's `compose` stage, which a later step records. For
# each importer, the output under `<compiled>/<scenario>/` then holds:
#
# - `<importer>.compose-status`: `wac`'s exit status, `0` when the
#   components composed.
# - `<importer>.compose-log`: everything `wac` wrote to standard error
#   and standard output. `wac` runs in the scenario's directory on
#   relative names, so the text names no build directory.
# - `<importer>.wasm`: the composition when `wac` succeeded, and the
#   importer as it compiled otherwise.
#
# Each exporter's `.status`, `.log`, and `.wasm` are removed either
# way: a composed exporter is inside the composition, and a refused
# composition stops every subject before any component runs.
#
# A composition whose importer or exporter did not compile is not
# attempted, and its files stay as they are: the scenario stops at
# `compile`, which comes first.
#
# The script fails when a composition link names a program the scenario
# does not have, when an exporter is plugged into two importers, or when
# a program is both an importer and an exporter of compositions: that
# is a wiring it cannot make, not an outcome of `wac`.
set -euo pipefail
shopt -s nullglob

scenarios=$1
compiled=$2

for directory in "$compiled"/*/; do
  name=$(basename "$directory")
  wiring="$scenarios/$name/wiring.txt"
  [ -e "$wiring" ] || continue

  importers=()
  declare -A plugs=()
  declare -A plugged=()
  line_number=0
  while IFS= read -r line || [ -n "$line" ]; do
    line_number=$((line_number + 1))
    read -r -a words <<<"$line"
    if [ ${#words[@]} -eq 0 ] || [ "${words[0]}" != composition ]; then
      continue
    fi
    if [ ${#words[@]} -ne 4 ]; then
      echo "compose: $name/wiring.txt:$line_number: expected \`<linking> <importer> <import> <exporter>\`" >&2
      exit 1
    fi
    importer=${words[1]}
    exporter=${words[3]}
    for program in "$importer" "$exporter"; do
      if [ ! -e "$directory/$program.status" ]; then
        echo "compose: $name/wiring.txt:$line_number: scenario $name has no program $program" >&2
        exit 1
      fi
    done
    if [ -n "${plugged[$exporter]:-}" ] && [ "${plugged[$exporter]}" != "$importer" ]; then
      echo "compose: $name/wiring.txt:$line_number: $exporter is plugged into ${plugged[$exporter]} and $importer" >&2
      exit 1
    fi
    plugged[$exporter]=$importer
    if [ -z "${plugs[$importer]+set}" ]; then
      importers+=("$importer")
      plugs[$importer]=""
    fi
    plugs[$importer]+=" $exporter"
  done <"$wiring"

  for importer in "${importers[@]}"; do
    if [ -n "${plugged[$importer]:-}" ]; then
      echo "compose: $name/wiring.txt: $importer is both an importer and an exporter of compositions" >&2
      exit 1
    fi
  done

  for importer in "${importers[@]}"; do
    read -r -a exporters <<<"${plugs[$importer]}"
    compiled_all=true
    for program in "$importer" "${exporters[@]}"; do
      if [ "$(cat "$directory/$program.status")" != 0 ]; then
        compiled_all=false
      fi
    done
    if [ "$compiled_all" = false ]; then
      echo "compose: $name/$importer: not attempted, a program did not compile"
      continue
    fi
    plug_args=()
    for exporter in "${exporters[@]}"; do
      plug_args+=(--plug "$exporter.wasm")
    done
    status=0
    (
      cd "$directory"
      wac plug "$importer.wasm" "${plug_args[@]}" -o "$importer.composed" \
        >"$importer.compose-log" 2>&1
    ) || status=$?
    echo "$status" >"$directory/$importer.compose-status"
    if [ "$status" -eq 0 ]; then
      mv "$directory/$importer.composed" "$directory/$importer.wasm"
    else
      rm -f "$directory/$importer.composed"
    fi
    for exporter in "${exporters[@]}"; do
      rm -f "$directory/$exporter.status" "$directory/$exporter.log" \
        "$directory/$exporter.wasm"
    done
    echo "compose: $name/$importer: exit $status"
  done
  unset plugs plugged
done

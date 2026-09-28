#!/usr/bin/env bash
# Compiles every Zena scenario under a directory to components. The
# flake's `zena-scenarios` package runs this script inside the build
# sandbox with the `zena` command of the pinned toolchain in `$ZENA`, so
# the components are built at test time and never committed.
#
# Arguments: the directory that holds one directory per scenario, and
# the output directory.
#
# A scenario directory holds its Zena programs as `<program>.zena`, one
# component per program, and optionally its WIT under `wit/`. Without
# `wit/`, Zena derives each program's world. With it, each program
# compiles against the world named after the program, through `--wit wit
# --world <program>`. A contributor adds a scenario by adding its
# directory; nothing here names one. The scenario's `expectations.txt`,
# and its `wiring.txt` when its components link, are for the steps
# after this one, which read them from the sources.
#
# The script fails when the directory holds no scenario, or a scenario
# holds no program: that is a layout it cannot read, not an outcome of
# Zena.
#
# Zena refusing a program does not fail the build: that outcome is the
# scenario's `compile` stage, which a later step records. For each
# program the output holds, under `<out>/<scenario>/`:
#
# - `<program>.status`: Zena's exit status, `0` when the program
#   compiled.
# - `<program>.log`: everything Zena wrote to standard error and
#   standard output. Zena names a program by its absolute path, which
#   here is a build directory; the script strips the scenario
#   directory's prefix so the text names the file as the scenario does.
# - `<program>.wasm`: the component, present only when the program
#   compiled.
set -euo pipefail
shopt -s nullglob

scenarios=$1
out=$2

scenario_dirs=("$scenarios"/*/)
if [ ${#scenario_dirs[@]} -eq 0 ]; then
  echo "zena: $scenarios holds no scenario directory" >&2
  exit 1
fi

mkdir -p "$out"
for scenario in "${scenario_dirs[@]}"; do
  name=$(basename "$scenario")
  mkdir -p "$out/$name"
  (
    cd "$scenario"
    programs=(*.zena)
    if [ ${#programs[@]} -eq 0 ]; then
      echo "zena: scenario $name holds no .zena program" >&2
      exit 1
    fi
    for program in "${programs[@]}"; do
      stem=${program%.zena}
      world_args=()
      if [ -d wit ]; then
        world_args=(--wit wit --world "$stem")
      fi
      status=0
      "$ZENA" build "$program" --target component "${world_args[@]}" \
        -o "$out/$name/$stem.wasm" >"$out/$name/$stem.raw" 2>&1 || status=$?
      sed "s|$PWD/||g" "$out/$name/$stem.raw" >"$out/$name/$stem.log"
      rm "$out/$name/$stem.raw"
      echo "$status" >"$out/$name/$stem.status"
      if [ "$status" -ne 0 ]; then
        rm -f "$out/$name/$stem.wasm"
      fi
      echo "zena: $name/$program: exit $status"
    done
  )
done

---
id: 0bb7af
title: The gate runs the benchmarks and sees the whole public API
type: chore
blocked_by: []
labels: [bench, api, backlog-burndown-001-q3]
created: 2026-09-22T10:18:02Z
---

## What to build
The review of card `0ed715` accepted the benchmark suite (`rust/wcmp-bench`) and left findings that need one card before the throughput cards lean on its numbers. First, `measurement.rs` and `report.rs` have no tests: `percentile_ns`, `median_ns` and `spread_percent` (`measurement.rs:91-118`) are what every before/after comparison rests on, and `Report::json`'s top-level shape (`target`, `plan`, `benchmarks`) is asserted nowhere; a unit test over known samples (`[1,2,3,4,5]` → median 3, p10 1, p90 5, spread 400 percent) and one over the JSON shape close that. Second, nothing in the gate runs the 17 real benchmarks: `nix flake check` builds `bench-native` and `bench-web` but never executes them, the harness tests drive synthetic loops, and the two inline guests in `guests.rs` go through `component!` (parse and encode, no validation), so a wrong `canon lift` shape or a renamed export passes `lint` and `tests all` and fails only when a person runs `bench`. Add one `#[wcmp_macros::test]` that measures every benchmark once with `Plan { warmup_iterations: 1, samples: 1, .. }` and asserts no failure, so `tests all` holds the "runs on both targets" half of the suite's promise. Third, `README.md:106` and `plan.rs:14` call the calibration batch "untimed" while `run.rs:121-133` times it and discards the reading; and when `max_batch` is reached before a batch lasts half the target sample (`run.rs:127`), a sample sits at the clock's resolution and the report does not mark it. Fourth, `web/run.py:37-67` and `:72-112` duplicate `rust/wcmp-smoke/web/check.py:30-59` and `:66-103` (port, server, wait, request, chromedriver session), with a third `serve` in `smoke/web/serve.py`; share one module before a third driver script appears. Fifth, `Plan`'s fields are public but `validate()` is private, so a directly built `Plan { max_batch: 0 }` reaches the division at `run.rs:143` and writes `null`; validate in `Run::new` or make the fields private.

## Absorbed cards
The triage of 2026-09-26 merged these cards into this one and cancelled them. Both make the flake check hold something it currently only builds: the benchmark suite's numbers and the complete public API surface. The runtime-layer PDD (f259d6) measures its memory model with the benchmarks, and publication needs the API check.

This card's own findings are the ones above, filed as "The benchmark suite's numbers and its 17 benchmarks are held by the gate". Each absorbed card's What to build and acceptance criteria carry over in full. Read them in `project/kanban/issues/<id>.md`:

- `c41ebb`: The public-api check sees derived impls, caches its listing, and its last internal item is settled

## Acceptance criteria
- [x] Unit tests pin the median, the percentiles, the spread and the JSON report's shape.
- [x] `tests all` runs every benchmark once on both targets with a one-sample plan and fails if any benchmark fails.
- [x] The calibration batch is described as it behaves, and a report marks a sample that hit the batch ceiling below the clock floor.
- [x] The WebDriver plumbing is shared by the smoke check and the bench runner rather than duplicated.
- [x] A `Plan` cannot reach `Run` unvalidated.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.
- [x] Every acceptance criterion of the absorbed cards (`c41ebb`) is met.

## Review notes
- 2026-09-22T19:30:13Z **this card's premise moved under it.** Commit `bb449ff` deleted `rust/wcmp-smoke/web/check.py` and `web/serve.py` and replaced them with `web/check.sh`, a curl-driven WebDriver script served by static-web-server, while the bench runner still carries `rust/wcmp-bench/web/run.py` in Python. The duplication this card was filed for (identical `free_port`/`serve`/`wait_for_port`/`request` helpers and chromedriver session dance) is therefore no longer two copies of the same Python — it is now one shell script and one Python script doing the same job in different languages. Decide which one the project keeps before sharing anything; the criterion above no longer says Python.
- 2026-09-28: PDD025 (Runtime Layer, drafted from f259d6) measures its memory-access model with the benchmark suite (string and list round trips on every backend, before and after the switch step). This card should land before that switch.
- 2026-10-07 delivered (host session, backlog-burndown-001-q3) as
  `fix(bench): Validate plans, mark capped batches, run the suite in tests`,
  `refactor(web): Share WebDriver plumbing between smoke check and bench`,
  and `chore(api): List derived impls and hide the indexed resource type`.
  - Statistics: `measurement.rs` tests pin nearest-rank percentiles over
    unsorted `[4,1,5,3,2]` (median 3, p10 1, p90 5) and the spread. **The
    card's "spread 400 percent" is wrong for the code's definition**, which is
    (p90 - p10) / median = 4 / 3 = 133.3 percent; the test pins 133.3 and the
    doc's definition. `report.rs` pins the top-level JSON shape exactly.
  - `it_measures_every_benchmark_once_on_this_target` (lib test) runs every
    benchmark with a one-sample, one-iteration-batch plan and fails on any
    error, on both lanes (0.18 s native, 0.28 s web).
  - Calibration: README and `Plan` docs now say the calibration batches are
    timed and their readings thrown away. `Run::capped` marks a batch that
    reached `max-batch` while it still lasted less than half a sample. The
    row's note reads "batch capped below the clock floor: raise max-batch",
    and the JSON has `batch_capped`.
  - WebDriver: the project keeps shell. `nix/webdriver.sh` holds the
    serving, session and execute plumbing. The flake puts it in front of
    `rust/wcmp-smoke/web/check.sh` and of the new `rust/wcmp-bench/web/run.sh`,
    which replaces `run.py`. `WCMP_SMOKE_WEBDRIVER` is now `WCMP_WEBDRIVER`
    (only `smokeWebkit` set it). `smoke-web` check green; `bench web
    samples=1 warmup=1` ran end to end.
  - `Plan::validate` is public, and `Run::new` calls it.
  - c41ebb: the listing uses `-ss`, so derived impls are in the snapshot
    (+520 lines); the snapshot is excluded from the listing's source; the
    nightly and cargo-public-api coupling and its failure are in the
    comment; the `Component` doc says what the compile-fail block proves and
    why the `imports`/`exports` reach is bounded; `ResourceType::indexed`
    moved to `internal::ResourceTypeInternal` (the one removed public item).
  - Gate so far: native 1712/1712, web 1697/1697, `lint clippy clippy-wasm32
    rustfmt public-api doctests markdown smoke-web bench-native bench-web`
    green. `tests all` and full `lint` run on the composed series tip.


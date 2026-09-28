# Zena Toolchain Compatibility

[Zena] is a young, statically typed language that compiles to WebAssembly with
garbage collection (WasmGC). Its compiler can emit a component. Zena changes
every day, and the polyfill changes often too. Each project iterates toward more
compatibility with the other over time. This document designs the tests that
measure that compatibility, so that both projects can see where it stands after
each change.

The tests compile real Zena programs with Zena's own toolchain at a pinned
revision. They run each component under Wasmtime first, and then under the
polyfill in the browser and natively. They record where each run stops. They do
not assert that every run passes. A person moves the pin, runs the tests again,
and reads the difference.

The browser is the main subject. The Component Model is least likely to reach
the browser by other means soon, and a browser host is where it has the most
value. The native run of the polyfill is secondary.

Nine terms recur:

- The toolchain is the Zena compiler and its standard library, as Zena's own Nix
  package builds them.
- The pin is the Zena revision that this repository's flake lock records.
- A scenario is a small program, or a small group of programs, with a list of
  the calls to make and the results to expect.
- A partner is a component in a scenario that another toolchain builds.
- A subject is one run of a scenario: `wasmtime`, `web`, or `native`.
- The Wasmtime run is the `wasmtime` subject. It sets the behavior that the
  polyfill must match.
- A stage is the step where a subject stopped.
- The record is the committed file that holds the stage of every scenario for
  every subject.
- The scenario runner is the test code that builds, links, instantiates, and
  calls each scenario.

## Goals

- Each scenario compiles from source with the pinned toolchain at test time. No
  compiled Zena output is committed.
- The toolchain enters the build as a flake input. Nothing Zena needs to build,
  such as Node.js or npm, enters this repository's development shell.
- Wasmtime runs each scenario natively. The Wasmtime run separates a fault of
  the polyfill from a fault of the program or of Zena's output.
- The polyfill runs each scenario in the browser and natively. The browser run
  is the primary subject.
- The record holds one stage per scenario per subject. A Zena compile failure is
  a stage, not a broken build.
- A run fails when any stage differs from the record, in either direction.
- The record names the pin it was made from, and a run fails when the two
  differ.
- One command regenerates the record from all three subjects.
- Scenarios cover one component alone, two Zena components linked together, and
  a Zena component linked with a component from another toolchain. The links are
  made at run time through a `Linker` and ahead of time through composition.
- A menu command prints a compatibility report that leads with the browser and
  shows why each subject stopped. `tests all` runs the scenarios as one of its
  lanes.
- Each failure that code reading predicts today is confirmed or refuted by a
  scenario.

## Non-goals

- A fix for any failure that a scenario finds. Each root cause gets its own
  work.
- Running the Zena compiler inside the polyfill. The compiler is a WASI Preview
  1 core module, not a component.
- A WASI host for the polyfill. The scenario runner supplies only the few WASI
  functions that the scenarios call.
- The `wasi:http` interfaces. Zena can emit a `fetch` over `wasi:http`, and no
  scenario uses it until a host for those interfaces exists.
- A general framework for other external toolchains. The design keeps its
  toolchain-neutral parts apart, and it builds no extension point.
- Runs with the suspend provider turned off. The conformance suite covers those
  states.
- A scheduled or automatic move of the pin.
- A Zena story on the smoke test page.

## Facts This Design Rests On

Each fact below was read from the cited source. The Zena facts are from revision
`b2237f7` of its repository, dated 2026-09-26.

- Zena's component target emits WasmGC core modules inside a Canonical ABI shell
  of linear memory. Strings cross the boundary as `utf8`. Async exports are
  lifted with a callback ([Zena component emission], "Status").
- On the component target, console output goes through the WASI Preview 3 (p3)
  interfaces `wasi:cli/stdout@0.3.0` and `wasi:cli/stderr@0.3.0`. A write passes
  a `stream<u8>` to `write-via-stream`, which returns a
  `future<result<_, error-code>>` that Zena does not read. The `error-code` type
  comes from `wasi:cli/types@0.3.0`. A program imports `wasi:cli/stderr` only
  when it writes an error line. The component target has no WASI Preview 2 (p2)
  stdio path ([Zena console], [Zena stdlib manifest], [Zena component emission],
  C6).
- Timers go through the p3 interfaces `wasi:clocks/monotonic-clock@0.3.0` and
  `wasi:clocks/types@0.3.0`. Zena calls `now` and the async `wait-for` ([Zena
  timers]).
- A program that uses memory becomes two core modules. One module defines the
  memory, and the program module imports it ([Zena component emission], 1.3).
- `--wit` and `--world` declare a world that the program must match. A
  difference in either direction is a compile error ([Zena component emission],
  C3.2).
- When a program uses exceptions, Zena exports a tag named `__zena_exception`
  with no parameters. It also defines a mutable global whose type is a nullable
  reference to the error struct, or `eqref`. The global carries the thrown value
  ([Zena exception tag]).
- Zena's string exports force the same exception machinery into the component,
  for a reason that Zena does not record ([Zena component emission], C3.1).
- The browser backend refuses a core module that imports or exports a tag
  ([browser backend tags]). It refuses a global whose type is a reference type
  other than `funcref` or `externref` ([browser backend refs]).
- The Wasmtime backend refuses a core module that imports or exports a tag
  ([Wasmtime backend tags]).
- Zena's flake exports a package that builds the whole Zena monorepo. The build
  runs `npm run build`, which also compiles four Rust crates with
  `cargo build --release`. It uses a fixed npm dependency hash and a vendored
  cargo lock, so it runs offline ([Zena flake]).
- The one prebuilt input of that build is a checked-in seed of the compiler
  ([Zena development]).
- Zena's public continuous integration runs `npm ci` and `npm test` inside
  `nix develop`. It does not build the Nix package ([Zena CI]).
- `wasmtime-wasi` is published at the version of Wasmtime that this workspace
  pins ([wasmtime-wasi versions]).

## The Toolchain Pin

The toolchain enters the build as a flake input for Zena's repository. The
scenario builds use Zena's `zena` package without change. They use only the
`zena` command from it.

The input does not follow this flake's `nixpkgs`. Zena builds against its own
`nixpkgs`, as its author builds and tests it. A different Node.js or Rust
compiler can break the build for reasons that have nothing to do with Zena. The
cost is a second `nixpkgs` in the lock and a second toolchain closure on disk.

The Zena package is used only inside the build of a scenario. It never enters
the development shell, so a contributor never has Node.js or npm on the path.

The design assumes that each Zena revision builds under Nix. If a revision does
not build, the pin stays where it is. The owner of this repository raises the
failure with Zena's author directly. This repository does not patch Zena's
build.

The package builds every part of Zena, including its website, its editor
plug-ins, and four Rust crates. The public binary cache does not hold it. Nix
builds it once per pin and keeps it in the store. A machine whose store already
holds the build reuses it. The first build on a cold store pays the full cost.

## Scenarios

### What a Scenario Holds

A scenario holds these sources:

- One or more Zena programs.
- Optional WIT. When a scenario has WIT, each Zena program compiles against the
  world named after the program, which Zena reads through `--wit` and `--world`.
- For a scenario with a partner, the partner's Rust source and its locked cargo
  manifest.
- For a scenario with links, the wiring: which component's exports satisfy which
  component's imports, and whether the link is made at run time or by
  composition.
- An expectations file.

The expectations file lists the calls in order. Each entry names the component
and the export, gives the arguments, and gives one of three outcomes:

- The results the call returns.
- A failure of the call.
- No outcome. The Wasmtime run decides it.

An entry can ask for a typed call. The file also lists the lines that the
scenario prints, in order.

A scenario meets four criteria:

- It uses one feature of Zena's component output that a real program uses.
- It is deterministic. For example, a program that sleeps prints "before" and
  "after", but never a duration.
- Its Zena source compiles with the pinned toolchain. Nobody writes its core
  modules by hand, in text format or in any other form.
- It is small, so that a failure points to one cause.

### Integration Scenarios

An integration scenario has more than one component. Each component under test
has one WIT world: each Zena program compiles against the world in the
scenario's WIT that is named after the program, with `--wit` and `--world`. A
partner is not under test, so it declares whatever world it needs, in the
scenario's WIT or in its own, and compiles against it with `wit-bindgen`. When
the partner's copy of an interface differs from the Zena program's, the run
stops at `link`. That stage reports a fault of the scenario, not of Zena or the
polyfill.

The scenario runner links the components in one of two ways:

- At run time. The runner instantiates the exporting component first. It then
  gives that instance's exports to the importing component through the
  polyfill's `Linker`. The Wasmtime run does the same through Wasmtime's
  `Linker`.
- By composition. The build composes the components with `wac` into one
  component. The flake already pins `wac` for the conformance fixtures. The
  subjects then run the composed component like a scenario with one component.

A Rust partner uses `wit-bindgen`, as the Rust fixtures of the conformance suite
do. The build compiles it at test time, with this workspace's Rust toolchain and
a locked cargo manifest. The partner changes only when this repository changes
it. So a move of the pin changes only the Zena side.

Both directions matter. The importer lowers the call and the exporter lifts it.
Each toolchain writes a different half of the Canonical ABI.

### The First Set

The first set has seven scenarios with one component:

| #   | Scenario                                                             | What it proves                                                             |
| --- | -------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| 1   | A scalar export, called untyped and typed                            | The baseline. Whether the smallest program meets a refusal in the browser. |
| 2   | A string in and a string out                                         | The `utf8` shell and the shape with two core modules.                      |
| 3   | Classes and arrays, used inside the program only                     | The browser backend's refusal of a global with a GC reference type.        |
| 4   | Exceptions, one caught inside the program and one uncaught           | The refusal of a tag export on both backends, and an uncaught exception.   |
| 5   | Console output, with one error line                                  | The p3 `wasi:cli/stdout` and `wasi:cli/stderr` stream writes.              |
| 6   | An async export that sleeps on a timer                               | The export lifted with a callback, and the p3 `wasi:clocks` imports.       |
| 7   | A custom world through `--wit` and `--world` that imports a function | An import that the host supplies.                                          |

It also has six integration scenarios. Each one passes a string across the link:

| #   | Importer | Exporter | Link        |
| --- | -------- | -------- | ----------- |
| 8   | Zena     | Zena     | Run time    |
| 9   | Zena     | Zena     | Composition |
| 10  | Zena     | Rust     | Run time    |
| 11  | Rust     | Zena     | Run time    |
| 12  | Zena     | Rust     | Composition |
| 13  | Rust     | Zena     | Composition |

A string crosses two shells of linear memory. A fault in a copy or in a
`realloc` shows there first. A link from Rust to Rust is not in the set. The
conformance fixtures cover it.

Scenario 4 makes the uncaught call twice. The second entry has no outcome, so
the Wasmtime run decides whether the instance still answers. This design states
no rule for an exception that leaves a lifted export. The Wasmtime run sets the
expectation, and a difference is a `mismatch` for a person to diagnose.

The facts above predict that most of the first set stops at `instantiate` in the
browser. Scenario 2 and every integration scenario pass a string, so each
component carries the tag export and its global. The record confirms or refutes
the prediction. A GC type cannot cross the component boundary, because Zena
lifts through linear memory. So the design expects no failure in the conversion
of export types. Scenarios 2 and 3 confirm or refute that too.

### Typed Calls

The scenario runner calls every export untyped, through `Func::call` with `Val`
arguments. A typed call through `TypedFunc` needs its Rust types at compile
time. So the runner supports typed calls for a closed set of signatures only:
scalars and `string`. Scenario 1 makes a typed call on its scalar export, and
scenario 2 on its string export.

### Adding a Scenario

A contributor adds a scenario in four steps:

1. Write the Zena program, and the WIT world if it needs one.
2. Write the expectations file.
3. Run the command that regenerates the record.
4. Read the new lines of the record, and commit them with the sources.

The build finds each scenario on its own. The contributor writes no Rust code in
the scenario runner.

## The Wasmtime Run

The Wasmtime run uses Wasmtime at the version this workspace pins, through its
component API. `wasmtime-wasi` at the same version supplies the p2 and p3 WASI
imports. It captures standard output in memory, so that the runner can compare
it.

The Wasmtime run is native only, once per scenario. Wasmtime does not run in the
browser. So the Wasmtime run happens in the build, before either polyfill run.
It writes its observations: the results or the failure of each call, and the
output lines. Both polyfill subjects read those observations. The observations
are a build product and are not committed. The record keeps only the Wasmtime
stage.

The Wasmtime run is judged against the expectations file. The polyfill subjects
are judged against the Wasmtime run. A polyfill subject passes when every call
completes as the Wasmtime call did, with the same results, and the output lines
are the same. If the Wasmtime run did not pass, the polyfill subjects are judged
against the expectations file instead.

A failure is classified from the Wasmtime stage:

- If the Wasmtime run stops before `pass`, the fault is on Zena's side. The
  owner raises it with Zena's author. The polyfill subjects still run, and their
  stages are recorded beside it.
- If the Wasmtime run passes and a polyfill subject stops earlier, the fault is
  the polyfill's.
- If a polyfill subject passes where the Wasmtime run fails, the polyfill can be
  too lenient. A person examines it.

Zena's own flake pins a different Wasmtime. That version never enters these
tests. It is a detail of Zena. A difference between the two versions is one
possible diagnosis of a real failure, and nothing more.

## Host Imports

The Wasmtime run gets its WASI imports from `wasmtime-wasi`.

The polyfill subjects get them from test host functions in the scenario runner.
The functions use the polyfill's public `Linker` API, and the crate does not
export them. They cover only what the first set calls:

- The p3 `write-via-stream` functions of `wasi:cli/stdout@0.3.0` and
  `wasi:cli/stderr@0.3.0`, and the `wasi:cli/types@0.3.0` interface that holds
  `error-code`. Each function reads the guest's stream into a buffer and answers
  a future that resolves when the guest drops its end. The runner compares the
  standard output buffer with the output of the Wasmtime run. The runner keeps
  standard error apart and does not compare it.
- The p3 `wasi:clocks/monotonic-clock@0.3.0` functions `now` and `wait-for`,
  with `wasi:clocks/types@0.3.0`. `wait-for` is an async host function. A
  `setTimeout` timer backs it in the browser, and a native timer backs it
  natively.
- One fixed test interface with one function that takes a string and returns it.
  Scenario 7 imports it. The Wasmtime run defines the same function on
  Wasmtime's `Linker`.

The functions use the interface versions that the pinned toolchain emits. A
method that the functions do not implement returns an error. So a Zena change
that calls a new method shows as a stage, not as a silent pass.

The test host functions are not a WASI host. If the scenarios need more than a
small set of them, the project builds a real WASI host for the polyfill. When
one exists, the scenario runner uses it in place of the test host functions. The
scenarios and the format of the record do not change. Stages can move, and that
move goes through regeneration like any other.

## Outcomes

### Stages

Each subject records the first stage where it stopped:

| Stage         | Meaning                                                                                           |
| ------------- | ------------------------------------------------------------------------------------------------- |
| `compile`     | Zena refused a program. The stage is the same for every subject, and nothing else runs.           |
| `compose`     | `wac` refused the components. The stage is the same for every subject.                            |
| `parse`       | `Component::new` failed.                                                                          |
| `link`        | The `Linker` did not supply an import.                                                            |
| `instantiate` | Instantiation failed.                                                                             |
| `call`        | A call failed where its expectation is a result.                                                  |
| `mismatch`    | Every call ran, but a result or an output line differs, or a call succeeded where it had to fail. |
| `pass`        | Every call and every output line met its expectation.                                             |

The build of a Zena program never fails. The build keeps Zena's exit status and
its error output. A failed compile becomes the `compile` stage. A failed
composition becomes the `compose` stage in the same way.

### The Record

The record is one committed text file. A header names the pin, and comments
explain the columns and the stages. Each other line holds one scenario and one
subject. The stages in this example are invented:

```
# zena b2237f7e65847eda43ef1f4094eea77fe225ce0d
string-roundtrip  wasmtime   pass
string-roundtrip  web        instantiate  "tags are not supported in the js_wasm_runtime_layer backend"
string-roundtrip  native     instantiate  "tags are not supported in the wasm_runtime_layer"
```

Every subject has its own line. The record has no overlay files. The expected
failures of the conformance suite use a shared list and an overlay for the
browser because they hold thousands of lines. This record holds tens of lines,
and explicit lines make the change of a pin move a plain line difference.

The record is separate from the expected failures of the conformance suite. Its
stages are not the categories of that list.

### The Gate

A run fails in three cases:

- A subject's stage differs from the record. A scenario that passes and has a
  failing stage in the record fails the run too.
- The record has no line for a scenario, or a line for a scenario that does not
  exist.
- The pin in the record's header differs from the pin in the flake lock. The
  build gives the revision of the flake input to the scenario runner.

The run compares stages only. The error text is in the record for a person to
read. It holds names and numbers from Zena's output, which change at each move
of the pin. Regeneration rewrites the text, so it does not go stale.

### Regeneration

One command runs all three subjects and writes the record again. The browser run
is part of it, because the browser subject matters most. A dry run prints the
difference and writes nothing.

## Moving the Pin

The pin tracks Zena's `main` branch. Zena has no releases.

The owner moves the pin on demand, when Zena lands something relevant. A
contributor moves it only as part of work that needs a newer compiler. No
schedule and no automation moves it, because each move costs a cold build of
Zena.

A person moves the pin in four steps:

1. Run `nix flake update zena`.
2. Run the command that regenerates the record.
3. Read the difference in the record.
4. Commit the flake lock and the record together, in one commit.

The difference decides what else the move needs:

- A stage that moved later, or a new `pass`, needs nothing more.
- A new failure of `web` or `native`, where the Wasmtime run passes, needs one
  card per root cause. The record line stays as the expected stage until that
  work fixes it.
- A new `compile` stage, or a new failure of the Wasmtime run, is on Zena's
  side. The owner raises it with Zena's author. It needs no work in the
  polyfill, unless a diagnosis shows that the polyfill's Wasmtime is at fault.

A revert of the one commit restores the lock and the record together.

## Reporting

The menu has two commands:

- `tests zena` runs the three subjects and prints the compatibility report.
- `tests zena regenerate` writes the record again. It accepts `--dry-run`.

`tests all` runs the scenarios as one more timed lane.

The report is a list with one entry per scenario. Under each scenario it lists
the subjects `Browser`, `Native`, and `Wasmtime`, in that order, one per line.
Each line holds the stage. When the stage comes before `pass`, the line also
holds the reason: the error text of the step that stopped the subject, as the
run observed it. A header line names the pin. A footer counts the passes of each
subject.

```text
zena at b2237f7

- async-sleep
  - Browser: parse (<error text>)
  - Native: parse (<error text>)
  - Wasmtime: pass
- scalar-export
  - Browser: pass
  - Native: pass
  - Wasmtime: pass

Passes: Browser 1/2, Native 1/2, Wasmtime 2/2
```

The README has a section on Zena compatibility beside its section on
conformance. It holds a dated copy of the report with the pin. A person updates
it at a move of the pin, as the conformance table is updated. The section states
that the record is the source of truth and names the menu command.

The scenarios run with the suspend provider turned on, which is the default. The
async scenario lifts with a callback and needs no stack switch.

## Relationship to the Conformance Suite

[PDD016] builds its real-guest fixtures with a command and commits the result.
That model suits a fixture whose toolchain moves on purpose, rarely. Zena moves
every day, and the purpose of these tests is to follow it. So Zena components
are built at test time and never committed. The flake lock already makes the
build reproducible.

The scenarios do not run through the `.wast` harness of [PDD016]. That harness
asserts results, and a scenario records stages. The two suites share the
toolchain for composition and the recipe for Rust partners.

## Generality

The design has two parts that are specific to Zena: the flake input, and the
compile step that runs `zena build --target component`. Every other part is
neutral to the toolchain: the stages, the record, the scenario runner, the two
kinds of link, the Wasmtime run, and the test host functions. The Rust partners
already make the design use two toolchains.

When a second pinned toolchain arrives, it adds its input and its compile step.
The model of outcomes stays as it is. The design builds no extension point for a
toolchain that does not exist yet. The record, the menu command, and the README
section carry Zena's name, because Zena is what they gate.

## User Stories

The owner of this repository hears that Zena changed how it emits exceptions.

> The owner runs `nix flake update zena` and then `tests zena regenerate`. The
> difference shows scenario 4 moved from `instantiate` to `pass` in the browser.
> Two other scenarios moved from `pass` to `call` natively. The owner commits
> the lock and the record together and files one piece of work for the new
> native failure.

A developer builds a tool in the browser that compiles Zena programs to
components and runs their tests on the polyfill.

> The developer reads the report in the README. The browser lines show which
> Zena features run today and where the others stop. The developer sees that a
> program with a string export stops at `parse` in the browser, and reads the
> error text on the same line to learn why.

A contributor finds a Zena feature that no scenario covers.

> The contributor writes a short Zena program and an expectations file, and runs
> `tests zena regenerate`. The new lines show that the Wasmtime run passes and
> the browser stops at `link`. The contributor commits the scenario with its
> record lines. The gap is now visible in every later run.

Zena's author asks whether a change on Zena's side broke anything for the
polyfill.

> The owner moves the pin to the author's revision. A scenario now stops at
> `compile`. The error output in the record shows Zena's message, and the owner
> sends it to the author. The polyfill needs no change.

## Test Cases

A scenario compiles from the pinned toolchain and runs under every subject. The
build compiles scenario 1 with the `zena` package of the flake input, at test
time, with no committed Zena output. The Wasmtime, browser, and native subjects
each run it and write a stage to the record.

The toolchain stays out of the development shell. After the flake input is
added, `nix develop` puts neither `node` nor `npm` on the path.

The polyfill subjects read the observations of the Wasmtime run. A test changes
one result in the observations of a passing scenario. The browser subject and
the native subject each record `mismatch`.

A Zena compile failure is a recorded outcome. A test scenario whose program does
not compile records `compile` for all three subjects. The build of the scenario
succeeds, and the record keeps Zena's error output. A failed `wac` composition
records `compose` in the same way.

A change of stage fails the gate in either direction. A test gives the runner a
record in which one subject's stage is one step later than the real run. The run
fails. A second test gives a record in which the stage is one step earlier. The
run fails. A third test gives a record with a missing line and a line for a
scenario that does not exist. The run fails for each line.

The record names its pin. A test gives the runner a record whose header names a
different revision from the flake lock. The run fails with a message that names
both revisions, even when every stage matches.

The record regenerates after a move of the pin. After `nix flake update zena`,
`tests zena regenerate` writes a new record. Its header names the new revision.
Its lines hold the stages of all three subjects, the browser included. With
`--dry-run`, the command prints the difference and changes no file.

The Wasmtime run classifies each failure. A scenario whose program fails the
expectations file under Wasmtime records a Wasmtime stage before `pass`. The
browser and native subjects still run, and the runner judges them against the
expectations file. For a scenario that passes under Wasmtime, the runner judges
the polyfill subjects against the results and output of the Wasmtime run.

Each predicted failure is confirmed or refuted. After the first run, the stages
and the error text of the named scenarios show whether each prediction holds, in
the browser and natively:

- A tag export stops the component at `instantiate`, with scenario 4 as the
  evidence.
- A global with a GC reference type stops the component at `instantiate` in the
  browser, with scenario 3 as the evidence.
- A string export carries the tag and its global, with scenario 2 as the
  evidence.
- A GC type never reaches the conversion of export types, with scenarios 2 and 3
  as the evidence.

An uncaught exception follows the Wasmtime run. Scenario 4 makes its uncaught
call and then a second call on every subject. The browser and native subjects
pass only when each call fails or succeeds as it did in the Wasmtime run.

The test host functions supply the imports. Scenario 5 prints through the p3
`wasi:cli/stdout` stream write and writes one error line through
`wasi:cli/stderr`. Its output lines are the same under the Wasmtime run and the
polyfill subjects, and its error line never enters them. Scenario 6 sleeps on
the p3 timer in the browser and natively. A test calls a method that the
functions do not implement, and the call returns an error.

Integration scenarios link at run time and by composition. Scenarios 8 to 13 run
under all three subjects. Scenarios 10 to 13 build the Rust partner from its
locked manifest at test time. The partner's bytes do not change when the pin
moves.

Typed calls cover scalars and strings. Scenario 1 calls its scalar export with
`TypedFunc`, and scenario 2 calls its string export with `TypedFunc`. The typed
call and the untyped call give the same results on every subject.

The report leads with the browser. `tests zena` prints each scenario with one
line per subject, in the order `Browser`, `Native`, `Wasmtime`, the reason
beside each stage that comes before `pass`, a footer of pass counts, and the
pin. `tests all` runs the lane and reports its time.

## References

- [PDD001], the development environment and the menu.
- [PDD004], the test macros and their cross-target attribute.
- [PDD015], component composition.
- [PDD016], the conformance suite and its real-guest fixtures.
- [PDD023], the poisoned store.
- [Zena], the language and its repository.
- [Zena component emission], the design of Zena's component target.
- [Zena exception tag], where Zena exports its tag and defines its payload
  global.
- [Zena flake], the Nix package of the toolchain.
- [Zena development], the bootstrap and the build.
- [Zena CI], Zena's public continuous integration.
- [browser backend tags] and [browser backend refs], the refusals of the browser
  backend.
- [Wasmtime backend tags], the refusal of the Wasmtime backend.
- [wasmtime-wasi versions], the published versions of `wasmtime-wasi`.
- [`wac`], the composition tool.

[PDD001]: ./PDD001%20Development%20Environment.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD015]: ./PDD015%20Component%20Composition.md
[PDD016]: ./PDD016%20Conformance%20Suite.md
[PDD023]:
  ./PDD023%20Cancellation,%20Error%20Contexts,%20and%20the%20Poisoned%20Store.md
[Zena]: https://zena-lang.dev/
[Zena component emission]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/docs/design/component-emission.md
[Zena console]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/stdlib/zena/console/component.zena#L25-L29
[Zena stdlib manifest]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/stdlib/stdlib-manifest.json#L19-L27
[Zena timers]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/stdlib/zena/time/p3.zena#L29-L41
[Zena exception tag]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/zena-compiler/zena/lib/codegen/wasm-module.zena#L1680-L1699
[Zena flake]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/flake.nix
[Zena development]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/DEVELOPMENT.md
[Zena CI]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/.github/workflows/test.yml
[browser backend tags]: ../../rust/vendor/js_wasm_runtime_layer/src/module.rs
[browser backend refs]: ../../rust/vendor/js_wasm_runtime_layer/src/module.rs
[Wasmtime backend tags]: ../../rust/vendor/wasmtime_runtime_layer/src/lib.rs
[wasmtime-wasi versions]: https://crates.io/crates/wasmtime-wasi/versions
[`wac`]: https://github.com/bytecodealliance/wac

# Wasm Component Model Polyfill

A Rust library that runs Wasm Component Model components on top of
`wasm_runtime_layer`, natively and in the browser. Read
`project/design/README.md` for the design corpus and `project/kanban/README.md`
for the board.

## Environment

- Enter the shell with `nix develop`. It prints the menu. Run `menu` to print it
  again.
- Every build and test goes through a menu command, never through bare `cargo`:
  `build <profile>`, `tests native|web <profile>`, `tests all`,
  `tests conformance`, `lint`. Each one builds a Nix derivation.
- Outside the shell, prefix a command with `nix develop -c`, for example
  `nix develop -c tests native debug`.
- Nix sees only tracked files. Commit (or at least snapshot with `jj`) before
  `lint` or a `tests` command, or the run measures a stale tree.
- On Linux the flake supplies the WebDriver configuration for the browser tests.
  No manual Chrome setup is needed.

## Design corpus

- PDDs live under `project/design/`. The README there states the six-section
  template and the writing rules. Write every PDD in plain English per those
  rules.
- Format documents with `markdown format`. The `markdown` flake check fails on
  an unformatted document.
- A PDD describes its own scope only. Do not name or anticipate a later PDD.
- Do not link PDDs from Rust doc comments. Inline the design context instead.

## Code conventions

- Test names are BDD style: `it_<verb_phrase>`.
- Runtime code returns structured `Error` variants. `todo!()` is only for a
  branch that is unreachable today and planned; a public boundary returns
  `Error::Unsupported`.
- One public type per module. No `pub(crate)`; control the API from `lib.rs`
  re-exports.
- Every crate inherits dependencies from `[workspace.dependencies]`.

## Version control

- The repository is a colocated `jj` repo. Use `jj`, not `git`.
- Commit messages follow Conventional Commits with a Sentence-case summary and
  no trailers. Do not reference PDD numbers in commit messages.

## Board

- `project status --available` lists the grabbable cards.
  `project status set <id> in-progress` claims one; take it to `needs-review`
  when done.
- Only the owner moves a card to `accepted`.
- Re-snapshot the board after a CLI mutation so the `project-lint` check sees
  it.

---
id: ec3a3d
title: Drive the web smoke page headlessly so smoke web is a check
type: chore
blocked_by: []
labels: [PDD001, wave-4]
created: 2026-09-15T14:44:21Z
disposition: accepted
disposition_at: 2026-09-16T03:57:12Z
---

## What to build
`smoke native` builds the host binary and exits non-zero on a failed step, so it is a check. `smoke web` builds the page and serves it until Ctrl-C; nothing reads its report, and the page is not in the flake's checks. `tests web debug` covers the shared Rust code, but not the page's own entry point (the `spawn_local` in `main` and the `report` bridge into the page). A person has to watch a browser to know the page still works.

Make the page a check. Add a flake check (and a `smoke web` mode, or a sibling subcommand, that runs it) which builds `.#smoke-web`, serves it on a loopback port, loads it in the flake's headless Chromium with a virtual-time budget, reads the `#out` element, and fails unless the last line is `smoke: N passed, 0 failed, 0 skipped` with the same N the native run reports. Keep the serving mode for a person who wants to open the page. The check must run inside the Nix sandbox the way the browser tests do (use the same WebDriver or headless-Chromium plumbing the web test lane uses on Linux and Darwin).

## Acceptance criteria
- [x] `nix flake check` fails when a smoke step fails in the browser, and passes on the current tree.
- [x] `smoke web` (or its check mode) prints the page's report and exits non-zero on a failure, like `smoke native`.
- [x] The serving mode still works for a person who wants to open the page.
- [ ] The check runs on Linux and Darwin through the flake's existing browser plumbing.

## Review notes
Done (2026-09-15) as the `smoke-web` flake check plus the `smoke check` menu subcommand. `rust/wcmp-smoke/web/check.py` serves the built page on a loopback port inside the build sandbox, opens it through chromedriver with the flake's `webdriver.json` capabilities (the browser tests' plumbing), polls the `#out` element until its last line is the `smoke:` summary, and fails unless that line equals the native binary's and reports no failure and no skip. The report lands in `$out/report.txt`, which `smoke check` prints; `smoke web` still serves the page. Verified on Linux: the check passes on this tree (six steps), a mismatched summary makes the script exit non-zero, and the page finishes in a few seconds. Darwin is not verified here: the check uses the same `chrome`, `chromedriver`, and `webdriver.json` the web test lane uses on Darwin, so it needs one run there before the last box is ticked.

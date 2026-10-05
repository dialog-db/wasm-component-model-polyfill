# Demo Lane

The demo lane drives the Zena TodoMVC demo of `rust/wcmp-demo` in headless
Chrome. It serves the built demo on a loopback port, starts ChromeDriver, and
runs each test in a browser session of its own, with a fresh profile. A test
reaches into shadow roots with scripts. It defines the elements and routes of
`fixtures/` through `window.demo`, the hooks the page exposes for the lane.

## Running It

- `tests demo` runs every test. `tests all` runs it too.
- `tests demo -j <n>` runs `n` sessions at once. The default is 2.
- `tests demo <filter>...` runs the tests whose names contain a filter.
- `tests demo --report <file>` writes the report to a file.
- `demo serve` serves the demo for a person, on port 8766 unless a port follows.

Each session compiles in two contexts, and under the memory of several sessions
Chrome's renderer can crash. A test that fails because its tab crashed runs once
more, and the report marks it "after a tab crash". Any other failure fails the
test at once.

A person can look into a running demo. With `WCMP_DEMO_LANE_EVAL` set to a
script, the lane opens the demo, at the path `WCMP_DEMO_LANE_PATH` names (`/` by
default), waits `WCMP_DEMO_LANE_WAIT` seconds (10 by default), runs the script
with the lane's helpers in scope, and prints its answer and the page's console.
With `WCMP_DEMO_LANE_CPU_PROFILE` set to a file, it writes a V8 CPU profile of
the page while the script runs, which Chrome's performance panel opens.
`WCMP_DEMO_LANE_CDP` names CDP commands to send before the script, separated by
semicolons, each a method and optionally its parameters as JSON, such as
`Debugger.enable`, which is what an open DevTools does. `WCMP_DEMO_LANE_HEADED`
opens a browser window instead of a headless browser, for the lane and for
inspect mode. `WCMP_DEMO_LANE_SCREENSHOT` names a PNG file to write a screenshot
of the window to once the script has run.

## Report

The last full run, on Chromium 154.0.8037.57, on 2026-10-02:

| Group     | Tests | Passes |
| --------- | ----- | ------ |
| Platform  | 15    | 15     |
| Elements  | 16    | 16     |
| Routes    | 6     | 6      |
| TodoMVC   | 11    | 11     |
| Shelf     | 20    | 20     |
| **Total** | 68    | 68     |

## Safari

The lane does not run Safari. A person opens the demo in Epiphany, which stands
in for Safari, and completes the TodoMVC behaviors by hand: add, toggle, toggle
all, delete, edit with Enter, blur, and Escape, the filters, "Clear completed",
and a reload. The person records the result here.

| Date | Epiphany | WebKit | Result      |
| ---- | -------- | ------ | ----------- |
| —    | —        | —      | Not yet run |

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The smoke page's own script. It lives in a file rather than inline in
// `index.html` because the page's content-security policy is
// `script-src 'self' 'wasm-unsafe-eval'`, which admits no inline script.
// That is also why Trunk's own loader is switched off in `Trunk.toml`:
// this module instantiates the wasm itself.
//
// Before it starts the run it asks the browser whether that policy is
// actually in force, by trying the one thing the policy forbids and the
// polyfill must not need: building a function from source text. A
// browser enforcing the policy throws `EvalError`. The answer goes on
// `document.documentElement.dataset.csp`, where `check.sh` reads it, so
// a page that quietly lost its policy fails the check instead of
// passing it for the wrong reason.
//
// `buildsAFunctionFromSource` is the same probe the browser tests of
// `rust/wcmp/tests/baseline_content_security_policy.rs` run before and
// after they install the policy, written the same way on purpose: both build a function from source
// and call it, and read a throw as an enforced policy. Those two are
// the only places the repository asks the browser that question, and an
// answer that differed between them would make one of the two artifacts
// measure something else. Change them together.
//
// The Rust side reports through `window.smoke`: `begin` with the number
// of stories, `chapter` when the chapter changes, `step` after every
// story, and `finish` with the summary line. Each call adds to the page
// and to the plain transcript, which is the text `check.sh` reads and
// which ends in the same summary line `tests smoke native` prints.

import init from "./wcmp-smoke.js";

function buildsAFunctionFromSource() {
  try {
    return new Function("return 1")() === 1;
  } catch {
    return false;
  }
}

const byId = (id) => document.getElementById(id);
const chapters = byId("chapters");
const report = byId("report");
const status = byId("status");
const bar = byId("bar");

const enforced = !buildsAFunctionFromSource();
document.documentElement.dataset.csp = enforced ? "enforced" : "unenforced";
byId("csp").textContent = enforced
  ? "script-src 'self' 'wasm-unsafe-eval', enforced"
  : "not enforced by this browser";
byId("csp").dataset.state = enforced ? "ok" : "warn";
byId("browser").textContent = navigator.userAgent;

let total = 0;
let done = 0;
let stories = null;

function line(text) {
  report.textContent += text + "\n";
  console.log(text);
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

// The stories are written for the transcript too, so a span between
// backticks is code; on the page it becomes a `<code>` element.
function prose(tag, className, text) {
  const node = element(tag, className);
  text.split("`").forEach((piece, index) => {
    node.append(index % 2 ? element("code", null, piece) : piece);
  });
  return node;
}

window.smoke = {
  begin(count) {
    total = count;
    status.textContent = `running, 0 of ${total}`;
  },

  chapter(name, text) {
    line(text);
    const section = element("section", "chapter");
    section.append(element("h2", null, name));
    stories = element("div", "stories");
    section.append(stories);
    chapters.append(section);
  },

  step(chapter, title, goal, label, detail, elapsed, text) {
    line(text);
    done += 1;
    bar.style.width = `${(100 * done) / total}%`;
    status.textContent = `running, ${done} of ${total}`;

    const story = element("article", "story");
    story.dataset.outcome =
      label === "ok" ? "ok" : label === "FAIL" ? "fail" : "skip";
    const body = element("div", "body");
    body.append(
      prose("h3", null, title),
      prose("p", "goal", goal),
      element("p", "evidence", detail),
    );
    story.append(
      element("span", "mark", label),
      body,
      element("span", "elapsed", elapsed),
    );
    stories.append(story);
  },

  finish(text, passed, failed, skipped) {
    line(text);
    bar.style.width = "100%";
    document.documentElement.dataset.state = failed ? "failed" : "passed";
    status.dataset.state = failed ? "fail" : "ok";
    status.textContent = `${passed} passed, ${failed} failed, ${skipped} skipped`;
  },
};

try {
  await init();
} catch (error) {
  // The module never ran, so no summary line is coming; say so in the
  // transcript's last line, which is what the check compares.
  status.dataset.state = "fail";
  status.textContent = `failed to start: ${error}`;
  line(`smoke: failed to start: ${error}`);
}

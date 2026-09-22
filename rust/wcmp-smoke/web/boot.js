// The smoke page's own script. It lives in a file rather than inline in
// `index.html` because the page's content-security policy is
// `script-src 'self' 'wasm-unsafe-eval'`, which admits no inline script.
//
// Before it starts the run it asks the browser whether that policy is
// actually in force, by trying the one thing the policy forbids and the
// polyfill must not need: building a function from source text. A
// browser enforcing the policy throws `EvalError`. The answer goes on
// `document.documentElement.dataset.csp`, where `check.py` reads it, so
// a page that quietly lost its policy fails the check instead of
// passing it for the wrong reason.
//
// `buildsAFunctionFromSource` is the same probe the browser test
// `it_runs_a_prepared_call_under_a_policy_without_unsafe_eval` runs
// (`rust/wasm-component-model-polyfill/tests/baseline_prepared_call.rs`),
// written the same way on purpose: both build a function from source
// and call it, and read a throw as an enforced policy. Those two are
// the only places the repository asks the browser that question, and an
// answer that differed between them would make one of the two artifacts
// measure something else. Change them together.

import init from "./wcmp-smoke.js";

function buildsAFunctionFromSource() {
  try {
    return new Function("return 1")() === 1;
  } catch {
    return false;
  }
}

const out = document.getElementById("out");
out.textContent = "";
window.report = (line) => {
  out.textContent += line + "\n";
  console.log(line);
};

document.documentElement.dataset.csp = buildsAFunctionFromSource()
  ? "unenforced"
  : "enforced";

await init();

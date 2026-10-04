// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The page, the service worker, the lane's own tools, and the compiler
//! in each context.

use serde_json::Value;

use super::{check, js, pause, route_status};
use crate::browser::Browser;

pub fn it_loads_the_page_and_the_service_worker_controls_it(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let controlled = browser.eval("return navigator.serviceWorker.controller !== null;")?;
    check(controlled == true, || {
        "no service worker controls the page".to_string()
    })
}

pub fn it_shows_a_spinner_until_the_list_draws_and_then_reveals_it(
    browser: &Browser,
) -> Result<(), String> {
    browser.goto("/")?;
    let early = browser.eval(
        "const app = document.querySelector('todo-app');
         return {
           spinner: app !== null && app.querySelector('.loading .spinner') !== null,
           defined: customElements.get('todo-app') !== undefined,
           text: [...document.body.querySelectorAll('p, footer')].length,
         };",
    )?;
    check(
        early["spinner"] == true && early["defined"] == false && early["text"] == 0,
        || format!("before the elements were defined the page showed {early}"),
    )?;
    browser.wait_ready()?;
    let late = browser.eval(
        "const root = app().shadowRoot;
         await until(() => root.querySelector('.frame[data-phase=\"shown\"]'), 'the reveal', 10000);
         const frame = root.querySelector('.frame');
         const content = root.querySelector('.content');
         return {
           spinner: root.querySelector('.spinner, slot') !== null,
           style: frame.getAttribute('style'),
           fits: Math.abs(frame.getBoundingClientRect().height
                          - content.getBoundingClientRect().height) < 1,
           opacity: getComputedStyle(content).opacity,
           hint: root.querySelector('.info').textContent,
         };",
    )?;
    check(
        late["spinner"] == false
            && late["style"].is_null()
            && late["fits"] == true
            && late["opacity"] == "1"
            && late["hint"].as_str().is_some_and(|hint| {
                hint.starts_with("Double-click a todo to edit it. Every part of this page")
            }),
        || format!("after the reveal the list showed {late}"),
    )
}

pub fn it_stops_the_service_worker_and_the_next_request_starts_it(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    browser.eval("await api('GET', '/api/todos'); return true;")?;
    let before = route_status(browser, "/api/todos")?;
    browser.stop_service_workers()?;
    // The stop takes a moment after the command returns.
    pause(500);
    let answer = browser.eval("return await api('GET', '/api/todos');")?;
    check(answer["status"] == 200, || {
        format!("after a stop the API answered {answer}")
    })?;
    // A new worker compiles the route again, later than the old one did,
    // and its count of compiles starts over from one.
    let after = route_status(browser, "/api/todos")?;
    let compiled_at = |status: &Value| status["compiledAt"].as_f64().unwrap_or(0.0);
    check(
        compiled_at(&after) > compiled_at(&before) && after["compiles"] == 1,
        || format!("the route's status was {before} before the stop and {after} after it"),
    )
}

pub fn it_reads_each_network_response_with_its_headers(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    browser.responses()?;
    browser.eval("await fetch('/style.css'); return true;")?;
    let responses = browser.responses_until(|responses| {
        responses
            .iter()
            .any(|response| response.url.ends_with("/style.css"))
    })?;
    let style = responses
        .iter()
        .find(|response| response.url.ends_with("/style.css"))
        .ok_or_else(|| format!("no response for style.css among {responses:?}"))?;
    check(
        style.status == 200 && style.header("content-type").is_some(),
        || format!("style.css answered {style:?}"),
    )
}

pub fn it_sets_the_color_scheme(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    browser.color_scheme("dark")?;
    let dark = browser.eval("return matchMedia('(prefers-color-scheme: dark)').matches;")?;
    browser.color_scheme("light")?;
    let light = browser.eval("return matchMedia('(prefers-color-scheme: light)').matches;")?;
    check(dark == true && light == true, || {
        format!("dark {dark}, light {light}")
    })
}

pub fn it_clears_indexed_db(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let open = "const db = await new Promise((resolve, reject) => {
                  const request = indexedDB.open('lane-check', 1);
                  request.onupgradeneeded = () => request.result.createObjectStore('kv');
                  request.onsuccess = () => resolve(request.result);
                  request.onerror = () => reject(request.error);
                });";
    browser.eval(&format!(
        "{open}
         await new Promise((resolve) => {{
           const tx = db.transaction('kv', 'readwrite');
           tx.objectStore('kv').put('kept', 'key');
           tx.oncomplete = resolve;
         }});
         db.close();
         return true;"
    ))?;
    browser.clear_indexed_db()?;
    let kept = browser.eval(&format!(
        "{open}
         const value = await new Promise((resolve) => {{
           const request = db.transaction('kv').objectStore('kv').get('key');
           request.onsuccess = () => resolve(request.result ?? null);
         }});
         db.close();
         return value;"
    ))?;
    check(kept.is_null(), || format!("IndexedDB still holds {kept}"))
}

pub fn it_loads_a_static_file_through_the_service_worker_from_the_network(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    browser.responses()?;
    let answer = browser.eval(
        "const response = await fetch('/elements.js');
         return { status: response.status, route: response.headers.get('x-demo-route') };",
    )?;
    check(answer["status"] == 200 && answer["route"].is_null(), || {
        format!("elements.js answered {answer}")
    })?;
    let responses = browser.responses_until(|responses| {
        responses
            .iter()
            .any(|response| response.url.ends_with("/elements.js"))
    })?;
    let through = responses
        .iter()
        .find(|response| response.url.ends_with("/elements.js"))
        .ok_or("no response for elements.js")?;
    check(through.from_service_worker, || {
        format!("elements.js did not come through the service worker: {through:?}")
    })
}

/// The program the compile checks compile: `answer` returns 42.
const PROGRAM: &str = "export let answer = (): i32 => 6 * 7;\n";

pub fn it_compiles_and_runs_a_program_on_the_page(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = compile_check(browser, "compileCheck", PROGRAM, "{}")?;
    check(answer["result"] == 42, || {
        format!("the page's compile answered {answer}")
    })
}

pub fn it_compiles_and_runs_a_program_in_the_service_worker(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let answer = compile_check(browser, "workerCompileCheck", PROGRAM, "{}")?;
    check(answer["result"] == 42, || {
        format!("the worker's compile answered {answer}")
    })
}

pub fn it_reports_the_file_and_line_of_a_compile_error(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let source = "export let answer = (): i32 => {\n  return 'not a number';\n};\n";
    let answer = compile_check(browser, "compileCheck", source, "{}")?;
    let diagnostics = answer["diagnostics"].as_str().unwrap_or_default();
    check(diagnostics.contains("check.zena:2:"), || {
        format!("the diagnostics name no file and line: {answer}")
    })
}

pub fn it_compiles_a_program_that_imports_a_file_beside_it(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let source = "import { six } from './six.zena';\nexport let answer = (): i32 => six() * 7;\n";
    let files = r#"{ "six.zena": "export let six = (): i32 => 6;\n" }"#;
    let answer = compile_check(browser, "compileCheck", source, files)?;
    check(answer["result"] == 42, || {
        format!("the compile answered {answer}")
    })
}

/// Run the compile check `hook` of `window.demo` on `source`, with
/// `files` beside it, as `check.zena`, calling `answer`.
fn compile_check(
    browser: &Browser,
    hook: &str,
    source: &str,
    files: &str,
) -> Result<Value, String> {
    browser.eval(&format!(
        "return await window.demo.{hook}({{
           source: {}, entry: 'check.zena', export: 'answer', files: {files},
         }});",
        js(source)
    ))
}

/// A script that watches the page's performance measures from now on,
/// as the performance panel would, adds a todo, and answers the names
/// of the measures it saw once the todo shows, or after `wait`
/// milliseconds with no measure of an element's render.
fn measures_around_a_new_todo(wait: u32) -> String {
    format!(
        "const seen = [];
         new PerformanceObserver((list) => {{
           for (const entry of list.getEntries()) seen.push(entry.name);
         }}).observe({{ entryTypes: ['measure'] }});
         await addTodo('traced');
         await until(() => seen.some((name) => name.startsWith('element render')), 'the measures', {wait})
           .catch(() => null);
         return seen;"
    )
}

pub fn it_records_the_spans_of_the_page_as_performance_measures(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let seen = browser.eval(&measures_around_a_new_todo(20_000))?;
    let names: Vec<&str> = seen
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let missing: Vec<&str> = ["element render", "element op", "view diff", "Func::call"]
        .into_iter()
        .filter(|span| !names.iter().any(|name| name.starts_with(span)))
        .collect();
    check(missing.is_empty(), || {
        format!("no measure of {missing:?} among {names:?}")
    })?;
    let console = browser.console();
    check(
        console
            .iter()
            .any(|line| line.contains("tracing to the performance panel")),
        || format!("the console never said the page traces: {console:?}"),
    )
}

pub fn it_records_no_spans_when_the_trace_parameter_is_off(
    browser: &Browser,
) -> Result<(), String> {
    browser.goto("/?trace=off")?;
    browser.wait_ready()?;
    let seen = browser.eval(&measures_around_a_new_todo(3_000))?;
    check(seen.as_array().is_some_and(Vec::is_empty), || {
        format!("with tracing off the page measured {seen}")
    })
}

pub fn it_compiles_a_program_again_after_a_file_it_imports_changes(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    // The entry stays as it was, and the file beside it changes and
    // changes back: the shape of an element's glue and its author's
    // source, edited in the shelf and reset.
    let source = "import { six } from './six.zena';\nexport let answer = (): i32 => six() * 7;\n";
    let mut answers = Vec::new();
    for six in ["6", "5 + 2", "6"] {
        let files = format!(r#"{{ "six.zena": "export let six = (): i32 => {six};\n" }}"#);
        answers.push(compile_check(browser, "compileCheck", source, &files)?);
    }
    check(
        answers[0]["result"] == 42 && answers[1]["result"] == 49 && answers[2]["result"] == 42,
        || format!("the compiles answered {answers:?}"),
    )
}

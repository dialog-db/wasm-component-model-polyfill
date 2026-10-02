// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The drawer: every source with its timings and counts, and edits,
//! resets, and restarts of elements and routes.

use serde_json::Value;

use super::{check, pause, route_status};
use crate::browser::Browser;

/// Helpers for the drawer and the application, on top of the prelude.
const DRAWER: &str = r#"
const openDrawer = async () => {
  if (document.getElementById('drawer').hidden) {
    document.getElementById('drawer-toggle').click();
  }
  await settle(150);
};
const section = (kind, name) =>
  document.querySelector(`#drawer section[data-kind="${kind}"][data-name="${name}"]`);
const field = (kind, name, which) =>
  section(kind, name).querySelector(`[data-field="${which}"]`);
const shown = (kind, name, which) => {
  const node = field(kind, name, which);
  return node && !node.hidden ? node.textContent : '';
};
const source = (kind, name) => section(kind, name).querySelector('textarea').value;
const edit = (kind, name, change) => {
  const text = section(kind, name).querySelector('textarea');
  text.value = change(text.value);
  text.dispatchEvent(new Event('input', { bubbles: true }));
};
const act = (kind, name, action) =>
  section(kind, name).querySelector(`[data-action="${action}"]`).click();
const item = (index) => items()[index].shadowRoot;
const editKey = async (key) => {
  const db = await new Promise((resolve, reject) => {
    const request = indexedDB.open('wcmp-demo', 1);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  const value = await new Promise((resolve) => {
    const request = db.transaction('kv').objectStore('kv').get(key);
    request.onsuccess = () => resolve(request.result ?? null);
  });
  db.close();
  return value;
};
const later = (test, what) => until(test, what, 30000);
"#;

/// Run `body` with the drawer's helpers in scope.
fn run(browser: &Browser, body: &str) -> Result<Value, String> {
    browser.eval(&format!("{DRAWER}\n{body}"))
}

pub fn it_lists_every_source_with_its_timings(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await openDrawer();
         const elements = ['todo-input', 'todo-item', 'todo-footer', 'todo-app'].map((tag) => ({
           tag,
           source: source('element', tag).length > 0,
           compile: shown('element', tag, 'compile'),
           instantiate: shown('element', tag, 'instantiate'),
         }));
         await api('PATCH', '/api/todos/t1', { completed: true });
         const routes = await later(() => {
           const both = ['/api/todos', '/api/todos/:id'].map((pattern) => ({
             pattern,
             source: source('route', pattern).length > 0,
             compile: shown('route', pattern, 'compile'),
             instantiate: shown('route', pattern, 'instantiate'),
           }));
           return both.every((route) => /\\d+ ms/.test(route.compile)) && both;
         }, 'a compile time for each route');
         return { elements, routes, sections: document.querySelectorAll('#drawer section').length };",
    )?;
    let timed = |entries: &Value| {
        entries.as_array().is_some_and(|entries| {
            entries.iter().all(|entry| {
                entry["source"] == true
                    && entry["compile"]
                        .as_str()
                        .is_some_and(|text| text.ends_with(" ms"))
                    && entry["instantiate"]
                        .as_str()
                        .is_some_and(|text| text.ends_with(" ms"))
            })
        })
    };
    check(
        answer["sections"] == 6 && timed(&answer["elements"]) && timed(&answer["routes"]),
        || format!("the drawer showed {answer}"),
    )
}

pub fn it_shows_a_new_compile_of_a_route_after_the_worker_stops(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let first = run(
        browser,
        "await openDrawer();
         return await later(() => Number(section('route', '/api/todos').dataset.compiledAt),
                            'the first compile');",
    )?;
    browser.stop_service_workers()?;
    pause(500);
    let second = run(
        browser,
        &format!(
            "await api('GET', '/api/todos');
             return await later(() => {{
               const at = Number(section('route', '/api/todos').dataset.compiledAt);
               return at > {first} && at;
             }}, 'the compile of the new worker');"
        ),
    )?;
    check(second.as_f64() > first.as_f64(), || {
        format!("the drawer showed a compile at {first} and then at {second}")
    })
}

pub fn it_shows_one_instance_and_the_connected_elements(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "for (let index = 0; index < 50; index += 1) {
           await api('POST', '/api/todos', { title: 'todo ' + index });
         }
         location.hash = '#/active';
         await later(() => items().length === 50, 'fifty items');
         await openDrawer();
         const fifty = await later(() => {
           const text = shown('element', 'todo-item', 'counts');
           return text.startsWith('50 connected') && text;
         }, 'fifty connected');
         const listed = (await api('GET', '/api/todos')).body.todos;
         for (const todo of listed.slice(10)) {
           await api('DELETE', '/api/todos/' + todo.id);
         }
         location.hash = '#/';
         await later(() => items().length === 10, 'ten items');
         const ten = await later(() => {
           const text = shown('element', 'todo-item', 'counts');
           return text.startsWith('10 connected') && text;
         }, 'ten connected');
         return { fifty, ten };",
    )?;
    check(
        answer["fifty"] == "50 connected · 1 instance"
            && answer["ten"] == "10 connected · 1 instance",
        || format!("the drawer showed {answer}"),
    )
}

pub fn it_applies_an_element_edit_in_place(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('one');
         await addTodo('two');
         await openDrawer();
         edit('element', 'todo-item', (text) => text.replace(\"['×']\", \"['Delete']\"));
         act('element', 'todo-item', 'save');
         await later(() => items().length === 2 && items().every((element) =>
           element.shadowRoot.querySelector('.destroy')?.textContent === 'Delete'),
           'the edited button on every item');
         return titles();",
    )?;
    check(answer == serde_json::json!(["one", "two"]), || {
        format!("after the edit the list is {answer}")
    })
}

pub fn it_keeps_the_last_good_element_after_a_failed_edit(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('one');
         await openDrawer();
         edit('element', 'todo-item', (text) => text.replace('render(): View {', 'render(): View {\\n    let broken = ;'));
         act('element', 'todo-item', 'save');
         const diagnostics = await later(() => shown('element', 'todo-item', 'diagnostics'), 'the diagnostics');
         item(0).querySelector('.toggle').click();
         await later(() => items()[0].getAttribute('completed') === 'true', 'the old element to work');
         return { diagnostics };",
    )?;
    check(
        answer["diagnostics"]
            .as_str()
            .is_some_and(|text| text.contains("todo-item.zena:")),
        || format!("the drawer showed {answer}"),
    )
}

pub fn it_falls_back_to_the_shipped_element_when_an_edit_fails_at_boot(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    run(
        browser,
        "await addTodo('one');
         await openDrawer();
         edit('element', 'todo-item', (text) => text + '\\nthis is not zena\\n');
         act('element', 'todo-item', 'save');
         await later(() => shown('element', 'todo-item', 'diagnostics'), 'the diagnostics');
         return true;",
    )?;
    browser.reload()?;
    browser.wait_ready()?;
    let answer = run(
        browser,
        "await later(() => items().length === 1 && item(0).querySelector('label'), 'the shipped item');
         await openDrawer();
         return {
           label: item(0).querySelector('label').textContent,
           edit: source('element', 'todo-item').includes('this is not zena'),
           diagnostics: shown('element', 'todo-item', 'diagnostics'),
         };",
    )?;
    check(
        answer["label"] == "one" && answer["edit"] == true && answer["diagnostics"] != "",
        || format!("after the reload the drawer showed {answer}"),
    )
}

pub fn it_resets_an_element_to_the_shipped_source(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('one');
         await openDrawer();
         edit('element', 'todo-item', (text) => text.replace(\"['×']\", \"['Delete']\"));
         act('element', 'todo-item', 'save');
         await later(() => item(0).querySelector('.destroy')?.textContent === 'Delete', 'the edit');
         act('element', 'todo-item', 'reset');
         await later(() => item(0).querySelector('.destroy')?.textContent === '×', 'the reset');
         return { kept: await editKey('edit:element:todo-item') };",
    )?;
    check(answer["kept"].is_null(), || {
        format!("IndexedDB still holds the edit: {answer}")
    })
}

pub fn it_restarts_an_element_after_a_trap(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('one');
         await addTodo('two');
         await openDrawer();
         const starts = () => window.demo.elements().find((status) => status.tag === 'todo-item').starts;
         const before = starts();
         edit('element', 'todo-item', (text) =>
           text.replace(\"this.emit('todo-destroy', id);\", \"throw new Error('edited to trap');\"));
         act('element', 'todo-item', 'save');
         await later(() => starts() > before && item(0).querySelector('.destroy'), 'the edit');
         await settle(300);
         item(0).querySelector('.destroy').click();
         await later(() => items().every((element) =>
           element.shadowRoot.querySelector('.error-card')), 'an error card on every item');
         const restart = await later(() => {
           const button = section('element', 'todo-item').querySelector('[data-action=\"restart\"]');
           return button && !button.hidden && button;
         }, 'the restart control');
         restart.click();
         await later(() => items().every((element) =>
           element.shadowRoot.querySelector('label')), 'the items after the restart');
         const cards = items().filter((element) =>
           element.shadowRoot.querySelector('.error-card')).length;
         return { titles: titles(), cards };",
    )?;
    check(
        answer["titles"] == serde_json::json!(["one", "two"]) && answer["cards"] == 0,
        || format!("after the restart the list is {answer}"),
    )
}

/// The route edit the drawer tests make: `PATCH` refuses titles shorter
/// than three letters.
const SHORT_TITLES: &str = "(text) => text
  .replace(\"import { Option, Ok, Err, some, none } from 'zena:core';\",
           \"import { Option, Some, Ok, Err, some, none } from 'zena:core';\")
  .replace('    let updated = await update(id, title, completed);',
           '    if (title is Some<String> && (title as Some<String>).value.length < 3) {\\n      return status(422);\\n    }\\n    let updated = await update(id, title, completed);')";

pub fn it_applies_a_route_edit_in_the_service_worker(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let before = run(
        browser,
        &format!(
            "const added = await api('POST', '/api/todos', {{ title: 'long title' }});
             await openDrawer();
             const before = Number(section('route', '/api/todos/:id').dataset.compiledAt ?? 0);
             edit('route', '/api/todos/:id', {SHORT_TITLES});
             act('route', '/api/todos/:id', 'save');
             await later(() => Number(section('route', '/api/todos/:id').dataset.compiledAt ?? 0) > before
                         && !shown('route', '/api/todos/:id', 'diagnostics'), 'the route compile');
             const refused = await api('PATCH', '/api/todos/' + added.body.id, {{ title: 'ab' }});
             return {{ id: added.body.id, refused: refused.status }};"
        ),
    )?;
    check(before["refused"] == 422, || {
        format!("the edited route answered {before}")
    })?;
    let compiled = route_status(browser, "/api/todos/:id")?;
    browser.stop_service_workers()?;
    pause(500);
    let after = browser.eval(&format!(
        "return (await api('PATCH', '/api/todos/{}', {{ title: 'ab' }})).status;",
        before["id"].as_str().unwrap_or_default()
    ))?;
    // The new worker compiled the route from the edit it read back.
    let recompiled = route_status(browser, "/api/todos/:id")?;
    let compiled_at = |status: &Value| status["compiledAt"].as_f64().unwrap_or(0.0);
    check(
        after == 422 && compiled_at(&recompiled) > compiled_at(&compiled),
        || format!("after the worker stopped the route answered {after}, with {recompiled}"),
    )
}

pub fn it_answers_500_for_a_saved_route_that_traps(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await openDrawer();
         edit('route', '/api/todos', (text) => text.replace(
           \"if (request.method == 'GET') {\",
           \"if (request.method == 'GET') {\\n    if (request.path != '') {\\n      throw new Error('edited to trap');\\n    }\"));
         act('route', '/api/todos', 'save');
         await later(() => /compiled 2 times/.test(shown('route', '/api/todos', 'counts')), 'the route compile');
         const trapped = await api('GET', '/api/todos');
         const shownTrap = await later(() => shown('route', '/api/todos', 'trapped'), 'the trap in the drawer');
         await api('GET', '/api/todos');
         const counts = await later(() => {
           const text = shown('route', '/api/todos', 'counts');
           return /compiled 3 times/.test(text) && text;
         }, 'the compile after the trap');
         return { status: trapped.status, shownTrap, counts };",
    )?;
    check(
        answer["status"] == 500
            && answer["shownTrap"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
        || format!("the trapping route answered {answer}"),
    )
}

pub fn it_keeps_the_old_route_after_a_failed_edit(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await openDrawer();
         edit('route', '/api/todos', (text) => text + '\\nthis is not zena\\n');
         act('route', '/api/todos', 'save');
         const diagnostics = await later(() => shown('route', '/api/todos', 'diagnostics'), 'the diagnostics');
         const still = await api('GET', '/api/todos');
         return { diagnostics, status: still.status };",
    )?;
    check(
        answer["status"] == 200
            && answer["diagnostics"]
                .as_str()
                .is_some_and(|text| text.contains("api-todos.zena:")),
        || format!("after a failed edit the route answered {answer}"),
    )
}

pub fn it_resets_a_route_to_the_shipped_source(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        &format!(
            "const added = await api('POST', '/api/todos', {{ title: 'long title' }});
             await openDrawer();
             edit('route', '/api/todos/:id', {SHORT_TITLES});
             act('route', '/api/todos/:id', 'save');
             await later(async () => (await api('PATCH', '/api/todos/' + added.body.id, {{ title: 'ab' }})).status === 422,
                         'the edit');
             act('route', '/api/todos/:id', 'reset');
             await later(async () => (await api('PATCH', '/api/todos/' + added.body.id, {{ title: 'ab' }})).status === 200,
                         'the reset');
             return {{ kept: await editKey('edit:route:/api/todos/:id') }};"
        ),
    )?;
    check(answer["kept"].is_null(), || {
        format!("IndexedDB still holds the edit: {answer}")
    })
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The shelf: every source with its timings and counts, and edits,
//! resets, and restarts of elements and routes.

use serde_json::Value;

use super::{check, pause, route_status};
use crate::browser::Browser;

/// Helpers for the shelf and the application, on top of the prelude.
const SHELF: &str = r#"
const openShelf = async () => {
  if (document.getElementById('shelf-panel').hidden) {
    document.getElementById('shelf-toggle').click();
  }
  await settle(150);
};
const section = (kind, name) =>
  document.querySelector(`#shelf section[data-kind="${kind}"][data-name="${name}"]`);
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
const openTab = async (kind, name) => {
  await openShelf();
  document.querySelector(`#shelf .shelf-tab[data-tab="${kind}:${name}"]`).click();
  await settle(200);
  return section(kind, name);
};
// The point of the viewport at the middle of the character at `offset`
// of the editor `text`, whose font is monospaced and whose lines do not
// wrap.
const pointAt = (text, offset) => {
  const style = getComputedStyle(text);
  const probe = document.createElement('span');
  probe.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;font:' + style.font;
  probe.textContent = 'M'.repeat(64);
  document.body.append(probe);
  const width = probe.getBoundingClientRect().width / 64;
  probe.remove();
  const before = text.value.slice(0, offset);
  const row = before.split('\n').length - 1;
  const column = offset - before.lastIndexOf('\n') - 1;
  const rect = text.getBoundingClientRect();
  return {
    x: rect.left + parseFloat(style.paddingLeft) + (column + 0.5) * width - text.scrollLeft,
    y: rect.top + parseFloat(style.paddingTop) + (row + 0.5) * parseFloat(style.lineHeight) - text.scrollTop,
  };
};
// Put the caret of `text` at `offset`, in view, as a click would.
const caretAt = async (text, offset) => {
  text.focus();
  text.setSelectionRange(offset, offset);
  text.blur();
  text.focus();
  await settle(300);
};
// Change the text of `text` as typing `typed` at the end of the change
// would, with the caret after it.
const typeInto = (text, value, caret, typed) => {
  text.value = value;
  text.setSelectionRange(caret, caret);
  text.dispatchEvent(new InputEvent('input', { bubbles: true, data: typed }));
};
"#;

/// Run `body` with the shelf's helpers in scope.
fn run(browser: &Browser, body: &str) -> Result<Value, String> {
    browser.eval(&format!("{SHELF}\n{body}"))
}

pub fn it_lists_every_source_with_its_timings(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await openShelf();
         const elements = ['todo-input', 'todo-item', 'todo-footer', 'todo-app'].map((tag) => ({
           tag,
           source: source('element', tag).length > 0,
           compile: shown('element', tag, 'compile'),
           size: shown('element', tag, 'size'),
           bytes: field('element', tag, 'size').title,
           wasm: shown('element', tag, 'wasm'),
           instantiate: shown('element', tag, 'instantiate'),
         }));
         await api('PATCH', '/api/todos/t1', { completed: true });
         const routes = await later(() => {
           const both = ['/api/todos', '/api/todos/:id'].map((pattern) => ({
             pattern,
             source: source('route', pattern).length > 0,
             compile: shown('route', pattern, 'compile'),
             size: shown('route', pattern, 'size'),
             bytes: field('route', pattern, 'size').title,
             wasm: shown('route', pattern, 'wasm'),
             instantiate: shown('route', pattern, 'instantiate'),
           }));
           return both.every((route) => /\\d+ ms/.test(route.compile)) && both;
         }, 'a compile time for each route');
         return { elements, routes, sections: document.querySelectorAll('#shelf section').length };",
    )?;
    let timed = |entries: &Value| {
        entries.as_array().is_some_and(|entries| {
            entries.iter().all(|entry| {
                entry["source"] == true
                    && entry["compile"].as_str().is_some_and(|text| {
                        text.starts_with("Zena → Wasm ") && text.ends_with(" ms")
                    })
                    && entry["size"]
                        .as_str()
                        .is_some_and(|text| text.starts_with("component ") && text.ends_with(" kB"))
                    && entry["bytes"].as_str().is_some_and(|text| {
                        text.ends_with(" bytes")
                            && text
                                .trim_end_matches(" bytes")
                                .chars()
                                .all(|c| c.is_ascii_digit() || c == ',')
                    })
                    && entry["wasm"].as_str().is_some_and(|text| {
                        text.starts_with("Wasm compile ") && text.ends_with(" ms")
                    })
                    && entry["instantiate"]
                        .as_str()
                        .is_some_and(|text| text.ends_with(" ms"))
            })
        })
    };
    check(
        answer["sections"] == 6 && timed(&answer["elements"]) && timed(&answer["routes"]),
        || format!("the shelf showed {answer}"),
    )
}

pub fn it_shows_a_new_compile_of_a_route_after_the_worker_stops(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let first = run(
        browser,
        "await openShelf();
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
        format!("the shelf showed a compile at {first} and then at {second}")
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
         await openShelf();
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
        || format!("the shelf showed {answer}"),
    )
}

pub fn it_applies_an_element_edit_in_place(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('one');
         await addTodo('two');
         await openShelf();
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
         await openShelf();
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
        || format!("the shelf showed {answer}"),
    )
}

pub fn it_falls_back_to_the_shipped_element_when_an_edit_fails_at_boot(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    run(
        browser,
        "await addTodo('one');
         await openShelf();
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
         await openShelf();
         return {
           label: item(0).querySelector('label').textContent,
           edit: source('element', 'todo-item').includes('this is not zena'),
           diagnostics: shown('element', 'todo-item', 'diagnostics'),
         };",
    )?;
    check(
        answer["label"] == "one" && answer["edit"] == true && answer["diagnostics"] != "",
        || format!("after the reload the shelf showed {answer}"),
    )
}

pub fn it_resets_an_element_to_the_shipped_source(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('one');
         await openShelf();
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
         await openShelf();
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

/// The route edit the shelf tests make: `PATCH` refuses titles shorter
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
             await openShelf();
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
        "await openShelf();
         edit('route', '/api/todos', (text) => text.replace(
           \"if (request.method == 'GET') {\",
           \"if (request.method == 'GET') {\\n    if (request.path != '') {\\n      throw new Error('edited to trap');\\n    }\"));
         act('route', '/api/todos', 'save');
         await later(() => /compiled 2 times/.test(shown('route', '/api/todos', 'counts')), 'the route compile');
         const trapped = await api('GET', '/api/todos');
         const shownTrap = await later(() => shown('route', '/api/todos', 'trapped'), 'the trap in the shelf');
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
        "await openShelf();
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
             await openShelf();
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

pub fn it_shows_one_source_at_a_time_by_its_tab_and_keeps_the_choice(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    // What the browser shows, not only what the attributes say.
    let shown = "const visible = () => [...document.querySelectorAll('#shelf-panel section')]
                   .filter((section) => section.checkVisibility())
                   .map((section) => section.dataset.name);";
    let before = run(
        browser,
        &format!(
            "{shown}
             const collapsed = document.getElementById('shelf-panel').hidden;
             const tabs = document.querySelectorAll('#shelf .shelf-tab').length;
             document.querySelector('#shelf .shelf-tab[data-tab=\"route:/api/todos\"]').click();
             await settle(100);
             return {{ collapsed, tabs, open: !document.getElementById('shelf-panel').hidden,
                       visible: visible() }};"
        ),
    )?;
    check(
        before["collapsed"] == true
            && before["tabs"] == 6
            && before["open"] == true
            && before["visible"] == serde_json::json!(["/api/todos"]),
        || format!("the shelf showed {before}"),
    )?;
    browser.reload()?;
    browser.wait_ready()?;
    let after = run(
        browser,
        &format!(
            "{shown}
             const open = !document.getElementById('shelf-panel').hidden;
             const shownAfter = visible();
             document.getElementById('shelf-toggle').click();
             await settle(100);
             return {{ open, visible: shownAfter, closed: document.getElementById('shelf-panel').hidden }};"
        ),
    )?;
    check(
        after["open"] == true
            && after["visible"] == serde_json::json!(["/api/todos"])
            && after["closed"] == true,
        || format!("after a reload the shelf showed {after}"),
    )
}

pub fn it_marks_a_tab_with_an_edit_and_with_a_failure(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await openShelf();
         const tab = () => document.querySelector('#shelf .shelf-tab[data-tab=\"element:todo-footer\"]');
         const clean = tab().dataset.state;
         edit('element', 'todo-footer', (text) => text + '\\n');
         const edited = await later(() => tab().dataset.state === 'edited' && 'edited', 'the edited dot');
         edit('element', 'todo-footer', (text) => text.replace('render(): View {', 'render(): View {\\n    let broken = ;'));
         act('element', 'todo-footer', 'save');
         const failed = await later(() => tab().dataset.state === 'failed' && 'failed', 'the failed dot');
         return { clean, edited, failed };",
    )?;
    check(
        answer["clean"] == "clean" && answer["edited"] == "edited" && answer["failed"] == "failed",
        || format!("the tab's states were {answer}"),
    )
}

pub fn it_shows_no_problems_in_the_shipped_sources(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "const found = {};
         for (const tab of document.querySelectorAll('#shelf .shelf-tab')) {
           const [kind, name] = [tab.dataset.tab.slice(0, tab.dataset.tab.indexOf(':')),
                                 tab.dataset.tab.slice(tab.dataset.tab.indexOf(':') + 1)];
           const editor = await openTab(kind, name);
           // A check of a source with no problems leaves the list hidden,
           // so wait for the check itself: the marks layer repaints.
           await settle(2500);
           found[tab.dataset.tab] = [...editor.querySelector('.problems').children]
             .map((item) => item.textContent);
         }
         return found;",
    )?;
    let clean = answer.as_object().is_some_and(|found| {
        found.len() == 6
            && found
                .values()
                .all(|problems| problems == &serde_json::json!([]))
    });
    check(clean, || {
        format!("the shipped sources had problems: {answer}")
    })
}

pub fn it_marks_and_lists_the_problems_of_a_source_as_a_person_types(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "const editor = await openTab('element', 'todo-item');
         const text = editor.querySelector('textarea');
         const value = text.value.replace('var editing = false;',
                                          \"var editing = false;\\n  var broken: i32 = 'text';\");
         typeInto(text, value, value.indexOf(\"'text';\") + 7, ';');
         const problems = editor.querySelector('.problems');
         await later(() => !problems.hidden && problems.textContent, 'the problems');
         const marked = [...editor.querySelectorAll('.marks .error')].map((mark) => mark.textContent);
         problems.firstElementChild.click();
         const placed = text.selectionStart === value.indexOf('var broken');
         return { problems: problems.textContent, marked, placed };",
    )?;
    let problems = answer["problems"].as_str().unwrap_or_default();
    check(
        problems.contains("9:3")
            && problems.contains("Type mismatch")
            && answer["marked"] == serde_json::json!(["var broken: i32 = 'text';"])
            && answer["placed"] == true,
        || format!("the check showed {answer}"),
    )
}

pub fn it_shows_what_is_under_the_pointer(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "const editor = await openTab('element', 'todo-item');
         const text = editor.querySelector('textarea');
         const at = text.value.indexOf('extends Element') + 'extends '.length + 2;
         await caretAt(text, at);
         const point = pointAt(text, at);
         text.dispatchEvent(new MouseEvent('mousemove', { clientX: point.x, clientY: point.y, bubbles: true }));
         const tip = editor.querySelector('.tooltip');
         await later(() => !tip.hidden && tip.textContent, 'the hover');
         return tip.innerText;",
    )?;
    let tip = answer.as_str().unwrap_or_default();
    check(
        tip.contains("Element") && tip.contains("The base of an element's definition."),
        || format!("the hover showed {answer}"),
    )
}

pub fn it_completes_the_members_after_a_dot(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "const editor = await openTab('element', 'todo-item');
         const text = editor.querySelector('textarea');
         const anchor = 'render(): View {';
         // Zena's parser recovers from `this.` when a `;` follows it.
         const value = text.value.replace(anchor, anchor + '\\n    let probe = this.;');
         const caret = value.indexOf('let probe = this.') + 'let probe = this.'.length;
         await caretAt(text, caret);
         typeInto(text, value, caret, '.');
         const popup = editor.querySelector('.completions');
         await later(() => !popup.hidden && popup.children.length, 'the completions');
         const labels = [...popup.children].map((row) => row.dataset.label);
         const chosen = popup.children[0].dataset.label;
         text.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
         await settle(200);
         return { labels, chosen, line: text.value.split('\\n').find((line) => line.includes('let probe')) };",
    )?;
    let labels = answer["labels"].as_array().cloned().unwrap_or_default();
    let has = |label: &str| labels.iter().any(|known| known == label);
    let chosen = answer["chosen"].as_str().unwrap_or_default();
    check(
        has("attribute")
            && has("emit")
            && has("editing")
            && answer["line"].as_str() == Some(&format!("    let probe = this.{chosen};")),
        || format!("the completions were {answer}"),
    )
}

pub fn it_goes_to_a_definition_and_formats_the_source(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "const editor = await openTab('element', 'todo-item');
         const text = editor.querySelector('textarea');
         const use = text.value.indexOf('if (this.editing)') + 'if (this.'.length + 2;
         await caretAt(text, use);
         text.dispatchEvent(new KeyboardEvent('keydown', { key: 'F12', bubbles: true }));
         const declared = text.value.indexOf('var editing');
         await later(() => text.selectionStart === declared, 'the definition');
         typeInto(text, text.value.replace('var editing = false;', 'var   editing=false ;'), 0, ' ');
         editor.querySelector('[data-action=\"format\"]').click();
         await later(() => text.value.includes('var editing = false;'), 'the format');
         return true;",
    )?;
    check(answer == true, || {
        format!("the definition and the format answered {answer}")
    })
}

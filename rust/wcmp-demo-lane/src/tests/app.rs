// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The TodoMVC application: each behavior of the TodoMVC
//! specification, through the shadow roots, and its look.

use serde_json::Value;

use super::{check, pause};
use crate::browser::Browser;

/// Helpers for the application's elements, on top of the prelude.
const APP: &str = r#"
const item = (index) => items()[index].shadowRoot;
const footer = () => app().shadowRoot.querySelector('todo-footer')?.shadowRoot;
const count = () => footer()?.querySelector('.todo-count')?.textContent ?? '';
const toggle = (index) => item(index).querySelector('.toggle').click();
const done = () => items().map((element) => element.getAttribute('completed') === 'true');
const startEdit = async (index) => {
  item(index).querySelector('label').dispatchEvent(
    new MouseEvent('dblclick', { bubbles: true, composed: true }));
  return await until(() => item(index).querySelector('.edit'), 'the edit field');
};
const rendered = (test, what) => until(test, what, 20000);
"#;

/// Run `body` with the application's helpers in scope.
fn run(browser: &Browser, body: &str) -> Result<Value, String> {
    browser.eval(&format!("{APP}\n{body}"))
}

pub fn it_adds_toggles_toggles_all_and_deletes_todos(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('one');
         await addTodo('  two  ');
         await addTodo('three');
         const added = titles();
         const cleared = deep('todo-app', 'todo-input', '.new-todo').value;
         toggle(1);
         await rendered(() => done()[1], 'the toggle');
         const toggled = done();
         deep('todo-app', 'todo-input', '.toggle-all').click();
         await rendered(() => done().every((flag) => flag), 'toggle all on');
         deep('todo-app', 'todo-input', '.toggle-all').click();
         await rendered(() => done().every((flag) => !flag), 'toggle all off');
         item(0).querySelector('.destroy').click();
         await rendered(() => items().length === 2, 'the delete');
         return { added, cleared, toggled, after: titles() };",
    )?;
    check(
        answer["added"] == serde_json::json!(["one", "two", "three"])
            && answer["cleared"] == ""
            && answer["toggled"] == serde_json::json!([false, true, false])
            && answer["after"] == serde_json::json!(["two", "three"]),
        || format!("the list answered {answer}"),
    )
}

pub fn it_edits_a_todo_with_enter_blur_and_escape(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('first');
         let field = await startEdit(0);
         const shown = field.value;
         const focused = await until(() => item(0).activeElement === field, 'the focus', 5000)
           .catch(() => false);
         field.value = 'by enter';
         key(field, 'Enter');
         await rendered(() => titles()[0] === 'by enter', 'the edit by Enter');
         field = await startEdit(0);
         field.value = 'by blur';
         // Headless Chrome gives a field the focus only while its window
         // has it, so the test sends the blur a person's click away would.
         field.dispatchEvent(new FocusEvent('blur'));
         await rendered(() => titles()[0] === 'by blur', 'the edit by blur');
         field = await startEdit(0);
         field.value = 'not kept';
         key(field, 'Escape');
         await rendered(() => !item(0).querySelector('.edit'), 'the cancel');
         await settle(300);
         return { shown, focused, after: titles() };",
    )?;
    check(
        answer["shown"] == "first"
            && answer["focused"] == true
            && answer["after"] == serde_json::json!(["by blur"]),
        || format!("the edits answered {answer}"),
    )
}

pub fn it_deletes_a_todo_whose_title_an_edit_empties(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('keep');
         await addTodo('empty me');
         const field = await startEdit(1);
         field.value = '   ';
         key(field, 'Enter');
         await rendered(() => items().length === 1, 'the delete by an empty edit');
         return titles();",
    )?;
    check(answer == serde_json::json!(["keep"]), || {
        format!("the list is {answer}")
    })
}

pub fn it_counts_the_active_todos(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('a');
         const one = await rendered(() => count() === '1 item left' && count(), 'one left');
         await addTodo('b');
         const two = await rendered(() => count() === '2 items left' && count(), 'two left');
         toggle(0);
         const again = await rendered(() => count() === '1 item left' && count(), 'one left again');
         return [one, two, again];",
    )?;
    check(
        answer.as_array().is_some_and(|counts| counts.len() == 3),
        || format!("the counts were {answer}"),
    )
}

pub fn it_filters_by_the_url_hash(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('active one');
         await addTodo('done one');
         toggle(1);
         await rendered(() => done()[1], 'the toggle');
         location.hash = '#/active';
         await rendered(() => titles().join() === 'active one', 'the active filter');
         const selected = footer().querySelector('a.selected')?.textContent;
         location.hash = '#/completed';
         await rendered(() => titles().join() === 'done one', 'the completed filter');
         location.hash = '#/';
         await rendered(() => titles().length === 2, 'every todo');
         return { selected, filter: app().getAttribute('filter') };",
    )?;
    check(
        answer["selected"] == "Active" && answer["filter"] == "all",
        || format!("the filters answered {answer}"),
    )
}

pub fn it_clears_the_completed_todos_and_shows_the_button_only_for_them(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('stays');
         await addTodo('goes');
         const before = footer().querySelector('.clear-completed') !== null;
         toggle(1);
         const button = await rendered(() => footer().querySelector('.clear-completed'),
                                       'the clear button');
         button.click();
         await rendered(() => items().length === 1, 'the clear');
         await rendered(() => !footer().querySelector('.clear-completed'), 'the button to go');
         return { before, after: titles() };",
    )?;
    check(
        answer["before"] == false && answer["after"] == serde_json::json!(["stays"]),
        || format!("the clear answered {answer}"),
    )
}

pub fn it_keeps_the_list_after_a_reload(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    run(
        browser,
        "await addTodo('saved'); await addTodo('kept'); toggle(1);
         await rendered(() => done()[1], 'the toggle'); return true;",
    )?;
    browser.reload()?;
    browser.wait_ready()?;
    let answer = run(
        browser,
        "await rendered(() => items().length === 2, 'the list'); return { titles: titles(), done: done() };",
    )?;
    check(
        answer["titles"] == serde_json::json!(["saved", "kept"])
            && answer["done"] == serde_json::json!([false, true]),
        || format!("after a reload the list is {answer}"),
    )
}

pub fn it_keeps_the_list_after_the_worker_stops_and_the_page_reloads(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    run(browser, "await addTodo('survives'); return true;")?;
    browser.stop_service_workers()?;
    pause(500);
    browser.reload()?;
    browser.wait_ready()?;
    let answer = run(
        browser,
        "await rendered(() => items().length === 1, 'the list'); return titles();",
    )?;
    check(answer == serde_json::json!(["survives"]), || {
        format!("the list is {answer}")
    })
}

pub fn it_calls_the_routes_through_wasi_http_and_the_service_worker(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    browser.responses()?;
    run(browser, "await addTodo('watched'); return true;")?;
    let responses = browser.responses_until(|responses| {
        responses
            .iter()
            .filter(|response| response.url.contains("/api/"))
            .count()
            >= 2
    })?;
    let api: Vec<_> = responses
        .iter()
        .filter(|response| response.url.contains("/api/"))
        .collect();
    check(
        api.len() >= 2
            && api.iter().all(|response| {
                response.from_service_worker && response.header("x-demo-route").is_some()
            }),
        || format!("the API requests were {api:?}"),
    )
}

pub fn it_follows_the_color_scheme(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let read = "return {
                  background: getComputedStyle(document.body).backgroundColor,
                  primary: getComputedStyle(document.documentElement)
                    .getPropertyValue('--md-sys-color-primary').trim(),
                };";
    browser.color_scheme("light")?;
    let light = browser.eval(read)?;
    browser.color_scheme("dark")?;
    let dark = browser.eval(read)?;
    check(
        light["background"] != dark["background"] && light["primary"] != dark["primary"],
        || format!("light {light}, dark {dark}"),
    )
}

pub fn it_reads_the_color_tokens_in_every_element(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let answer = run(
        browser,
        "await addTodo('styled');
         const root = document.documentElement.style;
         root.setProperty('--md-sys-color-primary', 'rgb(250, 0, 0)');
         root.setProperty('--md-sys-color-on-surface', 'rgb(0, 250, 0)');
         root.setProperty('--md-sys-color-on-surface-variant', 'rgb(0, 0, 250)');
         await settle(50);
         const color = (node) => node && getComputedStyle(node).color;
         return {
           app: color(app().shadowRoot.querySelector('h1')),
           input: color(deep('todo-app', 'todo-input', '.new-todo')),
           item: color(item(0).querySelector('label')),
           footer: color(footer().querySelector('footer')),
         };",
    )?;
    check(
        answer["app"] == "rgb(250, 0, 0)"
            && answer["input"] == "rgb(0, 250, 0)"
            && answer["item"] == "rgb(0, 250, 0)"
            && answer["footer"] == "rgb(0, 0, 250)",
        || format!("the elements' colors are {answer}"),
    )
}

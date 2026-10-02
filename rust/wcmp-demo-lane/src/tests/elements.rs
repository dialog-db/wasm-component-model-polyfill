// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Elements: the lifecycle, one instance per tag, the diff, styles,
//! events, the two forms of a definition, and traps. Each test defines
//! the elements of `fixtures/` it needs through `window.demo`.

use super::{check, define, element_status, js};
use crate::browser::Browser;

const LABEL: &str = include_str!("../../fixtures/test-label.zena");
const LIST: &str = include_str!("../../fixtures/test-list.zena");
const COUNTER: &str = include_str!("../../fixtures/test-counter.zena");
const CHILD: &str = include_str!("../../fixtures/test-child.zena");
const PARENT: &str = include_str!("../../fixtures/test-parent.zena");
const FUNCTION: &str = include_str!("../../fixtures/test-function.zena");
const PLAIN: &str = include_str!("../../fixtures/test-plain.zena");
const TRAP: &str = include_str!("../../fixtures/test-trap.zena");
const UNSUPPORTED: &str = include_str!("../../fixtures/test-unsupported.zena");

/// Script that puts `html` in a new container at the end of the body,
/// and answers nothing.
fn mount(html: &str) -> String {
    format!(
        "const box = document.createElement('div');
         box.id = 'test-box';
         box.innerHTML = {};
         document.body.append(box);",
        js(html)
    )
}

pub fn it_gives_create_the_attributes_set_before_the_element_connects(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-label", LABEL)?;
    let text = browser.eval(
        "const element = document.createElement('test-label');
         element.setAttribute('label', 'set early');
         document.body.append(element);
         return await until(() => element.shadowRoot.querySelector('span')?.textContent,
                            'the first render');",
    )?;
    check(text == "set early", || {
        format!("the first render showed {text}")
    })
}

pub fn it_gives_an_element_that_connects_again_a_new_id(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-label", LABEL)?;
    let ids = browser.eval(
        "const element = document.createElement('test-label');
         document.body.append(element);
         const id = () => element.shadowRoot.querySelector('p')?.textContent;
         const first = await until(id, 'the first render');
         element.remove();
         await settle(100);
         document.body.append(element);
         const second = await until(() => id() !== first && id(), 'a render with a new id');
         return [first, second];",
    )?;
    check(ids[0] != ids[1], || {
        format!("the element kept its id: {ids}")
    })
}

pub fn it_serves_fifty_elements_of_a_tag_with_one_instance(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-label", LABEL)?;
    browser.eval(&format!(
        "{}
         for (let index = 0; index < 50; index += 1) {{
           const element = document.createElement('test-label');
           element.setAttribute('label', 'n' + index);
           box.append(element);
         }}
         await until(() => [...box.children].every((element) =>
           element.shadowRoot.querySelector('span')), 'fifty renders');
         return true;",
        mount("")
    ))?;
    let fifty = element_status(browser, "test-label")?;
    check(fifty["connected"] == 50 && fifty["instances"] == 1, || {
        format!("with fifty elements the status was {fifty}")
    })?;
    browser.eval(
        "const box = document.getElementById('test-box');
         [...box.children].slice(10).forEach((element) => element.remove());
         return true;",
    )?;
    let ten = element_status(browser, "test-label")?;
    check(ten["connected"] == 10 && ten["instances"] == 1, || {
        format!("with ten elements the status was {ten}")
    })
}

pub fn it_answers_the_diagnostics_of_a_source_that_does_not_compile(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let broken = LABEL.replace(
        "render(): View {",
        "render(): View {\n    let x: i32 = 'text';",
    );
    let answer = browser.eval(&format!(
        "try {{ await window.demo.define('test-broken', {}); return 'defined'; }}
         catch (error) {{ return String(error); }}",
        js(&broken)
    ))?;
    let text = answer.as_str().unwrap_or_default();
    check(
        text.contains("test-broken.zena:") && text.contains("Error"),
        || format!("the define answered {answer}"),
    )
}

pub fn it_changes_nodes_in_place_when_an_attribute_changes(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-label", LABEL)?;
    let answer = browser.eval(
        "const element = document.createElement('test-label');
         element.setAttribute('label', 'before');
         element.setAttribute('mode', 'one');
         document.body.append(element);
         const root = element.shadowRoot;
         await until(() => root.querySelector('span')?.textContent === 'before', 'the first render');
         const nodes = [root.querySelector('div'), root.querySelector('span'),
                        root.querySelector('p'), root.querySelector('span').firstChild];
         element.setAttribute('label', 'after');
         element.setAttribute('mode', 'two');
         await until(() => root.querySelector('span')?.textContent === 'after'
                     && root.querySelector('div').className === 'two', 'the second render');
         const now = [root.querySelector('div'), root.querySelector('span'),
                      root.querySelector('p'), root.querySelector('span').firstChild];
         return {
           same: nodes.every((node, index) => node === now[index]),
           title: root.querySelector('p').title,
         };",
    )?;
    check(answer["same"] == true && answer["title"] == "after", || {
        format!("the second render answered {answer}")
    })
}

pub fn it_keeps_keyed_nodes_when_a_list_reverses(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-list", LIST)?;
    let answer = browser.eval(
        "const element = document.createElement('test-list');
         element.setAttribute('items', 'a,b,c,d,e');
         document.body.append(element);
         const items = () => [...element.shadowRoot.querySelectorAll('li')];
         await until(() => items().length === 5, 'five items');
         const before = items();
         element.setAttribute('items', 'e,d,c,b,a');
         await until(() => items()[0]?.textContent === 'e', 'the reversed list');
         const after = items();
         return {
           order: after.map((item) => item.textContent).join(','),
           same: after.every((item, index) => item === before[4 - index]),
         };",
    )?;
    check(
        answer["order"] == "e,d,c,b,a" && answer["same"] == true,
        || format!("the reversed list answered {answer}"),
    )
}

pub fn it_applies_the_styles_an_element_returns(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-label", LABEL)?;
    let color = browser.eval(
        "const element = document.createElement('test-label');
         document.body.append(element);
         const p = await until(() => element.shadowRoot.querySelector('p'), 'the render');
         return getComputedStyle(p).color;",
    )?;
    check(color == "rgb(1, 2, 3)", || {
        format!("the paragraph's color is {color}")
    })
}

pub fn it_makes_a_child_element_with_its_own_shadow_root(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-child", CHILD)?;
    define(browser, "test-parent", PARENT)?;
    let answer = browser.eval(
        "const parent = document.createElement('test-parent');
         document.body.append(parent);
         const child = await until(() => parent.shadowRoot.querySelector('test-child'), 'the child');
         const button = await until(() => child.shadowRoot?.querySelector('button'), 'the child render');
         return { own: child.shadowRoot !== parent.shadowRoot, text: button.textContent };",
    )?;
    check(answer["own"] == true && answer["text"] == "ping", || {
        format!("the child answered {answer}")
    })
}

pub fn it_calls_the_handler_a_view_names_for_a_click(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-counter", COUNTER)?;
    let count = browser.eval(
        "const element = document.createElement('test-counter');
         document.body.append(element);
         const root = element.shadowRoot;
         const button = await until(() => root.querySelector('.bump'), 'the render');
         button.click();
         return await until(() => root.querySelector('.count')?.textContent === '1' && '1',
                            'the count after a click');",
    )?;
    check(count == "1", || format!("the count is {count}"))
}

pub fn it_sends_a_child_event_to_its_parent(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-child", CHILD)?;
    define(browser, "test-parent", PARENT)?;
    let heard = browser.eval(
        "const parent = document.createElement('test-parent');
         document.body.append(parent);
         const child = await until(() => parent.shadowRoot.querySelector('test-child'), 'the child');
         const button = await until(() => child.shadowRoot?.querySelector('button'), 'the child render');
         button.click();
         return await until(() => {
           const text = parent.shadowRoot.querySelector('.heard')?.textContent;
           return text !== 'nothing' && text;
         }, 'the parent to hear');",
    )?;
    check(heard == "hello from the child", || {
        format!("the parent heard {heard}")
    })
}

pub fn it_runs_the_calls_of_one_element_in_order(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-counter", COUNTER)?;
    let log = browser.eval(
        "const element = document.createElement('test-counter');
         document.body.append(element);
         const root = element.shadowRoot;
         await until(() => root.querySelector('.slow'), 'the render');
         root.querySelector('.slow').click();
         root.querySelector('.fast').click();
         return await until(() => {
           const text = root.querySelector('.log')?.textContent ?? '';
           return text.includes('fast') && text;
         }, 'both handlers');",
    )?;
    check(log == "slow-start,slow-end,fast,", || {
        format!("the handlers ran as {log}")
    })
}

pub fn it_runs_calls_of_two_elements_of_a_tag_at_the_same_time(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-counter", COUNTER)?;
    let answer = browser.eval(
        "const first = document.createElement('test-counter');
         const second = document.createElement('test-counter');
         document.body.append(first, second);
         await until(() => first.shadowRoot.querySelector('.slow')
                     && second.shadowRoot.querySelector('.fast'), 'both renders');
         first.shadowRoot.querySelector('.slow').click();
         await settle(50);
         second.shadowRoot.querySelector('.fast').click();
         await until(() => second.shadowRoot.querySelector('.log')?.textContent === 'fast,',
                     'the second element');
         const firstLog = first.shadowRoot.querySelector('.log')?.textContent ?? '';
         return { firstLog };",
    )?;
    check(answer["firstLog"] != "slow-start,slow-end,", || {
        format!("the second element waited for the first: {answer}")
    })
}

pub fn it_renders_and_handles_a_click_in_the_class_and_the_function_form(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-counter", COUNTER)?;
    define(browser, "test-function", FUNCTION)?;
    let answer = browser.eval(
        "const counter = document.createElement('test-counter');
         const plain = document.createElement('test-function');
         plain.setAttribute('label', 'go now');
         document.body.append(counter, plain);
         const go = await until(() => plain.shadowRoot.querySelector('.go'), 'the function form');
         const bump = await until(() => counter.shadowRoot.querySelector('.bump'), 'the class form');
         const heard = new Promise((resolve) =>
           plain.addEventListener('test-clicked', (event) => resolve(event.detail)));
         go.click();
         bump.click();
         const detail = await heard;
         const count = await until(() => counter.shadowRoot.querySelector('.count')?.textContent === '1'
                                   && '1', 'the class form count');
         return { label: go.textContent, detail, count };",
    )?;
    check(
        answer["label"] == "go now" && answer["detail"] == "go now" && answer["count"] == "1",
        || format!("the two forms answered {answer}"),
    )
}

pub fn it_renders_a_function_form_element_without_an_event_function(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-plain", PLAIN)?;
    let text = browser.eval(
        "const element = document.createElement('test-plain');
         element.setAttribute('label', 'plain');
         document.body.append(element);
         return await until(() => element.shadowRoot.querySelector('p')?.textContent, 'the render');",
    )?;
    check(text == "plain", || format!("the element showed {text}"))
}

pub fn it_shows_an_error_card_when_an_element_traps_and_works_after_a_restart(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-trap", TRAP)?;
    define(browser, "test-plain", PLAIN)?;
    let answer = browser.eval(
        "const elements = [document.createElement('test-trap'), document.createElement('test-trap')];
         const other = document.createElement('test-plain');
         other.setAttribute('label', 'still here');
         document.body.append(...elements, other);
         await until(() => elements.every((element) => element.shadowRoot.querySelector('.boom')),
                     'both renders');
         elements[0].shadowRoot.querySelector('.boom').click();
         const cards = await until(() => {
           const texts = elements.map((element) =>
             element.shadowRoot.querySelector('.error-card')?.textContent);
           return texts.every((text) => text) && texts;
         }, 'an error card on every element of the tag');
         await window.demo.restart('test-trap');
         await until(() => elements.every((element) => element.shadowRoot.querySelector('.boom')),
                     'the elements after the restart');
         const cardsLeft = elements.filter((element) =>
           element.shadowRoot.querySelector('.error-card')).length;
         elements[1].shadowRoot.querySelector('.bump').click();
         const count = await until(() =>
           elements[1].shadowRoot.querySelector('.count')?.textContent === '1' && '1',
           'a click after the restart');
         const otherText = other.shadowRoot.querySelector('p')?.textContent;
         return { cards, cardsLeft, count, otherText };",
    )?;
    let cards = answer["cards"].as_array().cloned().unwrap_or_default();
    check(
        cards.len() == 2
            && cards.iter().all(|card| {
                card.as_str()
                    .is_some_and(|card| card.contains("<test-trap> trapped"))
            })
            && answer["cardsLeft"] == 0
            && answer["count"] == "1"
            && answer["otherText"] == "still here",
        || format!("the trap answered {answer}"),
    )
}

pub fn it_fails_a_call_outside_the_http_subset(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define(browser, "test-unsupported", UNSUPPORTED)?;
    let card = browser.eval(
        "const element = document.createElement('test-unsupported');
         document.body.append(element);
         const button = await until(() => element.shadowRoot.querySelector('.go'), 'the render');
         button.click();
         return await until(() => element.shadowRoot.querySelector('.error-card')?.textContent,
                            'the failure');",
    )?;
    check(
        card.as_str().unwrap_or_default().contains("get-and-delete"),
        || format!("the failure said {card}"),
    )
}

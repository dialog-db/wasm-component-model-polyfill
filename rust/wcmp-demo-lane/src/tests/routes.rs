// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Routes: patterns and parameters, `wasi:http` on both ends, failures,
//! and the demo's two routes over the todo model.

use serde_json::Value;

use super::{check, define, define_route, route_status};
use crate::browser::Browser;

const ECHO: &str = include_str!("../../fixtures/echo-route.zena");
const PARAMS: &str = include_str!("../../fixtures/params-route.zena");
const SENDER: &str = include_str!("../../fixtures/test-sender.zena");

/// The answer of `api(method, path, body)` on the page.
fn api(browser: &Browser, method: &str, path: &str, body: Option<&str>) -> Result<Value, String> {
    let body = body.map_or("undefined".to_string(), str::to_string);
    browser.eval(&format!("return await api('{method}', '{path}', {body});"))
}

pub fn it_answers_a_route_with_its_pattern_and_parameters(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    define_route(browser, "/api/test/:name", PARAMS)?;
    let answer = api(browser, "GET", "/api/test/a%20name", None)?;
    check(
        answer["status"] == 200
            && answer["route"] == "/api/test/:name"
            && answer["body"]["name"] == "a name"
            && answer["body"]["method"] == "GET",
        || format!("the route answered {answer}"),
    )
}

pub fn it_sends_every_method_with_headers_and_a_body_through_wasi_http(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define_route(browser, "/api/echo", ECHO)?;
    define(browser, "test-sender", SENDER)?;
    browser.responses()?;
    let results = browser.eval(
        "const element = document.createElement('test-sender');
         document.body.append(element);
         const button = await until(() => element.shadowRoot.querySelector('.send'), 'the render');
         button.click();
         return await until(() => element.shadowRoot.querySelector('.results')?.textContent,
                            'the four answers', 60000);",
    )?;
    let results = results.as_str().unwrap_or_default();
    for method in ["GET", "POST", "PATCH", "DELETE"] {
        let body = if method == "GET" {
            String::new()
        } else {
            format!("body-{method}")
        };
        let expected = format!(
            "{method} 200 /api/echo {{\"method\":\"{method}\",\"header\":\"header-{method}\",\"body\":\"{body}\"}}"
        );
        check(results.contains(&expected), || {
            format!("no line `{expected}` in the answers:\n{results}")
        })?;
    }
    let responses = browser.responses_until(|responses| {
        responses
            .iter()
            .filter(|response| response.url.contains("/api/echo"))
            .count()
            >= 4
    })?;
    let echoes: Vec<_> = responses
        .iter()
        .filter(|response| response.url.contains("/api/echo"))
        .collect();
    check(
        echoes.len() == 4
            && echoes.iter().all(|response| {
                response.from_service_worker && response.header("x-demo-route") == Some("/api/echo")
            }),
        || format!("the echoes crossed the network as {echoes:?}"),
    )
}

pub fn it_answers_500_for_a_route_that_traps_and_compiles_it_again(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    define_route(browser, "/api/test/:name", PARAMS)?;
    let trapped = api(browser, "GET", "/api/test/x?trap=1", None)?;
    check(trapped["status"] == 500, || {
        format!("a trap answered {trapped}")
    })?;
    let again = api(browser, "GET", "/api/test/x", None)?;
    check(again["status"] == 200, || {
        format!("after the trap the route answered {again}")
    })?;
    let status = route_status(browser, "/api/test/:name")?;
    check(status["compiles"] == 2, || {
        format!("the route's status is {status}")
    })
}

pub fn it_answers_500_with_the_diagnostics_of_a_route_that_does_not_compile(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let broken = PARAMS.replace(
        "let out = new JsonObject();",
        "let out: i32 = new JsonObject();",
    );
    define_route(browser, "/api/test/:name", &broken)?;
    let answer = api(browser, "GET", "/api/test/x", None)?;
    let body = answer["body"].as_str().unwrap_or_default();
    check(answer["status"] == 500 && body.contains("Error"), || {
        format!("a broken route answered {answer}")
    })
}

pub fn it_answers_each_request_of_the_route_tables(browser: &Browser) -> Result<(), String> {
    browser.boot()?;
    let todo = |title: &str| format!("{{ title: '{title}' }}");
    let added = api(browser, "POST", "/api/todos", Some(&todo("one")))?;
    check(
        added["status"] == 201 && added["body"]["title"] == "one",
        || format!("POST answered {added}"),
    )?;
    let id = added["body"]["id"].as_str().unwrap_or_default().to_string();
    api(browser, "POST", "/api/todos", Some(&todo("two")))?;
    let patched = api(
        browser,
        "PATCH",
        &format!("/api/todos/{id}"),
        Some("{ completed: true }"),
    )?;
    check(
        patched["status"] == 200 && patched["body"]["completed"] == true,
        || format!("PATCH one answered {patched}"),
    )?;
    let renamed = api(
        browser,
        "PATCH",
        &format!("/api/todos/{id}"),
        Some("{ title: 'uno' }"),
    )?;
    check(
        renamed["status"] == 200 && renamed["body"]["title"] == "uno",
        || format!("PATCH title answered {renamed}"),
    )?;
    let all = api(browser, "GET", "/api/todos?filter=all", None)?;
    let active = api(browser, "GET", "/api/todos?filter=active", None)?;
    let completed = api(browser, "GET", "/api/todos?filter=completed", None)?;
    check(
        all["body"]["todos"].as_array().map(Vec::len) == Some(2)
            && active["body"]["todos"][0]["title"] == "two"
            && completed["body"]["todos"][0]["title"] == "uno"
            && all["body"]["counts"]["active"] == 1
            && all["body"]["counts"]["completed"] == 1,
        || format!("GET answered {all}, {active}, {completed}"),
    )?;
    let toggled = api(browser, "PATCH", "/api/todos", Some("{ completed: true }"))?;
    let after = api(browser, "GET", "/api/todos", None)?;
    check(
        toggled["status"] == 204 && after["body"]["counts"]["completed"] == 2,
        || format!("PATCH all answered {toggled}, then {after}"),
    )?;
    let cleared = api(browser, "DELETE", "/api/todos", None)?;
    let empty = api(browser, "GET", "/api/todos", None)?;
    check(
        cleared["status"] == 204 && empty["body"]["todos"].as_array().map(Vec::len) == Some(0),
        || format!("DELETE all answered {cleared}, then {empty}"),
    )?;
    let missing = api(
        browser,
        "PATCH",
        "/api/todos/t999",
        Some("{ completed: true }"),
    )?;
    let missing_delete = api(browser, "DELETE", "/api/todos/t999", None)?;
    check(
        missing["status"] == 404 && missing_delete["status"] == 404,
        || format!("an unknown id answered {missing} and {missing_delete}"),
    )?;
    let added = api(browser, "POST", "/api/todos", Some(&todo("three")))?;
    let id = added["body"]["id"].as_str().unwrap_or_default().to_string();
    let deleted = api(browser, "DELETE", &format!("/api/todos/{id}"), None)?;
    check(deleted["status"] == 204, || {
        format!("DELETE one answered {deleted}")
    })?;
    let refused = [
        api(browser, "PUT", "/api/todos", Some("{}"))?,
        api(browser, "GET", "/api/todos/t1", None)?,
    ];
    check(refused.iter().all(|answer| answer["status"] == 405), || {
        format!("a method a route does not accept answered {refused:?}")
    })
}

pub fn it_enforces_the_rules_of_the_todo_model_through_the_routes(
    browser: &Browser,
) -> Result<(), String> {
    browser.boot()?;
    let empty = api(browser, "POST", "/api/todos", Some("{ title: '   ' }"))?;
    let trimmed = api(
        browser,
        "POST",
        "/api/todos",
        Some("{ title: '  spaced out  ' }"),
    )?;
    let id = trimmed["body"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let renamed = api(
        browser,
        "PATCH",
        &format!("/api/todos/{id}"),
        Some("{ title: '' }"),
    )?;
    check(
        empty["status"] == 422
            && trimmed["body"]["title"] == "spaced out"
            && renamed["status"] == 422,
        || format!("the model answered {empty}, {trimmed}, and {renamed}"),
    )
}

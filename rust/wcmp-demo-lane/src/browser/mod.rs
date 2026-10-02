// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One browser session on the served demo, with the tools the tests use:
//! scripts that reach into shadow roots, the Chrome DevTools Protocol
//! through ChromeDriver, and Chrome's performance log, which records each
//! network response with its headers.

#[allow(clippy::module_inception)]
mod browser;
mod response;

pub use browser::Browser;
pub use response::Response;

/// Helpers every script the tests run can call. `deep(selectors)` walks
/// from the document through each selector, into the shadow root of each
/// element it finds on the way. `app()` is `<todo-app>`, `items()` its
/// `<todo-item>`s, and `titles()` their titles.
pub const PRELUDE: &str = r#"
const deep = (...selectors) => {
  let node = document;
  for (const selector of selectors) {
    const root = node.shadowRoot ?? node;
    node = root.querySelector(selector);
    if (!node) return null;
  }
  return node;
};
const app = () => document.querySelector('todo-app');
const items = () => app() && app().shadowRoot
  ? [...app().shadowRoot.querySelectorAll('todo-item')]
  : [];
const titles = () => items().map((item) => item.shadowRoot.querySelector('label')?.textContent ?? '');
const fire = (node, event) => node.dispatchEvent(event);
const key = (node, key) => fire(node, new KeyboardEvent('keydown', { key, bubbles: true, composed: true }));
const type = (node, text) => {
  node.focus();
  node.value = text;
  fire(node, new Event('input', { bubbles: true, composed: true }));
};
const settle = (ms = 50) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (test, what, ms = 30000) => {
  const start = performance.now();
  while (performance.now() - start < ms) {
    const value = await test();
    if (value) return value;
    await settle(25);
  }
  throw new Error('timed out waiting for ' + what);
};
const api = async (method, path, body) => {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  return {
    status: response.status,
    route: response.headers.get('x-demo-route'),
    body: text === '' ? null : (() => { try { return JSON.parse(text); } catch { return text; } })(),
  };
};
const addTodo = async (title) => {
  const before = items().length;
  const input = deep('todo-app', 'todo-input', '.new-todo');
  type(input, title);
  key(input, 'Enter');
  await until(() => items().length > before, 'the todo ' + title);
};
"#;

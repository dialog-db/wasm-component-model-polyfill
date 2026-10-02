// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One browser session.

use std::time::Duration;

use serde_json::{Value, json};

use super::{PRELUDE, Response};
use crate::webdriver::WebDriver;

/// One browser session.
pub struct Browser {
    driver: WebDriver,
    session: String,
    /// The demo's origin, such as `http://127.0.0.1:8766`.
    pub origin: String,
}

impl Browser {
    /// A session of `driver`, on the demo at `origin`.
    pub fn new(driver: WebDriver, session: String, origin: String) -> Result<Self, String> {
        let browser = Browser {
            driver,
            session,
            origin,
        };
        browser.command(
            "POST",
            "/timeouts",
            Some(&json!({ "script": 170_000, "pageLoad": 120_000 })),
        )?;
        Ok(browser)
    }

    /// Send a command of this session.
    fn command(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        self.driver
            .request(method, &format!("/session/{}{path}", self.session), body)
    }

    /// A PNG of the browser's window as it is now.
    ///
    /// # Errors
    ///
    /// The WebDriver error, or an answer that is not base64.
    pub fn screenshot(&self) -> Result<Vec<u8>, String> {
        let encoded = self.command("GET", "/screenshot", None)?;
        decode_base64(encoded.as_str().unwrap_or_default())
    }

    /// Open `path` on the demo's origin.
    pub fn goto(&self, path: &str) -> Result<(), String> {
        let url = format!("{}{path}", self.origin);
        self.command("POST", "/url", Some(&json!({ "url": url })))?;
        Ok(())
    }

    /// Reload the page.
    pub fn reload(&self) -> Result<(), String> {
        self.command("POST", "/refresh", Some(&json!({})))?;
        Ok(())
    }

    /// Open the demo and wait until it has started: its elements are
    /// defined, the service worker controls the page, and `<todo-app>`
    /// has rendered.
    pub fn boot(&self) -> Result<(), String> {
        self.goto("/")?;
        self.wait_ready()
    }

    /// Wait until the page has started.
    pub fn wait_ready(&self) -> Result<(), String> {
        self.eval(
            "try {
               await until(() => document.documentElement.dataset.boot === 'ready'
                 && app() && app().shadowRoot && app().shadowRoot.querySelector('.todoapp'),
                 'the demo to start', 110000);
             } catch (error) {
               throw new Error('the demo did not start: it stopped at the step '
                 + document.documentElement.dataset.boot);
             }
             return true;",
        )?;
        Ok(())
    }

    /// Run `body`, the body of an async function with the [`PRELUDE`] in
    /// scope, and answer what it returns.
    ///
    /// # Errors
    ///
    /// What the function threw, or the WebDriver error.
    pub fn eval(&self, body: &str) -> Result<Value, String> {
        let script = format!(
            "const done = arguments[arguments.length - 1];
             {PRELUDE}
             (async () => {{ {body} }})().then(
               (value) => done({{ ok: value === undefined ? null : value }}),
               (error) => done({{ error: String(error && error.stack || error) }}));"
        );
        let answer = self.command(
            "POST",
            "/execute/async",
            Some(&json!({ "script": script, "args": [] })),
        )?;
        if let Some(error) = answer.get("error") {
            return Err(error.as_str().unwrap_or("the script threw").to_string());
        }
        Ok(answer["ok"].clone())
    }

    /// Send the Chrome DevTools Protocol command `method` with `params`.
    pub fn cdp(&self, method: &str, params: Value) -> Result<Value, String> {
        self.command(
            "POST",
            "/goog/cdp/execute",
            Some(&json!({ "cmd": method, "params": params })),
        )
    }

    /// Stop every service worker of the browser, as Chromium does to an
    /// idle one.
    pub fn stop_service_workers(&self) -> Result<(), String> {
        self.cdp("ServiceWorker.enable", json!({}))?;
        self.cdp("ServiceWorker.stopAllWorkers", json!({}))?;
        Ok(())
    }

    /// Render with the color scheme `scheme`, `light` or `dark`.
    pub fn color_scheme(&self, scheme: &str) -> Result<(), String> {
        self.cdp(
            "Emulation.setEmulatedMedia",
            json!({ "features": [{ "name": "prefers-color-scheme", "value": scheme }] }),
        )?;
        Ok(())
    }

    /// Remove the demo's IndexedDB data.
    pub fn clear_indexed_db(&self) -> Result<(), String> {
        self.cdp(
            "Storage.clearDataForOrigin",
            json!({ "origin": self.origin, "storageTypes": "indexeddb" }),
        )?;
        Ok(())
    }

    /// Each network response since the last call: its URL, status, and
    /// headers, and whether a service worker answered it, from Chrome's
    /// performance log.
    pub fn responses(&self) -> Result<Vec<Response>, String> {
        let entries = self.command("POST", "/se/log", Some(&json!({ "type": "performance" })))?;
        let mut responses = Vec::new();
        for entry in entries.as_array().into_iter().flatten() {
            let Some(message) = entry["message"].as_str() else {
                continue;
            };
            let Ok(message) = serde_json::from_str::<Value>(message) else {
                continue;
            };
            let message = &message["message"];
            if message["method"] != "Network.responseReceived" {
                continue;
            }
            let response = &message["params"]["response"];
            let headers = response["headers"]
                .as_object()
                .map(|headers| {
                    headers
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.to_ascii_lowercase(),
                                value.as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            responses.push(Response {
                url: response["url"].as_str().unwrap_or_default().to_string(),
                status: response["status"].as_u64().unwrap_or(0) as u16,
                headers,
                from_service_worker: response["fromServiceWorker"].as_bool().unwrap_or(false),
            });
        }
        Ok(responses)
    }

    /// Each network response from now on, as [`Browser::responses`]
    /// reads them, until `done` holds of them or ten seconds pass: the
    /// performance log can trail the page by a moment.
    ///
    /// # Errors
    ///
    /// The WebDriver error.
    pub fn responses_until(
        &self,
        done: impl Fn(&[Response]) -> bool,
    ) -> Result<Vec<Response>, String> {
        let started = std::time::Instant::now();
        let mut responses = Vec::new();
        loop {
            responses.extend(self.responses()?);
            if done(&responses) || started.elapsed() > Duration::from_secs(10) {
                return Ok(responses);
            }
            Self::sleep(Duration::from_millis(100));
        }
    }

    /// The console lines the page wrote, for a failure's report.
    pub fn console(&self) -> Vec<String> {
        self.command("POST", "/se/log", Some(&json!({ "type": "browser" })))
            .ok()
            .and_then(|entries| entries.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry["message"].as_str().map(str::to_string))
            .collect()
    }

    /// Wait `duration`.
    pub fn sleep(duration: Duration) {
        std::thread::sleep(duration);
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self
            .driver
            .request("DELETE", &format!("/session/{}", self.session), None);
    }
}

/// The bytes the standard base64 text `text` encodes.
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'=')
    {
        let six = value(byte).ok_or_else(|| format!("`{}` is not base64", byte as char))?;
        buffer = (buffer << 6) | u32::from(six);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    Ok(bytes)
}

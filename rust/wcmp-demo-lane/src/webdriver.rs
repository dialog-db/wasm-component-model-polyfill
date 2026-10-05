// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A WebDriver client over plain HTTP: the few commands the lane sends
//! to ChromeDriver, each a JSON request on a connection of its own.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{Value, json};

/// How long a command may take: a script that waits for the demo to
/// compile can take tens of seconds.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(180);

/// A WebDriver server on a loopback port.
#[derive(Clone)]
pub struct WebDriver {
    port: u16,
}

impl WebDriver {
    /// The WebDriver server on `port`.
    pub fn new(port: u16) -> Self {
        WebDriver { port }
    }

    /// Send `method` `path` with `body`, and answer the response's
    /// `value`.
    ///
    /// # Errors
    ///
    /// The connection's error, or the WebDriver error the server
    /// answered.
    pub fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port))
            .map_err(|error| format!("connecting to WebDriver: {error}"))?;
        stream
            .set_read_timeout(Some(COMMAND_TIMEOUT))
            .map_err(|error| error.to_string())?;
        let body = body.map(Value::to_string).unwrap_or_default();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|error| format!("{method} {path}: {error}"))?;
        let response =
            read_response(&mut stream).map_err(|error| format!("{method} {path}: {error}"))?;
        let response = String::from_utf8_lossy(&response);
        let (head, body) = response
            .split_once("\r\n\r\n")
            .ok_or_else(|| format!("{method} {path}: a response with no body"))?;
        let status: u16 = head
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .ok_or_else(|| format!("{method} {path}: a response with no status: {head}"))?;
        let body = if head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
        {
            unchunk(body)
        } else {
            body.to_string()
        };
        let value: Value = serde_json::from_str(&body)
            .map_err(|error| format!("{method} {path}: {error} in {body}"))?;
        if status >= 400 {
            let message = value["value"]["message"].as_str().unwrap_or("").to_string();
            return Err(format!("{method} {path}: {status} {message}"));
        }
        Ok(value["value"].clone())
    }

    /// Whether the server answers its status.
    pub fn ready(&self) -> bool {
        self.request("GET", "/status", None)
            .is_ok_and(|status| status["ready"].as_bool() == Some(true))
    }

    /// Start a session with the browser `chrome`, its profile in
    /// `profile`. With `WCMP_DEMO_LANE_HEADED` set, the browser opens a
    /// window, with its GPU, as a person's would.
    ///
    /// # Errors
    ///
    /// The WebDriver error.
    pub fn session(&self, chrome: &str, profile: &str) -> Result<String, String> {
        let headed = std::env::var_os("WCMP_DEMO_LANE_HEADED").is_some();
        // The allocation samplers that walk frame pointers crash a
        // renderer that runs the Zena compiler: their stack walk
        // segfaults on V8's wasm frames. The flake turns them off in every
        // browser it launches, for the same reason.
        let mut args = vec![
            "--no-sandbox".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--disable-features=HeapProfilerReporting,GwpAsanMalloc,GwpAsanPartitionAlloc"
                .to_string(),
            "--window-size=1280,1000".to_string(),
            format!("--user-data-dir={profile}"),
        ];
        if !headed {
            args.push("--headless=new".to_string());
            args.push("--disable-gpu".to_string());
        }
        let capabilities = json!({
            "capabilities": {
                "alwaysMatch": {
                    "browserName": "chrome",
                    "goog:chromeOptions": {
                        "binary": chrome,
                        "args": args,
                    },
                    "goog:loggingPrefs": { "performance": "ALL", "browser": "ALL" },
                },
            },
        });
        let session = self.request("POST", "/session", Some(&capabilities))?;
        session["sessionId"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("a session with no id: {session}"))
    }
}

/// Read one response from `stream`: its head, and then its body, by its
/// `Content-Length` when it has one and otherwise to the end. ChromeDriver
/// keeps a connection open after its response, whatever the request
/// asked, so reading to the end would wait for the read timeout.
fn read_response(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(response);
        }
        response.extend_from_slice(&buffer[..read]);
        let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&response[..end]).to_ascii_lowercase();
        let length = head.lines().find_map(|line| {
            line.strip_prefix("content-length:")
                .and_then(|length| length.trim().parse::<usize>().ok())
        });
        if let Some(length) = length {
            while response.len() < end + 4 + length {
                let read = stream.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                response.extend_from_slice(&buffer[..read]);
            }
            return Ok(response);
        }
        stream.read_to_end(&mut response)?;
        return Ok(response);
    }
}

/// The body of a chunked response, joined.
fn unchunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 || after.len() < size {
            break;
        }
        out.push_str(&after[..size]);
        rest = after[size..].trim_start_matches("\r\n");
    }
    out
}

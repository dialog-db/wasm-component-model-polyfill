// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The lane's harness: the server, ChromeDriver, a session per test, and
//! the report.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::browser::Browser;
use crate::tests::{TESTS, Test};
use crate::webdriver::WebDriver;

/// The tools the lane runs, from the environment.
struct Tools {
    site: String,
    chrome: String,
    chromedriver: String,
    server: String,
}

impl Tools {
    fn from_env() -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).map_err(|_| format!("{name} is not set"));
        Ok(Tools {
            site: var("WCMP_DEMO_SITE")?,
            chrome: var("CHROME")?,
            chromedriver: var("CHROMEDRIVER")?,
            server: var("STATIC_WEB_SERVER")?,
        })
    }
}

/// A child process the lane kills when it ends.
struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A free loopback port.
fn free_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| error.to_string())
}

/// How one test ended.
struct Outcome {
    name: &'static str,
    passed: Result<(), String>,
    /// Whether the test ran again after Chrome's tab crashed.
    retried: bool,
    seconds: f64,
}

/// Run the lane with its command line, and answer its exit status.
pub fn run(arguments: Vec<String>) -> i32 {
    let mut jobs = 2;
    let mut report = None;
    let mut filters = Vec::new();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--jobs" | "-j" => {
                jobs = arguments
                    .next()
                    .and_then(|jobs| jobs.parse().ok())
                    .unwrap_or(jobs);
            }
            "--report" => report = arguments.next().map(PathBuf::from),
            _ => filters.push(argument),
        }
    }
    if let Ok(script) = std::env::var("WCMP_DEMO_LANE_EVAL") {
        return match inspect(&script) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("demo lane: {error}");
                1
            }
        };
    }
    match run_tests(jobs.max(1), &filters) {
        Ok((lines, passed)) => {
            let text = lines.join("\n") + "\n";
            print!("{text}");
            if let Some(report) = report
                && let Err(error) = std::fs::write(&report, &text)
            {
                eprintln!("demo lane: writing {}: {error}", report.display());
                return 1;
            }
            i32::from(!passed)
        }
        Err(error) => {
            eprintln!("demo lane: {error}");
            1
        }
    }
}

/// Run every test that a filter selects, `jobs` at a time, and answer
/// the report's lines and whether every test passed.
fn run_tests(jobs: usize, filters: &[String]) -> Result<(Vec<String>, bool), String> {
    let tools = Arc::new(Tools::from_env()?);
    let tests: Vec<&'static Test> = TESTS
        .iter()
        .filter(|test| {
            filters.is_empty()
                || filters
                    .iter()
                    .any(|filter| test.name.contains(filter.as_str()))
        })
        .collect();
    if tests.is_empty() {
        return Err("no test matches the filters".to_string());
    }

    let server_port = free_port()?;
    let _server = Process(
        Command::new(&tools.server)
            .args([
                "--root",
                &tools.site,
                "--host",
                "127.0.0.1",
                "--port",
                &server_port.to_string(),
                "--cache-control-headers=false",
                "--log-level",
                "error",
            ])
            .stdout(Stdio::null())
            .spawn()
            .map_err(|error| format!("starting {}: {error}", tools.server))?,
    );
    let driver_port = free_port()?;
    let _chromedriver = Process(
        Command::new(&tools.chromedriver)
            .arg(format!("--port={driver_port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("starting {}: {error}", tools.chromedriver))?,
    );
    let driver = WebDriver::new(driver_port);
    let started = Instant::now();
    while !driver.ready() {
        if started.elapsed() > Duration::from_secs(30) {
            return Err("ChromeDriver did not start".to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    while TcpStream::connect(("127.0.0.1", server_port)).is_err() {
        if started.elapsed() > Duration::from_secs(30) {
            return Err(format!("{} did not start", tools.server));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let origin = format!("http://127.0.0.1:{server_port}");
    let profiles = std::env::temp_dir().join(format!("wcmp-demo-lane-{}", std::process::id()));

    let next = Arc::new(AtomicUsize::new(0));
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let tests = Arc::new(tests);
    let workers: Vec<_> = (0..jobs)
        .map(|_| {
            let (next, outcomes, tests) = (next.clone(), outcomes.clone(), tests.clone());
            let (tools, driver, origin, profiles) = (
                tools.clone(),
                driver.clone(),
                origin.clone(),
                profiles.clone(),
            );
            std::thread::spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(test) = tests.get(index) else {
                        break;
                    };
                    let started = Instant::now();
                    let profile = profiles.join(format!("{index}"));
                    // A test that panics fails, and the lane goes on.
                    let attempt = || {
                        let passed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            run_one(test, &tools, &driver, &origin, &profile)
                        }))
                        .unwrap_or_else(|_| Err("the test panicked".to_string()));
                        let _ = std::fs::remove_dir_all(&profile);
                        passed
                    };
                    // Chrome's renderer can crash under the memory of
                    // several sessions, each compiling in two contexts.
                    // That says nothing of the demo, so the test runs once
                    // more, and the report says so.
                    let mut passed = attempt();
                    let mut retried = false;
                    if passed
                        .as_ref()
                        .is_err_and(|reason| reason.contains("tab crashed"))
                    {
                        retried = true;
                        passed = attempt();
                    }
                    let outcome = Outcome {
                        name: test.name,
                        passed,
                        retried,
                        seconds: started.elapsed().as_secs_f64(),
                    };
                    println!("{}", line(&outcome));
                    outcomes.lock().expect("the outcomes").push(outcome);
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
    let _ = std::fs::remove_dir_all(&profiles);

    let mut outcomes = std::mem::take(&mut *outcomes.lock().expect("the outcomes"));
    outcomes.sort_by_key(|outcome| TESTS.iter().position(|test| test.name == outcome.name));
    let passed = outcomes
        .iter()
        .filter(|outcome| outcome.passed.is_ok())
        .count();
    let mut lines = vec![
        format!("demo lane on {}", chrome_version(&tools.chrome)),
        String::new(),
    ];
    lines.extend(outcomes.iter().map(line));
    lines.push(String::new());
    lines.push(format!("Passes: {passed}/{}", tests.len()));
    Ok((lines, passed == tests.len()))
}

/// For a person looking into the demo: open it, at the path
/// `WCMP_DEMO_LANE_PATH` names, `/` by default, wait the seconds
/// `WCMP_DEMO_LANE_WAIT` names, 10 by default, run `script` with the
/// prelude in scope, and print what it answers and the page's console.
/// With `WCMP_DEMO_LANE_CPU_PROFILE` set to a file, write a V8 CPU
/// profile of the page while the script runs to it. With
/// `WCMP_DEMO_LANE_CDP` set to CDP commands, separated by semicolons,
/// each a method and optionally its parameters as JSON, send each before
/// the script runs. With `WCMP_DEMO_LANE_SCREENSHOT` set to
/// a file, write a PNG of the window to it after the script runs.
fn inspect(script: &str) -> Result<(), String> {
    let tools = Tools::from_env()?;
    let server_port = free_port()?;
    let _server = Process(
        Command::new(&tools.server)
            .args([
                "--root",
                &tools.site,
                "--host",
                "127.0.0.1",
                "--port",
                &server_port.to_string(),
            ])
            .stdout(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?,
    );
    let driver_port = free_port()?;
    let _chromedriver = Process(
        Command::new(&tools.chromedriver)
            .arg(format!("--port={driver_port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?,
    );
    let driver = WebDriver::new(driver_port);
    while !driver.ready() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let profile = std::env::temp_dir().join(format!("wcmp-demo-inspect-{}", std::process::id()));
    let session = driver.session(&tools.chrome, &profile.to_string_lossy())?;
    let browser = Browser::new(driver, session, format!("http://127.0.0.1:{server_port}"))?;
    let path = std::env::var("WCMP_DEMO_LANE_PATH").unwrap_or_else(|_| "/".to_string());
    browser.goto(&path)?;
    let wait = std::env::var("WCMP_DEMO_LANE_WAIT")
        .ok()
        .and_then(|wait| wait.parse().ok())
        .unwrap_or(10);
    std::thread::sleep(Duration::from_secs(wait));
    // A V8 CPU profile of the page while the script runs, when
    // `WCMP_DEMO_LANE_CPU_PROFILE` names a file to write it to. It opens
    // in the performance panel of Chrome's DevTools.
    // CDP commands to send before the script runs, when
    // `WCMP_DEMO_LANE_CDP` names them, separated by semicolons, each a
    // method and optionally its parameters as JSON: such as
    // `Debugger.enable`, which is what an open DevTools does.
    for command in std::env::var("WCMP_DEMO_LANE_CDP")
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|command| !command.is_empty())
    {
        let (method, params) = command.split_once(' ').unwrap_or((command, "{}"));
        let params: serde_json::Value = serde_json::from_str(params)
            .map_err(|error| format!("the parameters of {method}: {error}"))?;
        browser.cdp(method, params)?;
    }
    let cpu_profile = std::env::var("WCMP_DEMO_LANE_CPU_PROFILE").ok();
    if cpu_profile.is_some() {
        browser.cdp("Profiler.enable", serde_json::json!({}))?;
        browser.cdp(
            "Profiler.setSamplingInterval",
            serde_json::json!({ "interval": 100 }),
        )?;
        browser.cdp("Profiler.start", serde_json::json!({}))?;
    }
    let answer = browser.eval(script);
    if let Some(file) = cpu_profile {
        let stopped = browser.cdp("Profiler.stop", serde_json::json!({}))?;
        std::fs::write(&file, stopped["profile"].to_string())
            .map_err(|error| format!("writing {file}: {error}"))?;
    }
    // A screenshot of the window once the script ran, when
    // `WCMP_DEMO_LANE_SCREENSHOT` names a file to write it to.
    if let Ok(file) = std::env::var("WCMP_DEMO_LANE_SCREENSHOT") {
        std::fs::write(&file, browser.screenshot()?)
            .map_err(|error| format!("writing {file}: {error}"))?;
    }
    for line in browser.console() {
        println!("console: {line}");
    }
    println!("answer: {answer:?}");
    drop(browser);
    let _ = std::fs::remove_dir_all(&profile);
    Ok(())
}

/// One test in a session of its own.
fn run_one(
    test: &Test,
    tools: &Tools,
    driver: &WebDriver,
    origin: &str,
    profile: &std::path::Path,
) -> Result<(), String> {
    let session = driver.session(&tools.chrome, &profile.to_string_lossy())?;
    let browser = Browser::new(driver.clone(), session, origin.to_string())?;
    let result = (test.run)(&browser);
    if result.is_err() {
        for line in browser.console().iter().rev().take(20).rev() {
            eprintln!("  console [{}]: {line}", test.name);
        }
    }
    result
}

/// The report's line for `outcome`.
fn line(outcome: &Outcome) -> String {
    let retried = if outcome.retried {
        ", after a tab crash"
    } else {
        ""
    };
    match &outcome.passed {
        Ok(()) => format!(
            "- pass {} ({:.1} s{retried})",
            outcome.name, outcome.seconds
        ),
        Err(reason) => format!(
            "- FAIL {} ({:.1} s): {}",
            outcome.name,
            outcome.seconds,
            reason.lines().next().unwrap_or_default()
        ),
    }
}

/// The version `chrome` reports.
fn chrome_version(chrome: &str) -> String {
    Command::new(chrome)
        .arg("--version")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "an unknown Chrome".to_string())
}

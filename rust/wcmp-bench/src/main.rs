//! The benchmark runner's entry point.
//!
//! Natively it measures the backend `WCMP_BENCH_BACKEND` names,
//! Wasmtime or Wasmi, prints the table, writes the JSON report where
//! `WCMP_BENCH_REPORT` points, and exits non-zero when a benchmark
//! failed. Under `wasm-bindgen` the page's `report` function receives
//! the same two strings instead; `main` runs on instantiation and
//! schedules the run on the browser's event loop.
//!
//! Both runners take their run controls as `key=value` words — the
//! native one from its command line, the browser one from the page's
//! query string — and hand them to the same parser, so a run is asked
//! for the same way on either target.

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let plan =
        match wcmp_bench::Plan::default().with_overrides(arguments.iter().map(String::as_str)) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        };

    let backend = match wcmp_bench::backend() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    let report = wcmp_bench::measure(plan, backend).await;
    print!("{}", report.table());

    if let Ok(path) = std::env::var("WCMP_BENCH_REPORT") {
        match std::fs::write(&path, report.json()) {
            Ok(()) => println!("report written to {path}"),
            Err(error) => {
                eprintln!("cannot write the report to {path}: {error}");
                std::process::exit(2);
            }
        }
    }

    if report.failed() {
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(js_namespace = window)]
    extern "C" {
        /// The page's sink for a finished run: the table for a reader
        /// and the JSON for the driver that scraped the page.
        pub fn report(table: &str, json: &str);
    }

    /// The page's query string as run-control words, so that
    /// `?samples=5&warmup=2` is the browser's spelling of the
    /// `samples=5 warmup=2` the native runner takes.
    pub fn controls() -> Vec<String> {
        let global = js_sys::global();
        let Ok(location) = js_sys::Reflect::get(&global, &JsValue::from_str("location")) else {
            return Vec::new();
        };
        let Ok(search) = js_sys::Reflect::get(&location, &JsValue::from_str("search")) else {
            return Vec::new();
        };
        search
            .as_string()
            .unwrap_or_default()
            .trim_start_matches('?')
            .split('&')
            .filter(|word| !word.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    wasm_bindgen_futures::spawn_local(async {
        let controls = web::controls();
        let plan =
            match wcmp_bench::Plan::default().with_overrides(controls.iter().map(String::as_str)) {
                Ok(plan) => plan,
                Err(error) => {
                    web::report(&error.to_string(), "null");
                    return;
                }
            };
        let report = wcmp_bench::measure(plan, "web").await;
        web::report(&report.table(), &report.json());
    });
}

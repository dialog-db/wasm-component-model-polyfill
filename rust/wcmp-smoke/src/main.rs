//! The smoke test entry point. Natively it prints the report and exits
//! non-zero on a failure. Under `wasm-bindgen` the page's `report`
//! function receives each line instead; `main` runs on instantiation
//! and schedules the asynchronous run on the browser's event loop.

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(js_namespace = window)]
    extern "C" {
        pub fn report(line: &str);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let steps = wcmp_smoke::run().await;
    print!("{}", wcmp_smoke::render(&steps));
    if !wcmp_smoke::all_passed(&steps) {
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    wasm_bindgen_futures::spawn_local(async {
        let steps = wcmp_smoke::run().await;
        for line in wcmp_smoke::render(&steps).lines() {
            web::report(line);
        }
    });
}

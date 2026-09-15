//! The smoke test entry point. Natively it prints the report and exits
//! non-zero on a failure. Under `wasm-bindgen` the page's `report`
//! function receives each line instead; `main` runs on instantiation.

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(js_namespace = window)]
    extern "C" {
        pub fn report(line: &str);
    }
}

fn main() {
    let steps = wcmp_smoke::run();
    let report = wcmp_smoke::render(&steps);

    #[cfg(not(target_arch = "wasm32"))]
    {
        print!("{report}");
        if !wcmp_smoke::all_passed(&steps) {
            std::process::exit(1);
        }
    }

    #[cfg(target_arch = "wasm32")]
    for line in report.lines() {
        web::report(line);
    }
}

//! The smoke test entry point. Natively it prints the report a line at
//! a time and exits non-zero on a failure. Under `wasm-bindgen` the
//! page's `window.smoke` object receives each chapter and story
//! instead; `main` runs on instantiation and schedules the
//! asynchronous run on the browser's event loop.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use wcmp_smoke::{Reporter, Step};

    /// Prints the report as it is told, in the transcript's shape.
    pub struct Transcript;

    impl Reporter for Transcript {
        fn begin(&mut self, _total: usize) {}

        fn chapter(&mut self, chapter: &'static str) {
            println!("{}", wcmp_smoke::chapter_line(chapter));
        }

        fn step(&mut self, step: &Step) {
            println!("{}", step.line());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let steps = wcmp_smoke::run(&mut native::Transcript).await;
    println!("{}", wcmp_smoke::summary(&steps));
    if !wcmp_smoke::all_passed(&steps) {
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;
    use wcmp_smoke::{Reporter, Step};

    // The page's side of the report, which `web/boot.js` defines
    // before it instantiates the module.
    #[wasm_bindgen(js_namespace = ["window", "smoke"])]
    extern "C" {
        pub fn begin(total: u32);
        pub fn chapter(chapter: &str, line: &str);
        pub fn step(
            chapter: &str,
            title: &str,
            goal: &str,
            label: &str,
            detail: &str,
            elapsed: &str,
            line: &str,
        );
        pub fn finish(line: &str, passed: u32, failed: u32, skipped: u32);
    }

    /// Hands each chapter and story to the page as it completes.
    pub struct Page;

    impl Reporter for Page {
        fn begin(&mut self, total: usize) {
            begin(total as u32);
        }

        fn chapter(&mut self, chapter: &'static str) {
            self::chapter(chapter, &wcmp_smoke::chapter_line(chapter));
        }

        fn step(&mut self, step: &Step) {
            self::step(
                step.story.chapter,
                step.story.title,
                step.story.goal,
                step.outcome.label(),
                step.outcome.detail(),
                &step.elapsed(),
                &step.line(),
            );
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    wasm_bindgen_futures::spawn_local(async {
        let steps = wcmp_smoke::run(&mut web::Page).await;
        let (passed, failed, skipped) = wcmp_smoke::counts(&steps);
        web::finish(
            &wcmp_smoke::summary(&steps),
            passed as u32,
            failed as u32,
            skipped as u32,
        );
    });
}

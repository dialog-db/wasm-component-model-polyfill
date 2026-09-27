//! A component with a bug, built by `cargo` and wit-bindgen.
//!
//! `average` divides by the number of values it was given and never
//! checks that there were any. Rust turns the divide by zero into a
//! panic, and the release profile aborts on a panic, which in a
//! WebAssembly guest is an `unreachable` trap.

wit_bindgen::generate!({
    path: "../wit",
    world: "stats",
});

struct Stats;

impl Guest for Stats {
    fn sum(values: Vec<u32>) -> u32 {
        values.into_iter().fold(0, u32::wrapping_add)
    }

    fn average(values: Vec<u32>) -> u32 {
        let count = values.len() as u32;
        Self::sum(values) / count
    }
}

export!(Stats);

//! Scenario 10, the exporter: a Rust partner, built by `cargo` and
//! wit-bindgen's async support, that implements the `greeter` interface
//! `importer.zena` imports. The runner links the two at run time.

wit_bindgen::generate!({
    path: "../wit",
    world: "exporter",
});

use exports::wcmp::rust_link::greeter::Guest;

struct Exporter;

impl Guest for Exporter {
    /// A greeting that names Rust, so the importer's answer shows which
    /// component made it.
    async fn greet(name: String) -> String {
        format!("hello from Rust, {name}")
    }
}

export!(Exporter);

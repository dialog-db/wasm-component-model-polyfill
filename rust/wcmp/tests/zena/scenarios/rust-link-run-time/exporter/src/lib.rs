//! Scenario 14, the exporter: a Rust partner, built by `cargo` and
//! wit-bindgen's async support, that implements the `greeter` interface
//! the Rust importer imports. The runner links the two at run time.

wit_bindgen::generate!({
    path: "../wit",
    world: "exporter",
});

use exports::wcmp::rust_link::greeter::Guest;

struct Exporter;

impl Guest for Exporter {
    /// A greeting that names the exporter, so the importer's answer
    /// shows that the call crossed the link.
    async fn greet(name: String) -> String {
        format!("hello from the Rust exporter, {name}")
    }
}

export!(Exporter);

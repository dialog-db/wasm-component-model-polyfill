//! Scenario 12, the exporter: a Rust partner, built by `cargo` and
//! wit-bindgen, that implements the `greeter` interface `importer.zena`
//! imports. The build composes the two with `wac`.

wit_bindgen::generate!({
    path: "../wit",
    world: "exporter",
});

use exports::wcmp::rust_compose::greeter::Guest;

struct Exporter;

impl Guest for Exporter {
    /// A greeting that names Rust, so the importer's answer shows which
    /// component made it.
    fn greet(name: String) -> String {
        format!("hello from Rust, {name}")
    }
}

export!(Exporter);

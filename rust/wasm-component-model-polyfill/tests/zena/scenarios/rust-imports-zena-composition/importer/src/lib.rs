//! Scenario 13, the importer: a Rust partner, built by `cargo` and
//! wit-bindgen, whose `welcome` passes its string to the Zena exporter's
//! `greet` inside the composition and passes the answer back. The build
//! composes the two with `wac`.

wit_bindgen::generate!({
    path: "../wit",
    world: "importer",
});

use wcmp::rust_compose::greeter::greet;

struct Importer;

impl Guest for Importer {
    fn welcome(name: String) -> String {
        let greeting = greet(&name);
        format!("{greeting}!")
    }
}

export!(Importer);

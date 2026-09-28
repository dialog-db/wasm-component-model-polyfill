//! Scenario 14, the importer: a Rust partner, built by `cargo` and
//! wit-bindgen's async support, whose `welcome` passes its string to
//! the Rust exporter's `greet` across the link and passes the answer
//! back. The runner links the two at run time.

wit_bindgen::generate!({
    path: "../wit",
    world: "importer",
});

use wcmp::rust_link::greeter::greet;

struct Importer;

impl Guest for Importer {
    async fn welcome(name: String) -> String {
        let greeting = greet(name).await;
        format!("{greeting}!")
    }
}

export!(Importer);

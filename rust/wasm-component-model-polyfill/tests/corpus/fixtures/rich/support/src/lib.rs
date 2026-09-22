//! The support component of the `rich` fixture: the far side of every
//! shape the guest imports, and the component that owns the `tally`
//! resource. Built by `cargo` and `wasm-tools component new`, so the
//! binding layer is wit-bindgen's own lift and lower code over the
//! allocator's `cabi_realloc`.

use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

wit_bindgen::generate!({
    path: "../wit",
    world: "support",
});

use exports::wcmp::rich::host_ops::{Guest, GuestTally};
use wcmp::rich::shapes::{Colour, Drawing, Failure, Label, Outline, Point, Style};

/// How many `tally` destructors have run. The guest reads it back
/// through `tally-drops`, so a destructor that never ran shows up as
/// a wrong number rather than as silence.
static TALLY_DROPS: AtomicU32 = AtomicU32::new(0);

struct Support;

/// The resource the guest holds a handle to. Its methods take
/// `&self`, so the running total lives in a `Cell`.
struct Tally {
    total: Cell<u32>,
}

impl Drop for Tally {
    fn drop(&mut self) {
        TALLY_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

impl GuestTally for Tally {
    fn new(seed: u32) -> Self {
        Tally {
            total: Cell::new(seed),
        }
    }

    fn add(&self, amount: u32) {
        self.total.set(self.total.get().wrapping_add(amount));
    }

    fn total(&self) -> u32 {
        self.total.get()
    }
}

impl Guest for Support {
    type Tally = Tally;

    /// Rewrite every field of the drawing, so each shape crosses the
    /// boundary in both directions and comes back changed in a way
    /// an assertion can name.
    fn transform(picture: Drawing) -> Drawing {
        Drawing {
            outline: match picture.outline {
                Outline::Empty => Outline::Empty,
                Outline::Dot(point) => Outline::Dot(Point {
                    x: point.y,
                    y: point.x,
                }),
                Outline::Path(points) => {
                    Outline::Path(points.into_iter().rev().collect::<Vec<_>>())
                }
                Outline::Tagged(label) => Outline::Tagged(Label {
                    name: label.name.to_ascii_uppercase(),
                    tags: label.tags.into_iter().rev().collect::<Vec<_>>(),
                }),
            },
            colour: match picture.colour {
                Colour::Red => Colour::Green,
                Colour::Green => Colour::Blue,
                Colour::Blue => Colour::Red,
            },
            style: picture.style ^ Style::BOLD,
            title: picture.title.map(|title| title + "!"),
            rows: picture
                .rows
                .into_iter()
                .map(|row| row.into_iter().rev().collect::<Vec<_>>())
                .collect(),
        }
    }

    /// Both arms of the `result`, and both arms of the `failure`
    /// variant, so an assertion can reach each one.
    fn measure(shape: Outline) -> Result<Point, Failure> {
        match shape {
            Outline::Empty => Err(Failure::Blank),
            Outline::Dot(point) => Ok(point),
            Outline::Path(points) if points.is_empty() => Err(Failure::Blank),
            Outline::Path(points) => Ok(Point {
                x: points.iter().map(|point| point.x).sum(),
                y: points.iter().map(|point| point.y).sum(),
            }),
            Outline::Tagged(label) if label.tags.is_empty() => {
                Err(Failure::Unpaintable(Colour::Red))
            }
            Outline::Tagged(label) => Ok(Point {
                x: label.name.len() as i32,
                y: label.tags.len() as i32,
            }),
        }
    }

    fn describe(colour: Colour, style: Style, title: Option<String>) -> String {
        let mut out = String::new();
        out.push_str(match colour {
            Colour::Red => "red",
            Colour::Green => "green",
            Colour::Blue => "blue",
        });
        out.push('[');
        let mut named = Vec::new();
        if style.contains(Style::BOLD) {
            named.push("bold");
        }
        if style.contains(Style::ITALIC) {
            named.push("italic");
        }
        if style.contains(Style::UNDERLINE) {
            named.push("underline");
        }
        if named.is_empty() {
            out.push_str("plain");
        } else {
            out.push_str(&named.join(","));
        }
        out.push_str("] ");
        out.push_str(title.as_deref().unwrap_or("untitled"));
        out
    }

    /// One sum per row, then the sum of the sums, so a nested list
    /// goes in and a flat one comes out.
    fn fold(rows: Vec<Vec<u32>>) -> Vec<u32> {
        let mut out: Vec<u32> = rows
            .into_iter()
            .map(|row| row.into_iter().fold(0u32, u32::wrapping_add))
            .collect();
        let total = out.iter().copied().fold(0u32, u32::wrapping_add);
        out.push(total);
        out
    }

    fn tally_drops() -> u32 {
        TALLY_DROPS.load(Ordering::Relaxed)
    }
}

export!(Support);

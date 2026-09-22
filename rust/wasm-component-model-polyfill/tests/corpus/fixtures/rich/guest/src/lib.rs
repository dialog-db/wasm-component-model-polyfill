//! The guest of the `rich` fixture: it imports the support
//! component's shapes and its `tally` resource, and exports its own
//! `counter` resource beside the functions the driver forwards.
//! Built by `cargo` and `wasm-tools component new`, so the binding
//! layer is wit-bindgen's own lift and lower code over the
//! allocator's `cabi_realloc`.

use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

wit_bindgen::generate!({
    path: "../wit",
    world: "guest",
});

use exports::wcmp::rich::counters::{CounterBorrow, Guest as CountersGuest, GuestCounter};
use wcmp::rich::host_ops::{self, Tally};
// `world guest` names the shapes with `use`, so wit-bindgen puts
// them at the root of the generated module already.

/// How many `counter` destructors have run, read back through
/// `counters.drops`.
static COUNTER_DROPS: AtomicU32 = AtomicU32::new(0);

struct Fixture;

/// The resource the guest defines. Its methods take `&self`, so the
/// running value lives in a `Cell`.
struct Counter {
    value: Cell<u32>,
}

impl Drop for Counter {
    fn drop(&mut self) {
        COUNTER_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

impl GuestCounter for Counter {
    fn new(start: u32) -> Self {
        Counter {
            value: Cell::new(start),
        }
    }

    fn bump(&self, amount: u32) -> u32 {
        self.value.set(self.value.get().wrapping_add(amount));
        self.value.get()
    }

    fn value(&self) -> u32 {
        self.value.get()
    }

    fn sum(first: CounterBorrow<'_>, second: CounterBorrow<'_>) -> u32 {
        let first: &Counter = first.get();
        let second: &Counter = second.get();
        first.value.get().wrapping_add(second.value.get())
    }
}

impl CountersGuest for Fixture {
    type Counter = Counter;

    fn drops() -> u32 {
        COUNTER_DROPS.load(Ordering::Relaxed)
    }
}

impl Guest for Fixture {
    fn round_trip(picture: Drawing) -> Drawing {
        host_ops::transform(&picture)
    }

    fn measure_all(shapes: Vec<Outline>) -> Vec<Result<Point, Failure>> {
        shapes.iter().map(host_ops::measure).collect()
    }

    /// The guest decides the failure arms itself and asks the support
    /// component for the string, so both arms of the `result` and a
    /// string built on the far side reach the caller.
    fn paint(picture: Drawing) -> Result<String, Failure> {
        if matches!(picture.outline, Outline::Empty) {
            return Err(Failure::Blank);
        }
        if picture.colour == Colour::Blue && picture.style.contains(Style::UNDERLINE) {
            return Err(Failure::Unpaintable(Colour::Blue));
        }
        Ok(host_ops::describe(
            picture.colour,
            picture.style,
            picture.title.as_deref(),
        ))
    }

    fn describe(colour: Colour, style: Style, title: Option<String>) -> String {
        host_ops::describe(colour, style, title.as_deref())
    }

    fn fold(rows: Vec<Vec<u32>>) -> Vec<u32> {
        host_ops::fold(&rows)
    }

    /// Drives the support component's resource across the boundary:
    /// two constructors, a method per step, a method to read each
    /// total back, and two handle drops that run the destructor on
    /// the far side.
    fn exercise_tallies(steps: Vec<u32>) -> u32 {
        let first = Tally::new(0);
        let second = Tally::new(100);
        for (index, step) in steps.iter().enumerate() {
            if index % 2 == 0 {
                first.add(*step);
            } else {
                second.add(*step);
            }
        }
        let total = first.total().wrapping_add(second.total());
        drop(first);
        drop(second);
        total
    }

    fn tally_drops() -> u32 {
        host_ops::tally_drops()
    }
}

export!(Fixture);

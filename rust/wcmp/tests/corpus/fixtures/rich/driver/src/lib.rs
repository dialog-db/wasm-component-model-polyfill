// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The driver of the `rich` fixture: the component the assertions
//! reach. It holds no logic of its own. Every value it answers with
//! has crossed the driver-to-guest boundary and the guest-to-support
//! boundary and come back, and the counter calls drive the guest's
//! exported resource from outside the component that defines it.

wit_bindgen::generate!({
    path: "../wit",
    world: "driver",
});

use wcmp::rich::counters::{self, Counter};
// `world driver` names the shapes with `use`, so wit-bindgen puts
// them at the root of the generated module already.

struct Driver;

impl Guest for Driver {
    fn round_trip(picture: Drawing) -> Drawing {
        crate::round_trip(&picture)
    }

    fn measure_all(shapes: Vec<Outline>) -> Vec<Result<Point, Failure>> {
        crate::measure_all(&shapes)
    }

    fn paint(picture: Drawing) -> Result<String, Failure> {
        crate::paint(&picture)
    }

    fn describe(colour: Colour, style: Style, title: Option<String>) -> String {
        crate::describe(colour, style, title.as_deref())
    }

    fn fold(rows: Vec<Vec<u32>>) -> Vec<u32> {
        crate::fold(&rows)
    }

    fn exercise_tallies(steps: Vec<u32>) -> u32 {
        crate::exercise_tallies(&steps)
    }

    fn tally_drops() -> u32 {
        crate::tally_drops()
    }

    /// The guest's own resource, driven from another component: two
    /// constructors, a method per step, the static method over two
    /// borrows, and two handle drops that run the guest's destructor.
    fn exercise_counters(steps: Vec<u32>) -> u32 {
        let first = Counter::new(0);
        let second = Counter::new(100);
        for (index, step) in steps.iter().enumerate() {
            if index % 2 == 0 {
                first.bump(*step);
            } else {
                second.bump(*step);
            }
        }
        let total = Counter::sum(&first, &second)
            .wrapping_add(first.value())
            .wrapping_sub(second.value());
        drop(first);
        drop(second);
        total
    }

    fn counter_drops() -> u32 {
        counters::drops()
    }
}

export!(Driver);

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Step;

/// Where the smoke test tells its report as it runs: a line at a time
/// natively, a row at a time on the page. `run` calls `begin` once
/// with the number of stories, `chapter` each time the chapter
/// changes, and `step` after every story, in order.
pub trait Reporter {
    fn begin(&mut self, total: usize);
    fn chapter(&mut self, chapter: &'static str);
    fn step(&mut self, step: &Step);
}

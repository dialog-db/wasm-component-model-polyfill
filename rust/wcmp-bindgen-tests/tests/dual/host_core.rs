// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The host core: the logic of the host of `test:dual/dual`, with no
//! runtime type in it. Each glue module converts its runtime's
//! generated types to these and calls the core.

/// A shape, as the core reads one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Circle(u32),
    Square(u32, u32),
    None,
}

/// The host's counter, which `with` maps the WIT `counter` to.
pub struct Counter {
    pub value: u32,
}

/// An error of the core's own, which `checked` fails with.
#[derive(Debug, PartialEq, Eq)]
pub struct CheckFailed {
    pub value: u32,
}

impl core::fmt::Display for CheckFailed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "the core refused {}", self.value)
    }
}

impl std::error::Error for CheckFailed {}

/// The core's error of `read`, which `trappable_error_type` maps the
/// WIT `fs-error` to.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Converts to the WIT `not-found`.
    Missing,
    /// Has no WIT error, and traps.
    Broken,
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ReadError {}

/// The core: what it was given, in order, which a run records.
#[derive(Default)]
pub struct Core {
    pub log: Vec<String>,
}

impl Core {
    /// The point a shape and its permissions describe.
    pub fn describe(&mut self, shape: Shape, read: bool, write: bool) -> (u32, u32) {
        let point = match shape {
            Shape::Circle(r) => (r * r, u32::from(read) + u32::from(write)),
            Shape::Square(x, y) => (x * y, 0),
            Shape::None => (0, 0),
        };
        self.log.push(format!(
            "describe {shape:?} read={read} write={write} -> {point:?}"
        ));
        point
    }

    /// `x`, or the core's refusal of zero.
    pub fn checked(&mut self, x: u32) -> Result<u32, CheckFailed> {
        self.log.push(format!("checked {x}"));
        if x == 0 {
            Err(CheckFailed { value: x })
        } else {
            Ok(x + 1)
        }
    }

    /// A value, a missing file, or a broken one.
    pub fn read(&mut self, code: u32) -> Result<u32, ReadError> {
        self.log.push(format!("read {code}"));
        match code {
            0 => Ok(7),
            1 => Err(ReadError::Missing),
            _ => Err(ReadError::Broken),
        }
    }

    /// The answer to `x`, which an async import awaits.
    pub fn answer(&mut self, x: u32) -> u32 {
        self.log.push(format!("answer {x}"));
        x * 2
    }

    /// The value of the future `lost` returns.
    pub fn lost(&mut self) -> u32 {
        self.log.push("lost".to_owned());
        99
    }

    /// A new counter.
    pub fn counter(&mut self, start: u32) -> Counter {
        self.log.push(format!("counter {start}"));
        Counter { value: start }
    }

    /// Record that the guest dropped a counter.
    pub fn dropped(&mut self, counter: &Counter) {
        self.log.push(format!("dropped {}", counter.value));
    }
}

/// A future that is pending the first time it is polled, as a host
/// call that does not finish where it starts.
#[derive(Default)]
pub struct PendingOnce {
    polled: bool,
}

impl core::future::Future for PendingOnce {
    type Output = ();

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        let this = self.get_mut();
        if this.polled {
            return core::task::Poll::Ready(());
        }
        this.polled = true;
        cx.waker().wake_by_ref();
        core::task::Poll::Pending
    }
}

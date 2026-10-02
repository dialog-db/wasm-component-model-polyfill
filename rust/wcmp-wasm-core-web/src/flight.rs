// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A call into a guest that runs on after the code that started it
//! returned.

use core::cell::{Cell, RefCell};
use core::future::{Future, poll_fn};
use core::task::{Context, Poll, Waker};
use std::rc::{Rc, Weak};

use js_sys::{Function, Promise};
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

use crate::calls::Calls;
use crate::cell::StoreCell;
use crate::js;

/// A call into a guest that runs on after the code that started it
/// returned: a resumable call, whose stack JavaScript Promise Integration
/// resumes on a microtask, or an instantiation, whose start function the
/// browser can run after `WebAssembly.instantiate` returned.
///
/// A flight reaches its store through the store's cell, and not through a
/// pointer that a borrow of the store made. So the store is where the
/// flight left it whatever became of the code that started the flight.
/// The flight may reach the store only while its permit holds (see
/// [`Flight::may_run`]), and it keeps the store's cell alive from the
/// moment it starts to run until its next stop, so a store that drops
/// meanwhile keeps its state until then. The stack of a resumable call that
/// would reach the store without a permit parks instead, and keeps the cell
/// no longer: it waits where it resumed until a wait grants it the store
/// again, and runs on from there.
pub struct Flight {
    /// The number of the flight. The frames of the host functions that its
    /// own guest calls carry it.
    id: u64,
    /// Whether the flight is a resumable call, which may suspend.
    resumable: bool,
    /// What the host functions of the store share.
    calls: Weak<Calls>,
    state: RefCell<State>,
    /// The waker of the code that waits for the flight's next stop.
    waker: RefCell<Option<Waker>>,
    /// The epoch of the store when the flight was last started or resumed.
    permit: Cell<u64>,
    /// Whether the code that waited for the flight dropped its future
    /// before the flight stopped.
    revoked: Cell<bool>,
    /// The store's cell, from the moment the flight runs until it stops.
    keep: RefCell<Option<Rc<StoreCell>>>,
    /// The error of the host function that trapped the flight.
    error: RefCell<Option<anyhow::Error>>,
}

/// Where a flight is.
enum State {
    /// It runs, or its stack waits for the microtask that resumes it.
    Running,
    /// Its stack waits in a suspending host function, whose frame is
    /// `frame`. `resolve` resumes it.
    Suspended { frame: u32, resolve: Function },
    /// Its resumed stack reached the store after the host took the store
    /// back, and is on its way to wait again in the frame it resumed from.
    Parking,
    /// Its resumed stack waits again in the suspending host function whose
    /// frame is `frame`, because it reached the store after the host took
    /// the store back, until a wait takes the flight up. `resolve` lets it
    /// run on. This is no stop: the call has not reached its next
    /// suspension.
    Parked { frame: u32, resolve: Function },
    /// It returned this value.
    Returned(JsValue),
    /// It failed with this reason.
    Failed(JsValue),
    /// Its end was taken.
    Ended,
}

/// Where a flight stopped.
pub enum Stop {
    /// Its stack waits in a suspending host function.
    Suspended,
    /// It returned this value: the results of the call, or the instance.
    Returned(JsValue),
    /// It failed with `reason`, and `host` is the error of the host
    /// function that trapped it, where one did.
    Failed {
        reason: JsValue,
        host: Option<anyhow::Error>,
    },
}

impl Flight {
    /// A new flight numbered `id` of the store whose host functions share
    /// `calls`, a resumable call where `resumable`. It runs.
    pub fn new(id: u64, resumable: bool, calls: &Rc<Calls>) -> Rc<Self> {
        Rc::new(Self {
            id,
            resumable,
            calls: Rc::downgrade(calls),
            state: RefCell::new(State::Running),
            waker: RefCell::new(None),
            permit: Cell::new(0),
            revoked: Cell::new(false),
            keep: RefCell::new(None),
            error: RefCell::new(None),
        })
    }

    /// The number of the flight.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether the flight is a resumable call, which may suspend.
    pub fn resumable(&self) -> bool {
        self.resumable
    }

    /// Whether the flight is one of the store whose host functions share
    /// `calls`.
    pub fn belongs_to(&self, calls: &Rc<Calls>) -> bool {
        Weak::ptr_eq(&self.calls, &Rc::downgrade(calls))
    }

    /// Grants the flight the store at `epoch`, and keeps `cell` alive
    /// until the flight stops.
    pub fn grant(&self, epoch: u64, cell: Option<Rc<StoreCell>>) {
        self.permit.set(epoch);
        self.revoked.set(false);
        *self.keep.borrow_mut() = cell;
    }

    /// Whether the flight may reach its store now.
    ///
    /// It may where the store's owner dropped the store, since nothing
    /// else can then reach it. Otherwise it may only while the code that
    /// started, resumed, or last took it up still waits for it, and the
    /// owner has not reached the store since. Each method of the owner moves the store's
    /// epoch on, so a flight whose permit is an earlier epoch finds that
    /// the host took the store back. The owner also refuses the store while
    /// a host function that a flight called runs. So the owner's references
    /// to the store never meet the flight's.
    pub fn may_run(&self, epoch: u64, owner_dropped: bool) -> bool {
        owner_dropped || (!self.revoked.get() && self.permit.get() == epoch)
    }

    /// Records `error`, the error of the host function that trapped the
    /// flight, for the failure its stack reports.
    pub fn fail(&self, error: anyhow::Error) {
        *self.error.borrow_mut() = Some(error);
    }

    /// Records that the stack suspended in the host function whose frame
    /// is `frame`, until `resolve` resumes it, and wakes the code that
    /// waits. It answers the store's cell, which the flight no longer
    /// keeps, so that the caller frees it where no code of the store runs.
    ///
    /// A flight on its way to park (see [`Flight::park`]) waits there
    /// instead: that is no stop, and nothing wakes.
    pub fn suspend(&self, frame: u32, resolve: Function) -> Option<Rc<StoreCell>> {
        let mut state = self.state.borrow_mut();
        if matches!(*state, State::Parking) {
            *state = State::Parked { frame, resolve };
        } else {
            *state = State::Suspended { frame, resolve };
            drop(state);
            self.wake();
        }
        self.keep.borrow_mut().take()
    }

    /// Records that the resumed stack reached the store after the host took
    /// the store back, and goes to wait again in the frame it resumed from,
    /// until a wait takes the flight up and [`Flight::unpark`] lets it run
    /// on. Only a resumable call parks: its stack can wait.
    pub fn park(&self) {
        *self.state.borrow_mut() = State::Parking;
    }

    /// The resolver of the parked stack, which runs on from here, as it
    /// would have had the host not taken the store back.
    pub fn unpark(&self) -> Option<Function> {
        let mut state = self.state.borrow_mut();
        match core::mem::replace(&mut *state, State::Running) {
            State::Parked { resolve, .. } => Some(resolve),
            other => {
                *state = other;
                None
            }
        }
    }

    /// Whether the flight has yet to reach its next stop: it runs, or it
    /// is parked.
    pub fn runs(&self) -> bool {
        matches!(
            *self.state.borrow(),
            State::Running | State::Parking | State::Parked { .. }
        )
    }

    /// The frame and the resolver of the suspended stack, which runs
    /// again from here on.
    pub fn resume(&self) -> Option<(u32, Function)> {
        let mut state = self.state.borrow_mut();
        match core::mem::replace(&mut *state, State::Running) {
            State::Suspended { frame, resolve } => Some((frame, resolve)),
            other => {
                *state = other;
                None
            }
        }
    }

    /// The frame of the suspended stack.
    pub fn frame(&self) -> Option<u32> {
        match *self.state.borrow() {
            State::Suspended { frame, .. } => Some(frame),
            _ => None,
        }
    }

    /// Watches `promise`, the promise of the flight's end: its stack's
    /// promise, or the instantiation's. When it settles, the flight ends.
    ///
    /// The handler holds the flight weakly: a flight that nothing holds
    /// never ends, and its handler is never called.
    pub fn watch(self: &Rc<Self>, promise: &Promise) {
        let (returned, failed) = (Rc::downgrade(self), Rc::downgrade(self));
        let returned = Closure::once_into_js(move |value: JsValue| {
            end(&returned, State::Returned(value));
        });
        let failed = Closure::once_into_js(move |reason: JsValue| {
            end(&failed, State::Failed(reason));
        });
        // `then` never throws for a promise.
        let _ = js::call_method(promise, "then", &[returned, failed]);
    }

    /// Ends the flight at once: its call returned `value` in its first
    /// stretch, and the store read the return without its promise, which
    /// nothing then watches.
    ///
    /// This runs inside a method of the store, while the owner of the
    /// store, or a flight whose host function runs, keeps the store's cell.
    /// So the cell that this flight kept is not the last, and the store
    /// does not drop here.
    pub fn returned(self: &Rc<Self>, value: JsValue) {
        end(&Rc::downgrade(self), State::Returned(value));
    }

    /// The flight's next stop.
    ///
    /// Where the future drops before the flight stops, the flight loses
    /// its permit: the host has its store back. The stack of a resumable
    /// call parks the next time it would reach the store, until a later
    /// wait grants it the store again, and the start function of an
    /// instantiation traps there. Where the store drops first, the flight
    /// runs on with the store, since nothing else can reach it.
    pub fn stop(self: &Rc<Self>) -> impl Future<Output = Stop> + use<> {
        let watch = Watch(self.clone());
        poll_fn(move |context| watch.0.poll_stop(context))
    }

    /// The flight's stop, where it stopped.
    fn poll_stop(&self, context: &mut Context<'_>) -> Poll<Stop> {
        let mut state = self.state.borrow_mut();
        match &*state {
            State::Running | State::Parking | State::Parked { .. } => {
                *self.waker.borrow_mut() = Some(context.waker().clone());
                Poll::Pending
            }
            State::Suspended { .. } => Poll::Ready(Stop::Suspended),
            State::Returned(_) | State::Failed(_) | State::Ended => {
                Poll::Ready(match core::mem::replace(&mut *state, State::Ended) {
                    State::Returned(value) => Stop::Returned(value),
                    State::Failed(reason) => Stop::Failed {
                        reason,
                        host: self.error.borrow_mut().take(),
                    },
                    _ => Stop::Failed {
                        reason: JsValue::from_str("the call already ended"),
                        host: None,
                    },
                })
            }
        }
    }

    /// Wakes the code that waits for the flight.
    fn wake(&self) {
        if let Some(waker) = self.waker.borrow_mut().take() {
            waker.wake();
        }
    }
}

/// Ends the flight `flight`, whose promise settled, at `state`.
///
/// This runs as the handler of a promise, where no code of the store
/// runs, so the store may drop here, or from [`Flight::returned`].
fn end(flight: &Weak<Flight>, state: State) {
    let Some(flight) = flight.upgrade() else {
        return;
    };
    *flight.state.borrow_mut() = state;
    if let Some(calls) = flight.calls.upgrade() {
        calls.ended(&flight);
    }
    let keep = flight.keep.borrow_mut().take();
    flight.wake();
    drop(flight);
    drop(keep);
}

impl Drop for Flight {
    fn drop(&mut self) {
        // A suspended or parked stack that nothing can resume any more: its
        // frame closes, and the stack never runs again.
        if let State::Suspended { frame, .. } | State::Parked { frame, .. } = *self.state.get_mut()
            && let Some(calls) = self.calls.upgrade()
        {
            calls.close(frame);
        }
    }
}

/// The watch of a future that waits for a flight's stop, which revokes
/// the flight's permit where the future drops first.
struct Watch(Rc<Flight>);

impl Drop for Watch {
    fn drop(&mut self) {
        if matches!(*self.0.state.borrow(), State::Running) {
            self.0.revoked.set(true);
        }
    }
}

/// Frees `cell` on a microtask, where no code of the store runs.
///
/// A flight that suspends stops keeping its store's cell from inside a
/// JavaScript function of the store. Where the cell is the last, the store
/// would drop that function while it runs, so the cell drops later.
pub fn release_later(cell: Option<Rc<StoreCell>>) {
    let Some(cell) = cell else {
        return;
    };
    let release = Closure::once_into_js(move || drop(cell));
    // `then` never throws for a promise, and holds the handler until it
    // runs.
    let _ = js::call_method(&Promise::resolve(&JsValue::UNDEFINED), "then", &[release]);
}

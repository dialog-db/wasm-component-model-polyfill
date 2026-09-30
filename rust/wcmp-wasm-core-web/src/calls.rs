//! What the host functions of a store share with the store while a guest
//! runs in it.

use core::cell::{Cell, RefCell};
use core::task::Poll;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};

use js_sys::Promise;
use wasm_bindgen::JsValue;
use wcmp_wasm_core::backend::HostFunc;
use wcmp_wasm_core::{FuncType, Result};

use crate::cell::StoreCell;
use crate::entry::Entry;
use crate::errors;
use crate::flight::{self, Flight};
use crate::store::WebStore;

/// What the host functions of one store share with the store: where a
/// guest runs in the store, each host function, a frame for each call of
/// a host function, and the flights of the store.
///
/// A guest calls a host function through its wrapper module, which reaches
/// the host through JavaScript functions: `enter`, `arg`, `invoke`,
/// `result`, and `leave`, and for a suspending host function `suspend` and
/// `resumed`. Each is a method here. A call of a host function opens a
/// frame of its own, which holds its arguments and its results, so no call
/// shares a buffer with another. A frame outlives the stack that holds it
/// where the stack suspends, so the frames are numbered, and not a stack.
///
/// A host function reaches the store in one of two ways:
///
/// - Through a lease, while a synchronous call from the store into a guest
///   runs: a method of the store calls [`Calls::enter`] with the store it
///   borrows, and the lease ends before the method touches the store
///   again. The leases nest, innermost last.
/// - As a [`Flight`], a call that runs on a microtask after the code that
///   started it returned: the resumed stack of a resumable call, or the
///   start function of an instantiation. A flight reaches the store
///   through the store's cell, and only while its permit holds.
///
/// While a host function that a flight calls runs, the flight's reference
/// to the store lives, and nothing else may reach the store. A host
/// function can hold its own store by another path than its caller, such
/// as a global, once the future that waited for the flight was forgotten
/// and no longer borrows the store. So the store's owner asks
/// [`Calls::claim`] before it reaches the store, and the claim fails while
/// such a host function runs.
pub struct Calls {
    /// The cell that owns the store.
    cell: RefCell<Weak<StoreCell>>,
    /// The type and the body of each host function, by its index.
    funcs: RefCell<Vec<Rc<(FuncType, HostFunc)>>>,
    /// The frame of each call of a host function that runs or waits.
    frames: RefCell<Frames>,
    /// The synchronous calls from the store into a guest that run now,
    /// innermost last.
    leases: RefCell<Vec<Lease>>,
    /// The resumable call whose stack runs now on a microtask, from the
    /// moment it resumes until it stops.
    current: RefCell<Option<Rc<Flight>>>,
    /// The instantiations whose promises have not settled, oldest first.
    /// The browser runs their start functions in this order.
    instantiations: RefCell<VecDeque<Rc<Flight>>>,
    /// Each flight that runs, kept until it stops, whether or not code
    /// still waits for it.
    running: RefCell<HashMap<u64, Rc<Flight>>>,
    /// The number of times the store's owner reached the store. A flight
    /// may reach the store only at the epoch of its permit.
    epoch: Cell<u64>,
    /// The number of calls of a host function that a flight made and that
    /// run now. While one runs, the store's owner may not reach the store.
    hosting: Cell<u32>,
    /// Whether the store's owner dropped the store.
    owner_dropped: Cell<bool>,
    /// The number of the next lease or flight. `0` numbers none.
    next_id: Cell<u64>,
}

impl Default for Calls {
    fn default() -> Self {
        Self::new()
    }
}

/// A synchronous call from the store into a guest, while it runs.
struct Lease {
    /// The number of the lease, or of the flight it starts.
    id: u64,
    /// The store, as the method that made the call borrows it.
    store: *mut WebStore,
    /// The flight whose first stretch the call runs, where it starts one.
    flight: Option<Rc<Flight>>,
    /// The error of the host function that trapped the call.
    error: Option<anyhow::Error>,
}

/// The frame of one call of a host function.
struct Frame {
    /// The number of the lease or the flight whose guest made the call.
    owner: u64,
    /// The flight whose stack waits in this call, while it waits.
    parked: Option<Weak<Flight>>,
    /// The index of the host function.
    func: u32,
    /// The arguments, as the wrapper carries them.
    args: Vec<JsValue>,
    /// The results, as the wrapper carries them, once the host function
    /// returned or the host resumed the call.
    results: Vec<JsValue>,
}

/// The frames of a store, by their numbers.
#[derive(Default)]
struct Frames {
    slots: Vec<Option<Frame>>,
    free: Vec<u32>,
}

impl Frames {
    fn insert(&mut self, frame: Frame) -> u32 {
        match self.free.pop() {
            Some(index) => {
                self.slots[index as usize] = Some(frame);
                index
            }
            None => {
                self.slots.push(Some(frame));
                (self.slots.len() - 1) as u32
            }
        }
    }

    fn get_mut(&mut self, index: u32) -> Option<&mut Frame> {
        self.slots.get_mut(index as usize)?.as_mut()
    }

    fn remove(&mut self, index: u32) -> Option<Frame> {
        let frame = self.slots.get_mut(index as usize)?.take()?;
        self.free.push(index);
        Some(frame)
    }

    /// Closes every frame of `owner` whose stack does not wait in it.
    fn close_owned(&mut self, owner: u64) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot
                .as_ref()
                .is_some_and(|frame| frame.owner == owner && frame.parked.is_none())
            {
                *slot = None;
                self.free.push(index as u32);
            }
        }
    }
}

/// Where the host function that a guest calls now reaches the store.
enum Reach {
    /// Through the innermost lease: numbered `id`, over `store`, where a
    /// suspension is allowed where `suspends`.
    Lease {
        id: u64,
        store: *mut WebStore,
        suspends: bool,
    },
    /// As this flight.
    Flight(Rc<Flight>),
    /// Nowhere: no call of the store runs.
    Nowhere,
}

impl Reach {
    /// The number of the lease or the flight.
    fn id(&self) -> u64 {
        match self {
            Reach::Lease { id, .. } => *id,
            Reach::Flight(flight) => flight.id(),
            Reach::Nowhere => 0,
        }
    }
}

/// The status that `invoke` and `resumed` return to the wrapper where the
/// call goes on.
const RETURNED: i32 = 0;

/// The status that `invoke` and `resumed` return to the wrapper where the
/// call failed. The wrapper then traps.
const FAILED: i32 = 1;

/// The status that `invoke` returns to the wrapper of a suspending host
/// function that answered "not yet" where the call may suspend, and that
/// `resumed` returns where the resumed call parks. The wrapper then calls
/// `suspend`.
const SUSPENDED: i32 = 2;

/// The error of a flight that reaches its store after the host took the
/// store back.
const TAKEN_BACK: &str = "the host took the store back before this call reached it: the future \
                          that waited for the call dropped, or the host used the store since";

/// The error of the store's owner where a host function that a flight
/// called runs, and holds the store through its caller.
const HOSTING: &str = "a host function of a call that runs on its own holds the store: it reaches \
                       the store through its caller, and not through the store itself";

/// A call of a host function that a flight made, while it runs: the
/// store's owner may not reach the store until it drops.
struct Hosting<'a>(&'a Cell<u32>);

impl<'a> Hosting<'a> {
    fn new(hosting: &'a Cell<u32>) -> Self {
        hosting.set(hosting.get() + 1);
        Self(hosting)
    }
}

impl Drop for Hosting<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl Calls {
    /// No host function, and no guest running.
    pub fn new() -> Self {
        Self {
            cell: RefCell::new(Weak::new()),
            funcs: RefCell::new(Vec::new()),
            frames: RefCell::new(Frames::default()),
            leases: RefCell::new(Vec::new()),
            current: RefCell::new(None),
            instantiations: RefCell::new(VecDeque::new()),
            running: RefCell::new(HashMap::new()),
            epoch: Cell::new(0),
            hosting: Cell::new(0),
            owner_dropped: Cell::new(false),
            next_id: Cell::new(1),
        }
    }

    /// Records `cell`, the cell that owns the store.
    pub fn attach(&self, cell: Weak<StoreCell>) {
        *self.cell.borrow_mut() = cell;
    }

    /// A number that no other lease or flight of the store has.
    pub fn next_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// Records that the store's owner reaches the store: every flight's
    /// permit ends.
    ///
    /// [`Error::Backend`](wcmp_wasm_core::Error::Backend) where a host
    /// function that a flight called runs, since the flight's reference to
    /// the store lives until it returns. Only a host function that holds
    /// its store by another path than its caller gets here then: the code
    /// that waits for a flight borrows the store until its future drops,
    /// a dropped future ends the flight's permit, and only a forgotten one
    /// lets go of the store while the flight may still run.
    pub fn claim(&self) -> Result<()> {
        if self.hosting.get() > 0 {
            return Err(errors::backend(HOSTING));
        }
        self.epoch.set(self.epoch.get().wrapping_add(1));
        Ok(())
    }

    /// Records that the store's owner dropped the store: every flight may
    /// reach it from now on, since nothing else can.
    pub fn drop_owner(&self) {
        self.owner_dropped.set(true);
    }

    /// Keeps the host function `func` of type `ty`, and returns its index.
    pub fn add(&self, ty: FuncType, func: HostFunc) -> u32 {
        let mut funcs = self.funcs.borrow_mut();
        funcs.push(Rc::new((ty, func)));
        (funcs.len() - 1) as u32
    }

    /// Marks the start of a synchronous call from `store` into a guest,
    /// until the entry drops. Where the call is the first stretch of the
    /// flight `flight`, the flight's frames are the call's.
    ///
    /// While the entry lives, a host function reaches the store through a
    /// pointer made from `store`. The caller must not touch the store
    /// until the entry drops, so that the host function's access through
    /// the pointer is the only one. The entry drops before the caller's
    /// method returns or awaits, so the pointer never outlives the borrow
    /// it was made from. When the entry drops, every frame the call opened
    /// and did not close, such as the frame of a host function that a trap
    /// unwound, closes.
    pub fn enter(self: &Rc<Self>, store: &mut WebStore, flight: Option<Rc<Flight>>) -> Entry {
        let id = match &flight {
            Some(flight) => flight.id(),
            None => self.next_id(),
        };
        let mut leases = self.leases.borrow_mut();
        leases.push(Lease {
            id,
            store,
            flight,
            error: None,
        });
        Entry::new(self.clone(), leases.len() - 1)
    }

    /// Ends the lease at `depth`, and every lease inside it, and answers
    /// the error of the host function that trapped its call. The drop of
    /// an [`Entry`] calls this.
    pub fn leave_entry(&self, depth: usize) -> Option<anyhow::Error> {
        let lease = {
            let mut leases = self.leases.borrow_mut();
            leases.truncate(depth + 1);
            leases.pop()
        }?;
        self.frames.borrow_mut().close_owned(lease.id);
        lease.error
    }

    /// Starts `flight` running: grants it the store at the current epoch,
    /// and keeps it, and the store's cell, until it stops.
    pub fn run_flight(&self, flight: &Rc<Flight>) {
        flight.grant(self.epoch.get(), self.cell.borrow().upgrade());
        self.running
            .borrow_mut()
            .insert(flight.id(), flight.clone());
    }

    /// Takes up `flight`, a resumable call of the store, for a wait that
    /// borrows the store until the flight stops: grants it the store again,
    /// as [`Calls::run_flight`] does, and lets its stack run on where it
    /// parked. A flight that stopped already needs nothing: the wait takes
    /// its stop.
    ///
    /// The wait borrows the store from here until the flight stops, so the
    /// flight's reference to the store meets no other, as for a flight that
    /// was just started or resumed.
    pub fn adopt(&self, flight: &Rc<Flight>) -> Result<()> {
        if !flight.runs() {
            return Ok(());
        }
        self.run_flight(flight);
        match flight.unpark() {
            Some(resolve) => resolve
                .call0(&JsValue::UNDEFINED)
                .map(|_| ())
                .map_err(|error| errors::call(&error)),
            None => Ok(()),
        }
    }

    /// Records that `flight`, an instantiation, waits for the browser.
    pub fn instantiating(&self, flight: &Rc<Flight>) {
        self.instantiations.borrow_mut().push_back(flight.clone());
    }

    /// Records that `flight` ended: its promise settled.
    pub fn ended(&self, flight: &Rc<Flight>) {
        let current = {
            let mut current = self.current.borrow_mut();
            if current
                .as_ref()
                .is_some_and(|current| Rc::ptr_eq(current, flight))
            {
                current.take()
            } else {
                None
            }
        };
        drop(current);
        self.instantiations
            .borrow_mut()
            .retain(|pending| !Rc::ptr_eq(pending, flight));
        self.frames.borrow_mut().close_owned(flight.id());
        let running = self.running.borrow_mut().remove(&flight.id());
        drop(running);
    }

    /// The type of the host function whose call has the frame `frame`.
    pub fn frame_type(&self, frame: u32) -> Option<FuncType> {
        let func = self.frames.borrow_mut().get_mut(frame)?.func;
        let funcs = self.funcs.borrow();
        Some(funcs.get(func as usize)?.0.clone())
    }

    /// Sets the results of the call whose frame is `frame`, as the wrapper
    /// carries them.
    pub fn set_results(&self, frame: u32, results: Vec<JsValue>) {
        if let Some(frame) = self.frames.borrow_mut().get_mut(frame) {
            frame.results = results;
        }
    }

    /// Where the host function that a guest calls now reaches the store.
    fn reach(&self) -> Reach {
        if let Some(lease) = self.leases.borrow().last() {
            return Reach::Lease {
                id: lease.id,
                store: lease.store,
                suspends: lease
                    .flight
                    .as_ref()
                    .is_some_and(|flight| flight.resumable()),
            };
        }
        if let Some(flight) = self.current.borrow().clone() {
            return Reach::Flight(flight);
        }
        if let Some(flight) = self.instantiations.borrow().front().cloned() {
            return Reach::Flight(flight);
        }
        Reach::Nowhere
    }

    /// Records `error`, the error of the host function that failed where
    /// `reach` says, for the call that its trap ends.
    fn fail(&self, reach: &Reach, error: anyhow::Error) {
        match reach {
            Reach::Lease { .. } => {
                if let Some(lease) = self.leases.borrow_mut().last_mut() {
                    lease.error = Some(error);
                }
            }
            Reach::Flight(flight) => {
                flight.fail(error);
                // The trap ends the stretch of the flight that runs now.
                let current = {
                    let mut current = self.current.borrow_mut();
                    if current
                        .as_ref()
                        .is_some_and(|current| Rc::ptr_eq(current, flight))
                    {
                        current.take()
                    } else {
                        None
                    }
                };
                drop(current);
            }
            Reach::Nowhere => {}
        }
    }

    /// `enter`: opens the frame of a call of the host function `func`,
    /// and returns the number of the frame.
    pub fn open(&self, func: u32) -> u32 {
        let owner = self.reach().id();
        self.frames.borrow_mut().insert(Frame {
            owner,
            parked: None,
            func,
            args: Vec::new(),
            results: Vec::new(),
        })
    }

    /// `arg`: adds `value` to the arguments of the frame `frame`.
    pub fn arg(&self, frame: u32, value: JsValue) {
        if let Some(frame) = self.frames.borrow_mut().get_mut(frame) {
            frame.args.push(value);
        }
    }

    /// `invoke`: runs the host function of the frame `frame` with its
    /// arguments, and returns [`RETURNED`], [`FAILED`], or, for a
    /// suspending host function that answered "not yet" where its call may
    /// suspend, [`SUSPENDED`].
    ///
    /// Where the host function fails, the frame closes, because the
    /// wrapper traps and does not close it, and the error waits for the
    /// call that the trap ends.
    pub fn invoke(&self, frame: u32) -> i32 {
        let reach = self.reach();
        match self.run(frame, &reach) {
            Ok(Poll::Ready(results)) => {
                self.set_results(frame, results);
                RETURNED
            }
            Ok(Poll::Pending) => SUSPENDED,
            Err(error) => {
                self.frames.borrow_mut().remove(frame);
                self.fail(&reach, error);
                FAILED
            }
        }
    }

    /// `result`: the result at `index` of the frame `frame`.
    pub fn result(&self, frame: u32, index: u32) -> JsValue {
        self.frames
            .borrow_mut()
            .get_mut(frame)
            .and_then(|frame| frame.results.get(index as usize))
            .cloned()
            .unwrap_or(JsValue::UNDEFINED)
    }

    /// `leave`: closes the frame `frame`.
    pub fn close(&self, frame: u32) {
        let closed = self.frames.borrow_mut().remove(frame);
        drop(closed);
    }

    /// `suspend`: suspends the resumable call that runs now in the frame
    /// `frame`, and answers the promise its stack waits on.
    ///
    /// The wrapper calls it through its `WebAssembly.Suspending` import,
    /// and only where `invoke` answered [`SUSPENDED`], so a resumable call
    /// runs, with only WebAssembly frames between its start and the
    /// wrapper. The host resumes the call by resolving the promise.
    pub fn suspend(&self, frame: u32) -> JsValue {
        let flight = match self.leases.borrow().last() {
            Some(lease) => lease.flight.clone(),
            None => self.current.borrow_mut().take(),
        };
        let mut resolver = None;
        let promise = Promise::new(&mut |resolve, _| resolver = Some(resolve));
        let (Some(flight), Some(resolve)) = (flight, resolver) else {
            // Nothing could resume the stack, so it resumes at once, and
            // `resumed` finds no call that waits and traps it.
            return Promise::resolve(&JsValue::UNDEFINED).into();
        };
        if let Some(frame) = self.frames.borrow_mut().get_mut(frame) {
            frame.parked = Some(Rc::downgrade(&flight));
        }
        let keep = flight.suspend(frame, resolve);
        let running = self.running.borrow_mut().remove(&flight.id());
        // This runs inside a function of the store, so the store must not
        // drop here.
        flight::release_later(keep);
        drop(running);
        drop(flight);
        promise.into()
    }

    /// `resumed`: the resumed stack of a resumable call that waited in the
    /// frame `frame` runs again. Answers [`RETURNED`] where the call may
    /// reach its store. Where the host took the store back, the flight
    /// parks: this answers [`SUSPENDED`], and the wrapper waits in
    /// `suspend` again, until a wait takes the flight up and lets it run on
    /// ([`Calls::adopt`]). Where no flight waited in the frame, this answers
    /// [`FAILED`], and the wrapper traps.
    pub fn resumed(&self, frame: u32) -> i32 {
        let flight = self
            .frames
            .borrow_mut()
            .get_mut(frame)
            .and_then(|frame| frame.parked.take())
            .and_then(|flight| flight.upgrade());
        match flight {
            Some(flight) if flight.may_run(self.epoch.get(), self.owner_dropped.get()) => {
                let previous = self.current.borrow_mut().replace(flight);
                drop(previous);
                RETURNED
            }
            // Nothing waits for the flight now, and the host may hold the
            // store. The stack waits again, without reaching the store, and
            // `suspend` finds the flight where it finds the one that runs.
            Some(flight) => {
                flight.park();
                let previous = self.current.borrow_mut().replace(flight);
                drop(previous);
                SUSPENDED
            }
            None => {
                let closed = self.frames.borrow_mut().remove(frame);
                drop(closed);
                FAILED
            }
        }
    }

    /// Runs the host function of the frame `frame` in the store, which it
    /// reaches where `reach` says, and returns its results as the wrapper
    /// carries them, or [`Poll::Pending`] where a suspending host function
    /// answered "not yet" and its call may suspend.
    ///
    /// No borrow of the frames or of the host functions lasts across the
    /// host function, which can call back into a guest, and so into this
    /// method again.
    fn run(&self, frame: u32, reach: &Reach) -> anyhow::Result<Poll<Vec<JsValue>>> {
        let (func, args) = {
            let mut frames = self.frames.borrow_mut();
            let frame = frames
                .get_mut(frame)
                .ok_or_else(|| anyhow::anyhow!("the host function has no frame {frame}"))?;
            (frame.func, core::mem::take(&mut frame.args))
        };
        let func = self
            .funcs
            .borrow()
            .get(func as usize)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the store has no host function {func}"))?;
        let (ty, body) = &*func;
        match reach {
            Reach::Lease {
                store, suspends, ..
            } => {
                // SAFETY: the pointer is set only by `enter`, from a store
                // that its caller borrows mutably and does not touch until
                // the entry drops, which ends the lease before the caller
                // returns or awaits. The lease is the innermost, so every
                // access to the store under it has ended or waits on this
                // call, and this is the one access to the store.
                let store = unsafe { &mut **store };
                store.call_host(ty, body, args, *suspends)
            }
            Reach::Flight(flight) => {
                anyhow::ensure!(
                    flight.may_run(self.epoch.get(), self.owner_dropped.get()),
                    TAKEN_BACK
                );
                let cell = self
                    .cell
                    .borrow()
                    .upgrade()
                    .ok_or_else(|| anyhow::anyhow!("the store of the call is gone"))?;
                let _hosting = Hosting::new(&self.hosting);
                // SAFETY: the flight's permit holds, so the owner has not
                // reached the store since the flight was started or
                // resumed, or the owner is gone, and no reference the
                // owner made lives. No lease runs, so no method of the
                // store runs either. The flight runs on a microtask, so
                // no other code of the page runs until the host function
                // returns. The host function itself may hold its store by
                // a global, but while `_hosting` lives the owner refuses
                // to reach the store. So this is the one reference to the
                // store until the call returns.
                let store = unsafe { &mut *cell.get() };
                // The flight keeps the cell while it runs, so `cell` is
                // not the last reference, and the store does not drop
                // inside this function of the store.
                store.call_host(ty, body, args, flight.resumable())
            }
            Reach::Nowhere => {
                anyhow::bail!("a guest called a host function outside every call of its store")
            }
        }
    }
}

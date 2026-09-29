//! What the host functions of a store share with the store while a guest
//! runs in it.

use core::cell::{Cell, RefCell};
use core::ptr;
use std::rc::Rc;

use wasm_bindgen::JsValue;
use wcmp_wasm_core::backend::HostFunc;
use wcmp_wasm_core::{Error, FuncType, TrapKind};

use crate::entry::Entry;
use crate::store::WebStore;

/// What the host functions of one store share with the store: the store a
/// guest runs in, each host function, a frame for each call of a host
/// function that runs, and the error of the host function that failed.
///
/// A guest calls a host function through its wrapper module, which reaches
/// the host through five JavaScript functions: `enter`, `arg`, `invoke`,
/// `result`, and `leave`. Each is a method here. A call of a host function
/// opens a frame of its own, which holds its arguments and its results, so
/// no call shares a buffer with another. The calls of host functions nest,
/// so the frames are a stack, and the index of a frame names it.
///
/// A host function reaches the store through the pointer that the store
/// sets each time it calls into a guest, and puts back when the call
/// returns. See [`Calls::enter`].
pub struct Calls {
    /// The store a guest runs in, or null where no guest runs.
    store: Cell<*mut WebStore>,
    /// The type and the body of each host function, by its index.
    funcs: RefCell<Vec<Rc<(FuncType, HostFunc)>>>,
    /// The frame of each call of a host function that runs, the innermost
    /// last.
    frames: RefCell<Vec<Frame>>,
    /// The error of the host function that failed, from its failure until
    /// the store that called into the guest takes it.
    error: RefCell<Option<anyhow::Error>>,
}

impl Default for Calls {
    fn default() -> Self {
        Self::new()
    }
}

/// The frame of one call of a host function.
struct Frame {
    /// The index of the host function.
    func: u32,
    /// The arguments, as the wrapper carries them.
    args: Vec<JsValue>,
    /// The results, as the wrapper carries them, once the host function
    /// returned.
    results: Vec<JsValue>,
}

/// The status that `invoke` returns to the wrapper where the host function
/// returned its results.
const RETURNED: i32 = 0;

/// The status that `invoke` returns to the wrapper where the host function
/// failed. The wrapper then traps.
const FAILED: i32 = 1;

impl Calls {
    /// No host function, and no guest running.
    pub fn new() -> Self {
        Self {
            store: Cell::new(ptr::null_mut()),
            funcs: RefCell::new(Vec::new()),
            frames: RefCell::new(Vec::new()),
            error: RefCell::new(None),
        }
    }

    /// Keeps the host function `func` of type `ty`, and returns its index.
    pub fn add(&self, ty: FuncType, func: HostFunc) -> u32 {
        let mut funcs = self.funcs.borrow_mut();
        funcs.push(Rc::new((ty, func)));
        (funcs.len() - 1) as u32
    }

    /// Marks the start of a call from `store` into a guest, until the
    /// entry drops.
    ///
    /// While the entry lives, a host function reaches the store through a
    /// pointer made from `store`. The caller must not touch the store
    /// until the entry drops, so that the host function's access through
    /// the pointer is the only one. The store then holds for the life of
    /// the entry: the caller borrows it mutably, and a guest runs only
    /// while the caller waits on it.
    ///
    /// When the entry drops, the pointer is the one before, so a call that
    /// a host function makes returns the store to the call of the host
    /// function. Every frame the call opened and did not close, such as the
    /// frame of a host function that a trap unwound, closes.
    pub fn enter(self: &Rc<Self>, store: &mut WebStore) -> Entry {
        // No host function failed between its failure and the call that
        // takes its error, because each runs only inside a call. So an
        // error left here is from a guest that some other code called, and
        // it belongs to no call of this store.
        self.error.borrow_mut().take();
        let previous = self.store.replace(store);
        Entry::new(self.clone(), previous, self.frames.borrow().len())
    }

    /// Puts back the pointer `store`, and closes every frame from
    /// `frames` on. The drop of an [`Entry`] calls this.
    pub fn leave_entry(&self, store: *mut WebStore, frames: usize) {
        self.store.set(store);
        self.frames.borrow_mut().truncate(frames);
    }

    /// The trap of a call into a guest that failed, where a host function
    /// failed in it: [`TrapKind::Host`] with the host function's own error.
    ///
    /// A host function that fails traps the guest at once, and the trap
    /// unwinds WebAssembly frames alone up to the call into the guest. So
    /// the error that a failed call finds here is the error of its own
    /// trap.
    pub fn trap(&self) -> Option<Error> {
        self.error
            .borrow_mut()
            .take()
            .map(|error| Error::Trap(TrapKind::Host(error)))
    }

    /// `enter`: opens the frame of a call of the host function `func`,
    /// and returns the index of the frame.
    pub fn open(&self, func: u32) -> u32 {
        let mut frames = self.frames.borrow_mut();
        frames.push(Frame {
            func,
            args: Vec::new(),
            results: Vec::new(),
        });
        (frames.len() - 1) as u32
    }

    /// `arg`: adds `value` to the arguments of the frame `frame`.
    pub fn arg(&self, frame: u32, value: JsValue) {
        if let Some(frame) = self.frames.borrow_mut().get_mut(frame as usize) {
            frame.args.push(value);
        }
    }

    /// `invoke`: runs the host function of the frame `frame` with its
    /// arguments, and returns [`RETURNED`] or [`FAILED`].
    ///
    /// Where the host function fails, the frame closes, because the
    /// wrapper traps and does not close it, and the error waits for the
    /// call into the guest to take it.
    pub fn invoke(&self, frame: u32) -> i32 {
        match self.run(frame) {
            Ok(results) => {
                if let Some(frame) = self.frames.borrow_mut().get_mut(frame as usize) {
                    frame.results = results;
                }
                RETURNED
            }
            Err(error) => {
                self.frames.borrow_mut().truncate(frame as usize);
                *self.error.borrow_mut() = Some(error);
                FAILED
            }
        }
    }

    /// `result`: the result at `index` of the frame `frame`.
    pub fn result(&self, frame: u32, index: u32) -> JsValue {
        self.frames
            .borrow()
            .get(frame as usize)
            .and_then(|frame| frame.results.get(index as usize))
            .cloned()
            .unwrap_or(JsValue::UNDEFINED)
    }

    /// `leave`: closes the frame `frame`, and every frame after it.
    pub fn close(&self, frame: u32) {
        self.frames.borrow_mut().truncate(frame as usize);
    }

    /// Runs the host function of the frame `frame` in the store a guest
    /// runs in, and returns its results as the wrapper carries them.
    ///
    /// No borrow of the frames or of the host functions lasts across the
    /// host function, which can call back into a guest, and so into this
    /// method again.
    fn run(&self, frame: u32) -> anyhow::Result<Vec<JsValue>> {
        let store = self.store.get();
        anyhow::ensure!(
            !store.is_null(),
            "a guest called a host function outside a call of its store"
        );
        let (func, args) = {
            let mut frames = self.frames.borrow_mut();
            let frame = frames
                .get_mut(frame as usize)
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
        // SAFETY: the pointer is set only by `enter`, from a store that
        // its caller borrows mutably and does not touch until the entry
        // drops, which puts the pointer back. A guest runs only inside
        // that call, so this is the one access to the store.
        let store = unsafe { &mut *store };
        store.call_host(ty, body, args)
    }
}

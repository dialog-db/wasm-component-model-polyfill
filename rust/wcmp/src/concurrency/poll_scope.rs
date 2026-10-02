// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The slot this thread's running poll leaves its store in.

use core::any::TypeId;
use core::cell::Cell;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::task::Waker;

use crate::error::{Error, Result, SchedulerCause};
use crate::store::StoreContextInternalExt;
use crate::store::{StoreContext, StoreId};

/// The slot this thread's running poll leaves its store in.
///
/// An [`Accessor`] carries a store's identity and nothing else, so
/// something else has to hold the store itself while a poll runs.
/// This is that something: a scope entered around one poll, which
/// puts the store's context in the thread's slot and takes it out
/// again before the poll returns. Outside a poll the slot is empty,
/// and an accessor that reaches for its store there fails with
/// [`SchedulerCause::StoreNotInPoll`].
///
/// One slot suffices, because only the innermost poll running on a
/// thread has a store to lend. A poll holds the store by `&mut`,
/// and this scope holds that borrow for its own length: nothing can
/// touch the store through the reference the pointer was built from
/// while the pointer is in the slot. A scope entered inside another
/// — a host task started from inside a reach, whose body is polled
/// while the outer poll is still on the stack, or a poll of a
/// second store entered from inside a reach of the first —
/// displaces what it found and gives it back when it ends, the way
/// a nested turn gives back the waker it displaced. What it
/// displaced is unreachable meanwhile, which is what a reach with
/// the displaced store's accessor reports.
///
/// The slot records which store is inside a poll beside the
/// pointer, and the two are not taken away together. A reach takes
/// the pointer and leaves the name, so that a second reach from
/// inside the first finds a slot that is empty but still names the
/// store, which is the recursive-driver case and not the
/// store-not-in-poll one.
///
/// What names the store is an identity and a host data type, not an
/// identity alone. The pointer in the slot is type-erased and a
/// reach casts it back, so the `T` the reach asks for is checked
/// rather than read off the identity: an accessor is built from a
/// store's identity, and that identity is a value safe code outside
/// the crate can hold, so safe code can ask for a `T` the store
/// does not have. What it gets is the store-not-in-poll refusal,
/// never the cast.
///
/// [`Accessor`]: super::Accessor
pub struct PollScope<'a> {
    displaced: Slot,
    /// The borrow of the store whose pointer is in the slot. The
    /// scope holds it so that the store is unreachable by any other
    /// path while the poll runs.
    store: PhantomData<&'a mut ()>,
}

impl<'a> PollScope<'a> {
    /// Put `store` in this thread's slot for the length of the
    /// scope, with `waker` as the waker of the poll that is
    /// running.
    ///
    /// The waker goes in beside the store because a reach runs its
    /// body inside a turn, and a turn records the waker a host task
    /// started from within it is polled with. The poll's waker is
    /// the one that reaches whoever is driving the store, so it is
    /// the one that turn takes. Workspace-internal.
    pub fn enter<T: 'static>(store: &'a mut StoreContext<'_, T>, waker: &'a Waker) -> Self {
        let held = Slot {
            in_poll: Some(Polled {
                store: store.internal().id(),
                data: TypeId::of::<T>(),
            }),
            lent: Some(Lent {
                store: NonNull::from(store).cast::<()>(),
                waker: NonNull::from(waker),
            }),
        };
        Self {
            displaced: slot(|slot| slot.replace(held)),
            store: PhantomData,
        }
    }

    /// Run `body` against the store in this thread's slot, when the
    /// slot holds the store `store` names.
    ///
    /// The store leaves the slot for the length of `body` and goes
    /// back afterwards, whether `body` returned or unwound, so that
    /// a reach from inside `body` finds the slot empty rather than a
    /// second borrow of the same store.
    ///
    /// The failures are the two an accessor can meet. A slot that
    /// names no store, or names another one, or names this store
    /// with a host data type other than `T`, is a reach made
    /// outside any poll of the store this reach asks for. A slot
    /// that names this store and this `T` with nothing in it is a
    /// reach made from inside another reach. Workspace-internal.
    pub fn reach<T: 'static, R>(
        store: StoreId,
        body: impl FnOnce(&mut StoreContext<'_, T>, &Waker) -> R,
    ) -> Result<R> {
        let lent = slot(|cell| {
            let held = cell.get();
            let asked_for = Polled {
                store,
                data: TypeId::of::<T>(),
            };
            if held.in_poll != Some(asked_for) {
                return Err(Error::Scheduler(SchedulerCause::StoreNotInPoll));
            }
            let Some(lent) = held.lent else {
                return Err(Error::Scheduler(SchedulerCause::RecursiveDriver));
            };
            cell.set(Slot {
                in_poll: held.in_poll,
                lent: None,
            });
            Ok(lent)
        })?;
        let _returned = Returned(lent);
        // SAFETY: the pointer was taken from the `&mut
        // StoreContext<'_, T>` a live `PollScope` holds, and that
        // scope keeps the borrow to itself for its whole length, so
        // nothing else reaches the store through it. The pointer
        // has just left the slot, so no other reach holds it
        // either, and it goes back only when `_returned` is
        // dropped. The type is the one it was taken from: the slot
        // records the host data type the scope was entered with
        // beside the store's identity, and the reach above compared
        // both, so this `T` is the `T` the pointer points at. The
        // identity alone would not say so, because an accessor
        // naming this store can be built for any `T` by safe code
        // outside the crate.
        let held = unsafe { lent_store::<T>(lent.store) };
        // SAFETY: the waker is the one the poll was entered with,
        // borrowed for the scope's length by the same argument.
        let waker = unsafe { lent.waker.as_ref() };
        Ok(body(held, waker))
    }
}

impl Drop for PollScope<'_> {
    fn drop(&mut self) {
        slot(|cell| cell.set(self.displaced));
    }
}

/// What one thread's slot holds.
#[derive(Clone, Copy)]
struct Slot {
    /// The store a poll running on this thread is polling against,
    /// which stays here for the whole of that poll.
    in_poll: Option<Polled>,
    /// The store itself, while no reach has it.
    lent: Option<Lent>,
}

impl Slot {
    /// The slot of a thread with no poll running on it.
    const EMPTY: Self = Self {
        in_poll: None,
        lent: None,
    };
}

/// The store a poll on this thread is running against: which store
/// it is, and what host data that store holds. A reach matches both
/// before the slot lends anything. The identity says that the
/// pointer in the slot is this store's, and the type says what the
/// pointer points at, which the slot erased and a reach casts back.
/// Nothing else pairs the two: an accessor is built from an
/// identity, and safe code outside the crate can hold a store's
/// identity and name whatever `T` it likes beside it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Polled {
    /// The store, as the identity it minted once and never reuses.
    store: StoreId,
    /// The host data type of the `StoreContext<'_, T>` whose
    /// pointer the scope put in the slot.
    data: TypeId,
}

/// The store a poll lent to this thread's slot, and the waker of
/// that poll. Both point into the frame the poll is running in,
/// which the [`PollScope`] keeps alive and unreachable by any other
/// path.
#[derive(Clone, Copy)]
struct Lent {
    /// The poll's `&mut StoreContext<'_, T>`, with the `T` erased:
    /// a thread's slot is one declaration and cannot be generic,
    /// and the store's identity beside it says which `T` it is.
    store: NonNull<()>,
    /// The waker the poll was entered with.
    waker: NonNull<Waker>,
}

/// Put the store back in the slot when a reach ends, however it
/// ends. A reach runs a closure the embedder wrote, which can
/// panic; a store that stayed out of the slot after such a panic
/// would leave every later reach of that poll reporting a recursion
/// that is over.
struct Returned(Lent);

impl Drop for Returned {
    fn drop(&mut self) {
        slot(|cell| {
            let held = cell.get();
            cell.set(Slot {
                in_poll: held.in_poll,
                lent: Some(self.0),
            });
        });
    }
}

/// The store the slot lends, as the borrow a reach runs against.
///
/// # Safety
///
/// `store` must be the pointer a live [`PollScope`] put in the slot,
/// taken out for the length of the reach, and the `T` must be the
/// one the scope recorded beside it.
///
/// Both lifetimes on the borrow this returns are unbounded: the
/// slot erased them along with the type, and nothing in the
/// signature ties them to anything the caller holds. What bounds
/// them is the one caller. [`PollScope::reach`] hands the borrow
/// straight to a body that takes `&mut StoreContext<'_, T>` —
/// higher-ranked, so the body is written for whatever lifetimes it
/// is handed and can keep nothing it was given past the call — and
/// lets the borrow go when the body returns. A signature that let
/// the borrow out of `reach` by another path, or a body whose
/// lifetimes the caller chose rather than the callee, would lose
/// that bound without changing a line of this function.
unsafe fn lent_store<'reach, 'store, T: 'static>(
    store: NonNull<()>,
) -> &'reach mut StoreContext<'store, T> {
    // SAFETY: the caller's obligation is exactly that this pointer
    // is a live, unaliased `&mut StoreContext<'store, T>`.
    unsafe { store.cast().as_mut() }
}

/// Reach this thread's slot.
///
/// One declaration serves both targets. Natively a store can be
/// polled on any thread, so the slot is thread-local and each
/// thread answers for its own poll. In the browser the module has
/// one thread, so the same declaration is one static cell, which is
/// all a single-threaded target needs.
fn slot<R>(body: impl FnOnce(&Cell<Slot>) -> R) -> R {
    thread_local! {
        static SLOT: Cell<Slot> = const { Cell::new(Slot::EMPTY) };
    }
    SLOT.with(body)
}

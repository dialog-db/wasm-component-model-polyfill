// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The allocation that owns a store.

use core::cell::UnsafeCell;

use crate::store::WebStore;

/// The allocation that owns a store, shared by the store's owner and by
/// each flight that runs in it.
///
/// A flight runs on a microtask, after the code that started it returned,
/// and it can outlive the owner. So the store lives here, behind a counted
/// reference, and not in a place that a borrow of the owner names.
///
/// A reference made from [`StoreCell::get`] must be the only one in use
/// while it lives. The store's owner makes one only where no host function
/// that a flight called runs, and only after it moved the store's epoch on,
/// which ends the permit of every flight, and only for the length of one
/// of its methods, or of one step of its futures between two awaits. A
/// flight makes one only while its permit holds, and only for the length
/// of one call of a host function, which runs to its end before any other
/// code of the page can run. That host function may reach the owner, since
/// it can hold its store by a global, but the owner then refuses to make
/// its reference.
pub struct StoreCell {
    store: UnsafeCell<WebStore>,
}

impl StoreCell {
    /// The cell that owns `store`.
    pub fn new(store: WebStore) -> Self {
        Self {
            store: UnsafeCell::new(store),
        }
    }

    /// A pointer to the store, which a caller that holds the store's
    /// access by the rules above may make a reference from.
    pub fn get(&self) -> *mut WebStore {
        self.store.get()
    }
}

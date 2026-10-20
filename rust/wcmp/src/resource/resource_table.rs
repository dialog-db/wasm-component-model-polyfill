// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A table of host values that typed handles name, as Wasmtime's
//! `ResourceTable` (`crates/wasmtime/src/runtime/component/resource_table.rs`
//! at `v50.0.0`).

use core::any::Any;
use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use crate::error::ResourceTableError;

use super::host_static::Resource;

/// A value the table holds: `Send` natively, as in Wasmtime, and any
/// value in the browser, whose one thread holds values such as a
/// `JsValue` that are not `Send`.
#[cfg(not(target_arch = "wasm32"))]
type Value = dyn Any + Send;
#[cfg(target_arch = "wasm32")]
type Value = dyn Any;

/// How many entries a table holds unless
/// [`ResourceTable::set_max_capacity`] says otherwise: Wasmtime's
/// default.
const DEFAULT_MAX_CAPACITY: usize = 1_000_000;

/// A table of host values, each of which a [`Resource<T>`] names by its
/// slot index, the handle's rep.
///
/// The host keeps the values behind its resources here: [`push`] puts a
/// value in and answers the owned handle that a guest receives, and a
/// host function that receives the handle back reads the value with
/// [`get`] or [`get_mut`]. A value can have children, which must be
/// deleted before it is. A slot that [`delete`] frees takes the next
/// value pushed.
///
/// The table is a plain data structure that the host keeps, often in
/// its store data. It shares nothing with the store's own handle
/// tables: a guest's handle and the store's host table name the
/// resource by its rep, and this table is what the rep indexes.
///
/// Natively a value must be `Send`, as in Wasmtime. In the browser it
/// need not be, so the table can hold a `JsValue`.
///
/// [`push`]: ResourceTable::push
/// [`get`]: ResourceTable::get
/// [`get_mut`]: ResourceTable::get_mut
/// [`delete`]: ResourceTable::delete
pub struct ResourceTable {
    entries: Vec<Slot>,
    free_head: Option<usize>,
    max_capacity: usize,
}

/// One slot of the table.
enum Slot {
    Free { next: Option<usize> },
    Occupied { entry: TableEntry },
}

/// A value of the table, with its parent and its children.
struct TableEntry {
    value: Box<Value>,
    parent: Option<u32>,
    children: BTreeSet<u32>,
}

impl TableEntry {
    fn new(value: Box<Value>, parent: Option<u32>) -> Self {
        Self {
            value,
            parent,
            children: BTreeSet::new(),
        }
    }
}

impl ResourceTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    /// An empty table with room for `capacity` values before it grows.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            free_head: None,
            max_capacity: DEFAULT_MAX_CAPACITY,
        }
    }

    /// Whether the table holds no value.
    pub fn is_empty(&self) -> bool {
        self.entries
            .iter()
            .all(|slot| matches!(slot, Slot::Free { .. }))
    }

    /// How many slots the table may have.
    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    /// Set how many slots the table may have. A table already past it
    /// keeps its values, and a push that would add a slot fails.
    pub fn set_max_capacity(&mut self, max: usize) {
        self.max_capacity = max;
    }

    /// Put `entry` in the table and answer the owned handle that names
    /// it. Natively `entry` must be `Send`, as in Wasmtime.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::Full`] when no slot is free and the table
    /// is at its [`max_capacity`](Self::max_capacity).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn push<T>(&mut self, entry: T) -> Result<Resource<T>, ResourceTableError>
    where
        T: Send + 'static,
    {
        self.push_value(Box::new(entry), None)
    }

    /// Put `entry` in the table and answer the owned handle that names
    /// it. In the browser `entry` need not be `Send`.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::Full`] when no slot is free and the table
    /// is at its [`max_capacity`](Self::max_capacity).
    #[cfg(target_arch = "wasm32")]
    pub fn push<T>(&mut self, entry: T) -> Result<Resource<T>, ResourceTableError>
    where
        T: 'static,
    {
        self.push_value(Box::new(entry), None)
    }

    /// Put `entry` in the table as a child of `parent`, which cannot be
    /// deleted until the child is. Natively `entry` must be `Send`, as
    /// in Wasmtime.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::NotPresent`] when `parent` names no value,
    /// and [`ResourceTableError::Full`] as for [`push`](Self::push).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn push_child<T, U>(
        &mut self,
        entry: T,
        parent: &Resource<U>,
    ) -> Result<Resource<T>, ResourceTableError>
    where
        T: Send + 'static,
        U: 'static,
    {
        self.push_value(Box::new(entry), Some(parent.rep()))
    }

    /// Put `entry` in the table as a child of `parent`, which cannot be
    /// deleted until the child is. In the browser `entry` need not be
    /// `Send`.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::NotPresent`] when `parent` names no value,
    /// and [`ResourceTableError::Full`] as for [`push`](Self::push).
    #[cfg(target_arch = "wasm32")]
    pub fn push_child<T, U>(
        &mut self,
        entry: T,
        parent: &Resource<U>,
    ) -> Result<Resource<T>, ResourceTableError>
    where
        T: 'static,
        U: 'static,
    {
        self.push_value(Box::new(entry), Some(parent.rep()))
    }

    /// Put `value` in the table, as a child of `parent` when there is
    /// one, and answer the owned handle that names it.
    fn push_value<T>(
        &mut self,
        value: Box<Value>,
        parent: Option<u32>,
    ) -> Result<Resource<T>, ResourceTableError> {
        if let Some(parent) = parent {
            self.occupied(parent)?;
        }
        let index = self.push_entry(TableEntry::new(value, parent))?;
        if let Some(parent) = parent {
            self.occupied_mut(parent)?.children.insert(index);
        }
        Ok(Resource::new_own(index))
    }

    /// Make `child` a child of `parent`.
    ///
    /// # Panics
    ///
    /// When `child` has a parent already, as in Wasmtime.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::NotPresent`] when either names no value.
    pub fn add_child<T: 'static, U: 'static>(
        &mut self,
        child: Resource<T>,
        parent: Resource<U>,
    ) -> Result<(), ResourceTableError> {
        let entry = self.occupied_mut(child.rep())?;
        assert!(entry.parent.is_none(), "the child has a parent already");
        entry.parent = Some(parent.rep());
        self.occupied_mut(parent.rep())?
            .children
            .insert(child.rep());
        Ok(())
    }

    /// Make `child` no longer a child of `parent`.
    ///
    /// # Panics
    ///
    /// When `parent` is not `child`'s parent, as in Wasmtime.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::NotPresent`] when either names no value.
    pub fn remove_child<T: 'static, U: 'static>(
        &mut self,
        child: Resource<T>,
        parent: Resource<U>,
    ) -> Result<(), ResourceTableError> {
        let entry = self.occupied_mut(child.rep())?;
        assert_eq!(
            entry.parent,
            Some(parent.rep()),
            "the child has another parent"
        );
        entry.parent = None;
        self.occupied_mut(parent.rep())?
            .children
            .remove(&child.rep());
        Ok(())
    }

    /// The value `key` names.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::NotPresent`] when `key` names no value, and
    /// [`ResourceTableError::WrongType`] when the value is not a `T`.
    pub fn get<T: Any + Sized>(&self, key: &Resource<T>) -> Result<&T, ResourceTableError> {
        let value: &dyn Any = &*self.occupied(key.rep())?.value;
        value.downcast_ref().ok_or(ResourceTableError::WrongType)
    }

    /// The value `key` names, mutably.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub fn get_mut<T: Any + Sized>(
        &mut self,
        key: &Resource<T>,
    ) -> Result<&mut T, ResourceTableError> {
        self.get_any_mut(key.rep())?
            .downcast_mut()
            .ok_or(ResourceTableError::WrongType)
    }

    /// The value in the slot `key`, of whatever type.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::NotPresent`] when the slot holds no value.
    pub fn get_any_mut(&mut self, key: u32) -> Result<&mut dyn Any, ResourceTableError> {
        let value: &mut dyn Any = &mut *self.occupied_mut(key)?.value;
        Ok(value)
    }

    /// Take the value `resource` names out of the table, and free its
    /// slot.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::NotPresent`] when `resource` names no
    /// value, [`ResourceTableError::HasChildren`] when the value has a
    /// child, and [`ResourceTableError::WrongType`] when it is not a
    /// `T`. A value of another type leaves the table all the same, as
    /// in Wasmtime.
    pub fn delete<T: Any>(&mut self, resource: Resource<T>) -> Result<T, ResourceTableError> {
        debug_assert!(resource.owned());
        let key = resource.rep();
        if !self.occupied(key)?.children.is_empty() {
            return Err(ResourceTableError::HasChildren);
        }
        let entry = self.free_slot(key as usize);
        if let Some(parent) = entry.parent {
            self.occupied_mut(parent)
                .expect("a child's parent outlives it")
                .children
                .remove(&key);
        }
        let value: Box<dyn Any> = entry.value;
        value
            .downcast()
            .map(|value| *value)
            .map_err(|_| ResourceTableError::WrongType)
    }

    /// Answer each value of `map`'s keys, beside the key's value: the
    /// table's value, mutably, or [`ResourceTableError::NotPresent`].
    ///
    /// A map answers its keys in ascending order, so one walk of the
    /// slots finds them all, and no two answers alias.
    pub fn iter_entries<T>(
        &mut self,
        map: BTreeMap<u32, T>,
    ) -> impl Iterator<Item = (Result<&mut dyn Any, ResourceTableError>, T)> {
        let mut slots = self.entries.iter_mut().enumerate();
        map.into_iter().map(move |(key, value)| {
            let key = key as usize;
            let item = loop {
                match slots.next() {
                    Some((index, _)) if index < key => continue,
                    Some((_, Slot::Occupied { entry })) => {
                        break Ok(&mut *entry.value as &mut dyn Any);
                    }
                    _ => break Err(ResourceTableError::NotPresent),
                }
            };
            (item, value)
        })
    }

    /// The children of `parent`.
    ///
    /// # Errors
    ///
    /// [`ResourceTableError::NotPresent`] when `parent` names no value.
    pub fn iter_children<T: 'static>(
        &self,
        parent: &Resource<T>,
    ) -> Result<impl Iterator<Item = &Value> + use<'_, T>, ResourceTableError> {
        let parent = self.occupied(parent.rep())?;
        Ok(parent.children.iter().map(|child| {
            &*self
                .occupied(*child)
                .expect("a parent's children are in the table")
                .value
        }))
    }

    /// Every value of the table, mutably, with the index of its entry,
    /// which is the rep of the handle that names it.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut Value)> {
        self.entries
            .iter_mut()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Slot::Occupied { entry } => Some((u32::try_from(index).ok()?, &mut *entry.value)),
                Slot::Free { .. } => None,
            })
    }

    fn push_entry(&mut self, entry: TableEntry) -> Result<u32, ResourceTableError> {
        if let Some(free) = self.free_head {
            self.free_head = match self.entries[free] {
                Slot::Free { next } => next,
                Slot::Occupied { .. } => unreachable!("the free list holds free slots"),
            };
            self.entries[free] = Slot::Occupied { entry };
            return Ok(free as u32);
        }
        if self.entries.len() >= self.max_capacity {
            return Err(ResourceTableError::Full);
        }
        let index = u32::try_from(self.entries.len()).map_err(|_| ResourceTableError::Full)?;
        self.entries.push(Slot::Occupied { entry });
        Ok(index)
    }

    fn free_slot(&mut self, index: usize) -> TableEntry {
        let next = self.free_head;
        self.free_head = Some(index);
        match core::mem::replace(&mut self.entries[index], Slot::Free { next }) {
            Slot::Occupied { entry } => entry,
            Slot::Free { .. } => unreachable!("a deleted slot was occupied"),
        }
    }

    fn occupied(&self, key: u32) -> Result<&TableEntry, ResourceTableError> {
        match self.entries.get(key as usize) {
            Some(Slot::Occupied { entry }) => Ok(entry),
            _ => Err(ResourceTableError::NotPresent),
        }
    }

    fn occupied_mut(&mut self, key: u32) -> Result<&mut TableEntry, ResourceTableError> {
        match self.entries.get_mut(key as usize) {
            Some(Slot::Occupied { entry }) => Ok(entry),
            _ => Err(ResourceTableError::NotPresent),
        }
    }
}

impl Default for ResourceTable {
    fn default() -> Self {
        Self::new()
    }
}

/// A table renders as the indices of its values, as in Wasmtime.
impl fmt::Debug for ResourceTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(
                self.entries
                    .iter()
                    .enumerate()
                    .filter(|(_, slot)| matches!(slot, Slot::Occupied { .. }))
                    .map(|(index, _)| index),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reuses_a_freed_slot_first() {
        let mut table = ResourceTable::new();
        let x = table.push(()).unwrap();
        assert_eq!(x.rep(), 0);
        let y = table.push(()).unwrap();
        assert_eq!(y.rep(), 1);
        table.delete(x).unwrap();
        let x = table.push(()).unwrap();
        assert_eq!(x.rep(), 0);
        table.delete(x).unwrap();
        table.delete(y).unwrap();
        let y = table.push(()).unwrap();
        assert_eq!(y.rep(), 1);
        let x = table.push(()).unwrap();
        assert_eq!(x.rep(), 0);
        let z = table.push(()).unwrap();
        assert_eq!(z.rep(), 2);
        assert_eq!(format!("{table:?}"), "[0, 1, 2]");
    }

    #[wcmp_macros::test]
    fn it_yields_each_value_with_the_rep_of_its_handle() {
        let mut table = ResourceTable::new();
        let x = table.push(10_u32).unwrap();
        let y = table.push(20_u32).unwrap();
        let z = table.push(30_u32).unwrap();
        table.delete(y).unwrap();
        for (_, value) in table.iter_mut() {
            *value.downcast_mut::<u32>().unwrap() += 1;
        }
        let seen: Vec<(u32, u32)> = table
            .iter_mut()
            .map(|(rep, value)| (rep, *value.downcast_ref::<u32>().unwrap()))
            .collect();
        assert_eq!(seen, [(x.rep(), 11), (z.rep(), 31)]);
    }

    #[wcmp_macros::test]
    fn it_answers_each_error_where_wasmtime_does() {
        let mut table = ResourceTable::new();
        let first = table.push(1_u32).unwrap();
        let rep = first.rep();
        assert!(matches!(table.delete(first), Ok(1)));
        assert!(matches!(
            table.get(&Resource::<u32>::new_own(rep)),
            Err(ResourceTableError::NotPresent)
        ));

        let number = table.push(2_u32).unwrap();
        assert!(matches!(
            table.get(&Resource::<String>::new_own(number.rep())),
            Err(ResourceTableError::WrongType)
        ));

        let child = table.push_child(3_u32, &number).unwrap();
        assert!(matches!(
            table.delete(Resource::<u32>::new_own(number.rep())),
            Err(ResourceTableError::HasChildren)
        ));
        assert_eq!(
            table
                .iter_children(&number)
                .unwrap()
                .map(|value| *(value as &dyn Any).downcast_ref::<u32>().unwrap())
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert!(matches!(table.delete(child), Ok(3)));
        assert!(matches!(table.delete(number), Ok(2)));
        assert!(table.is_empty());

        // A push past the cap fails only when no slot is free: a freed
        // slot is reused whatever the cap, as in Wasmtime.
        let mut table = ResourceTable::new();
        table.set_max_capacity(1);
        let _kept = table.push(()).unwrap();
        assert!(matches!(table.push(()), Err(ResourceTableError::Full)));
    }
}

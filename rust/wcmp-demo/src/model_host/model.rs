// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The todo model over a storage.

use futures::lock::Mutex;

use crate::model::{Storage, TodoList};

/// The todo model over a storage.
pub struct Model<S> {
    storage: S,
    turn: Mutex<()>,
}

impl<S: Storage> Model<S> {
    /// The model over `storage`.
    pub fn new(storage: S) -> Self {
        Model {
            storage,
            turn: Mutex::new(()),
        }
    }

    /// Read the list, apply `change`, write the list back when the
    /// change asks for it, and answer what the change answered.
    ///
    /// # Errors
    ///
    /// The storage's error.
    pub async fn with<R>(
        &self,
        change: impl FnOnce(&mut TodoList) -> (R, bool),
    ) -> Result<R, String> {
        let _turn = self.turn.lock().await;
        let mut list = self.storage.load().await?;
        let (answer, write) = change(&mut list);
        if write {
            self.storage.save(&list).await?;
        }
        Ok(answer)
    }
}

// `Memory` holds the list natively, where these tests run; the browser
// holds it in IndexedDB.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::Model;
    use crate::model::{Filter, Memory};

    /// The titles of every todo of `model`.
    async fn titles(model: &Model<Memory>) -> Vec<String> {
        model
            .with(|list| {
                let (todos, _) = list.query(Filter::All);
                (todos.into_iter().map(|todo| todo.title).collect(), false)
            })
            .await
            .expect("the memory reads")
    }

    #[wcmp_macros::test]
    async fn it_writes_the_list_back_only_when_a_change_asks() {
        let model = Model::new(Memory::default());
        model
            .with(|list| (list.add("kept").is_ok(), true))
            .await
            .unwrap();
        model
            .with(|list| (list.add("dropped").is_ok(), false))
            .await
            .unwrap();
        assert_eq!(titles(&model).await, ["kept"]);
    }

    #[wcmp_macros::test]
    async fn it_keeps_both_of_two_changes_made_at_once() {
        let model = Model::new(Memory::default());
        let (first, second) = futures::join!(
            model.with(|list| (list.add("one").is_ok(), true)),
            model.with(|list| (list.add("two").is_ok(), true)),
        );
        assert_eq!((first, second), (Ok(true), Ok(true)));
        let mut titles = titles(&model).await;
        titles.sort();
        assert_eq!(titles, ["one", "two"]);
    }
}

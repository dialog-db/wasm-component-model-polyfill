// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The whole todo list.

use super::{Counts, Filter, ModelError, Todo};

/// The whole todo list as a [`Storage`] keeps it: the todos in order,
/// and the number of the next id.
///
/// [`Storage`]: super::Storage
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoList {
    /// Every todo, in the order it was added.
    pub todos: Vec<Todo>,
    /// The number the next todo's id takes.
    pub next: u64,
}

impl TodoList {
    /// The todos `filter` selects, and the counts of the whole list.
    pub fn query(&self, filter: Filter) -> (Vec<Todo>, Counts) {
        let todos = self
            .todos
            .iter()
            .filter(|todo| match filter {
                Filter::All => true,
                Filter::Active => !todo.completed,
                Filter::Completed => todo.completed,
            })
            .cloned()
            .collect();
        (todos, self.counts())
    }

    /// How many todos are active and how many done.
    pub fn counts(&self) -> Counts {
        let completed = self.todos.iter().filter(|todo| todo.completed).count();
        Counts {
            active: (self.todos.len() - completed) as u32,
            completed: completed as u32,
        }
    }

    /// Add a todo with `title`, trimmed.
    ///
    /// # Errors
    ///
    /// [`ModelError::EmptyTitle`] for a title that is empty once
    /// trimmed.
    pub fn add(&mut self, title: &str) -> Result<Todo, ModelError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(ModelError::EmptyTitle);
        }
        self.next += 1;
        let todo = Todo {
            id: format!("t{}", self.next),
            title: title.to_string(),
            completed: false,
        };
        self.todos.push(todo.clone());
        Ok(todo)
    }

    /// Change the title, the completion, or both, of the todo `id`.
    ///
    /// # Errors
    ///
    /// [`ModelError::NotFound`] for an unknown id, and
    /// [`ModelError::EmptyTitle`] for a title that is empty once
    /// trimmed. A refused change changes nothing.
    pub fn update(
        &mut self,
        id: &str,
        title: Option<&str>,
        completed: Option<bool>,
    ) -> Result<Todo, ModelError> {
        let todo = self
            .todos
            .iter_mut()
            .find(|todo| todo.id == id)
            .ok_or(ModelError::NotFound)?;
        if let Some(title) = title {
            let title = title.trim();
            if title.is_empty() {
                return Err(ModelError::EmptyTitle);
            }
            todo.title = title.to_string();
        }
        if let Some(completed) = completed {
            todo.completed = completed;
        }
        Ok(todo.clone())
    }

    /// Remove the todo `id`.
    ///
    /// # Errors
    ///
    /// [`ModelError::NotFound`] for an unknown id.
    pub fn remove(&mut self, id: &str) -> Result<(), ModelError> {
        let before = self.todos.len();
        self.todos.retain(|todo| todo.id != id);
        if self.todos.len() == before {
            return Err(ModelError::NotFound);
        }
        Ok(())
    }

    /// Set every todo's completion to `completed`.
    pub fn toggle_all(&mut self, completed: bool) {
        for todo in &mut self.todos {
            todo.completed = completed;
        }
    }

    /// Remove every todo that is done.
    pub fn clear_completed(&mut self) {
        self.todos.retain(|todo| !todo.completed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_refuses_an_empty_title_and_trims_the_others() {
        let mut list = TodoList::default();
        assert_eq!(list.add("   "), Err(ModelError::EmptyTitle));
        let todo = list.add("  buy milk  ").unwrap();
        assert_eq!(todo.title, "buy milk");
        assert_eq!(list.todos, [todo]);
    }

    #[wcmp_macros::test]
    fn it_filters_and_counts() {
        let mut list = TodoList::default();
        let a = list.add("a").unwrap();
        let b = list.add("b").unwrap();
        list.update(&b.id, None, Some(true)).unwrap();
        let b = list.todos[1].clone();
        let counts = Counts {
            active: 1,
            completed: 1,
        };
        assert_eq!(
            list.query(Filter::All),
            (vec![a.clone(), b.clone()], counts)
        );
        assert_eq!(list.query(Filter::Active), (vec![a], counts));
        assert_eq!(list.query(Filter::Completed), (vec![b], counts));
    }

    #[wcmp_macros::test]
    fn it_refuses_an_unknown_id() {
        let mut list = TodoList::default();
        assert_eq!(
            list.update("t9", Some("x"), None),
            Err(ModelError::NotFound)
        );
        assert_eq!(list.remove("t9"), Err(ModelError::NotFound));
    }

    #[wcmp_macros::test]
    fn it_refuses_an_edit_to_an_empty_title_without_a_change() {
        let mut list = TodoList::default();
        let todo = list.add("keep").unwrap();
        assert_eq!(
            list.update(&todo.id, Some(" "), Some(true)),
            Err(ModelError::EmptyTitle)
        );
        assert_eq!(list.todos, [todo]);
    }

    #[wcmp_macros::test]
    fn it_toggles_every_todo_and_clears_the_completed_ones() {
        let mut list = TodoList::default();
        list.add("a").unwrap();
        list.add("b").unwrap();
        list.toggle_all(true);
        assert_eq!(list.counts().completed, 2);
        list.update("t1", None, Some(false)).unwrap();
        list.clear_completed();
        assert_eq!(list.todos.len(), 1);
        assert_eq!(list.todos[0].id, "t1");
    }

    #[wcmp_macros::test]
    fn it_gives_each_todo_a_new_id_even_after_a_removal() {
        let mut list = TodoList::default();
        let a = list.add("a").unwrap();
        list.remove(&a.id).unwrap();
        let b = list.add("b").unwrap();
        assert_ne!(a.id, b.id);
    }
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Zena source the demo ships: its four elements and its two routes,
//! each as its author wrote it, against the authoring library and with
//! no WIT. A person can edit each in the drawer.

/// The elements, by tag, in the order the page defines them: the
/// children before the element that renders them.
pub const ELEMENTS: [(&str, &str); 4] = [
    ("todo-input", include_str!("../zena/app/todo-input.zena")),
    ("todo-item", include_str!("../zena/app/todo-item.zena")),
    ("todo-footer", include_str!("../zena/app/todo-footer.zena")),
    ("todo-app", include_str!("../zena/app/todo-app.zena")),
];

/// The routes, by pattern, in the order the service worker matches
/// them.
pub const ROUTES: [(&str, &str); 2] = [
    ("/api/todos", include_str!("../zena/app/todos.zena")),
    ("/api/todos/:id", include_str!("../zena/app/todo.zena")),
];

/// The shipped source of the element `tag`, if the demo has one.
pub fn element(tag: &str) -> Option<&'static str> {
    ELEMENTS
        .iter()
        .find(|(name, _)| *name == tag)
        .map(|(_, source)| *source)
}

/// The shipped source of the route `pattern`, if the demo has one.
pub fn route(pattern: &str) -> Option<&'static str> {
    ROUTES
        .iter()
        .find(|(name, _)| *name == pattern)
        .map(|(_, source)| *source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_ships_no_wit_and_no_wasi_import_in_any_source() {
        for (name, source) in ELEMENTS.iter().chain(ROUTES.iter()) {
            assert!(!source.contains("wasi:"), "{name} imports from wasi");
            for keyword in ["world ", "interface ", "package "] {
                assert!(
                    !source
                        .lines()
                        .any(|line| line.trim_start().starts_with(keyword)),
                    "{name} holds WIT"
                );
            }
        }
    }
}

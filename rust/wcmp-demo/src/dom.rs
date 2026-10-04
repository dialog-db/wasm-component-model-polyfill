// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The diff that renders a view into an element's shadow root.
//!
//! The host keeps the last view of each element beside the DOM nodes it
//! made for it. After a render, it compares the new view with the last
//! one, a level at a time. It pairs siblings by key where they have one
//! and by position where they do not ([`view::matches`]). A pair of
//! elements with the same tag keeps its DOM node, and changes its
//! attributes, properties, events, and children in place. A pair of
//! texts keeps its node and changes its text. Any other pair, and a node
//! with no pair, gets a new DOM node. A DOM node with no pair leaves.
//!
//! A property, `checked` or `value`, is compared with the DOM node, not
//! with the last view, because a person changes it without a render.
//!
//! The events of an element are a property of its DOM node, `__demoOn`,
//! an object from DOM event name to handler name. The element's shadow
//! root has one listener for each event name any of its views used,
//! which finds the handler on the event's path.
//!
//! An element whose view handles `resize` reports its size: the host
//! observes it with a `ResizeObserver`, and dispatches a `resize` custom
//! event on it, whose detail is the height of its border box in CSS
//! pixels, once it has been laid out and at each change after. The event
//! does not bubble. A view that stops handling `resize` stops the
//! observation.
//!
//! An element the diff creates with an `autofocus` attribute, or one that
//! gains the attribute, takes the focus once the diff has placed it.
//!
//! The first render after an element had nothing mounted, such as after
//! an error card, or over the slot an element's shadow root starts with,
//! clears the shadow root before it places the view.

use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, Element, Node};

use crate::view::{self, Kind, Property, View};

/// The property of a DOM element that holds its events.
pub const EVENTS: &str = "__demoOn";

/// The property of a DOM element that marks it observed for `resize`.
const RESIZE: &str = "__demoResize";

/// The event an element's view handles to learn its height.
const RESIZE_EVENT: &str = "resize";

thread_local! {
    /// The observer of every element whose view handles `resize`.
    static RESIZE_OBSERVER: Option<web_sys::ResizeObserver> = resize_observer();
}

/// A node of the last view, with the DOM node made for it.
pub struct Mounted {
    view: View,
    node: Node,
    children: Vec<Mounted>,
}

/// Render `views` under `parent`, whose children the diff made from
/// `mounted`, and answer what it mounted. Each DOM event name a view
/// uses goes to `listen`.
///
/// # Errors
///
/// The exception of the DOM call that failed.
#[tracing::instrument(level = "debug", name = "view diff", skip_all, fields(nodes = views.len()))]
pub fn patch(
    document: &Document,
    parent: &Node,
    mounted: Vec<Mounted>,
    views: &[View],
    listen: &mut dyn FnMut(&str),
) -> Result<Vec<Mounted>, JsValue> {
    let mut focus = Vec::new();
    // With nothing mounted, whatever the parent holds is not the view's,
    // such as an error card: the view replaces it.
    if mounted.is_empty() {
        while let Some(child) = parent.first_child() {
            parent.remove_child(&child)?;
        }
    }
    let result = patch_children(document, parent, mounted, views, listen, &mut focus)?;
    for element in focus {
        if let Some(element) = element.dyn_ref::<web_sys::HtmlElement>() {
            element.focus()?;
        }
    }
    Ok(result)
}

fn patch_children(
    document: &Document,
    parent: &Node,
    old: Vec<Mounted>,
    views: &[View],
    listen: &mut dyn FnMut(&str),
    focus: &mut Vec<Element>,
) -> Result<Vec<Mounted>, JsValue> {
    let old_keys: Vec<Option<&str>> = old
        .iter()
        .map(|mounted| mounted.view.key.as_deref())
        .collect();
    let new_keys: Vec<Option<&str>> = views.iter().map(|view| view.key.as_deref()).collect();
    let pairs = view::matches(&old_keys, &new_keys);
    let mut old: Vec<Option<Mounted>> = old.into_iter().map(Some).collect();
    let mut taken: Vec<Option<Mounted>> = pairs
        .iter()
        .map(|pair| pair.and_then(|index| old[index].take()))
        .collect();
    // A node with no pair leaves before the rest move.
    for leaving in old.into_iter().flatten() {
        parent.remove_child(&leaving.node)?;
    }
    let mut result = Vec::with_capacity(views.len());
    for (view, previous) in views.iter().zip(taken.iter_mut()) {
        let mounted = match previous.take() {
            Some(previous) if same_kind(&previous.view, view) => {
                update(document, previous, view, listen, focus)?
            }
            Some(previous) => {
                parent.remove_child(&previous.node)?;
                create(document, view, listen, focus)?
            }
            None => create(document, view, listen, focus)?,
        };
        result.push(mounted);
    }
    // Place the nodes in the view's order, moving only those out of it.
    let mut cursor = parent.first_child();
    for mounted in &result {
        match &cursor {
            Some(current) if current == &mounted.node => cursor = current.next_sibling(),
            _ => {
                parent.insert_before(&mounted.node, cursor.as_ref())?;
            }
        }
    }
    Ok(result)
}

/// Whether the DOM node of `old` can show `new`: two elements with the
/// same tag, or two texts.
fn same_kind(old: &View, new: &View) -> bool {
    match (&old.kind, &new.kind) {
        (Kind::Element { tag: a, .. }, Kind::Element { tag: b, .. }) => a == b,
        (Kind::Text(_), Kind::Text(_)) => true,
        _ => false,
    }
}

/// A new DOM node for `view` and its children.
fn create(
    document: &Document,
    view: &View,
    listen: &mut dyn FnMut(&str),
    focus: &mut Vec<Element>,
) -> Result<Mounted, JsValue> {
    let node: Node = match &view.kind {
        Kind::Text(text) => document.create_text_node(text).into(),
        Kind::Element {
            tag,
            attributes,
            properties,
            events,
        } => {
            let element = document.create_element(tag)?;
            for (name, value) in attributes {
                element.set_attribute(name, value)?;
            }
            set_properties(&element, properties)?;
            set_events(&element, events, listen)?;
            if attributes.iter().any(|(name, _)| name == "autofocus") {
                focus.push(element.clone());
            }
            element.into()
        }
    };
    let children = patch_children(document, &node, Vec::new(), &view.children, listen, focus)?;
    Ok(Mounted {
        view: shallow(view),
        node,
        children,
    })
}

/// Change the DOM node of `old` to show `view`, which has the same
/// kind.
fn update(
    document: &Document,
    old: Mounted,
    view: &View,
    listen: &mut dyn FnMut(&str),
    focus: &mut Vec<Element>,
) -> Result<Mounted, JsValue> {
    match (&old.view.kind, &view.kind) {
        (Kind::Text(before), Kind::Text(after)) => {
            if before != after {
                old.node.set_text_content(Some(after));
            }
        }
        (
            Kind::Element {
                attributes: before, ..
            },
            Kind::Element {
                attributes,
                properties,
                events,
                ..
            },
        ) => {
            let element: &Element = old.node.unchecked_ref();
            for (name, _) in before {
                if !attributes.iter().any(|(other, _)| other == name) {
                    element.remove_attribute(name)?;
                }
            }
            for (name, value) in attributes {
                if element.get_attribute(name).as_deref() != Some(value.as_str()) {
                    element.set_attribute(name, value)?;
                }
            }
            let gained = |name: &str| {
                attributes.iter().any(|(other, _)| other == name)
                    && !before.iter().any(|(other, _)| other == name)
            };
            if gained("autofocus") {
                focus.push(element.clone());
            }
            set_properties(element, properties)?;
            set_events(element, events, listen)?;
        }
        _ => unreachable!("an update joins two nodes of the same kind"),
    }
    let children = patch_children(
        document,
        &old.node,
        old.children,
        &view.children,
        listen,
        focus,
    )?;
    Ok(Mounted {
        view: shallow(view),
        node: old.node,
        children,
    })
}

/// Set each property of `properties` on `element` where the DOM node
/// differs.
fn set_properties(element: &Element, properties: &[(String, Property)]) -> Result<(), JsValue> {
    for (name, property) in properties {
        let value = match property {
            Property::Text(text) => JsValue::from_str(text),
            Property::Flag(flag) => JsValue::from_bool(*flag),
        };
        let current = js_sys::Reflect::get(element, &JsValue::from_str(name))?;
        if current != value {
            js_sys::Reflect::set(element, &JsValue::from_str(name), &value)?;
        }
    }
    Ok(())
}

/// Record the handler of each DOM event of `events` on `element`.
fn set_events(
    element: &Element,
    events: &[(String, String)],
    listen: &mut dyn FnMut(&str),
) -> Result<(), JsValue> {
    let table = js_sys::Object::new();
    for (event, handler) in events {
        js_sys::Reflect::set(
            &table,
            &JsValue::from_str(event),
            &JsValue::from_str(handler),
        )?;
        listen(event);
    }
    js_sys::Reflect::set(element, &JsValue::from_str(EVENTS), &table)?;
    let wanted = events.iter().any(|(event, _)| event == RESIZE_EVENT);
    let observed = js_sys::Reflect::get(element, &JsValue::from_str(RESIZE))?.is_truthy();
    // Observing an element again would report its size again, and the
    // render that report causes would observe it again.
    if wanted != observed {
        RESIZE_OBSERVER.with(|observer| {
            if let Some(observer) = observer {
                if wanted {
                    observer.observe(element);
                } else {
                    observer.unobserve(element);
                }
            }
        });
        js_sys::Reflect::set(
            element,
            &JsValue::from_str(RESIZE),
            &JsValue::from_bool(wanted),
        )?;
    }
    Ok(())
}

/// The observer that dispatches `resize` on each element it observes,
/// with the height of the element's border box, or none where the
/// browser has no `ResizeObserver`.
fn resize_observer() -> Option<web_sys::ResizeObserver> {
    let callback = Closure::<dyn Fn(js_sys::Array)>::new(|entries: js_sys::Array| {
        for entry in entries.iter() {
            let entry: web_sys::ResizeObserverEntry = entry.unchecked_into();
            let Some(size) = entry
                .border_box_size()
                .iter()
                .next()
                .map(JsCast::unchecked_into::<web_sys::ResizeObserverSize>)
            else {
                continue;
            };
            let init = web_sys::CustomEventInit::new();
            init.set_detail(&JsValue::from_str(&size.block_size().to_string()));
            if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict(RESIZE_EVENT, &init) {
                let _ = entry.target().dispatch_event(&event);
            }
        }
    });
    let observer = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
    callback.forget();
    Some(observer)
}

/// `view` without its children, which the mounted children keep.
fn shallow(view: &View) -> View {
    View {
        key: view.key.clone(),
        kind: view.kind.clone(),
        children: Vec::new(),
    }
}

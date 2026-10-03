// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The shelf's editor of Zena source: a text area over its highlighted
//! copy.
//!
//! The text area takes the typing, the selection, and the caret, with its
//! own text transparent. Behind it, a `pre` of the same font, padding,
//! and size shows the same text, highlighted (see `highlight`), and
//! scrolls with it. Neither wraps lines, so a line of the one is a line
//! of the other.
//!
//! Tab indents with two spaces rather than leaving the text area, and
//! Enter keeps the indentation of the line it breaks. Both insert their
//! text with the browser's `insertText` command, so undo takes them back
//! as it takes back typing.

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{Document, Element, HtmlTextAreaElement};

use crate::highlight;

/// The indentation Tab inserts.
const INDENT: &str = "  ";

/// A new editor in `container`, labelled `label`, which calls `on_input`
/// after each change a person makes. Answer its text area.
///
/// # Errors
///
/// The exception of the DOM call that failed.
pub fn mount(
    document: &Document,
    container: &Element,
    label: &str,
    on_input: impl Fn() + 'static,
) -> Result<HtmlTextAreaElement, JsValue> {
    let painted = document.create_element("pre")?;
    painted.set_attribute("class", "highlight")?;
    painted.set_attribute("aria-hidden", "true")?;
    container.append_child(&painted)?;
    let text: HtmlTextAreaElement = document.create_element("textarea")?.unchecked_into();
    for (name, value) in [
        ("spellcheck", "false"),
        ("autocapitalize", "off"),
        ("autocomplete", "off"),
        ("wrap", "off"),
        ("aria-label", label),
    ] {
        text.set_attribute(name, value)?;
    }
    container.append_child(&text)?;

    let typed = text.clone();
    let on_input = Closure::<dyn Fn()>::new(move || {
        paint(&typed);
        on_input();
    });
    text.add_event_listener_with_callback("input", on_input.as_ref().unchecked_ref())?;
    on_input.forget();

    let scrolled = text.clone();
    let on_scroll = Closure::<dyn Fn()>::new(move || follow_scroll(&scrolled));
    text.add_event_listener_with_callback("scroll", on_scroll.as_ref().unchecked_ref())?;
    on_scroll.forget();

    let keyed = text.clone();
    let on_key =
        Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            on_key(&keyed, &event);
        });
    text.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref())?;
    on_key.forget();
    Ok(text)
}

/// Put `source` in the editor of `text`, as a program would rather than
/// a person: no input event follows.
pub fn set_text(text: &HtmlTextAreaElement, source: &str) {
    text.set_value(source);
    paint(text);
}

/// Highlight the text of `text` into the `pre` behind it.
pub fn paint(text: &HtmlTextAreaElement) {
    let Some(painted) = text.previous_element_sibling() else {
        return;
    };
    // A text that ends in a line break ends in an empty line, which a
    // `pre` drops unless something is on it.
    let mut html = highlight::html(&text.value());
    html.push('\n');
    painted.set_inner_html(&html);
    follow_scroll(text);
}

/// Scroll the `pre` behind `text` to where `text` is scrolled.
fn follow_scroll(text: &HtmlTextAreaElement) {
    if let Some(painted) = text.previous_element_sibling() {
        painted.set_scroll_top(text.scroll_top());
        painted.set_scroll_left(text.scroll_left());
    }
}

/// Tab and Enter, as an editor of code takes them.
fn on_key(text: &HtmlTextAreaElement, event: &web_sys::KeyboardEvent) {
    if event.ctrl_key() || event.meta_key() || event.alt_key() || event.is_composing() {
        return;
    }
    match event.key().as_str() {
        "Tab" if !event.shift_key() => {
            event.prevent_default();
            insert(text, INDENT);
        }
        "Enter" => {
            let value = text.value();
            let caret = utf16_to_byte(&value, text.selection_start().ok().flatten().unwrap_or(0));
            let line_start = value[..caret].rfind('\n').map_or(0, |at| at + 1);
            let indent: String = value[line_start..caret]
                .chars()
                .take_while(|character| *character == ' ' || *character == '\t')
                .collect();
            event.prevent_default();
            insert(text, &format!("\n{indent}"));
        }
        _ => {}
    }
}

/// Insert `inserted` at the selection of `text`, as typing would, so the
/// browser's undo takes it back.
fn insert(text: &HtmlTextAreaElement, inserted: &str) {
    let document = text.owner_document();
    let typed = document
        .and_then(|document| document.dyn_into::<web_sys::HtmlDocument>().ok())
        .and_then(|document| {
            document
                .exec_command_with_show_ui_and_value("insertText", false, inserted)
                .ok()
        })
        .unwrap_or(false);
    if !typed {
        // A browser without the command: insert the text without undo.
        let start = text.selection_start().ok().flatten().unwrap_or(0);
        let end = text.selection_end().ok().flatten().unwrap_or(start);
        let _ = text.set_range_text_with_start_and_end(inserted, start, end);
        let caret = start + inserted.encode_utf16().count() as u32;
        let _ = text.set_selection_range(caret, caret);
        if let Ok(event) = web_sys::Event::new("input") {
            let _ = text.dispatch_event(&event);
        }
    }
}

/// The byte offset in `text` of the UTF-16 offset `offset`, which is how
/// the browser counts.
fn utf16_to_byte(text: &str, offset: u32) -> usize {
    let mut units = 0u32;
    for (byte, character) in text.char_indices() {
        if units >= offset {
            return byte;
        }
        units += character.len_utf16() as u32;
    }
    text.len()
}

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
//! Between the two, a second `pre` of the same text, transparent, draws a
//! wavy line under each range a diagnostic names (`set_marks`).
//!
//! Tab indents with two spaces rather than leaving the text area, and
//! Enter keeps the indentation of the line it breaks. Both insert their
//! text with the browser's `insertText` command, so undo takes them back
//! as it takes back typing.
//!
//! The browser counts a text area's offsets in UTF-16 code units, and
//! Zena in UTF-8 bytes. `utf16_to_byte` and `byte_to_utf16` convert.

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
    let marks = document.create_element("pre")?;
    marks.set_attribute("class", "marks")?;
    marks.set_attribute("aria-hidden", "true")?;
    container.append_child(&marks)?;
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

/// Highlight the text of `text` into the `pre` behind it, and clear its
/// marks, which named the text it had.
pub fn paint(text: &HtmlTextAreaElement) {
    if let Some(marks) = text.previous_element_sibling() {
        marks.set_inner_html("");
    }
    let Some(painted) = text
        .previous_element_sibling()
        .and_then(|marks| marks.previous_element_sibling())
    else {
        return;
    };
    // A text that ends in a line break ends in an empty line, which a
    // `pre` drops unless something is on it.
    let mut html = highlight::html(&text.value());
    html.push('\n');
    painted.set_inner_html(&html);
    follow_scroll(text);
}

/// Draw a wavy line under each range of `marks` in the text of `text`:
/// byte ranges of its text, each with the class that colors it.
pub fn set_marks(text: &HtmlTextAreaElement, marks: &[(usize, usize, &str)]) {
    let Some(layer) = text.previous_element_sibling() else {
        return;
    };
    let value = text.value();
    let mut ranges: Vec<(usize, usize, &str)> = marks
        .iter()
        .map(|(start, end, class)| {
            let start = floor_char(&value, (*start).min(value.len()));
            // A mark with no length still marks a character.
            let end = if *end > start {
                floor_char(&value, (*end).min(value.len()))
            } else {
                next_char(&value, start)
            };
            (start, end.max(start), *class)
        })
        .collect();
    ranges.sort_by_key(|range| range.0);
    let mut html = String::with_capacity(value.len() + 64 * ranges.len());
    let mut at = 0;
    for (start, end, class) in ranges {
        if start < at || start == end {
            continue;
        }
        escape_into(&mut html, &value[at..start]);
        html.push_str("<span class=\"");
        html.push_str(class);
        html.push_str("\">");
        escape_into(&mut html, &value[start..end]);
        html.push_str("</span>");
        at = end;
    }
    escape_into(&mut html, &value[at..]);
    html.push('\n');
    layer.set_inner_html(&html);
    follow_scroll(text);
}

/// Scroll the layers behind `text` to where `text` is scrolled.
fn follow_scroll(text: &HtmlTextAreaElement) {
    let mut layer = text.previous_element_sibling();
    while let Some(behind) = layer {
        behind.set_scroll_top(text.scroll_top());
        behind.set_scroll_left(text.scroll_left());
        layer = behind.previous_element_sibling();
    }
}

/// The metrics of the text in `text`: the width of a character and the
/// height of a line, and the padding on the left and at the top, in CSS
/// pixels. The font is monospaced, so every character is as wide.
fn metrics(text: &HtmlTextAreaElement) -> Option<(f64, f64, f64, f64)> {
    let window = web_sys::window()?;
    let style = window.get_computed_style(text).ok()??;
    let pixels = |name: &str| {
        style
            .get_property_value(name)
            .ok()
            .and_then(|value| value.trim_end_matches("px").parse::<f64>().ok())
    };
    let document = text.owner_document()?;
    let probe = document.create_element("span").ok()?;
    let _ = probe.set_attribute(
        "style",
        &format!(
            "position: absolute; visibility: hidden; white-space: pre; font: {}",
            style.get_property_value("font").ok()?
        ),
    );
    probe.set_text_content(Some(&"M".repeat(64)));
    document.body()?.append_child(&probe).ok()?;
    let width = probe.get_bounding_client_rect().width() / 64.0;
    probe.remove();
    Some((
        width,
        pixels("line-height")?,
        pixels("padding-left")?,
        pixels("padding-top")?,
    ))
}

/// The UTF-16 offset of the character at the point `x`, `y` of the
/// viewport in the text of `text`, if the point is on a character.
pub fn offset_at_point(text: &HtmlTextAreaElement, x: f64, y: f64) -> Option<u32> {
    let (width, height, left, top) = metrics(text)?;
    let rect = text.get_bounding_client_rect();
    let column = (x - rect.left() - left + f64::from(text.scroll_left())) / width;
    let row = (y - rect.top() - top + f64::from(text.scroll_top())) / height;
    if column < 0.0 || row < 0.0 {
        return None;
    }
    let (row, column) = (row as usize, column as usize);
    let value = text.value();
    let line = value.split('\n').nth(row)?;
    let line_start: usize = value.split('\n').take(row).map(|line| line.len() + 1).sum();
    let (byte, _) = line.char_indices().nth(column)?;
    Some(byte_to_utf16(&value, line_start + byte))
}

/// The point of the viewport just below the caret of `text`, where a
/// popup about the caret goes.
pub fn caret_point(text: &HtmlTextAreaElement) -> Option<(f64, f64)> {
    let (width, height, left, top) = metrics(text)?;
    let rect = text.get_bounding_client_rect();
    let value = text.value();
    let caret = utf16_to_byte(&value, text.selection_start().ok()??);
    let before = &value[..caret];
    let row = before.matches('\n').count();
    let column = before[before.rfind('\n').map_or(0, |at| at + 1)..]
        .chars()
        .count();
    Some((
        rect.left() + left + column as f64 * width - f64::from(text.scroll_left()),
        rect.top() + top + (row + 1) as f64 * height - f64::from(text.scroll_top()),
    ))
}

/// Put the caret of `text` at the UTF-16 offset `offset`, and bring it
/// into view.
pub fn place_caret(text: &HtmlTextAreaElement, offset: u32) {
    let _ = text.focus();
    let _ = text.set_selection_range(offset, offset);
    // Blurring and focusing again makes the browser scroll the caret into
    // view.
    let _ = text.blur();
    let _ = text.focus();
}

/// Replace the UTF-16 range `start` to `end` of `text` with `replacement`,
/// as typing would, so the browser's undo takes it back.
pub fn replace(text: &HtmlTextAreaElement, start: u32, end: u32, replacement: &str) {
    let _ = text.set_selection_range(start, end);
    insert(text, replacement);
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
pub fn utf16_to_byte(text: &str, offset: u32) -> usize {
    let mut units = 0u32;
    for (byte, character) in text.char_indices() {
        if units >= offset {
            return byte;
        }
        units += character.len_utf16() as u32;
    }
    text.len()
}

/// The UTF-16 offset in `text` of the byte offset `offset`.
pub fn byte_to_utf16(text: &str, offset: usize) -> u32 {
    text[..floor_char(text, offset.min(text.len()))]
        .chars()
        .map(|character| character.len_utf16() as u32)
        .sum()
}

/// The start of the character the byte offset `at` of `text` falls in.
fn floor_char(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The byte offset just past the character at `at` of `text`.
fn next_char(text: &str, at: usize) -> usize {
    text[at..]
        .chars()
        .next()
        .map_or(at, |character| at + character.len_utf8())
}

/// Append `text` to `out`, with the characters HTML gives a meaning
/// escaped.
fn escape_into(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

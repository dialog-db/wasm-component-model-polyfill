// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What Zena's language service adds to an editor of the shelf: live
//! diagnostics, hover, completion, going to a definition, and formatting.
//!
//! Every answer is Zena's own language service, which the compiler
//! component exports, about the program a save would compile: the glue a
//! helper writes, the author's source beside it, and the world of an
//! element or a route. The service answers about its last check, so each
//! query first checks the text the editor holds, when the last check was
//! of another text.
//!
//! - A check runs a moment after the last keystroke. Its diagnostics in
//!   the editor's file mark their ranges with a wavy line, and list below
//!   the editor, where a click moves the caret to one.
//! - Resting the pointer on the text shows what is under it, and the
//!   message of a diagnostic there.
//! - Ctrl-Space, or a `.`, opens the completions at the caret, which the
//!   word before the caret filters. The arrows choose one, Enter or Tab
//!   takes it, and Escape closes them.
//! - Ctrl-click or Cmd-click, or F12 at the caret, goes to a definition:
//!   in this file, or in another file of the shelf, whose tab it opens.
//!   A definition elsewhere, such as in the authoring library, is named.
//! - Format prints the source as Zena's formatter does, as one change
//!   that undo takes back.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlElement, HtmlTextAreaElement, KeyboardEvent, MouseEvent};

use crate::context::Context;
use crate::editor;
use crate::glue::{self, Compile};
use crate::language::{Completion, Diagnostic, Severity};

/// How long after the last keystroke a check runs, in milliseconds.
const CHECK_DELAY_MS: i32 = 350;

/// How long the pointer rests before a hover shows, in milliseconds.
const HOVER_DELAY_MS: i32 = 400;

/// How many completions the popup lists.
const COMPLETIONS_SHOWN: usize = 50;

/// Opens the tab of another source of the shelf by its file, and
/// answers its editor's text area.
type OpenFile = Box<dyn Fn(&str) -> Option<HtmlTextAreaElement>>;

/// The language service of one editor of the shelf.
pub struct Assist {
    /// `element` or `route`.
    kind: String,
    /// The tag or the pattern.
    name: String,
    /// The path of the author's source, as the program names it.
    path: String,
    context: Rc<Context>,
    text: HtmlTextAreaElement,
    tooltip: HtmlElement,
    popup: HtmlElement,
    problems: Element,
    /// Opens the tab of the file of another source of the shelf, and
    /// answers its editor's text area.
    open_file: OpenFile,
    state: RefCell<State>,
}

/// What an editor's language service keeps between events.
#[derive(Default)]
struct State {
    /// The text of the last check, which the service's answers are about.
    checked: Option<String>,
    /// The diagnostics of the last check, in this editor's file.
    diagnostics: Vec<Diagnostic>,
    /// The timer of the next check.
    check_timer: Option<i32>,
    /// The timer of the next hover.
    hover_timer: Option<i32>,
    /// The completions the popup lists, and which one is chosen.
    completions: Vec<Completion>,
    chosen: usize,
}

impl Assist {
    /// The language service of the editor of `kind` `name`, whose text
    /// area is `text`, in its `container`. Its problems list under the
    /// editor, in `problems`. `open_file` opens another source of the
    /// shelf by its file.
    ///
    /// # Errors
    ///
    /// The exception of the DOM call that failed.
    pub fn attach(
        kind: &str,
        name: &str,
        context: Rc<Context>,
        text: HtmlTextAreaElement,
        container: &Element,
        problems: Element,
        open_file: impl Fn(&str) -> Option<HtmlTextAreaElement> + 'static,
    ) -> Result<Rc<Self>, JsValue> {
        let document = text
            .owner_document()
            .ok_or("the editor is in no document")?;
        let tooltip: HtmlElement = document.create_element("div")?.unchecked_into();
        tooltip.set_attribute("class", "tooltip")?;
        tooltip.set_attribute("role", "tooltip")?;
        tooltip.set_attribute("hidden", "")?;
        container.append_child(&tooltip)?;
        let popup: HtmlElement = document.create_element("ul")?.unchecked_into();
        popup.set_attribute("class", "completions")?;
        popup.set_attribute("role", "listbox")?;
        popup.set_attribute("hidden", "")?;
        container.append_child(&popup)?;
        let path = if kind == "element" {
            glue::element_file(name)
        } else {
            glue::route_file(name)
        };
        let assist = Rc::new(Assist {
            kind: kind.to_string(),
            name: name.to_string(),
            path,
            context,
            text,
            tooltip,
            popup,
            problems,
            open_file: Box::new(open_file),
            state: RefCell::new(State::default()),
        });
        assist.listen(container)?;
        Ok(assist)
    }

    /// The path of the author's source, as the program names it.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The listeners of the editor.
    fn listen(self: &Rc<Self>, container: &Element) -> Result<(), JsValue> {
        let assist = self.clone();
        let on_input =
            Closure::<dyn Fn(web_sys::InputEvent)>::new(move |event: web_sys::InputEvent| {
                assist.hide_tooltip();
                assist.schedule_check();
                if assist.popup_open() {
                    assist.filter_completions();
                } else if event.data().as_deref() == Some(".") {
                    assist.clone().open_completions();
                }
            });
        self.text
            .add_event_listener_with_callback("input", on_input.as_ref().unchecked_ref())?;
        on_input.forget();

        // In the capture phase on the container, ahead of the editor's
        // own keys, so the popup takes the arrows, Enter, and Tab.
        let assist = self.clone();
        let on_key = Closure::<dyn Fn(KeyboardEvent)>::new(move |event: KeyboardEvent| {
            if assist.on_key(&event) {
                event.prevent_default();
                event.stop_propagation();
            }
        });
        container.add_event_listener_with_callback_and_bool(
            "keydown",
            on_key.as_ref().unchecked_ref(),
            true,
        )?;
        on_key.forget();

        let assist = self.clone();
        let on_move = Closure::<dyn Fn(MouseEvent)>::new(move |event: MouseEvent| {
            assist.schedule_hover(f64::from(event.client_x()), f64::from(event.client_y()));
        });
        self.text
            .add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())?;
        on_move.forget();

        let assist = self.clone();
        let on_leave = Closure::<dyn Fn()>::new(move || {
            assist.cancel_hover();
            assist.hide_tooltip();
        });
        self.text
            .add_event_listener_with_callback("mouseleave", on_leave.as_ref().unchecked_ref())?;
        on_leave.forget();

        let assist = self.clone();
        let on_click = Closure::<dyn Fn(MouseEvent)>::new(move |event: MouseEvent| {
            assist.close_completions();
            if event.ctrl_key() || event.meta_key() {
                let point = (f64::from(event.client_x()), f64::from(event.client_y()));
                if let Some(offset) = editor::offset_at_point(&assist.text, point.0, point.1) {
                    event.prevent_default();
                    assist.clone().go_to_definition(offset);
                }
            }
        });
        self.text
            .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())?;
        on_click.forget();

        let assist = self.clone();
        let on_scroll = Closure::<dyn Fn()>::new(move || {
            assist.hide_tooltip();
            assist.close_completions();
        });
        self.text
            .add_event_listener_with_callback("scroll", on_scroll.as_ref().unchecked_ref())?;
        on_scroll.forget();

        // A click on a listed problem moves the caret to it.
        let text = self.text.clone();
        let on_problem = Closure::<dyn Fn(MouseEvent)>::new(move |event: MouseEvent| {
            let start = event
                .target()
                .and_then(|target| target.dyn_into::<Element>().ok())
                .and_then(|target| target.closest("li[data-start]").ok().flatten())
                .and_then(|item| item.get_attribute("data-start"))
                .and_then(|start| start.parse::<usize>().ok());
            if let Some(start) = start {
                let offset = editor::byte_to_utf16(&text.value(), start);
                editor::place_caret(&text, offset);
            }
        });
        self.problems
            .add_event_listener_with_callback("click", on_problem.as_ref().unchecked_ref())?;
        on_problem.forget();

        let assist = self.clone();
        let on_blur = Closure::<dyn Fn()>::new(move || assist.close_completions());
        self.text
            .add_event_listener_with_callback("blur", on_blur.as_ref().unchecked_ref())?;
        on_blur.forget();
        Ok(())
    }

    /// The keys of the language service: whether it took `event`.
    fn on_key(self: &Rc<Self>, event: &KeyboardEvent) -> bool {
        let key = event.key();
        if self.popup_open() {
            match key.as_str() {
                "ArrowDown" | "ArrowUp" => {
                    let down = key == "ArrowDown";
                    self.move_choice(if down { 1 } else { -1 });
                    return true;
                }
                "Enter" | "Tab" => {
                    self.accept_completion();
                    return true;
                }
                "Escape" => {
                    self.close_completions();
                    return true;
                }
                _ => {}
            }
        }
        if (event.ctrl_key() || event.meta_key()) && key == " " {
            self.clone().open_completions();
            return true;
        }
        if key == "F12" {
            if let Ok(Some(caret)) = self.text.selection_start() {
                self.clone().go_to_definition(caret);
            }
            return true;
        }
        false
    }

    /// Check the editor's text a moment after the last change.
    pub fn schedule_check(self: &Rc<Self>) {
        let Some(window) = web_sys::window() else {
            return;
        };
        if let Some(timer) = self.state.borrow_mut().check_timer.take() {
            window.clear_timeout_with_handle(timer);
        }
        let assist = self.clone();
        let fire = Closure::once_into_js(move || {
            assist.state.borrow_mut().check_timer = None;
            spawn_local(async move {
                let _ = assist.check().await;
            });
        });
        let timer = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                fire.unchecked_ref(),
                CHECK_DELAY_MS,
            )
            .ok();
        self.state.borrow_mut().check_timer = timer;
    }

    /// Check the editor's text now, unless the last check was of it, and
    /// show its diagnostics. Answer whether the service's answers are
    /// about the text the editor holds.
    pub async fn check(self: &Rc<Self>) -> bool {
        let source = self.text.value();
        if self.state.borrow().checked.as_deref() == Some(source.as_str()) {
            return true;
        }
        let compile = if self.kind == "element" {
            Compile::element(&self.name, &source)
        } else {
            Ok(Compile::route(&self.name, &source))
        };
        let diagnostics = match compile {
            // Without an element to compile, the program has no glue,
            // and the diagnostic is the helper's.
            Err(message) => vec![Diagnostic {
                file: self.path.clone(),
                start: 0,
                length: 0,
                line: 1,
                column: 1,
                severity: Severity::Error,
                message: message
                    .split_once(" Error: ")
                    .map_or(message.clone(), |(_, text)| text.to_string()),
            }],
            Ok(compile) => {
                let compiler = self.context.compiler();
                let mut compiler = compiler.lock().await;
                match compiler.check(&compile.request()).await {
                    Ok(diagnostics) => diagnostics,
                    Err(error) => {
                        tracing::warn!(file = %self.path, "the check failed: {error}");
                        return false;
                    }
                }
            }
        };
        // A later change makes this check stale; the check of that
        // change shows its own.
        if self.text.value() != source {
            return false;
        }
        let mine: Vec<Diagnostic> = diagnostics
            .into_iter()
            .filter(|diagnostic| diagnostic.file == self.path)
            .collect();
        {
            let mut state = self.state.borrow_mut();
            state.checked = Some(source);
            state.diagnostics = mine;
        }
        self.show_diagnostics();
        true
    }

    /// Mark the diagnostics of the last check in the editor, and list
    /// them below it.
    fn show_diagnostics(self: &Rc<Self>) {
        let state = self.state.borrow();
        let marks: Vec<(usize, usize, &str)> = state
            .diagnostics
            .iter()
            .map(|diagnostic| {
                let start = diagnostic.start as usize;
                (
                    start,
                    start + diagnostic.length as usize,
                    class_of(diagnostic.severity),
                )
            })
            .collect();
        editor::set_marks(&self.text, &marks);
        self.problems.set_inner_html("");
        let Some(document) = self.text.owner_document() else {
            return;
        };
        for diagnostic in &state.diagnostics {
            let Ok(item) = document.create_element("li") else {
                continue;
            };
            let _ = item.set_attribute("class", class_of(diagnostic.severity));
            item.set_text_content(Some(&format!(
                "{}:{}  {}",
                diagnostic.line, diagnostic.column, diagnostic.message
            )));
            let _ = item.set_attribute("data-start", &diagnostic.start.to_string());
            let _ = self.problems.append_child(&item);
        }
        if state.diagnostics.is_empty() {
            let _ = self.problems.set_attribute("hidden", "");
        } else {
            let _ = self.problems.remove_attribute("hidden");
        }
    }

    /// Show what is under the point `x`, `y` once the pointer rests there.
    fn schedule_hover(self: &Rc<Self>, x: f64, y: f64) {
        self.cancel_hover();
        if self.popup_open() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let assist = self.clone();
        let fire = Closure::once_into_js(move || {
            assist.state.borrow_mut().hover_timer = None;
            spawn_local(async move {
                assist.hover(x, y).await;
            });
        });
        let timer = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                fire.unchecked_ref(),
                HOVER_DELAY_MS,
            )
            .ok();
        self.state.borrow_mut().hover_timer = timer;
    }

    /// Forget a hover that has not shown yet.
    fn cancel_hover(&self) {
        if let Some(timer) = self.state.borrow_mut().hover_timer.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(timer);
        }
    }

    /// Show what is at the point `x`, `y`: the message of a diagnostic
    /// there, and what the service says of the symbol.
    async fn hover(self: &Rc<Self>, x: f64, y: f64) {
        let Some(offset) = editor::offset_at_point(&self.text, x, y) else {
            self.hide_tooltip();
            return;
        };
        let value = self.text.value();
        let byte = editor::utf16_to_byte(&value, offset);
        if value[byte..].chars().next().is_none_or(char::is_whitespace) {
            self.hide_tooltip();
            return;
        }
        if !self.check().await {
            return;
        }
        let messages: Vec<String> = self
            .state
            .borrow()
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                let start = diagnostic.start as usize;
                byte >= start && byte < start + (diagnostic.length as usize).max(1)
            })
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        let found = {
            let compiler = self.context.compiler();
            let mut compiler = compiler.lock().await;
            compiler.hover(&self.path, byte as u32).await.ok().flatten()
        };
        if self.text.value() != value {
            return;
        }
        let mut lines: Vec<(&str, String)> = messages
            .into_iter()
            .map(|message| ("message", message))
            .collect();
        if let Some(found) = found {
            let label = if found.label.is_empty() {
                found.detail.clone()
            } else {
                found.label.clone()
            };
            if !label.is_empty() {
                lines.push(("signature", label));
            }
            if !found.doc.is_empty() {
                lines.push(("doc", found.doc));
            }
        }
        if lines.is_empty() {
            self.hide_tooltip();
            return;
        }
        self.show_tooltip(x, y, &lines);
    }

    /// Show `lines` in the tooltip, near the point `x`, `y`: each with
    /// the class that styles it.
    fn show_tooltip(&self, x: f64, y: f64, lines: &[(&str, String)]) {
        self.tooltip.set_inner_html("");
        let Some(document) = self.text.owner_document() else {
            return;
        };
        for (class, line) in lines {
            if let Ok(part) = document.create_element("div") {
                let _ = part.set_attribute("class", class);
                part.set_text_content(Some(line));
                let _ = self.tooltip.append_child(&part);
            }
        }
        self.place(&self.tooltip, x, y + 18.0);
        let _ = self.tooltip.remove_attribute("hidden");
    }

    /// Hide the tooltip.
    fn hide_tooltip(&self) {
        let _ = self.tooltip.set_attribute("hidden", "");
    }

    /// Place `floating` in the editor at the point `x`, `y` of the
    /// viewport, or above it when it would leave the editor below.
    fn place(&self, floating: &HtmlElement, x: f64, y: f64) {
        let Some(container) = self.text.parent_element() else {
            return;
        };
        let rect = container.get_bounding_client_rect();
        let left = (x - rect.left()).clamp(0.0, (rect.width() - 280.0).max(0.0));
        let top = y - rect.top();
        let style = floating.style();
        let _ = style.set_property("left", &format!("{left}px"));
        if top + 160.0 > rect.height() && top > rect.height() / 2.0 {
            let _ = style.set_property("top", "auto");
            let _ = style.set_property("bottom", &format!("{}px", rect.height() - top + 22.0));
        } else {
            let _ = style.set_property("bottom", "auto");
            let _ = style.set_property("top", &format!("{top}px"));
        }
    }

    /// Ask the service for the completions at the caret, and list them.
    fn open_completions(self: Rc<Self>) {
        spawn_local(async move {
            if !self.check().await {
                return;
            }
            let value = self.text.value();
            let Ok(Some(caret)) = self.text.selection_start() else {
                return;
            };
            let byte = editor::utf16_to_byte(&value, caret);
            let items = {
                let compiler = self.context.compiler();
                let mut compiler = compiler.lock().await;
                compiler
                    .complete(&self.path, byte as u32)
                    .await
                    .unwrap_or_default()
            };
            if self.text.value() != value {
                return;
            }
            self.state.borrow_mut().completions = items;
            self.filter_completions();
        });
    }

    /// The word before the caret, which filters the completions, and the
    /// UTF-16 offset where it starts.
    fn word_before_caret(&self) -> (String, u32) {
        let value = self.text.value();
        let caret = self.text.selection_start().ok().flatten().unwrap_or(0);
        let byte = editor::utf16_to_byte(&value, caret);
        let start = value[..byte]
            .char_indices()
            .rev()
            .take_while(|(_, character)| character.is_alphanumeric() || *character == '_')
            .last()
            .map_or(byte, |(at, _)| at);
        (
            value[start..byte].to_string(),
            editor::byte_to_utf16(&value, start),
        )
    }

    /// List the completions the word before the caret begins, or close
    /// the list when none does.
    fn filter_completions(&self) {
        let (word, _) = self.word_before_caret();
        let lower = word.to_lowercase();
        let shown: Vec<Completion> = self
            .state
            .borrow()
            .completions
            .iter()
            .filter(|item| item.label.to_lowercase().starts_with(&lower) && item.label != word)
            .take(COMPLETIONS_SHOWN)
            .cloned()
            .collect();
        if shown.is_empty() {
            self.close_completions();
            return;
        }
        let Some(document) = self.text.owner_document() else {
            return;
        };
        self.popup.set_inner_html("");
        for (index, item) in shown.iter().enumerate() {
            let Ok(row) = document.create_element("li") else {
                continue;
            };
            let _ = row.set_attribute("role", "option");
            let _ = row.set_attribute("data-label", &item.label);
            let _ = row.set_attribute("aria-selected", if index == 0 { "true" } else { "false" });
            for (class, text) in [
                ("kind", kind_of(item.kind)),
                ("label", item.label.as_str()),
                ("detail", item.detail.as_str()),
            ] {
                if let Ok(part) = document.create_element("span") {
                    let _ = part.set_attribute("class", class);
                    part.set_text_content(Some(text));
                    let _ = row.append_child(&part);
                }
            }
            let _ = self.popup.append_child(&row);
        }
        self.state.borrow_mut().chosen = 0;
        if let Some((x, y)) = editor::caret_point(&self.text) {
            self.place(&self.popup, x, y);
        }
        let _ = self.popup.remove_attribute("hidden");
    }

    /// Whether the completions are listed.
    fn popup_open(&self) -> bool {
        !self.popup.has_attribute("hidden")
    }

    /// Choose the completion `step` rows from the chosen one.
    fn move_choice(&self, step: i32) {
        let rows = self.popup.children();
        let count = rows.length() as i32;
        if count == 0 {
            return;
        }
        let chosen = (self.state.borrow().chosen as i32 + step).rem_euclid(count);
        self.state.borrow_mut().chosen = chosen as usize;
        for index in 0..rows.length() {
            if let Some(row) = rows.item(index) {
                let selected = index as i32 == chosen;
                let _ = row.set_attribute("aria-selected", if selected { "true" } else { "false" });
                if selected {
                    row.scroll_into_view_with_bool(false);
                }
            }
        }
    }

    /// Put the chosen completion in place of the word before the caret.
    fn accept_completion(&self) {
        let chosen = self.state.borrow().chosen;
        let label = self
            .popup
            .children()
            .item(chosen as u32)
            .and_then(|row| row.get_attribute("data-label"));
        self.close_completions();
        let Some(label) = label else {
            return;
        };
        let (_, start) = self.word_before_caret();
        let caret = self.text.selection_start().ok().flatten().unwrap_or(start);
        editor::replace(&self.text, start, caret, &label);
    }

    /// Close the list of completions.
    fn close_completions(&self) {
        let _ = self.popup.set_attribute("hidden", "");
    }

    /// Go to the definition of what is at the UTF-16 offset `offset`.
    fn go_to_definition(self: Rc<Self>, offset: u32) {
        spawn_local(async move {
            if !self.check().await {
                return;
            }
            let value = self.text.value();
            let byte = editor::utf16_to_byte(&value, offset);
            let found = {
                let compiler = self.context.compiler();
                let mut compiler = compiler.lock().await;
                compiler
                    .definition(&self.path, byte as u32)
                    .await
                    .ok()
                    .flatten()
            };
            let Some(location) = found else {
                return;
            };
            let target = if location.file == self.path {
                Some(self.text.clone())
            } else {
                (self.open_file)(&location.file)
            };
            match target {
                Some(text) => {
                    let offset = editor::byte_to_utf16(&text.value(), location.start as usize);
                    editor::place_caret(&text, offset);
                }
                None => {
                    if let Some((x, y)) = editor::caret_point(&self.text) {
                        self.show_tooltip(
                            x,
                            y - 18.0,
                            &[(
                                "signature",
                                format!(
                                    "Defined in {}:{}:{}",
                                    location.file, location.line, location.column
                                ),
                            )],
                        );
                    }
                }
            }
        });
    }

    /// Print the editor's source as Zena's formatter does, as one change
    /// that undo takes back, or list the error that stopped it.
    pub async fn format(self: &Rc<Self>) {
        let value = self.text.value();
        let formatted = {
            let compiler = self.context.compiler();
            let mut compiler = compiler.lock().await;
            compiler.format(&value).await
        };
        match formatted {
            Ok(Ok(text)) if text != value && self.text.value() == value => {
                let end = editor::byte_to_utf16(&value, value.len());
                editor::replace(&self.text, 0, end, &text);
            }
            Ok(Err(message)) => {
                self.state.borrow_mut().diagnostics = vec![Diagnostic {
                    file: self.path.clone(),
                    start: 0,
                    length: 0,
                    line: 1,
                    column: 1,
                    severity: Severity::Error,
                    message: format!("The formatter stopped: {message}"),
                }];
                self.show_diagnostics();
            }
            Err(error) => tracing::warn!(file = %self.path, "the format failed: {error}"),
            _ => {}
        }
    }
}

/// The class that colors a diagnostic of `severity`.
fn class_of(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "information",
    }
}

/// The short name of the Language Server Protocol's completion kind
/// `kind`.
fn kind_of(kind: u32) -> &'static str {
    match kind {
        2 => "method",
        3 => "function",
        7 => "class",
        14 => "keyword",
        _ => "value",
    }
}

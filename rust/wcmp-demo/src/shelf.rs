// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The shelf: a collapsible panel along the bottom of the page that
//! shows and edits every Zena source of the demo, one tab for each file.
//!
//! It is plain Rust and HTML, not an element, so a bad edit cannot break
//! the tool that fixes it. Its bar, always shown, holds the toggle that
//! opens and closes the panel and a tab for each element and each route,
//! whose dot tells an edit not saved and a failure apart. The panel
//! shows the active tab's source in a text area, the times of its last
//! Zena compile, Wasm compile, and instantiation, the compiler's
//! diagnostics after a failed compile, and for an element the count of
//! connected elements and of instances. Its controls save an edit, also
//! with Ctrl-S or Cmd-S, reset the source to the one the demo ships, and
//! restart an element after a trap. The viewer's browser keeps whether
//! the shelf is open and which tab is active.
//!
//! Saving an element writes the source to IndexedDB and compiles it on
//! the page. Saving a route writes the source and sends `route-changed`
//! to the service worker, which compiles it and answers with
//! `route-compiled`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Document, Element, HtmlTextAreaElement};

use crate::assist::Assist;
use crate::context::Context;
use crate::editor;
use crate::elements::{self, TagStatus};
use crate::idb::{self, Database};
use crate::page;
use crate::sources;

/// What the shelf keeps between refreshes.
#[derive(Default)]
struct State {
    database: Option<Database>,
    /// The page's context, whose compiler answers the language service.
    context: Option<Rc<Context>>,
    /// The language service of each section's editor, by section id.
    assists: HashMap<String, Rc<Assist>>,
    /// The last status of each route, from the service worker.
    routes: HashMap<String, JsValue>,
    /// The sources a person changed in a text area and has not saved.
    dirty: HashSet<String>,
    /// The ticks of an open shelf, which asks the worker for the routes'
    /// statuses every [`ROUTE_TICKS`] of them.
    ticks: u32,
    /// Whether a request for the routes' statuses is on its way.
    loading: bool,
}

/// How many ticks of an open shelf pass between two requests for the
/// routes' statuses: a status a broadcast missed shows within a second.
const ROUTE_TICKS: u32 = 4;

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

/// The id of a section of the shelf: `element:<tag>` or
/// `route:<pattern>`.
fn section_id(kind: &str, name: &str) -> String {
    format!("{kind}:{name}")
}

/// The key the shelf keeps its state under in `localStorage`: whether
/// it is open, and its active tab.
const PREFERENCES: &str = "wcmp-demo-shelf";

/// Build the shelf, its bar of tabs and its panel, and keep it up to
/// date.
///
/// # Errors
///
/// The reason a browser API failed.
pub fn mount(database: Database, context: Rc<Context>) -> Result<(), String> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.database = Some(database);
        state.context = Some(context);
    });
    let page = document()?;
    let body = page.body().ok_or("the page has no body")?;

    let shelf = make(
        &page,
        "aside",
        &[("id", "shelf"), ("aria-label", "Zena sources")],
    )?;
    let bar = make(&page, "div", &[("class", "shelf-bar")])?;
    let toggle = make(
        &page,
        "button",
        &[
            ("id", "shelf-toggle"),
            ("type", "button"),
            ("aria-controls", "shelf-panel"),
            ("aria-expanded", "false"),
            ("title", "Show or hide the Zena sources"),
        ],
    )?;
    toggle.set_text_content(Some("Zena sources"));
    bar.append_child(&toggle).map_err(js_text)?;
    let tabs = make(
        &page,
        "div",
        &[
            ("class", "shelf-tabs"),
            ("role", "tablist"),
            ("aria-label", "Zena files"),
        ],
    )?;
    let panel = make(&page, "div", &[("id", "shelf-panel")])?;
    let _ = panel.set_attribute("hidden", "");
    let files = sources::ELEMENTS
        .iter()
        .map(|(tag, _)| ("element", *tag))
        .chain(
            sources::ROUTES
                .iter()
                .map(|(pattern, _)| ("route", *pattern)),
        );
    for (kind, name) in files {
        let tab = tab(&page, kind, name)?;
        tabs.append_child(&tab).map_err(js_text)?;
        let section = section(&page, kind, name)?;
        panel.append_child(&section).map_err(js_text)?;
    }
    bar.append_child(&tabs).map_err(js_text)?;
    shelf.append_child(&bar).map_err(js_text)?;
    shelf.append_child(&panel).map_err(js_text)?;
    body.append_child(&shelf).map_err(js_text)?;

    let on_toggle = Closure::<dyn Fn()>::new(|| set_open(!is_open()));
    toggle
        .add_event_listener_with_callback("click", on_toggle.as_ref().unchecked_ref())
        .map_err(js_text)?;
    on_toggle.forget();

    // Ctrl-S or Cmd-S in the panel saves the active source.
    let on_key = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(|event: web_sys::KeyboardEvent| {
        if (event.ctrl_key() || event.meta_key()) && event.key() == "s" {
            event.prevent_default();
            if let Some(id) = active() {
                spawn_local(async move {
                    if let Some((kind, name)) = id.split_once(':')
                        && let Err(error) = act(kind, name, "save").await
                    {
                        tracing::error!(%kind, %name, "{error}");
                    }
                    refresh();
                });
            }
        }
    });
    panel
        .add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref())
        .map_err(js_text)?;
    on_key.forget();

    let (open, chosen) = preferences();
    let chosen = chosen
        .filter(|chosen| find_tab(&page, chosen).is_some())
        .unwrap_or_else(|| section_id("element", sources::ELEMENTS[0].0));
    select(&chosen);
    set_open(open);

    // The worker tells every page about each compile and each trap of a
    // route.
    let on_message =
        Closure::<dyn Fn(web_sys::MessageEvent)>::new(|event: web_sys::MessageEvent| {
            let data = event.data();
            let pattern = string(&data, "pattern");
            if pattern.is_empty() {
                return;
            }
            STATE.with(|state| state.borrow_mut().routes.insert(pattern, data));
            refresh();
        });
    let window = web_sys::window().ok_or("the page has no window")?;
    window
        .navigator()
        .service_worker()
        .add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref())
        .map_err(js_text)?;
    on_message.forget();

    // The counts of connected elements change without a message, so an
    // open shelf refreshes a few times a second. It asks the worker for
    // the routes' statuses too, though less often, in case it missed a
    // broadcast.
    let tick = Closure::<dyn Fn()>::new(|| {
        if !is_open() {
            return;
        }
        let load = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.ticks = state.ticks.wrapping_add(1);
            let load = !state.loading && state.ticks % ROUTE_TICKS == 0;
            state.loading |= load;
            load
        });
        if load {
            spawn_local(async {
                let _ = load_routes().await;
                STATE.with(|state| state.borrow_mut().loading = false);
                refresh();
            });
        }
        refresh();
    });
    window
        .set_interval_with_callback_and_timeout_and_arguments_0(tick.as_ref().unchecked_ref(), 250)
        .map_err(js_text)?;
    tick.forget();
    refresh();
    Ok(())
}

/// Whether the shelf's panel is open.
fn is_open() -> bool {
    document()
        .ok()
        .and_then(|document| document.get_element_by_id("shelf-panel"))
        .is_some_and(|panel| !panel.has_attribute("hidden"))
}

/// Open or close the shelf's panel. The page leaves room below the
/// application for whichever is shown, the bar or the whole shelf.
fn set_open(open: bool) {
    let Ok(document) = document() else {
        return;
    };
    let (Some(panel), Some(toggle)) = (
        document.get_element_by_id("shelf-panel"),
        document.get_element_by_id("shelf-toggle"),
    ) else {
        return;
    };
    let was_open = !panel.has_attribute("hidden");
    if open {
        let _ = panel.remove_attribute("hidden");
    } else {
        let _ = panel.set_attribute("hidden", "");
    }
    let _ = toggle.set_attribute("aria-expanded", if open { "true" } else { "false" });
    if let Some(root) = document.document_element() {
        let _ = root.set_attribute("data-shelf", if open { "open" } else { "closed" });
    }
    if open && !was_open {
        spawn_local(async {
            let _ = load_routes().await;
            refresh();
        });
        if let Some(assist) = active().and_then(|id| assist_of(&id)) {
            assist.schedule_check();
        }
    }
    save_preferences();
    refresh();
}

/// The tab of the source `kind` `name`, which selects its section and
/// shows whether the source has an edit not saved, or a failure.
fn tab(document: &Document, kind: &str, name: &str) -> Result<Element, String> {
    let id = section_id(kind, name);
    let tab = make(
        document,
        "button",
        &[
            ("type", "button"),
            ("role", "tab"),
            ("class", "shelf-tab"),
            ("data-tab", &id),
            ("aria-selected", "false"),
        ],
    )?;
    let label = make(document, "span", &[("class", "label")])?;
    label.set_text_content(Some(&if kind == "element" {
        format!("<{name}>")
    } else {
        name.to_string()
    }));
    let dot = make(
        document,
        "span",
        &[("class", "dot"), ("aria-hidden", "true")],
    )?;
    tab.append_child(&label).map_err(js_text)?;
    tab.append_child(&dot).map_err(js_text)?;
    let on_click = Closure::<dyn Fn()>::new(move || {
        select(&id);
        set_open(true);
    });
    tab.add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())
        .map_err(js_text)?;
    on_click.forget();
    Ok(tab)
}

/// The tab of the section `id`.
fn find_tab(document: &Document, id: &str) -> Option<Element> {
    let tabs = document.query_selector_all("#shelf .shelf-tab").ok()?;
    (0..tabs.length())
        .filter_map(|index| tabs.item(index))
        .filter_map(|node| node.dyn_into::<Element>().ok())
        .find(|tab| tab.get_attribute("data-tab").as_deref() == Some(id))
}

/// Show the section `id` in the panel, and only it.
fn select(id: &str) {
    let Ok(document) = document() else {
        return;
    };
    if let Ok(tabs) = document.query_selector_all("#shelf .shelf-tab") {
        for tab in (0..tabs.length()).filter_map(|index| tabs.item(index)) {
            let tab: Element = tab.unchecked_into();
            let chosen = tab.get_attribute("data-tab").as_deref() == Some(id);
            let _ = tab.set_attribute("aria-selected", if chosen { "true" } else { "false" });
        }
    }
    // The language service checks what the panel shows, and only once a
    // person can see it: not while the page starts.
    if is_open()
        && let Some(assist) = assist_of(id)
    {
        assist.schedule_check();
    }
    if let Ok(sections) = document.query_selector_all("#shelf-panel section") {
        for section in (0..sections.length()).filter_map(|index| sections.item(index)) {
            let section: Element = section.unchecked_into();
            let kind = section.get_attribute("data-kind").unwrap_or_default();
            let name = section.get_attribute("data-name").unwrap_or_default();
            if section_id(&kind, &name) == id {
                let _ = section.remove_attribute("hidden");
            } else {
                let _ = section.set_attribute("hidden", "");
            }
        }
    }
    save_preferences();
}

/// The language service of the editor of the section `id`.
fn assist_of(id: &str) -> Option<Rc<Assist>> {
    STATE.with(|state| state.borrow().assists.get(id).cloned())
}

/// Open the tab of the source whose file, as the program names it, is
/// `file`, and answer its editor's text area: where a definition the
/// language service found in another source of the shelf is.
fn open_file(file: &str) -> Option<HtmlTextAreaElement> {
    let id = STATE.with(|state| {
        state
            .borrow()
            .assists
            .iter()
            .find(|(_, assist)| assist.path() == file)
            .map(|(id, _)| id.clone())
    })?;
    select(&id);
    set_open(true);
    let (kind, name) = id.split_once(':')?;
    find_section(&document().ok()?, kind, name)?
        .query_selector("textarea")
        .ok()??
        .dyn_into()
        .ok()
}

/// The section the panel shows.
fn active() -> Option<String> {
    let document = document().ok()?;
    let tab = document
        .query_selector("#shelf .shelf-tab[aria-selected=\"true\"]")
        .ok()??;
    tab.get_attribute("data-tab")
}

/// Whether the shelf was open, and its active tab, as the viewer left
/// them. Storage can be missing or refuse, which leaves the shelf closed
/// on its first tab.
fn preferences() -> (bool, Option<String>) {
    let stored = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(PREFERENCES).ok().flatten());
    let Some(stored) = stored else {
        return (false, None);
    };
    let (open, tab) = stored.split_once(' ').unwrap_or((stored.as_str(), ""));
    (open == "open", (!tab.is_empty()).then(|| tab.to_string()))
}

/// Keep whether the shelf is open and its active tab for the next visit.
fn save_preferences() {
    let value = format!(
        "{} {}",
        if is_open() { "open" } else { "closed" },
        active().unwrap_or_default()
    );
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.set_item(PREFERENCES, &value);
    }
}

/// One section of the shelf: a source with its status and controls.
fn section(document: &Document, kind: &str, name: &str) -> Result<Element, String> {
    let id = section_id(kind, name);
    let section = make(
        document,
        "section",
        &[
            ("class", "source"),
            ("role", "tabpanel"),
            ("data-kind", kind),
            ("data-name", name),
            ("aria-label", &format!("The Zena source of {name}")),
        ],
    )?;
    let _ = section.set_attribute("hidden", "");
    // The toolbar: the source's times and counts, then its controls.
    let toolbar = make(document, "div", &[("class", "toolbar")])?;
    let meta = make(document, "p", &[("class", "meta")])?;
    for field in ["compile", "size", "wasm", "instantiate", "counts"] {
        let span = make(document, "span", &[("data-field", field)])?;
        meta.append_child(&span).map_err(js_text)?;
    }
    toolbar.append_child(&meta).map_err(js_text)?;
    section.append_child(&toolbar).map_err(js_text)?;
    let editor = make(document, "div", &[("class", "editor")])?;
    let dirty_id = id.clone();
    let text = editor::mount(
        document,
        &editor,
        &format!("The Zena source of {name}"),
        move || {
            STATE.with(|state| state.borrow_mut().dirty.insert(dirty_id.clone()));
        },
    )
    .map_err(js_text)?;
    section.append_child(&editor).map_err(js_text)?;
    // The problems the language service finds as a person types.
    let problems = make(document, "ul", &[("class", "problems")])?;
    let _ = problems.set_attribute("hidden", "");
    section.append_child(&problems).map_err(js_text)?;
    let context = STATE
        .with(|state| state.borrow().context.clone())
        .ok_or("the shelf has no context")?;
    let assist =
        Assist::attach(kind, name, context, text, &editor, problems, open_file).map_err(js_text)?;
    STATE.with(|state| state.borrow_mut().assists.insert(id.clone(), assist));
    // Below the source: a trap of the running component, and the
    // diagnostics of a compile that failed.
    let trapped = make(
        document,
        "p",
        &[("class", "trapped"), ("data-field", "trapped")],
    )?;
    let _ = trapped.set_attribute("hidden", "");
    section.append_child(&trapped).map_err(js_text)?;
    let diagnostics = make(
        document,
        "pre",
        &[("class", "diagnostics"), ("data-field", "diagnostics")],
    )?;
    let _ = diagnostics.set_attribute("hidden", "");
    section.append_child(&diagnostics).map_err(js_text)?;
    let actions = make(document, "div", &[("class", "actions")])?;
    for (action, label) in [
        ("format", "Format"),
        ("save", "Save"),
        ("reset", "Reset to original"),
        ("restart", "Restart"),
    ] {
        if action == "restart" && kind == "route" {
            continue;
        }
        let button = make(
            document,
            "button",
            &[("type", "button"), ("data-action", action)],
        )?;
        button.set_text_content(Some(label));
        let (kind, name, action) = (kind.to_string(), name.to_string(), action.to_string());
        let on_click = Closure::<dyn Fn()>::new(move || {
            let (kind, name, action) = (kind.clone(), name.clone(), action.clone());
            spawn_local(async move {
                if let Err(error) = act(&kind, &name, &action).await {
                    tracing::error!(%kind, %name, %action, "{error}");
                }
                refresh();
            });
        });
        button
            .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())
            .map_err(js_text)?;
        on_click.forget();
        actions.append_child(&button).map_err(js_text)?;
    }
    toolbar.append_child(&actions).map_err(js_text)?;
    Ok(section)
}

/// Do the shelf's `action` on the source `kind` `name`.
async fn act(kind: &str, name: &str, action: &str) -> Result<(), String> {
    let database = STATE
        .with(|state| state.borrow().database.clone())
        .ok_or("the shelf has no database")?;
    let key = idb::edit_key(kind, name);
    let id = section_id(kind, name);
    if action == "format" {
        if let Some(assist) = assist_of(&id) {
            assist.format().await;
        }
        return Ok(());
    }
    match (kind, action) {
        ("element", "save") => {
            let source = text_of(&id)?;
            database
                .put(&key, &JsValue::from_str(&source))
                .await
                .map_err(js_text)?;
            STATE.with(|state| state.borrow_mut().dirty.remove(&id));
            // A failed compile leaves the running instance, and the
            // status carries the diagnostics.
            let _ = elements::replace(name, &source).await;
        }
        ("element", "reset") => {
            database.delete(&key).await.map_err(js_text)?;
            STATE.with(|state| state.borrow_mut().dirty.remove(&id));
            let shipped = sources::element(name).ok_or("no such element")?;
            set_text(&id, shipped)?;
            let _ = elements::replace(name, shipped).await;
        }
        ("element", "restart") => {
            let _ = elements::restart(name).await;
        }
        ("route", "save") => {
            let source = text_of(&id)?;
            database
                .put(&key, &JsValue::from_str(&source))
                .await
                .map_err(js_text)?;
            STATE.with(|state| state.borrow_mut().dirty.remove(&id));
            route_changed(name).await?;
        }
        ("route", "reset") => {
            database.delete(&key).await.map_err(js_text)?;
            STATE.with(|state| state.borrow_mut().dirty.remove(&id));
            set_text(&id, sources::route(name).ok_or("no such route")?)?;
            route_changed(name).await?;
        }
        _ => return Err(format!("`{action}` is not an action on {kind} {name}")),
    }
    Ok(())
}

/// Tell the service worker that the route `pattern` has a new source,
/// once IndexedDB holds it, and keep the status it answers.
async fn route_changed(pattern: &str) -> Result<(), String> {
    let message = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&message, &"kind".into(), &"route-changed".into());
    let _ = js_sys::Reflect::set(&message, &"pattern".into(), &pattern.into());
    let status = page::ask_worker(&message).await.map_err(js_text)?;
    STATE.with(|state| {
        state
            .borrow_mut()
            .routes
            .insert(pattern.to_string(), status)
    });
    Ok(())
}

/// Ask the service worker for the status of every route.
async fn load_routes() -> Result<(), String> {
    let message = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&message, &"kind".into(), &"routes-status".into());
    let statuses = page::ask_worker(&message).await.map_err(js_text)?;
    let statuses: js_sys::Array = statuses.dyn_into().map_err(js_text)?;
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        for status in statuses.iter() {
            state.routes.insert(string(&status, "pattern"), status);
        }
    });
    Ok(())
}

/// Show the latest status of every source.
pub fn refresh() {
    let Ok(document) = document() else {
        return;
    };
    for status in elements::statuses() {
        if sources::element(&status.tag).is_none() {
            continue;
        }
        let id = section_id("element", &status.tag);
        let Some(section) = find_section(&document, "element", &status.tag) else {
            continue;
        };
        fill(
            &section,
            &id,
            &status.source,
            status.compile_ms,
            status.component_bytes.map(|bytes| bytes as f64),
            status.wasm_compile_ms,
            status.instantiate_ms,
            status.diagnostics.as_deref(),
            status.trapped.as_deref(),
            Some(format!(
                "{} connected · {} instance{}",
                status.connected,
                status.instances,
                if status.instances == 1 { "" } else { "s" }
            )),
        );
    }
    let routes: Vec<(String, JsValue)> = STATE.with(|state| {
        state
            .borrow()
            .routes
            .iter()
            .map(|(pattern, status)| (pattern.clone(), status.clone()))
            .collect()
    });
    for (pattern, _) in sources::ROUTES {
        let Some(section) = find_section(&document, "route", pattern) else {
            continue;
        };
        let id = section_id("route", pattern);
        let status = routes
            .iter()
            .find(|(known, _)| known == pattern)
            .map(|(_, status)| status);
        let source = status
            .map(|status| string(status, "source"))
            .filter(|source| !source.is_empty())
            .unwrap_or_else(|| sources::route(pattern).unwrap_or_default().to_string());
        let number = |key: &str| {
            status.and_then(|status| {
                js_sys::Reflect::get(status, &JsValue::from_str(key))
                    .ok()
                    .and_then(|value| value.as_f64())
            })
        };
        let text = |key: &str| {
            status
                .map(|status| string(status, key))
                .filter(|text| !text.is_empty())
        };
        // When the worker last compiled the route: a new worker compiles
        // it again on its first request, so the time moves on.
        let counts = number("compiles").map(|compiles| {
            let at = number("compiledAt")
                .map(|at| {
                    let time = js_sys::Date::new(&JsValue::from_f64(at));
                    format!(
                        " · last at {}",
                        String::from(time.to_locale_time_string("en-US"))
                    )
                })
                .unwrap_or_default();
            format!(
                "compiled {compiles} time{}{at}",
                if compiles == 1.0 { "" } else { "s" }
            )
        });
        if let Some(at) = number("compiledAt") {
            let _ = section.set_attribute("data-compiled-at", &at.to_string());
        }
        fill(
            &section,
            &id,
            &source,
            number("compileMs"),
            number("componentBytes"),
            number("wasmCompileMs"),
            number("instantiateMs"),
            text("diagnostics").as_deref(),
            text("trapped").as_deref(),
            counts,
        );
    }
}

/// Fill a section with a status.
#[allow(clippy::too_many_arguments)]
fn fill(
    section: &Element,
    id: &str,
    source: &str,
    compile_ms: Option<f64>,
    component_bytes: Option<f64>,
    wasm_compile_ms: Option<f64>,
    instantiate_ms: Option<f64>,
    diagnostics: Option<&str>,
    trapped: Option<&str>,
    counts: Option<String>,
) {
    let field = |name: &str| {
        section
            .query_selector(&format!("[data-field=\"{name}\"]"))
            .ok()
            .flatten()
    };
    let millis = |value: Option<f64>| match value {
        Some(value) => format!("{value:.0} ms"),
        None => "not yet".to_string(),
    };
    if let Some(span) = field("compile") {
        span.set_text_content(Some(&format!("Zena → Wasm {}", millis(compile_ms))));
    }
    if let Some(span) = field("size") {
        span.set_text_content(Some(&format!("component {}", size(component_bytes))));
        match component_bytes {
            Some(bytes) => {
                let _ = span.set_attribute("title", &format!("{} bytes", grouped(bytes as u64)));
            }
            None => {
                let _ = span.remove_attribute("title");
            }
        }
    }
    if let Some(span) = field("wasm") {
        span.set_text_content(Some(&format!("Wasm compile {}", millis(wasm_compile_ms))));
    }
    if let Some(span) = field("instantiate") {
        span.set_text_content(Some(&format!("instantiate {}", millis(instantiate_ms))));
    }
    if let Some(span) = field("counts") {
        span.set_text_content(counts.as_deref());
    }
    for (name, text) in [("diagnostics", diagnostics), ("trapped", trapped)] {
        if let Some(element) = field(name) {
            match text {
                Some(text) => {
                    element.set_text_content(Some(text));
                    let _ = element.remove_attribute("hidden");
                }
                None => {
                    element.set_text_content(None);
                    let _ = element.set_attribute("hidden", "");
                }
            }
        }
    }
    if let Ok(Some(button)) = section.query_selector("[data-action=\"restart\"]") {
        if trapped.is_some() {
            let _ = button.remove_attribute("hidden");
        } else {
            let _ = button.set_attribute("hidden", "");
        }
    }
    let dirty = STATE.with(|state| state.borrow().dirty.contains(id));
    if !dirty && let Ok(Some(text)) = section.query_selector("textarea") {
        let text: HtmlTextAreaElement = text.unchecked_into();
        if text.value() != source {
            editor::set_text(&text, source);
        }
    }
    // The tab's dot: a failure first, then an edit not saved.
    if let Ok(document) = document()
        && let Some(tab) = find_tab(&document, id)
    {
        let state = if diagnostics.is_some() || trapped.is_some() {
            "failed"
        } else if dirty {
            "edited"
        } else {
            "clean"
        };
        let _ = tab.set_attribute("data-state", state);
    }
}

/// The section of the source `kind` `name`.
fn find_section(document: &Document, kind: &str, name: &str) -> Option<Element> {
    let sections = document
        .query_selector_all(&format!("#shelf section[data-kind=\"{kind}\"]"))
        .ok()?;
    (0..sections.length())
        .filter_map(|index| sections.item(index))
        .filter_map(|node| node.dyn_into::<Element>().ok())
        .find(|section| section.get_attribute("data-name").as_deref() == Some(name))
}

/// The text in the text area of the section `id`.
fn text_of(id: &str) -> Result<String, String> {
    let (kind, name) = id.split_once(':').ok_or("not a section")?;
    let section = find_section(&document()?, kind, name).ok_or("no such section")?;
    let text = section
        .query_selector("textarea")
        .map_err(js_text)?
        .ok_or("the section has no text area")?;
    Ok(text.unchecked_into::<HtmlTextAreaElement>().value())
}

/// Put `source` in the text area of the section `id`.
fn set_text(id: &str, source: &str) -> Result<(), String> {
    let (kind, name) = id.split_once(':').ok_or("not a section")?;
    let section = find_section(&document()?, kind, name).ok_or("no such section")?;
    let text = section
        .query_selector("textarea")
        .map_err(js_text)?
        .ok_or("the section has no text area")?;
    editor::set_text(&text.unchecked_into::<HtmlTextAreaElement>(), source);
    Ok(())
}

/// The status of a tag as a JavaScript object, for the browser tests.
pub fn element_status_value(status: &TagStatus) -> JsValue {
    let object = js_sys::Object::new();
    let set = |key: &str, value: JsValue| {
        let _ = js_sys::Reflect::set(&object, &JsValue::from_str(key), &value);
    };
    let optional =
        |value: &Option<String>| value.as_deref().map_or(JsValue::NULL, JsValue::from_str);
    let millis = |value: Option<f64>| value.map_or(JsValue::NULL, JsValue::from_f64);
    set("tag", JsValue::from_str(&status.tag));
    set("source", JsValue::from_str(&status.source));
    set("compileMs", millis(status.compile_ms));
    set(
        "componentBytes",
        millis(status.component_bytes.map(|bytes| bytes as f64)),
    );
    set("wasmCompileMs", millis(status.wasm_compile_ms));
    set("instantiateMs", millis(status.instantiate_ms));
    set("diagnostics", optional(&status.diagnostics));
    set("trapped", optional(&status.trapped));
    set("connected", JsValue::from_f64(status.connected as f64));
    set("instances", JsValue::from_f64(status.instances as f64));
    set("starts", JsValue::from_f64(f64::from(status.starts)));
    object.into()
}

/// A size in bytes, as the shelf shows it: in bytes, kilobytes, or
/// megabytes, of a thousand each.
fn size(bytes: Option<f64>) -> String {
    match bytes {
        None => "not yet".to_string(),
        Some(bytes) if bytes < 1_000.0 => format!("{bytes:.0} B"),
        Some(bytes) if bytes < 1_000_000.0 => format!("{:.1} kB", bytes / 1_000.0),
        Some(bytes) => format!("{:.2} MB", bytes / 1_000_000.0),
    }
}

/// `number` with its digits in groups of three: `421,948`.
fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A new element `tag` with `attributes`.
fn make(document: &Document, tag: &str, attributes: &[(&str, &str)]) -> Result<Element, String> {
    let element = document.create_element(tag).map_err(js_text)?;
    for (name, value) in attributes {
        element.set_attribute(name, value).map_err(js_text)?;
    }
    Ok(element)
}

/// The text property `key` of `object`, or empty.
fn string(object: &JsValue, key: &str) -> String {
    js_sys::Reflect::get(object, &JsValue::from_str(key))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default()
}

/// The page's document.
fn document() -> Result<Document, String> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| "the page has no document".to_string())
}

/// A JavaScript exception as text.
fn js_text(error: impl core::fmt::Debug) -> String {
    format!("{error:?}")
}

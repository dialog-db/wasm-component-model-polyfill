// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Elements: custom HTML elements whose behavior is a component that
//! the page compiles from Zena source.
//!
//! [`define_element`] compiles a tag's source with the glue the helper
//! writes, makes one store and one instance for the tag, and registers a
//! custom element class whose lifecycle callbacks call the instance.
//! The instance serves every element of the tag: `create` answers an id
//! for each, and the glue keeps one object of the author's definition
//! per id.
//!
//! Each tag's store has one driver, a `run_concurrent` that takes calls
//! from a channel and runs them side by side. So calls for different
//! elements of a tag interleave where the Zena code awaits. The calls
//! for one element run one at a time, in order: each element has a
//! queue of its own, which waits for one call before it makes the next.
//!
//! A trap poisons the tag's store and ends its driver. Each element of
//! the tag then shows an error card that names the tag and the trap,
//! until [`restart`] compiles the tag again and gives each element a new
//! object in a new store.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use futures::stream::FuturesUnordered;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wcmp::{Accessor, Func, Linker, Val, ValField};
use web_sys::{Document, HtmlElement, ShadowRoot};

use crate::client;
use crate::compiler::CompileError;
use crate::context::Context;
use crate::dom::{self, Mounted};
use crate::glue::Compile;
use crate::http_types::{self, HttpTable};
use crate::view::View;
use crate::wasi;

mod element_host;
mod tag_status;

pub use element_host::ElementHost;
pub use tag_status::TagStatus;

/// The interface the element exports.
const ELEMENT: &str = "demo:element/element";

/// The interface the element imports `emit` from.
const HOST: &str = "demo:element/host";

/// The property of an element's DOM node that holds its key in
/// [`Registry::elements`].
const KEY: &str = "__demoKey";

/// One call for a tag's driver to make.
struct Job {
    func: Rc<Func>,
    args: Vec<Val>,
    reply: oneshot::Sender<Result<Vec<Val>, wcmp::Error>>,
}

/// The exports of the element interface.
#[derive(Clone)]
struct Exports {
    create: Rc<Func>,
    connected: Rc<Func>,
    disconnected: Rc<Func>,
    attribute_changed: Rc<Func>,
    handle_event: Rc<Func>,
    render: Rc<Func>,
}

/// A tag's running instance: its driver's channel, its exports, and
/// the host node of each element by id.
struct Runtime {
    jobs: mpsc::UnboundedSender<Job>,
    exports: Exports,
    nodes: Rc<RefCell<HashMap<u32, HtmlElement>>>,
}

impl Runtime {
    /// Call `func` with `args` through the driver.
    ///
    /// # Errors
    ///
    /// The text of the error the call failed with, or of the trap
    /// that ended the driver.
    async fn call(&self, func: &Rc<Func>, args: Vec<Val>) -> Result<Vec<Val>, String> {
        let (reply, answer) = oneshot::channel();
        self.jobs
            .unbounded_send(Job {
                func: func.clone(),
                args,
                reply,
            })
            .map_err(|_| "the element's component has stopped".to_string())?;
        match answer.await {
            Ok(Ok(results)) => Ok(results),
            Ok(Err(error)) => Err(crate::platform::describe(&error)),
            Err(_) => Err("the element's component has stopped".to_string()),
        }
    }
}

/// A tag the page defined.
struct Tag {
    status: RefCell<TagStatus>,
    runtime: RefCell<Option<Rc<Runtime>>>,
    /// The source the tag last started from: what a restart compiles.
    running: RefCell<String>,
    sheet: web_sys::CssStyleSheet,
}

/// One element of a defined tag in the document, or one that was.
struct Instance {
    tag: String,
    node: HtmlElement,
    root: ShadowRoot,
    /// The id `create` answered, while the element has one.
    id: Cell<Option<u32>>,
    /// The runtime that answered the id.
    runtime: RefCell<Weak<Runtime>>,
    ops: mpsc::UnboundedSender<Op>,
    mounted: RefCell<Vec<Mounted>>,
    /// The listener on the shadow root for each DOM event name a view
    /// used.
    listeners: RefCell<Vec<(String, Listener)>>,
}

/// A listener on an element's shadow root.
type Listener = Closure<dyn Fn(web_sys::Event)>;

/// One thing an element's queue does, in order.
enum Op {
    Connected,
    Disconnected,
    AttributeChanged(String, Option<String>),
    Event(String, Val),
    /// Show the tag's new instance: a new object, made from the
    /// element's attributes, and a render.
    Restart,
    /// Show the trap that poisoned the tag's store.
    Trapped,
}

impl Op {
    /// The operation's name, for its span.
    fn name(&self) -> &'static str {
        match self {
            Op::Connected => "connected",
            Op::Disconnected => "disconnected",
            Op::AttributeChanged(..) => "attribute-changed",
            Op::Event(..) => "event",
            Op::Restart => "restart",
            Op::Trapped => "trapped",
        }
    }
}

/// Every tag and element of the page.
#[derive(Default)]
struct Registry {
    context: RefCell<Option<Rc<Context>>>,
    tags: RefCell<HashMap<String, Rc<Tag>>>,
    elements: RefCell<HashMap<u32, Rc<Instance>>>,
    next: Cell<u32>,
    callbacks: RefCell<Option<JsValue>>,
}

thread_local! {
    static REGISTRY: Registry = Registry::default();
}

/// Give the elements of the page the context they compile in.
pub fn set_context(context: Rc<Context>) {
    REGISTRY.with(|registry| *registry.context.borrow_mut() = Some(context));
}

/// The context the elements compile in.
fn context() -> Result<Rc<Context>, String> {
    REGISTRY
        .with(|registry| registry.context.borrow().clone())
        .ok_or_else(|| "the elements have no context to compile in".to_string())
}

/// The status of every defined tag, in tag order.
pub fn statuses() -> Vec<TagStatus> {
    REGISTRY.with(|registry| {
        let tags = registry.tags.borrow();
        let mut statuses: Vec<TagStatus> = tags.values().map(|tag| status(tag)).collect();
        statuses.sort_by(|a, b| a.tag.cmp(&b.tag));
        statuses
    })
}

/// The status of `tag`, with its counts.
fn status(tag: &Tag) -> TagStatus {
    let mut status = tag.status.borrow().clone();
    status.connected = REGISTRY.with(|registry| {
        registry
            .elements
            .borrow()
            .values()
            .filter(|instance| instance.tag == status.tag && instance.node.is_connected())
            .count()
    });
    status.instances = usize::from(tag.runtime.borrow().is_some());
    status
}

/// The status of the tag `name`, if the page defined it.
pub fn status_of(name: &str) -> Option<TagStatus> {
    REGISTRY.with(|registry| registry.tags.borrow().get(name).map(|tag| status(tag)))
}

/// Define the element `tag` from Zena `source`.
///
/// # Errors
///
/// The compiler's diagnostics when the source does not compile, or the
/// reason the component does not instantiate.
#[tracing::instrument(level = "debug", name = "define element", skip_all, fields(tag = tag))]
pub async fn define_element(tag: &str, source: &str) -> Result<(), String> {
    if REGISTRY.with(|registry| registry.tags.borrow().contains_key(tag)) {
        return Err(format!("the element {tag} is defined already"));
    }
    let sheet = web_sys::CssStyleSheet::new().map_err(js_text)?;
    let entry = Rc::new(Tag {
        status: RefCell::new(TagStatus {
            tag: tag.to_string(),
            source: source.to_string(),
            ..TagStatus::default()
        }),
        runtime: RefCell::new(None),
        running: RefCell::new(String::new()),
        sheet,
    });
    let (runtime, observed) = start(&entry, source).await?;
    *entry.runtime.borrow_mut() = Some(runtime);
    REGISTRY.with(|registry| registry.tags.borrow_mut().insert(tag.to_string(), entry));
    let window = web_sys::window().ok_or("the page has no window")?;
    let elements = js_sys::Reflect::get(&window, &"demoElements".into()).map_err(js_text)?;
    let define: js_sys::Function = js_sys::Reflect::get(&elements, &"define".into())
        .map_err(js_text)?
        .dyn_into()
        .map_err(js_text)?;
    let observed: js_sys::Array = observed
        .iter()
        .map(|name| JsValue::from_str(name))
        .collect();
    define
        .call3(&elements, &JsValue::from_str(tag), &observed, &callbacks())
        .map_err(js_text)?;
    Ok(())
}

/// Define the element `tag` from `edit`, the source a person saved,
/// when there is one, and otherwise from `shipped`. An edit that does
/// not compile falls back to the shipped source, and the tag's status
/// keeps the edit and its diagnostics.
///
/// # Errors
///
/// As for [`define_element`], when the shipped source does not define
/// the element.
pub async fn define_with_edit(tag: &str, edit: Option<&str>, shipped: &str) -> Result<(), String> {
    let Some(edit) = edit else {
        return define_element(tag, shipped).await;
    };
    let Err(diagnostics) = define_element(tag, edit).await else {
        return Ok(());
    };
    REGISTRY.with(|registry| registry.tags.borrow_mut().remove(tag));
    define_element(tag, shipped).await?;
    if let Some(entry) = REGISTRY.with(|registry| registry.tags.borrow().get(tag).cloned()) {
        let mut status = entry.status.borrow_mut();
        status.source = edit.to_string();
        status.diagnostics = Some(diagnostics);
    }
    Ok(())
}

/// Compile `source` for `tag` again and give each element of the tag a
/// new object in a new store. When the compile fails, the instance that
/// runs keeps running, and the diagnostics are the error.
///
/// # Errors
///
/// The compiler's diagnostics, or the reason the component does not
/// instantiate.
pub async fn replace(tag: &str, source: &str) -> Result<(), String> {
    let entry = REGISTRY
        .with(|registry| registry.tags.borrow().get(tag).cloned())
        .ok_or_else(|| format!("no element {tag} is defined"))?;
    entry.status.borrow_mut().source = source.to_string();
    let (runtime, _) = start(&entry, source).await?;
    renew(tag, &entry, runtime);
    Ok(())
}

/// Compile the tag `tag` again from the source it last started from,
/// and give each of its elements a new object in a new store: the way
/// out of a trap. An edit that failed to compile since stays in the
/// status, with its diagnostics.
///
/// # Errors
///
/// The reason the component does not compile or instantiate.
pub async fn restart(tag: &str) -> Result<(), String> {
    let entry = REGISTRY
        .with(|registry| registry.tags.borrow().get(tag).cloned())
        .ok_or_else(|| format!("no element {tag} is defined"))?;
    let running = entry.running.borrow().clone();
    let (runtime, _) = start(&entry, &running).await?;
    renew(tag, &entry, runtime);
    Ok(())
}

/// Run the tag `tag` on `runtime` from now on, and give each of its
/// elements a new object there.
fn renew(tag: &str, entry: &Tag, runtime: Rc<Runtime>) {
    *entry.runtime.borrow_mut() = Some(runtime);
    entry.status.borrow_mut().trapped = None;
    for instance in instances_of(tag) {
        let _ = instance.ops.unbounded_send(Op::Restart);
    }
}

/// Every element of `tag` the page knows.
fn instances_of(tag: &str) -> Vec<Rc<Instance>> {
    REGISTRY.with(|registry| {
        registry
            .elements
            .borrow()
            .values()
            .filter(|instance| instance.tag == tag)
            .cloned()
            .collect()
    })
}

/// Compile `source` for the tag `entry`, instantiate it, and start its
/// driver. Answer the runtime and the attributes the tag observes, and
/// put the tag's styles in its sheet.
#[tracing::instrument(level = "debug", name = "element start", skip_all)]
async fn start(entry: &Rc<Tag>, source: &str) -> Result<(Rc<Runtime>, Vec<String>), String> {
    let tag = entry.status.borrow().tag.clone();
    let context = context()?;
    let compile = Compile::element(&tag, source).inspect_err(|text| {
        entry.status.borrow_mut().diagnostics = Some(text.clone());
    })?;
    let compiled = match context.compile(&compile.request()).await {
        Ok(compiled) => compiled,
        Err(error) => {
            let mut status = entry.status.borrow_mut();
            if let CompileError::Diagnostics { millis, .. } = &error {
                status.compile_ms = Some(*millis);
            }
            let text = error.to_string();
            status.diagnostics = Some(text.clone());
            return Err(text);
        }
    };
    {
        let mut status = entry.status.borrow_mut();
        status.compile_ms = Some(compiled.millis);
        status.diagnostics = None;
    }
    let (started, parse_ms) = context
        .parse(&compiled.bytes)
        .await
        .map_err(|error| error.to_string())?;
    let mut linker = Linker::new(context.engine());
    wasi::define(&mut linker, &tag).map_err(|error| error.to_string())?;
    let types = http_types::define(&mut linker, &started).map_err(|error| error.to_string())?;
    client::define(&mut linker, &started).map_err(|error| error.to_string())?;
    linker
        .instance(&HOST.parse().expect("the host interface's name parses"))
        .func_wrap("emit", |call, (id, name, detail): (u32, String, String)| {
            let host: &ElementHost = call.data();
            let node = host.nodes.borrow().get(&id).cloned();
            if let Some(node) = node {
                emit(&node, &name, &detail);
            }
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    let nodes = Rc::new(RefCell::new(HashMap::new()));
    let mut http = HttpTable::default();
    http.set_types(types);
    let instantiated = context
        .instantiate(
            &started,
            parse_ms,
            &linker,
            ElementHost {
                http,
                nodes: nodes.clone(),
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    {
        let mut status = entry.status.borrow_mut();
        status.instantiate_ms = Some(instantiated.millis);
        status.starts += 1;
    }
    *entry.running.borrow_mut() = source.to_string();
    let interface = instantiated
        .instance
        .exports()
        .instance(ELEMENT)
        .ok_or("the element exports no element interface")?;
    let func = |name: &str| {
        interface
            .func(name)
            .map(Rc::new)
            .ok_or_else(|| format!("the element interface has no `{name}`"))
    };
    let exports = Exports {
        create: func("create")?,
        connected: func("connected")?,
        disconnected: func("disconnected")?,
        attribute_changed: func("attribute-changed")?,
        handle_event: func("handle-event")?,
        render: func("render")?,
    };
    let observed_attributes = func("observed-attributes")?;
    let styles = func("styles")?;
    let (jobs, queue) = mpsc::unbounded();
    let runtime = Rc::new(Runtime {
        jobs,
        exports,
        nodes,
    });
    let trapped_tag = tag.clone();
    let trapped_runtime = Rc::downgrade(&runtime);
    wasm_bindgen_futures::spawn_local(drive(instantiated.store, queue, move |trap| {
        trapped(&trapped_tag, &trapped_runtime, trap);
    }));
    let observed = match runtime
        .call(&observed_attributes, Vec::new())
        .await?
        .first()
    {
        Some(Val::List(names)) => names
            .iter()
            .filter_map(|name| match name {
                Val::String(name) => Some(name.clone()),
                _ => None,
            })
            .collect(),
        other => return Err(format!("`observed-attributes` returned {other:?}")),
    };
    match runtime.call(&styles, Vec::new()).await?.first() {
        Some(Val::String(css)) => {
            entry.sheet.replace_sync(css).map_err(js_text)?;
        }
        other => return Err(format!("`styles` returned {other:?}")),
    }
    Ok((runtime, observed))
}

/// Run the calls `jobs` sends in `store`, side by side, until the
/// channel closes or a trap poisons the store, which `on_trap` hears.
async fn drive(
    mut store: wcmp::Store<ElementHost>,
    mut jobs: mpsc::UnboundedReceiver<Job>,
    on_trap: impl FnOnce(String) + 'static,
) {
    let result = store
        .run_concurrent(async move |accessor: &Accessor<ElementHost>| {
            let mut running = FuturesUnordered::new();
            loop {
                futures::select! {
                    job = jobs.next() => match job {
                        Some(Job { func, args, reply }) => running.push(async move {
                            let result = func
                                .call_concurrent(accessor, &args)
                                .await
                                .map(|results| results.into_vec());
                            let _ = reply.send(result);
                        }),
                        None => break,
                    },
                    () = running.select_next_some() => {}
                }
            }
            while running.next().await.is_some() {}
        })
        .await;
    if let Err(error) = result {
        on_trap(crate::platform::describe(&error));
    }
}

/// The trap `trap` poisoned the store of `tag`: forget the tag's
/// runtime, and show the error card on each of its elements.
fn trapped(tag: &str, runtime: &Weak<Runtime>, trap: String) {
    let entry = REGISTRY.with(|registry| registry.tags.borrow().get(tag).cloned());
    let Some(entry) = entry else {
        return;
    };
    // A runtime that a save or a restart replaced can trap in a call it
    // had in flight. Its trap is no longer the tag's.
    let current = entry.runtime.borrow().clone();
    let replaced = match (current, runtime.upgrade()) {
        (Some(current), Some(trapped)) => !Rc::ptr_eq(&current, &trapped),
        (None, _) => false,
        (Some(_), None) => true,
    };
    if replaced {
        return;
    }
    *entry.runtime.borrow_mut() = None;
    entry.status.borrow_mut().trapped = Some(trap.clone());
    for instance in instances_of(tag) {
        let _ = instance.ops.unbounded_send(Op::Trapped);
    }
}

/// The callbacks the JavaScript classes call, made once.
fn callbacks() -> JsValue {
    REGISTRY.with(|registry| {
        registry
            .callbacks
            .borrow_mut()
            .get_or_insert_with(|| {
                let object = js_sys::Object::new();
                let connected = Closure::<dyn Fn(HtmlElement)>::new(|node: HtmlElement| {
                    if let Some(instance) = instance(&node) {
                        let _ = instance.ops.unbounded_send(Op::Connected);
                    }
                });
                let disconnected = Closure::<dyn Fn(HtmlElement)>::new(|node: HtmlElement| {
                    if let Some(instance) = instance(&node) {
                        let _ = instance.ops.unbounded_send(Op::Disconnected);
                    }
                });
                let changed = Closure::<dyn Fn(HtmlElement, String, Option<String>)>::new(
                    |node: HtmlElement, name: String, value: Option<String>| {
                        if let Some(instance) = instance(&node) {
                            let _ = instance
                                .ops
                                .unbounded_send(Op::AttributeChanged(name, value));
                        }
                    },
                );
                let _ = js_sys::Reflect::set(&object, &"connected".into(), connected.as_ref());
                let _ =
                    js_sys::Reflect::set(&object, &"disconnected".into(), disconnected.as_ref());
                let _ = js_sys::Reflect::set(&object, &"attributeChanged".into(), changed.as_ref());
                connected.forget();
                disconnected.forget();
                changed.forget();
                object.into()
            })
            .clone()
    })
}

/// The instance of the DOM element `node`, made the first time the page
/// sees it.
fn instance(node: &HtmlElement) -> Option<Rc<Instance>> {
    let key = js_sys::Reflect::get(node, &KEY.into())
        .ok()
        .and_then(|key| key.as_f64())
        .map(|key| key as u32);
    if let Some(key) = key {
        return REGISTRY.with(|registry| registry.elements.borrow().get(&key).cloned());
    }
    let tag = node.tag_name().to_ascii_lowercase();
    let root = node.shadow_root()?;
    let entry = REGISTRY.with(|registry| registry.tags.borrow().get(&tag).cloned())?;
    let key = REGISTRY.with(|registry| {
        let key = registry.next.get() + 1;
        registry.next.set(key);
        key
    });
    let _ = js_sys::Reflect::set(node, &KEY.into(), &JsValue::from(key));
    let sheets = js_sys::Array::of1(&entry.sheet);
    let _ = js_sys::Reflect::set(&root, &"adoptedStyleSheets".into(), &sheets);
    let (ops, queue) = mpsc::unbounded();
    let instance = Rc::new(Instance {
        tag,
        node: node.clone(),
        root,
        id: Cell::new(None),
        runtime: RefCell::new(Weak::new()),
        ops,
        mounted: RefCell::new(Vec::new()),
        listeners: RefCell::new(Vec::new()),
    });
    REGISTRY.with(|registry| registry.elements.borrow_mut().insert(key, instance.clone()));
    wasm_bindgen_futures::spawn_local(run(instance.clone(), queue));
    Some(instance)
}

/// Do each of an element's operations, in order, until the element
/// leaves the document. Then the page forgets the element, and if it
/// connects again it starts over as a new one.
async fn run(instance: Rc<Instance>, mut queue: mpsc::UnboundedReceiver<Op>) {
    while let Some(op) = queue.next().await {
        let leaving = matches!(op, Op::Disconnected);
        if let Err(error) = step(&instance, op).await {
            tracing::error!(tag = %instance.tag, "{error}");
        }
        if leaving && !instance.node.is_connected() {
            release(&instance);
            return;
        }
    }
}

/// Forget the element of `instance`: the page drops it from the
/// registry and from its DOM node.
fn release(instance: &Instance) {
    let key = js_sys::Reflect::get(&instance.node, &KEY.into())
        .ok()
        .and_then(|key| key.as_f64())
        .map(|key| key as u32);
    if let Some(key) = key {
        REGISTRY.with(|registry| registry.elements.borrow_mut().remove(&key));
    }
    let _ = js_sys::Reflect::delete_property(&instance.node, &KEY.into());
    instance.mounted.borrow_mut().clear();
    for (event, listener) in instance.listeners.borrow_mut().drain(..) {
        let _ = instance.root.remove_event_listener_with_callback_and_bool(
            &event,
            listener.as_ref().unchecked_ref(),
            true,
        );
    }
}

/// The runtime that serves the element's tag now.
fn current_runtime(instance: &Instance) -> Option<Rc<Runtime>> {
    REGISTRY.with(|registry| {
        registry
            .tags
            .borrow()
            .get(&instance.tag)
            .and_then(|tag| tag.runtime.borrow().clone())
    })
}

/// Do one operation of an element.
#[tracing::instrument(level = "debug", name = "element op", skip_all, fields(tag = %instance.tag, op = op.name()))]
async fn step(instance: &Rc<Instance>, op: Op) -> Result<(), String> {
    match op {
        Op::Connected | Op::Restart => {
            if matches!(op, Op::Restart) {
                forget(instance);
            }
            if !instance.node.is_connected() {
                return Ok(());
            }
            let Some(runtime) = current_runtime(instance) else {
                return show_trap(instance);
            };
            if instance.id.get().is_none() {
                let attributes: Vec<Val> = attributes_of(&instance.node)
                    .into_iter()
                    .map(|(name, value)| {
                        Val::Tuple(Box::new([Val::String(name), Val::String(value)]))
                    })
                    .collect();
                let answer = runtime
                    .call(&runtime.exports.create, vec![Val::List(attributes.into())])
                    .await
                    .map_err(|error| failed(instance, error))?;
                let Some(Val::U32(id)) = answer.first() else {
                    return Err(format!("`create` returned {answer:?}"));
                };
                instance.id.set(Some(*id));
                *instance.runtime.borrow_mut() = Rc::downgrade(&runtime);
                runtime
                    .nodes
                    .borrow_mut()
                    .insert(*id, instance.node.clone());
                runtime
                    .call(&runtime.exports.connected, vec![Val::U32(*id)])
                    .await
                    .map_err(|error| failed(instance, error))?;
            }
            render(instance, &runtime).await
        }
        Op::Disconnected => {
            // The id belongs to the runtime that answered it. Whether or
            // not that runtime still runs, the element has no id now, so
            // its next connection asks the running one for a new id.
            let runtime = instance.runtime.borrow().upgrade();
            let id = instance.id.take();
            if let (Some(id), Some(runtime)) = (id, runtime) {
                runtime.nodes.borrow_mut().remove(&id);
                runtime
                    .call(&runtime.exports.disconnected, vec![Val::U32(id)])
                    .await
                    .map_err(|error| failed(instance, error))?;
            }
            Ok(())
        }
        Op::AttributeChanged(name, value) => {
            let Some((id, runtime)) = live(instance) else {
                return Ok(());
            };
            let value = Val::Option(value.map(|value| Box::new(Val::String(value))));
            runtime
                .call(
                    &runtime.exports.attribute_changed,
                    vec![Val::U32(id), Val::String(name), value],
                )
                .await
                .map_err(|error| failed(instance, error))?;
            render(instance, &runtime).await
        }
        Op::Event(handler, event) => {
            let Some((id, runtime)) = live(instance) else {
                return Ok(());
            };
            runtime
                .call(
                    &runtime.exports.handle_event,
                    vec![Val::U32(id), Val::String(handler), event],
                )
                .await
                .map_err(|error| failed(instance, error))?;
            render(instance, &runtime).await
        }
        Op::Trapped => show_trap(instance),
    }
}

/// The element's id and the runtime it belongs to, while the element
/// has an id in the runtime that serves its tag now.
fn live(instance: &Instance) -> Option<(u32, Rc<Runtime>)> {
    let id = instance.id.get()?;
    let runtime = instance.runtime.borrow().upgrade()?;
    let current = current_runtime(instance)?;
    Rc::ptr_eq(&runtime, &current).then_some((id, runtime))
}

/// Forget the element's id and what it rendered.
fn forget(instance: &Instance) {
    instance.id.set(None);
    *instance.runtime.borrow_mut() = Weak::new();
}

/// The error of a failed call, after the element shows it.
fn failed(instance: &Instance, error: String) -> String {
    let _ = show_card(instance, &format!("<{}> failed: {error}", instance.tag));
    error
}

/// Show the trap of the element's tag on the element.
fn show_trap(instance: &Instance) -> Result<(), String> {
    let trap = status_of(&instance.tag)
        .and_then(|status| status.trapped)
        .unwrap_or_else(|| "the component is not running".to_string());
    forget(instance);
    show_card(instance, &format!("<{}> trapped: {trap}", instance.tag))
}

/// Replace the element's contents with an error card that says `text`.
fn show_card(instance: &Instance, text: &str) -> Result<(), String> {
    let document = document()?;
    let card = document.create_element("div").map_err(js_text)?;
    card.set_attribute("class", "error-card").map_err(js_text)?;
    card.set_attribute("role", "alert").map_err(js_text)?;
    card.set_attribute(
        "style",
        "font: 14px/1.4 system-ui, sans-serif; padding: 12px 16px; border-radius: 12px; \
         background: var(--md-sys-color-error-container, #f9dedc); \
         color: var(--md-sys-color-on-error-container, #410e0b);",
    )
    .map_err(js_text)?;
    card.set_text_content(Some(text));
    let children = js_sys::Array::of1(&card);
    instance.root.replace_children_with_node(&children);
    instance.mounted.borrow_mut().clear();
    Ok(())
}

/// Render the element through `runtime` and patch its shadow root.
#[tracing::instrument(level = "debug", name = "element render", skip_all, fields(tag = %instance.tag))]
async fn render(instance: &Rc<Instance>, runtime: &Runtime) -> Result<(), String> {
    let Some(id) = instance.id.get() else {
        return Ok(());
    };
    let answer = runtime
        .call(&runtime.exports.render, vec![Val::U32(id)])
        .await
        .map_err(|error| failed(instance, error))?;
    let Some(Val::List(nodes)) = answer.first() else {
        return Err(format!("`render` returned {answer:?}"));
    };
    let views = View::from_nodes(nodes).map_err(|error| error.to_string())?;
    let document = document()?;
    let mounted = core::mem::take(&mut *instance.mounted.borrow_mut());
    let mut events = Vec::new();
    let patched = dom::patch(
        &document,
        instance.root.as_ref(),
        mounted,
        &views,
        &mut |event| {
            events.push(event.to_string());
        },
    )
    .map_err(js_text)?;
    *instance.mounted.borrow_mut() = patched;
    for event in events {
        listen(instance, &event);
    }
    Ok(())
}

/// Listen for `event` on the element's shadow root, once.
fn listen(instance: &Rc<Instance>, event: &str) {
    if instance
        .listeners
        .borrow()
        .iter()
        .any(|(name, _)| name == event)
    {
        return;
    }
    let weak = Rc::downgrade(instance);
    let listener = Closure::<dyn Fn(web_sys::Event)>::new(move |event: web_sys::Event| {
        let Some(instance) = weak.upgrade() else {
            return;
        };
        if let Some(handler) = handler_of(&instance, &event) {
            let record = event_record(&event);
            let _ = instance.ops.unbounded_send(Op::Event(handler, record));
        }
    });
    let options = web_sys::AddEventListenerOptions::new();
    options.set_capture(true);
    let _ = instance
        .root
        .add_event_listener_with_callback_and_add_event_listener_options(
            event,
            listener.as_ref().unchecked_ref(),
            &options,
        );
    instance
        .listeners
        .borrow_mut()
        .push((event.to_string(), listener));
}

/// The handler a view named for `event` on the nearest node of its path
/// inside the element's shadow root. Nodes in the shadow roots of child
/// elements on the path are theirs, so the walk passes over them.
fn handler_of(instance: &Instance, event: &web_sys::Event) -> Option<String> {
    let kind = event.type_();
    let root = JsValue::from(instance.root.clone());
    for target in event.composed_path().iter() {
        if target == root {
            break;
        }
        let inside = target
            .dyn_ref::<web_sys::Node>()
            .is_some_and(|node| JsValue::from(node.get_root_node()) == root);
        if !inside {
            continue;
        }
        let table = js_sys::Reflect::get(&target, &dom::EVENTS.into()).ok()?;
        if table.is_object()
            && let Some(handler) = js_sys::Reflect::get(&table, &JsValue::from_str(&kind))
                .ok()
                .and_then(|handler| handler.as_string())
        {
            return Some(handler);
        }
    }
    None
}

/// The `event` record of a DOM event: its type, the value of its target
/// or the detail of a custom event, whether a checkbox is checked, and
/// the key of a keyboard event.
fn event_record(event: &web_sys::Event) -> Val {
    let target = event.composed_path().get(0);
    let property = |name: &str| js_sys::Reflect::get(&target, &JsValue::from_str(name)).ok();
    let value = match event.dyn_ref::<web_sys::CustomEvent>() {
        Some(custom) => custom.detail().as_string(),
        None => property("value").and_then(|value| value.as_string()),
    };
    let checked = match property("type")
        .and_then(|kind| kind.as_string())
        .as_deref()
    {
        Some("checkbox" | "radio") => property("checked").and_then(|checked| checked.as_bool()),
        _ => None,
    };
    let key = event
        .dyn_ref::<web_sys::KeyboardEvent>()
        .map(web_sys::KeyboardEvent::key);
    let option = |text: Option<String>| Val::Option(text.map(|text| Box::new(Val::String(text))));
    Val::Record(Box::new([
        ValField {
            name: "kind".to_string(),
            value: Val::String(event.type_()),
        },
        ValField {
            name: "value".to_string(),
            value: option(value),
        },
        ValField {
            name: "checked".to_string(),
            value: Val::Option(checked.map(|checked| Box::new(Val::Bool(checked)))),
        },
        ValField {
            name: "key".to_string(),
            value: option(key),
        },
    ]))
}

/// Dispatch the custom event `name` on `node` with `detail`. It bubbles
/// and crosses shadow boundaries.
fn emit(node: &HtmlElement, name: &str, detail: &str) {
    let init = web_sys::CustomEventInit::new();
    init.set_bubbles(true);
    init.set_composed(true);
    init.set_detail(&JsValue::from_str(detail));
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict(name, &init) {
        let _ = node.dispatch_event(&event);
    }
}

/// Every attribute of `node`, in order.
fn attributes_of(node: &HtmlElement) -> Vec<(String, String)> {
    let attributes = node.attributes();
    (0..attributes.length())
        .filter_map(|index| attributes.item(index))
        .map(|attribute| (attribute.name(), attribute.value()))
        .collect()
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

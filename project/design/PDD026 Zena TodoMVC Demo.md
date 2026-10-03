# Zena TodoMVC Demo

This document designs a demo of the polyfill in a web browser. The demo is a
[TodoMVC] application with a Material 3 look. Every distinctive part of its user
interface is a web component whose behavior is a Wasm component. Every API route
is a Wasm component that a service worker serves. Each of those components
starts as a string of [Zena] source, and the browser compiles it at run time.
The compiler is a component too.

The demo exists to prove that this is possible. It is over-engineered on
purpose. A TodoMVC needs none of this machinery. It shows the Component Model
running end to end in a browser, on the polyfill. A compiler component makes
components. Element components draw the page. Route components answer HTTP
requests.

These terms recur:

- The page is the main thread of the browser tab.
- The service worker is the demo's service worker. It intercepts the page's API
  requests.
- A context is the page or the service worker. Each context runs its own copy of
  the polyfill.
- The compiler component is Zena's own compiler, built by Zena as a component.
- An author is a person who writes the Zena source of an element or a route.
- An element is a custom HTML element that the demo defines from Zena source.
- A route is a URL pattern that the service worker answers with a component that
  it compiled from Zena source.
- A helper is a function of the host framework that defines an element or a
  route.
- The authoring library is a set of Zena modules that authors import. It hides
  the WIT exports from the author.
- Glue is a Zena module that a helper writes around the author's source. It
  connects the authoring library to the WIT exports.
- The host framework is the Rust code that compiles, links, instantiates, and
  drives the components in each context.
- A view is the description of an element's contents that the element returns
  when it renders.
- The todo model is the Rust code in the service worker that owns the todo list.
- The shelf is a panel of the demo that shows and edits every Zena source.

## Goals

- The demo implements every TodoMVC behavior.
- The demo has a contemporary Material 3 look, in light and dark themes.
- Each distinctive part of the user interface is an element. A helper defines it
  from a tag name and a string of Zena source.
- Each API route is a route. A helper defines it from a URL pattern and a string
  of Zena source.
- Each context compiles all of its Zena source at run time, with the compiler
  component, on the polyfill.
- The compiler component builds from the pinned Zena toolchain. Zena's own
  compiler source makes up all of it, except a small entry module in this
  repository.
- Each route component exports `wasi:http/handler@0.3.0`.
- The root element calls the routes through `wasi:http/client@0.3.0`. The
  request goes from the element, through `fetch`, to the service worker, to a
  route component.
- Authors write against the authoring library and never write WIT.
- The business logic of the todo list is Rust.
- The shelf shows every Zena source of the demo and recompiles an edit in place.
- The shelf shows the Zena compile time, the Wasm compile time, and the
  instantiate time of each component.
- The demo builds from a crate in this workspace with Trunk. A menu command
  serves it.
- A browser test lane drives every TodoMVC behavior and the shelf. `tests all`
  runs the lane.
- The Zena compatibility suite gains scenarios that run the compiler component
  on every subject.

## Non-goals

- A cache of compiled components. Each context compiles every component each
  time that it needs one.
- A dedicated worker for compilation. The page compiles on its main thread.
- A code editor with highlighting, completion, or Zena's language service. The
  shelf edits plain text.
- Public hosting of the demo.
- The official TodoMVC test suite. Its selectors cannot see into a shadow root.
- A reusable framework. The authoring library and the host framework belong to
  the demo and have no API promise.
- A general WASI host. The demo supplies only the WASI functions that its
  components call.
- Zena components that export a resource. Zena refuses an exported resource at
  the pinned revision.
- Firefox.
- Safari releases that lack Wasm GC, exception handling with `exnref`, or tail
  calls.
- Persistence across browsers or devices. The todo list lives in one browser's
  IndexedDB.

## Facts This Design Rests On

Each fact below was read from the cited source or observed with Zena's
toolchain. The Zena facts are from revision `b2237f7` of its repository, dated
2026-09-26, unless a fact names another revision.

- Zena's compiler is written in Zena. Its library entry, `api.zena`, compiles a
  source string fully in memory. It reads other files, such as the standard
  library, through a host function ([Zena compiler api]).
- `api.zena` builds with the `host` target. Its exports take and return Zena
  strings, which are GC references. The Canonical ABI cannot lift a GC
  reference, so no adapter can turn that build into a component ([Zena compiler
  api], [Zena compiler package]).
- Zena emits a component directly. It writes the Canonical ABI shell itself. No
  separate wrapping tool runs ([Zena component emission]).
- At `b2237f7`, the compiler's clock imports
  `wasi_snapshot_preview1.clock_time_get` directly, and the component target
  refuses that import ([Zena compiler time]). Zena revision `04926ae4` moves the
  clock to `zena:time`, which uses `wasi:clocks@0.3.0` on the component target
  ([Zena WASI 0.3 move]).
- The compiler always reaches the WIT parser's file reader, which reads through
  `zena:fs` ([Zena WIT document]). On the component target, `zena:fs` imports
  `wasi:filesystem@0.3.0`. So a compiler component that serves its sources from
  memory cannot link without a change to Zena. The change lets the caller of the
  WIT module synthesis pass its own reader ([Zena WIT reader change]).
- With that change, on a revision after `04926ae4`, an entry module for the
  compiler builds with `--target component`. The result validates with
  `wasm-tools validate`. It is 2,291,982 bytes. It imports only its host
  interface, `wasi:cli@0.3.0`, and `wasi:clocks@0.3.0`.
- That compiler component compiles Zena source under Wasmtime, under the
  polyfill natively, and under the polyfill in headless Chrome. For each source,
  all three return the same bytes as `zena build`. The returned components
  instantiate and run. A program with an error returns `err` with the file, the
  line, and the column.
- The compiler returns the same bytes for the same input, in one instance and in
  a new one.
- In headless Chrome on the polyfill, the compiler component parses in 86 ms.
  One compile of a small program takes 0.1 to 0.4 seconds. Under Wasmtime with
  its default collector, the same compiles take 0.8 to 9.3 seconds.
- The compiler's linear memory grows to 128 MiB in one compile and to 256 MiB
  after several compiles in one instance.
- One compile of a small program makes about 100 `read-source` requests for
  about 50 distinct files. The compiler reads the standard library twice: once
  for the program and once for the runtime memory module.
- A synchronous export with a `list` or a `result` return fails to build on the
  component target. The same export declared `async func` builds.
- A declared world can import an interface that the program never uses. The
  compile succeeds, and the component keeps the import.
- The compiler writes console output. On the component target, console output
  goes through `wasi:cli/stdout@0.3.0` and `wasi:cli/stderr@0.3.0` ([PDD024]).
- The compiler component needs Wasm GC, typed function references, exception
  handling, and tail calls. It needs no SIMD.
- Chrome ships GC in 119, tail calls in 112, and `exnref` exceptions in 137.
  Safari ships GC and tail calls in 18.2 and `exnref` exceptions in 18.4. JSPI
  ships in Chrome 137 and Safari 27 ([Wasm features]).
- JSPI is exposed in every global scope, service workers included ([JSPI]).
- Zena refuses a component that exports a resource. It accepts an imported
  resource as a class ([Zena async exports]).
- Zena's standard library has `zena:json`, `zena:url`, and `zena:http`.
  `zena:http` gives a handler `requestText` and `textResponse` ([Zena stdlib
  http]).
- `zena:fetch` sends a request through `wasi:http/client@0.3.0`. It sends only
  `GET` ([Zena stdlib fetch]).
- A Zena program can export `wasi:http/handler@0.3.0`. Zena's own tests run such
  a program under `wasmtime serve` ([Zena http service]).
- The polyfill implements no WASI interface. A host supplies each import with
  the polyfill's `Linker` ([PDD000]).
- The polyfill's browser backend compiles a large module with the asynchronous
  `WebAssembly.compile`. Chromium refuses a synchronous compile over 8 MB on the
  main thread ([browser backend], [PDD025]).
- The polyfill's scheduler reaches `MessageChannel` and `setTimeout` through the
  global object. It uses no `window` and no `document` ([yield wake]).
- Trunk builds an extra Rust binary for a worker with `data-type="worker"`. It
  does not hash the name of a worker's files. It has no example of a service
  worker ([Trunk assets]).
- A service worker must add its `fetch` listener during the first evaluation of
  its script ([Service Workers]). A service worker cannot use top-level `await`
  or a dynamic `import()` ([HTML module loading]).
- Chromium stops an idle service worker after 30 seconds. All of its memory, and
  every component instance in it, goes away ([Chromium service worker]).
- A service worker cannot start a worker, and it has no synchronous access to
  the origin private file system ([File System]).

## The Demo at a Glance

```
  page (main thread)                         service worker
  ┌──────────────────────────────┐           ┌──────────────────────────────┐
  │ host framework               │           │ host framework               │
  │  compiler component          │           │  compiler component          │
  │  element components          │  fetch    │  route components            │
  │   <todo-app> ── wasi:http ───┼──────────►│   /api/todos ── todo model ──┼─► IndexedDB
  │   <todo-item> …              │           │   /api/todos/:id             │
  │ shelf (plain Rust and HTML)  │ messages  │                              │
  └──────────────────────────────┴◄────────►─┴──────────────────────────────┘
```

The page and the service worker each run the polyfill and the compiler
component. The page compiles the elements. The service worker compiles the
routes. They share an origin, so they share IndexedDB.

## The Compiler Component

### What It Is

The compiler component is Zena's compiler, compiled by Zena with
`--target component`. Its entry module belongs to this repository. The entry
module calls the compiler's own library code, as `api.zena` does, and exports
one function through this world:

```wit
package wcmp:zena-compiler;

interface host {
  // The text of a file that the compilation reads, or none.
  read-source: func(path: string) -> option<string>;
}

world compiler {
  import host;
  import wasi:cli/stdout@0.3.0;
  import wasi:cli/stderr@0.3.0;
  import wasi:clocks/monotonic-clock@0.3.0;

  export compile: async func(
    source: string,       // the entry module
    entry-path: string,   // the path of the entry module
    wit-source: string,   // a WIT document that holds the world
    world-name: string,
  ) -> result<list<u8>, string>;
}
```

`compile` returns the bytes of a component, or the text of the compiler's
diagnostics. Each diagnostic names the file, the line, and the column. The
program must match the named world. An empty `wit-source` compiles against the
world that Zena derives from the program, as `zena build` does without `--wit`.
For the same source, `compile` in a fresh instance returns the same bytes as
`zena build`.

The entry module keeps a compiler from one compile to the next, as Zena's
language service does. Each compile in a fresh instance checks the whole
standard library again and compiles the component runtime module again, which
takes most of its time. So the entry module keeps one compiler for each package
manifest and declared world. The standard library is parsed and checked once.
Every file of the program is parsed and checked again for each compile, because
a check that carried over would still refer to what its imports were in the last
compile. The runtime module compiles once for each manifest.

A compile in a fresh instance returns the bytes of `zena build`, and so does the
same compile again. After other programs, a compile can return a module that
orders its globals another way, because the checker keeps generic instantiations
from earlier programs. The module does the same thing. This is a candidate for a
report to Zena.

The compiler reads every file other than the entry module through `read-source`.
That includes the standard library, the WIT packages that a world refers to, and
modules that the entry module imports by a relative path. The entry module gives
the WIT parser a reader that calls `read-source`, so the parser never touches a
file system.

Zena reads a WIT package from a directory, such as the WASI WIT of its standard
library. It joins every `.wit` file of the directory in name order, each with a
newline after it. `read-source` cannot list a directory. So the source bundle
holds each directory of WIT at its own path, as that joined text, and the
compiler reads the directory with one request.

The compiler also reads the package manifest, `zena-packages.json`, through
`read-source`, as `zena build` reads it from its working directory. A manifest
lets a compile import a package of Zena modules by name.

The host framework answers `read-source` from two places:

- The source bundle. The build makes it from the pinned toolchain and this
  repository. It holds Zena's standard library, the WASI WIT that Zena vendors,
  the demo's WIT, and the authoring library. Each context fetches it once when
  it starts.
- The sources of the compile in progress. A helper puts the author's source at a
  path beside the glue, so that the glue imports it by a relative path.

The host framework sends the compiler's `stdout` and `stderr` to the browser
console.

### The Pin

The compiler component builds from the pinned Zena revision, as [PDD024] defines
the pin. The pinned revision must build the compiler for the component target.

Zena's own repository can lack a change that the demo needs. In that case, the
pin can name a revision of a fork of Zena that adds the change. The fork carries
only changes that are offered to Zena.

The work on the demo can find more than one such change. The fork then holds
them as a patch series, with these rules:

- Each change is one commit that addresses one concern.
- Each commit starts from Zena's main branch, so that it can go to Zena as its
  own pull request. A commit depends on another only when its concern does.
- Each commit has a pull request description and its own tests, in the form that
  Zena's contributor guide asks for.
- One revision of the fork combines every change in the series. The pin names
  that revision.
- A defect in Zena that the demo works around, and does not fix, gets a bug
  report for Zena instead of a commit.

When Zena accepts a change, the series drops it. When Zena holds every change,
the pin moves back to Zena's repository.

The build of the compiler component uses Zena's source tree at the pin, in
addition to the `zena` command. It needs the compiler's sources, the standard
library, and the WIT parser.

### Where It Runs

The page runs the compiler component on its main thread. A compile blocks the
page for its duration. The shelf shows that duration, so the cost is visible.

The service worker runs its own instance of the compiler component. A service
worker cannot start a worker, so no other place exists.

Each context compiles each component each time it needs one. No context keeps
compiled bytes from one start to the next.

## Elements

### What an Author Writes

An author defines an element with a tag name and a string of Zena source. The
author's source imports the authoring library and exports one element
definition. The authoring library offers two forms of definition. In the class
form, the author extends a base class:

```zena
import { Element, View, Event, h } from 'authoring:element';

export class TodoItem extends Element {
  attributes(): Array<String> { return ['todo-id', 'title', 'completed']; }

  styles(): String { return 'li { display: flex; gap: 12px; } …'; }

  render(): View {
    let done = this.attribute('completed') == 'true';
    return h('li', {'class' => if (done) 'completed' else ''}, [
      h('input', {'type' => 'checkbox', 'checked' => if (done) 'true' else 'false'},
          [], {'change' => 'toggle'}),
      h('label', new Map<String, String>(), [this.attribute('title')], {'dblclick' => 'edit'}),
      h('button', {'class' => 'destroy'}, ['×'], {'click' => 'destroy'}),
    ]);
  }

  async on(handler: String, event: Event): Future<void> {
    if (handler == 'toggle') { this.emit('todo-toggle', this.attribute('todo-id')); }
    // …
  }
}
```

The authoring library is a package of the source bundle, named `authoring` in
the bundle's package manifest. An author imports `authoring:element` for
elements and `authoring:route` for routes.

In the function form, the author exports a function from attributes to a view,
plus an optional event function. The function form keeps no state between
renders.

The Rust side defines the element with one call, for example:

```rust
define_element("todo-item", TODO_ITEM_SOURCE).await?;
```

### What the Helper Does

The helper does these steps:

1. It writes glue that imports the author's definition and exports the element
   interface below.
2. It calls `compile` with the glue as the entry module and the element world as
   the world. The author's source is a file beside the glue.
3. If the compile fails, the helper returns the diagnostics as an error.
4. It makes one `Store` and one instance of the compiled component for the tag.
5. It registers a custom element class for the tag. Each lifecycle callback of
   that class calls the instance.

The helper writes the element world. It is the same for every element:

```wit
interface element {
  record event {
    kind: string,               // the DOM event type
    value: option<string>,      // the value of an input, or a CustomEvent's detail
    checked: option<bool>,
    key: option<string>,        // for a keyboard event
  }

  observed-attributes: async func() -> list<string>;
  styles: async func() -> string;

  create: async func(attributes: list<tuple<string, string>>) -> u32;
  connected: async func(id: u32);
  disconnected: async func(id: u32);
  attribute-changed: async func(id: u32, name: string, value: option<string>);
  handle-event: async func(id: u32, handler: string, event: event);
  render: async func(id: u32) -> list<node>;
}

interface host {
  // Dispatch a CustomEvent on the element's host node. The event bubbles and
  // crosses shadow boundaries.
  emit: func(id: u32, name: string, detail: string);
}

world element-component {
  import host;
  import wasi:http/types@0.3.0;
  import wasi:http/client@0.3.0;
  import wasi:cli/stdout@0.3.0;
  import wasi:cli/stderr@0.3.0;
  import wasi:clocks/monotonic-clock@0.3.0;
  export element;
}
```

Every export is `async func`, because a synchronous export with a `list` return
does not build. The world imports every interface that the page supplies. An
element that does not use an import still compiles, and the host framework links
the import anyway.

### Lifecycle

The custom element class maps each callback of the browser to a call:

- `connectedCallback` calls `create` with the element's current attributes when
  the element has no id. It then calls `connected` and renders.
- `attributeChangedCallback` calls `attribute-changed` and renders, but only
  when the element has an id. Before the first connection, `create` receives the
  attributes, so no change is lost.
- `disconnectedCallback` calls `disconnected`. The glue drops the object, and
  the element forgets its id. If the element connects again, it gets a new id
  and a new object.

The calls for one element run one at a time, in order. A call waits until the
call before it on the same element completes. Calls for different elements of
one tag can run at the same time, and they interleave where the Zena code
awaits. So `<todo-app>` can wait for HTTP while a `<todo-item>` renders.

### One Instance Per Tag

Each tag has one component instance, and the instance serves every element of
that tag. `create` returns an id. The glue keeps a table from each id to one
object of the author's definition. `disconnected` removes the object.

This table stands in for an exported resource, which Zena refuses. The authoring
library hides the table. If Zena gains exported resources, the authoring library
can use one without a change to what an author writes.

### Failure

Zena does not trap on an exception that escapes an async export with no result.
The export returns normally, as if the call did nothing. So the glue catches an
exception from the author's code and traps on purpose. Zena has no trap
intrinsic in its current code generator, so the glue reads through a null
reference.

A trap poisons the tag's `Store`. The host framework then replaces the contents
of each element of that tag with an error card. An error card is a short message
that names the tag and the trap. The shelf has a "Restart" control for the tag.
It compiles the tag again from the source it last started from, makes a new
`Store`, and each element connects again. A later edit that failed to compile
does not take part.

A route that traps starts again on its next request from the source it last
started from, in the same way. The compiler component recovers too: a compile
that traps it makes a new instance for the next compile.

### Rendering

`render` returns a view. A view is a tree, and WIT types cannot recurse. So the
authoring library flattens the tree into a list of nodes in document order. Each
node names its parent:

```wit
record node {
  parent: option<u32>,        // index of the parent node in the list
  key: option<string>,        // a stable identity among siblings
  kind: node-kind,
}
variant node-kind {
  element(element-node),
  text(string),
}
record element-node {
  tag: string,
  attributes: list<tuple<string, string>>,
  properties: list<tuple<string, property>>,
  events: list<tuple<string, string>>,      // DOM event name to handler name
}
variant property {
  text(string),               // for example `value`
  flag(bool),                 // for example `checked`
}
```

`h` takes a tag, a map of attributes, a list of children, and a map of events.
It puts `checked` and `value` in properties and every other key in attributes.
`checked` is set when its value is `true`. The key `key` gives the node its
stable identity. The host framework focuses a new node that has the `autofocus`
attribute, so the field of an edit takes focus when it appears.

The host framework keeps the last view of each element. After a render, it
compares the new view with the last view inside the element's shadow root. It
matches siblings by key when they have one, and by position when they do not.
For each pair of matched nodes, it applies these rules:

- If the tags are the same, it changes the attributes, properties, events, and
  text of the existing DOM node in place.
- If the tags differ, it replaces the DOM node.
- It removes nodes that have no match and inserts new ones.

A view can hold another element by its tag, for example `todo-item`. That child
is a separate custom element with its own instance and its own shadow root.

The host framework renders an element after `connected`, after each
`attribute-changed`, and after each `handle-event`.

The host framework puts the result of `styles` in each shadow root of the tag.

### Events

A view names its events. For each event, it gives the DOM event name and a
handler name. When the DOM event fires, the host framework calls `handle-event`
with the element id, the handler name, and an `event` record.

An element sends information to its parent with a DOM `CustomEvent`. The
authoring library calls `emit`, and the host framework dispatches the event on
the element's host node. The parent's view listens for that event like any other
DOM event. The `value` field of the parent's `event` record carries the
`detail`.

### Data Down, Events Up

Only the root element, `<todo-app>`, calls the API. It passes data to its
children as attributes, which are strings. A child reports a user action as a
`CustomEvent`. The root handles the event, calls a route, and renders again with
the new data.

Attributes keep each element an ordinary web component. A person can use one in
plain HTML.

### The Elements of the Demo

The demo has these elements:

- `<todo-app>` owns the list. It calls the routes and renders the other
  elements. Its `filter` attribute holds the filter. The host framework on the
  page sets that attribute from the URL hash at start and on each `hashchange`.
- `<todo-input>` is the field for a new todo and the toggle-all control.
- `<todo-item>` is one todo. It shows the checkbox, the title, and the delete
  button, and it edits the title in place.
- `<todo-footer>` shows the count of active todos, the filter links, and the
  "Clear completed" button.

`<todo-app>` follows the TodoMVC rules for an edit. An edit to an empty title
deletes the todo, so `<todo-app>` sends a `DELETE` for it.

## Routes

### What an Author Writes

An author defines a route with a URL pattern and a string of Zena source. The
author's source imports the authoring library and exports one function,
`handle`. It takes a simple request and returns a simple response:

```zena
import { Request, Response, json, status, problem } from 'authoring:route';
import { update, remove, Todo, ModelError } from 'demo:todo/model';

export async function handle(request: Request): Future<Response> {
  let id = request.params['id'];
  if (request.method == 'PATCH') {
    let fields = fieldsOf(request.json());
    let updated = await update(id, fields.title, fields.completed);
    if (updated is Ok<Todo, ModelError>) {
      return json(todoJson((updated as Ok<Todo, ModelError>).value));
    }
    return problem((updated as Err<Todo, ModelError>).error);
  }
  if (request.method == 'DELETE') {
    let removed = await remove(id);
    if (removed is Ok<void, ModelError>) { return status(204); }
    return problem((removed as Err<void, ModelError>).error);
  }
  return status(405);
}
```

A simple request holds these parts:

- The method.
- The path parameters from the pattern.
- The query parameters.
- The headers.
- The body as text, with a JSON reader.

A simple response holds a status, headers, and a body. The authoring library
gives constructors for the common cases. `json` makes a 200 with a JSON body.
`status` makes a response with no body. `problem` maps a model error to a
status: `not-found` to 404, and `empty-title` to 422.

The Rust side defines the route with one call, for example:

```rust
define_route("/api/todos/:id", TODO_BY_ID_SOURCE);
```

### What the Helper Does

The helper writes glue around the author's source and records the route. The
glue exports `wasi:http/handler@0.3.0`. Its `handle` reads the `wasi:http`
request into a simple request, calls the author's `handle`, and writes the
simple response back as a `wasi:http` response. The glue adds an `x-demo-route`
header that names the pattern. The glue holds the pattern as a literal. It
extracts the path parameters itself, so the request needs no other channel.

The helper writes the route world. It is the same for every route:

```wit
world route-component {
  import demo:todo/model;
  import wasi:http/types@0.3.0;
  import wasi:cli/stdout@0.3.0;
  import wasi:cli/stderr@0.3.0;
  import wasi:clocks/monotonic-clock@0.3.0;
  export wasi:http/handler@0.3.0;
}
```

The service worker matches each request against the patterns in the order of
definition. A request that matches no pattern goes to the network.

### Lazy Compilation

The service worker compiles a route on the first request that matches it after
the worker starts. It keeps the instance until the browser stops the worker.
Chromium stops an idle service worker after 30 seconds, so a later request can
pay for a new compile. The shelf shows that cost.

### Failure

If a route fails to compile, the service worker answers its requests with 500
and a body that holds the diagnostics. If a route traps, the service worker
answers that request with 500 and drops the instance. The next request to the
route compiles it again. Both failures reach the shelf.

### The Routes of the Demo

The demo has two routes. The first is `/api/todos`:

| Method   | Request body                                            | Response                                                          |
| -------- | ------------------------------------------------------- | ----------------------------------------------------------------- |
| `GET`    | none, `?filter=` is one of `all`, `active`, `completed` | 200, `{"todos": [todo], "counts": {"active": n, "completed": n}}` |
| `POST`   | `{"title": string}`                                     | 201, the new todo, or 422 for an empty title                      |
| `PATCH`  | `{"completed": bool}`                                   | 204. It sets every todo to `completed`.                           |
| `DELETE` | none                                                    | 204. It removes every completed todo.                             |

The second is `/api/todos/:id`:

| Method   | Request body                             | Response                      |
| -------- | ---------------------------------------- | ----------------------------- |
| `PATCH`  | `{"title"?: string, "completed"?: bool}` | 200, the todo, or 404, or 422 |
| `DELETE` | none                                     | 204, or 404                   |

A todo is `{"id": string, "title": string, "completed": bool}`. Any other method
gets 405.

## HTTP on Both Ends

The root element imports `wasi:http/client@0.3.0`. The host framework on the
page implements its `send` with `fetch`. The service worker intercepts that
`fetch` and gives the request to a route component, which exports
`wasi:http/handler@0.3.0`. So each request is a `wasi:http` request at the
moment it leaves a component and at the moment it reaches one.

`zena:fetch` sends only `GET`. The authoring library adds a client that sends
any method, with headers and a body.

One Rust implementation of a subset of `wasi:http/types@0.3.0` serves both
contexts. The subset is what the authoring library calls on the client side and
on the handler side. That is fields, requests, responses, and their bodies as
streams. A function outside the subset returns an error. So a change in Zena's
output shows as a failure and not as a silent wrong answer.

A guest hands the host some futures it never reads back, such as the trailers of
a body and the result of consuming it. The host reads each such future to its
end and discards the value. A guest's write to a future completes only when the
other end reads, so a dropped future would keep the guest's task from ending.

## The Todo Model

The todo model is Rust in the service worker. Route components import it as an
interface:

```wit
package demo:todo;

interface model {
  record todo { id: string, title: string, completed: bool }
  record counts { active: u32, completed: u32 }
  enum filter { all, active, completed }
  variant model-error { not-found, empty-title }

  query: async func(filter: filter) -> tuple<list<todo>, counts>;
  add: async func(title: string) -> result<todo, model-error>;
  update: async func(id: string, title: option<string>, completed: option<bool>)
    -> result<todo, model-error>;
  remove: async func(id: string) -> result<_, model-error>;
  toggle-all: async func(completed: bool);
  clear-completed: async func();
}
```

The error variant is `model-error` and not `error`, because a WIT type named
`error` clashes with Zena's built-in `Error`.

The todo model trims titles, refuses an empty title, assigns ids, filters, and
counts. It stores the list in IndexedDB. Its functions are async because
IndexedDB is async. A route component stays thin. It turns HTTP into calls on
the model and back.

## The Two Contexts

### Build

The demo is one crate in this workspace. Trunk builds one binary for the page
and one for the service worker.

The service worker has a small JavaScript entry. During its first evaluation,
the entry adds the `fetch` and `message` listeners and starts the Rust module
without top-level `await`. Each event waits until that start completes, and then
calls into the Rust module.

The page registers the service worker itself, because Trunk does not.

### Boot

The page starts in this order:

1. The page registers the service worker. The service worker calls `skipWaiting`
   and `clients.claim`.
2. The page fetches the compiler component and the source bundle, and
   instantiates the compiler.
3. The page reads any edited element sources from IndexedDB.
4. The page compiles and defines each element. Until its tag is defined, an
   element shows a skeleton. A skeleton is a gray placeholder in the shape of
   the element.
5. The page waits until the service worker controls it.
6. The page adds `<todo-app>` to the document. Its first request goes to the
   service worker.

If an edited source fails to compile at boot, the page compiles the shipped
source instead. The shelf keeps the edit and shows its diagnostics.

The service worker fetches the compiler component and the source bundle when it
starts. It reads any edited route sources from IndexedDB. It compiles each route
on first use. An edited route that fails to compile falls back to the shipped
source in the same way.

### Messages

The page and the service worker exchange these messages:

- The page sends `route-changed` with a pattern after it writes a new route
  source to IndexedDB, or after it removes an edit. The write completes before
  the message leaves, so the service worker always reads the new source.
- The service worker sends `route-compiled` after each compile of a route. It
  carries the pattern, the compile time, the instantiate time, and the
  diagnostics of a failure.
- The service worker sends `route-trapped` with a pattern after a route traps.
- The page sends `routes-status` when the shelf opens, and again while it stays
  open. The service worker answers with the last `route-compiled` of each route.

After a hard reload, no service worker controls the page, and the worker that is
already active does not claim the page again by itself. So the page sends
`claim` to the active worker when it starts without a controller, and the
worker's script calls `clients.claim`.

A message that wants an answer carries a `MessageChannel` port, and the service
worker answers on that port. It answers a message that fails with the reason, so
the page never waits on a failure. It answers `route-changed` with the route's
new status. Each status carries `compiledAt`, the wall-clock time of its
compile, so a reader can tell a new compile from an old one.

The browser test lane needs two more messages. `define-route` adds a route from
a source, and `compile-check` compiles and runs a program in the service worker.
The page exposes both on `window.demo`, beside hooks that define an element and
read each tag's status.

## The Shelf

The shelf is a collapsible panel along the bottom of the page. It is plain Rust
and HTML, not an element. A bad edit cannot break the tool that fixes it.

A bar along the bottom of the page is always shown. It holds the control that
opens and closes the shelf, and a tab for each element and each route. A dot on
a tab marks an edit that is not saved, or a failure: diagnostics or a trap. The
open shelf shows the source of the active tab. The page leaves room below the
application for the bar, or for the whole shelf when it is open. The browser
keeps whether the shelf is open and which tab is active, for the next visit.

For the source of the active tab, the shelf shows:

- The Zena source in a text area. Ctrl-S or Cmd-S saves it.
- Three times of the last start: the compile of the Zena source to a component
  ("Zena → Wasm"), the polyfill's compile of that component ("Wasm compile"),
  and the link and instantiation.
- The compiler's diagnostics after a failed compile.
- For an element, the count of connected elements and of instances.
- A "Save" control and a "Reset to original" control.
- A "Restart" control after a trap.

When a person saves an element, the page writes the source to IndexedDB and
compiles it. If the compile succeeds, the page makes a new `Store` and instance
for the tag. The host framework calls `create` and `connected` again for each
connected element of that tag, and renders each one. The elements keep their
attributes. State that lived only in the old instance is gone. If the compile
fails, the old instance keeps running and the shelf shows the diagnostics.

When a person saves a route, the page writes the source and sends
`route-changed`. The service worker compiles the route at once and answers with
`route-compiled`. If the compile fails, the old instance keeps serving.

"Reset to original" removes the edit from IndexedDB and applies the shipped
source in the same way as a save.

## Look

The demo follows [Material 3]. Its colors, shapes, type scale, and elevation are
CSS custom properties on the document, in a light theme and a dark theme. The
theme follows the color scheme setting of the browser. Custom properties inherit
through shadow boundaries, so every element reads the same tokens.

Each element's styles live in its Zena source. `styles` returns them. A person
can change the look of an element from the shelf.

The demo does not use Material Web. Its controls are web components themselves,
so they compete with the demo's elements.

## Browsers

The demo runs in current Chrome and current Safari. Zena's output needs GC,
exceptions with `exnref`, and tail calls, which Safari has from 18.4. Safari
before 27 has no JSPI, and there the polyfill uses nested turns. The browser
test lane runs headless Chrome. A person runs the demo in Epiphany, which stands
in for Safari.

## Zena Compatibility Scenarios

The Zena compatibility suite of [PDD024] gains scenarios for the compiler
component. Each one runs on all three subjects, and the Wasmtime run sets the
expected results, as for every other scenario.

The compiler is not small, and a failure in it can have many causes. These
scenarios accept that cost. The other scenarios still locate small faults. The
compiler scenarios are also slow on the native subjects, where one compile can
take several seconds.

This design extends the suite in four ways:

- The pin can name a fork of Zena, under the rule in "The Pin" above.
- The build of these scenarios uses Zena's source tree, not only the `zena`
  command.
- The test host functions supply `read-source`. They answer it from the source
  bundle of the pinned toolchain.
- An expectations entry can name a component that a previous call returned.

For the last extension, the runner takes the `list<u8>` of the previous call's
`ok` result. It parses that list as a component, links it with the test host
functions, and instantiates it. Later entries can call its exports. The stages
of [PDD024] apply to the returned component in the same way. A parse failure
records `parse`, a missing import records `link`, and so on.

A polyfill subject passes a `compile` call when it returns the same bytes as the
Wasmtime run. So the compiler must give the same output for the same input.

The scenarios are:

1. The compiler compiles the program of scenario 1 of [PDD024]. The scenario
   instantiates the result and calls its export.
2. The compiler compiles a program with a type error. The call returns `err`.
   The diagnostics name the file and the line of the error.
3. The compiler compiles a program against a declared world with a custom
   import. The scenario instantiates the result, gives it the test host import,
   and calls it.
4. The compiler compiles a program that exports `wasi:http/handler@0.3.0`. The
   scenario does not instantiate the result, because the test host functions
   have no `wasi:http`.

## User Stories

A developer hears that the Component Model can run in a browser and doubts it.

> The developer opens the demo. A skeleton list appears, and a moment later the
> todo list draws. The developer adds three todos, completes one, and filters to
> "Active". Nothing looks unusual. Then the developer opens the shelf. It lists
> four elements and two routes, each with its Zena source and a compile time in
> milliseconds. The developer understands that the page compiled all of it a few
> seconds ago.

A person in the audience of a talk asks whether the source is real.

> The presenter opens `<todo-item>` in the shelf and changes the label of the
> delete button. The presenter saves. The shelf shows a new compile time, and
> every row in the list shows the new label. The todos stay. The presenter then
> types a syntax error and saves. The shelf shows Zena's diagnostic with its
> line number, and the list keeps working.

A developer wants to see a route compile.

> The developer opens the `/api/todos/:id` route in the shelf and makes `PATCH`
> refuse titles shorter than three letters. The developer saves. The shelf shows
> the service worker's compile time. The developer edits a todo to "ab", and the
> edit fails with the new rule.

The owner moves the Zena pin.

> The owner runs the Zena compatibility suite. The compiler scenarios pass on
> all three subjects. The owner runs `tests all`, and the demo lane passes. The
> owner knows that the demo still compiles and runs on the new pin.

## Test Cases

The compiler component builds from the pinned toolchain. The build compiles the
entry module with Zena's `--target component`. The result validates with
`wasm-tools`, and it imports no `wasi_snapshot_preview1` function.

The compiler component compiles in the browser. A test on the page instantiates
it on the polyfill and compiles a small program. The returned bytes instantiate
on the polyfill, and a call to their export returns the expected value.

The compiler is deterministic. A test compiles one source twice on one subject.
The two results are the same bytes.

The compiler reports diagnostics. A test compiles a program with an error. The
call returns `err`, and the text names the file and the line.

The compiler scenarios run on all three subjects. The four compiler scenarios
are in the record of the Zena compatibility suite. At acceptance, the record
shows `pass` for the browser subject in each one.

A returned component goes through the stages. A test gives the runner a scenario
whose compiled program imports an interface that the test host functions lack.
The record shows `link` for that scenario.

Authors never write WIT. The source of every element and route of the demo
contains no WIT and no `wasi:` import.

The class form and the function form both work. A test defines one element in
each form. Both render their attributes, and both handle a click.

An element renders and diffs. A test defines an element whose view depends on an
attribute. After a change to the attribute, every node whose tag did not change
is the same DOM object as before. Only its attributes or text changed.

Keys preserve identity. A test renders a keyed list of five items and then
reverses it. The five DOM nodes are the same objects as before, in the new
order.

An element handles events. A test clicks a node whose view names a `click`
handler. `handle-event` receives the handler name, and the element renders
again.

A child event reaches its parent. A test makes a child element call `emit`. The
parent's handler receives the event, with the detail in `value`.

The lifecycle callbacks map to calls. A test sets an attribute before it adds an
element to the document. `create` receives the attribute. A test removes an
element and adds it again. The element gets a new id.

The calls of one element run in order. A test fires two events on one element in
one task. The second `handle-event` starts after the first completes.

One instance serves every element of a tag. A test adds fifty elements of one
tag and then removes forty. The shelf shows one instance throughout, and it
shows fifty and then ten connected elements.

Styles apply. The computed style of a node in an element matches the CSS that
its `styles` returns.

A trap shows an error card. A test makes an element trap in `handle-event`.
Every element of that tag shows an error card. After "Restart", the tag works
again.

A route answers through `wasi:http`. A test sends each request in the two route
tables from the page through `fetch`. Each response has the status and the body
that the table gives. A method that a route does not accept returns 405.

The root element calls through `wasi:http/client`. A test adds a todo through
the user interface and watches the network. Each API request has a response with
the `x-demo-route` header, and no API request reaches the network.

An unmatched request goes to the network. A test fetches a static file of the
demo. The response has no `x-demo-route` header.

The todo model enforces its rules. A test adds a todo with an empty title and a
todo with a title with spaces at both ends. The first fails with 422. The second
stores the trimmed title.

The todo list persists. A test adds todos, stops the service worker, and reloads
the page. The list is the same.

A route compiles again after the service worker stops. A test stops the service
worker and sends a request. The response is correct, and the shelf shows a new
compile for that route.

A route failure answers 500. A test saves a route that traps. The request gets
500, and the shelf shows the trap. The next request compiles the route again.

Every TodoMVC behavior works. The browser test lane covers each behavior of the
TodoMVC specification through the shadow roots:

- Add, toggle, toggle all, and delete.
- Edit by double-click, with Enter, with blur, and with Escape.
- Delete by an edit to an empty title.
- The count of active items.
- The three filters by URL hash.
- "Clear completed", and when it shows.
- Persistence after a reload.

An element edit applies in place. A test changes the source of `<todo-item>` in
the shelf and saves. Every item renders with the change, and the list keeps its
todos.

A failed edit keeps the last good version. A test saves a source with a syntax
error. The shelf shows the diagnostics, and the old element keeps working.

An edit that fails at boot falls back. A test stores an edit that does not
compile and reloads the page. The element runs its shipped source, and the shelf
shows the edit and its diagnostics.

A route edit applies in the service worker. A test changes a route's source in
the shelf and saves. The next request gets the new behavior. After the service
worker stops and starts again, the request still gets the new behavior.

"Reset to original" restores the shipped source. After an edit, a test resets
the element and the route. Both behave as shipped, and IndexedDB holds no edit.

The shelf shows timings. After the page starts, the shelf shows a Zena compile
time, a Wasm compile time, and an instantiate time for every element. After the
first request to each route, it shows the same for that route.

The look follows the system theme. A test renders the demo with the light color
scheme and with the dark color scheme. The background color and the primary
color change between the two. A test changes a color token on the document, and
the computed color in each element changes with it.

The demo runs in Safari. A person opens the demo in Epiphany and completes the
TodoMVC behaviors by hand. The person records the result in the lane's report,
the README of the demo lane.

The menu serves the demo. A menu command builds the demo and serves it on a
local port. The page loads, and the service worker controls it after one reload
at most.

The demo lane runs in `tests all`. The menu runs the browser test lane for the
demo inside `tests all`, and a failure in the lane fails `tests all`.

## References

- [PDD000], the scope of the polyfill and its exclusion of WASI worlds.
- [PDD024], the Zena compatibility suite, its pin, and its subjects.
- [PDD025], the runtime layer and its browser backend.
- [Zena], the language and its repository.
- [Zena compiler api], the in-memory library entry of the compiler.
- [Zena compiler package], the build of that entry with the `host` target.
- [Zena compiler time] and [Zena WIT document], the clock and the WIT file
  reader.
- [Zena WASI 0.3 move], the move of the clock to `zena:time`.
- [Zena WIT reader change], the change that lets a host pass its WIT reader.
- [Zena component emission], Zena's component target.
- [Zena async exports], the refusal of an exported resource.
- [Zena stdlib http], [Zena stdlib fetch], and [Zena http service], Zena's
  `wasi:http` support.
- [Wasm features], browser support for Wasm features.
- [JSPI], the JavaScript Promise Integration proposal.
- [browser backend] and [yield wake], the polyfill's browser facts.
- [Trunk assets], Trunk's worker builds.
- [Service Workers], the rule for the `fetch` listener.
- [HTML module loading], the refusal of a dynamic `import()` in a service
  worker.
- [Chromium service worker], the idle timeout.
- [File System], where synchronous file access exists.
- [TodoMVC], the application specification.
- [Material 3], the design system.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD024]: ./PDD024%20Zena%20Toolchain%20Compatibility.md
[PDD025]: ./PDD025%20Runtime%20Layer.md
[Zena]: https://zena-lang.dev/
[Zena compiler api]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/zena-compiler/zena/cli/api.zena
[Zena compiler package]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/zena-compiler/package.json#L64-L65
[Zena compiler time]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/zena-compiler/zena/lib/time.zena#L3
[Zena WIT document]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/wit-parser/zena/document.zena
[Zena WASI 0.3 move]:
  https://github.com/elematic/zena/commit/04926ae40dc24ec56d2e25b091119c55d75a8ee7
[Zena WIT reader change]:
  https://github.com/cdata/zena/commit/1bbe472f4c34d3faf12c74f9876afcfe541c4ac3
[Zena component emission]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/docs/design/component-emission.md
[Zena async exports]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/docs/design/component-async-exports.md
[Zena stdlib http]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/stdlib/zena/http/index.zena
[Zena stdlib fetch]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/stdlib/zena/fetch/component.zena#L116
[Zena http service]:
  https://github.com/elematic/zena/blob/b2237f7e65847eda43ef1f4094eea77fe225ce0d/packages/zena-compiler/test-files/component/http-service.zena
[Wasm features]: https://webassembly.org/features/
[JSPI]: https://github.com/WebAssembly/js-promise-integration
[browser backend]: ../../rust/wcmp-wasm-core-web/src/lib.rs
[yield wake]: ../../rust/wcmp/src/concurrency/yield_wake.rs
[Trunk assets]: https://trunk-rs.github.io/trunk/guide/assets/
[Service Workers]:
  https://w3c.github.io/ServiceWorker/#run-service-worker-algorithm
[HTML module loading]:
  https://html.spec.whatwg.org/multipage/webappapis.html#hostloadimportedmodule
[Chromium service worker]:
  https://chromium.googlesource.com/chromium/src/+/main/content/browser/service_worker/service_worker_version.h
[File System]: https://fs.spec.whatwg.org/
[TodoMVC]: https://github.com/tastejs/todomvc/blob/master/app-spec.md
[Material 3]: https://m3.material.io/

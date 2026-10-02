// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The glue a helper writes around an author's source, and the compile
//! it asks for.
//!
//! An author writes Zena against the authoring library and never writes
//! WIT. The helper writes a small Zena module, the glue, that imports
//! the author's definition and exports the WIT interface of the world:
//! the `element` interface for an element, and `wasi:http/handler` for a
//! route. The author's source is a file beside the glue, which imports
//! it by a relative path, so the compiler's diagnostics name the
//! author's file.

mod compile;
mod form;

pub use compile::Compile;
pub use form::Form;

/// The WIT document every element compiles against.
pub const ELEMENT_WIT: &str = include_str!("../../zena/wit/element.wit");

/// The world of [`ELEMENT_WIT`] an element compiles against.
pub const ELEMENT_WORLD: &str = "element-component";

/// The WIT document every route compiles against, which holds the todo
/// model's interface too.
pub const ROUTE_WIT: &str = include_str!("../../zena/wit/route.wit");

/// The world of [`ROUTE_WIT`] a route compiles against.
pub const ROUTE_WORLD: &str = "route-component";

/// Which form `source` defines its element in, or `None` when it
/// exports neither a class nor a `render` function.
///
/// The test is textual, on the export declarations: the helper writes
/// glue for the form before the compiler has read the source, and the
/// compiler then holds the source to it.
pub fn form_of(source: &str) -> Option<Form> {
    let exports = |name: &str| {
        ["export function ", "export async function ", "export let "]
            .iter()
            .any(|prefix| {
                source.lines().any(|line| {
                    line.trim_start()
                        .strip_prefix(prefix)
                        .and_then(|rest| rest.strip_prefix(name))
                        .is_some_and(|rest| rest.starts_with(['(', ' ', '=', '<']))
                })
            })
    };
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("export class ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && rest[name.len()..].contains("extends") {
            return Some(Form::Class(name));
        }
    }
    exports("render").then(|| Form::Functions {
        on: exports("on"),
        styles: exports("styles"),
        attributes: exports("attributes"),
    })
}

/// The file name a tag's source takes beside its glue, such as
/// `todo-item.zena`.
pub fn element_file(tag: &str) -> String {
    format!("{tag}.zena")
}

/// The file name a route's source takes beside its glue: the pattern's
/// segments joined with `-`, such as `api-todos-id.zena` for
/// `/api/todos/:id`.
pub fn route_file(pattern: &str) -> String {
    let name: Vec<String> = pattern
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.trim_start_matches(':').to_string())
        .collect();
    format!("{}.zena", name.join("-"))
}

/// The glue of an element whose author's source is at `file` and
/// defines its element in `form`.
pub fn element_glue(file: &str, form: &Form) -> String {
    let mut glue = String::from(ELEMENT_GLUE_HEAD);
    match form {
        Form::Class(name) => {
            glue.push_str(&format!(
                "import {{ {name} }} from './{file}';\n\n\
                 let registry = new Registry((): Element => new {name}());\n"
            ));
        }
        Form::Functions {
            on,
            styles,
            attributes,
        } => {
            let mut names = vec!["render as authorRender"];
            if *on {
                names.push("on as authorOn");
            }
            if *styles {
                names.push("styles as authorStyles");
            }
            if *attributes {
                names.push("attributes as authorAttributes");
            }
            glue.push_str(&format!(
                "import {{ {} }} from './{file}';\n\n",
                names.join(", ")
            ));
            glue.push_str("class FunctionElement extends Element {\n");
            glue.push_str("  render(): View {\n    return authorRender(this);\n  }\n");
            if *on {
                glue.push_str(
                    "  async on(handler: String, event: Event): Future<void> {\n    \
                     await authorOn(handler, event, this);\n  }\n",
                );
            }
            if *styles {
                glue.push_str("  styles(): String {\n    return authorStyles();\n  }\n");
            }
            if *attributes {
                glue.push_str(
                    "  attributes(): Array<String> {\n    return authorAttributes();\n  }\n",
                );
            }
            glue.push_str(
                "}\n\nlet registry = new Registry((): Element => new FunctionElement());\n",
            );
        }
    }
    glue.push_str(ELEMENT_GLUE_TAIL);
    glue
}

/// The imports every element's glue starts with.
const ELEMENT_GLUE_HEAD: &str = "\
// The glue the demo writes around an element's source. It exports the
// `element` interface through the authoring library's registry.
import { Future } from 'zena:async';
import { Option } from 'zena:core';
import { Event as WireEvent, Node } from 'demo:element/element';
import { Element, Event, Registry, View } from 'authoring:element';
";

/// The exports every element's glue ends with.
const ELEMENT_GLUE_TAIL: &str = "
export async function observedAttributes(): Future<Array<String>> {
  return registry.observedAttributes();
}

export async function styles(): Future<String> {
  return registry.styles();
}

export async function create(attributes: Array<(String, String)>): Future<u32> {
  return registry.create(attributes);
}

export async function connected(id: u32): Future<void> {
  await registry.connected(id);
}

export async function disconnected(id: u32): Future<void> {
  registry.disconnected(id);
}

export async function attributeChanged(id: u32, name: String,
    value: Option<String>): Future<void> {
  await registry.attributeChanged(id, name, value);
}

export async function handleEvent(id: u32, handler: String,
    event: WireEvent): Future<void> {
  await registry.handleEvent(id, handler, event);
}

export async function render(id: u32): Future<Array<Node>> {
  return registry.render(id);
}
";

/// The glue of a route for `pattern`, whose author's source is at
/// `file`. Zena cannot pass an imported function as a value, so the
/// glue hands the authoring library a closure that calls it.
pub fn route_glue(pattern: &str, file: &str) -> String {
    let pattern = pattern.replace('\\', "\\\\").replace('\'', "\\'");
    format!(
        "\
// The glue the demo writes around a route's source. It exports
// `wasi:http/handler` through the authoring library.
import {{ Request, Response, ErrorCode }} from 'wasi:http/types';
import {{ Future }} from 'zena:async';
import {{ Outcome, Ok }} from 'zena:core';
import {{
  serve, Request as SimpleRequest, Response as SimpleResponse
}} from 'authoring:route';
import {{ handle as authorHandle }} from './{file}';

export async function handle(request: Request): Future<Outcome<Response, ErrorCode>> {{
  return new Ok<Response, ErrorCode>(await serve('{pattern}', request,
      (simple: SimpleRequest): Future<SimpleResponse> => authorHandle(simple)));
}}
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_finds_the_class_an_element_exports() {
        let source = "import { Element } from 'authoring:element';\n\
                      export class TodoItem extends Element {\n}\n";
        assert_eq!(form_of(source), Some(Form::Class("TodoItem".to_string())));
    }

    #[wcmp_macros::test]
    fn it_finds_the_functions_an_element_exports() {
        let source = "export function render(element: Element): View {}\n\
                      export async function on(handler: String, event: Event, element: Element): Future<void> {}\n";
        assert_eq!(
            form_of(source),
            Some(Form::Functions {
                on: true,
                styles: false,
                attributes: false,
            })
        );
        assert_eq!(form_of("export let renderer = 1;\n"), None);
    }

    #[wcmp_macros::test]
    fn it_names_a_route_file_after_its_pattern() {
        assert_eq!(route_file("/api/todos/:id"), "api-todos-id.zena");
        assert_eq!(route_file("/api/todos"), "api-todos.zena");
    }
}

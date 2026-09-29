//! Checking a registered core module against the module type an
//! import declares.
//!
//! The rules are Wasmtime's: the module must provide every export the
//! type lists, with a compatible type, and every import the module
//! asks for must be listed by the type, with a compatible type in the
//! opposite direction. The messages are Wasmtime's wording, so a host
//! that moves between the two sees the same diagnostics.

use core::fmt::Write as _;

use crate::component::ModuleType;
use crate::module::{CoreExternType, CoreValueType, Module};

/// Check that `actual` satisfies `expected`. The `Err` carries the
/// reason in Wasmtime's wording.
pub fn module_satisfies(expected: &ModuleType, actual: &Module) -> Result<(), String> {
    for export in &expected.exports {
        let found = actual
            .exports()
            .iter()
            .find(|candidate| candidate.name == export.name)
            .ok_or_else(|| format!("module export `{}` not defined", export.name))?;
        entity_matches(&export.ty, &found.ty).map_err(|reason| {
            format!(
                "module export `{}` has the wrong type: {reason}",
                export.name
            )
        })?;
    }
    for import in actual.imports() {
        let listed = expected
            .imports
            .iter()
            .find(|candidate| candidate.module == import.module && candidate.name == import.name)
            .ok_or_else(|| {
                format!(
                    "module import `{}::{}` not defined",
                    import.module, import.name
                )
            })?;
        entity_matches(&import.ty, &listed.ty).map_err(|reason| {
            format!(
                "module import `{}::{}` has the wrong type: {reason}",
                import.module, import.name
            )
        })?;
    }
    Ok(())
}

/// Whether `actual` is usable where `expected` is required: function
/// and tag types match exactly, globals match exactly, and a memory
/// or table may start larger and be bounded tighter than required.
fn entity_matches(expected: &CoreExternType, actual: &CoreExternType) -> Result<(), String> {
    match (expected, actual) {
        (CoreExternType::Func { .. }, CoreExternType::Func { .. }) => {
            if expected == actual {
                Ok(())
            } else {
                Err(format!(
                    "expected type `{}`, found type `{}`",
                    render(expected),
                    render(actual)
                ))
            }
        }
        (
            CoreExternType::Global {
                content: expected_content,
                mutable: expected_mutable,
            },
            CoreExternType::Global {
                content: actual_content,
                mutable: actual_mutable,
            },
        ) => {
            if expected_content != actual_content {
                return Err(format!(
                    "global types incompatible: expected type `{}`, found type `{}`",
                    render(expected),
                    render(actual)
                ));
            }
            if expected_mutable != actual_mutable {
                return Err(format!(
                    "global types incompatible: expected {} global, found {} global",
                    mutability(*expected_mutable),
                    mutability(*actual_mutable)
                ));
            }
            Ok(())
        }
        (
            CoreExternType::Memory {
                minimum_pages: expected_min,
                maximum_pages: expected_max,
                memory64: expected_64,
                shared: expected_shared,
            },
            CoreExternType::Memory {
                minimum_pages: actual_min,
                maximum_pages: actual_max,
                memory64: actual_64,
                shared: actual_shared,
            },
        ) => {
            if expected_shared != actual_shared {
                return Err(format!(
                    "memory types incompatible: expected {} memory, found {} memory",
                    sharedness(*expected_shared),
                    sharedness(*actual_shared)
                ));
            }
            if expected_64 != actual_64 {
                return Err(format!(
                    "memory types incompatible: expected {} memory, found {} memory",
                    index_width(*expected_64),
                    index_width(*actual_64)
                ));
            }
            limits_match(
                "memory",
                *expected_min,
                *expected_max,
                *actual_min,
                *actual_max,
            )
        }
        (
            CoreExternType::Table {
                element: expected_element,
                minimum: expected_min,
                maximum: expected_max,
            },
            CoreExternType::Table {
                element: actual_element,
                minimum: actual_min,
                maximum: actual_max,
            },
        ) => {
            if expected_element != actual_element {
                return Err(format!(
                    "table types incompatible: expected type `{}`, found type `{}`",
                    render(expected),
                    render(actual)
                ));
            }
            limits_match(
                "table",
                *expected_min,
                *expected_max,
                *actual_min,
                *actual_max,
            )
        }
        (CoreExternType::Tag { .. }, CoreExternType::Tag { .. }) => {
            if expected == actual {
                Ok(())
            } else {
                Err("incompatible tag types".to_owned())
            }
        }
        _ => Err(format!(
            "expected {} found {}",
            kind(expected),
            kind(actual)
        )),
    }
}

/// Wasmtime's limit rule: the actual item must start at least as
/// large as required, and when the requirement bounds the size, the
/// actual item must be bounded no looser.
fn limits_match(
    what: &str,
    expected_min: u64,
    expected_max: Option<u64>,
    actual_min: u64,
    actual_max: Option<u64>,
) -> Result<(), String> {
    if actual_min < expected_min {
        return Err(format!(
            "{what} types incompatible: expected minimum of {expected_min}, found minimum of {actual_min}"
        ));
    }
    match (expected_max, actual_max) {
        (Some(expected), Some(actual)) if actual > expected => Err(format!(
            "{what} types incompatible: expected maximum of {expected}, found maximum of {actual}"
        )),
        (Some(expected), None) => Err(format!(
            "{what} types incompatible: expected maximum of {expected}, found no maximum"
        )),
        _ => Ok(()),
    }
}

fn kind(ty: &CoreExternType) -> &'static str {
    match ty {
        CoreExternType::Func { .. } => "func",
        CoreExternType::Global { .. } => "global",
        CoreExternType::Memory { .. } => "memory",
        CoreExternType::Table { .. } => "table",
        CoreExternType::Tag { .. } => "tag",
    }
}

fn mutability(mutable: bool) -> &'static str {
    if mutable { "mutable" } else { "immutable" }
}

fn sharedness(shared: bool) -> &'static str {
    if shared { "shared" } else { "non-shared" }
}

fn index_width(memory64: bool) -> &'static str {
    if memory64 { "64-bit" } else { "32-bit" }
}

/// Render a core type in text-format style, as Wasmtime prints one.
fn render(ty: &CoreExternType) -> String {
    let mut out = String::new();
    match ty {
        CoreExternType::Func { params, results } => {
            out.push_str("(func");
            if !params.is_empty() {
                out.push_str(" (param");
                for param in params {
                    let _ = write!(out, " {}", value_type(*param));
                }
                out.push(')');
            }
            if !results.is_empty() {
                out.push_str(" (result");
                for result in results {
                    let _ = write!(out, " {}", value_type(*result));
                }
                out.push(')');
            }
            out.push(')');
        }
        CoreExternType::Global { content, mutable } => {
            if *mutable {
                let _ = write!(out, "(mut {})", value_type(*content));
            } else {
                out.push_str(value_type(*content));
            }
        }
        CoreExternType::Memory {
            minimum_pages,
            maximum_pages,
            ..
        } => {
            let _ = write!(out, "(memory {minimum_pages}");
            if let Some(maximum) = maximum_pages {
                let _ = write!(out, " {maximum}");
            }
            out.push(')');
        }
        CoreExternType::Table {
            element,
            minimum,
            maximum,
        } => {
            let _ = write!(out, "(table {minimum}");
            if let Some(maximum) = maximum {
                let _ = write!(out, " {maximum}");
            }
            let _ = write!(out, " {})", value_type(*element));
        }
        CoreExternType::Tag { params } => {
            out.push_str("(tag");
            if !params.is_empty() {
                out.push_str(" (param");
                for param in params {
                    let _ = write!(out, " {}", value_type(*param));
                }
                out.push(')');
            }
            out.push(')');
        }
    }
    out
}

fn value_type(ty: CoreValueType) -> &'static str {
    match ty {
        CoreValueType::I32 => "i32",
        CoreValueType::I64 => "i64",
        CoreValueType::F32 => "f32",
        CoreValueType::F64 => "f64",
        CoreValueType::V128 => "v128",
        CoreValueType::FuncRef => "funcref",
        CoreValueType::ExternRef => "externref",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func(params: &[CoreValueType], results: &[CoreValueType]) -> CoreExternType {
        CoreExternType::Func {
            params: params.to_vec(),
            results: results.to_vec(),
        }
    }

    #[wcmp_macros::test]
    fn it_renders_a_function_type_like_the_text_format() {
        assert_eq!(
            render(&func(&[CoreValueType::I32], &[CoreValueType::I64])),
            "(func (param i32) (result i64))"
        );
        assert_eq!(render(&func(&[], &[])), "(func)");
    }

    #[wcmp_macros::test]
    fn it_reports_a_kind_mismatch_in_wasmtimes_words() {
        let global = CoreExternType::Global {
            content: CoreValueType::I32,
            mutable: false,
        };
        assert_eq!(
            entity_matches(&global, &func(&[], &[])),
            Err("expected global found func".to_owned())
        );
    }

    #[wcmp_macros::test]
    fn it_lets_a_memory_start_larger_and_be_bounded_tighter() {
        assert!(limits_match("memory", 1, Some(4), 2, Some(3)).is_ok());
        assert!(limits_match("memory", 1, None, 1, Some(3)).is_ok());
        assert!(limits_match("memory", 2, None, 1, None).is_err());
        assert!(limits_match("memory", 1, Some(2), 1, None).is_err());
        assert!(limits_match("memory", 1, Some(2), 1, Some(3)).is_err());
    }
}

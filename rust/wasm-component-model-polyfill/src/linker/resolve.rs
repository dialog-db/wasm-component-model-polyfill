//! Identifier resolution: matching a component's imports against
//! the linker's registered [`LinkerInstance`]s.
//!
//! The matching rules implement the WIT specification's reading of
//! semver and are narrower than cargo-style caret matching:
//!
//! - For pre-`1.0.0` versions the compatibility range is the *minor*
//!   segment: `0.2.0` and `0.2.7` are compatible, `0.2.0` and
//!   `0.3.0` are not.
//! - For `>= 1.0.0` versions the compatibility range is the *major*
//!   segment: `1.4.0` and `1.7.2` are compatible, `1.4.0` and
//!   `2.0.0` are not.
//! - An import without a version matches a registration without a
//!   version exactly. A versioned import does not match an
//!   unversioned registration, and vice versa.
//!
//! When more than one registered candidate falls in an import's
//! compatibility range, resolution selects the *highest-versioned*
//! candidate.
//!
//! Beyond identifier matching, the resolver also checks that each
//! interface item the component imports has a registered host
//! function whose declared signature is structurally equal to the
//! item's. Missing items surface as
//! [`LinkError::UnresolvedImport`]; signature mismatches surface as
//! [`Error::TypeMismatch`].
//!
//! [`LinkerInstance`]: super::LinkerInstance

use semver::Version;

use crate::component::{
    Component, ComponentImport, ExternType, ExternalName, FunctionType, InstanceItem,
};
use crate::error::{Error, LinkError, Result, TypeMismatch, TypeMismatchPosition, TypeRendering};
use crate::identifier::InterfaceIdentifier;

use super::linker::Linker;
use super::registration::InstanceRegistration;

/// The outcome of resolving a single component import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportBinding {
    /// The import was satisfied by the named registered entry.
    Resolved {
        /// The registered linker-instance key chosen for this
        /// import.
        chosen: InterfaceIdentifier,
    },
    /// The import was an interface-typed instance with an empty
    /// item set, and no registered entry was needed to satisfy it.
    /// The linker may still have had a matching entry; this variant
    /// records that it was not consulted.
    Vacuous,
}

/// The outcome of resolving every import of a component, in
/// declaration order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolution {
    /// One binding per import; `bindings[i]` corresponds to
    /// `component.imports[i]`.
    pub bindings: Vec<ImportBinding>,
}

/// Walk the component's declared imports and resolve each against
/// the linker's registered linker instances.
///
/// Failures fall into three categories: identifier-resolution
/// failures (missing match, ambiguous match, unsatisfiable semver,
/// plain-named imports the polyfill does not yet handle) surface as
/// [`Error::Link`]; signature mismatches between a host registration
/// and the component's declared item surface as
/// [`Error::TypeMismatch`].
pub fn resolve_imports<T: 'static>(
    component: &Component,
    linker: &Linker<T>,
) -> Result<Resolution> {
    let registered: Vec<&InterfaceIdentifier> = linker.registered_keys().collect();
    let mut bindings = Vec::with_capacity(component.imports.len());
    for import in component.imports.iter() {
        let binding = resolve_one(import, &registered).map_err(Error::from)?;
        if let ImportBinding::Resolved { chosen } = &binding {
            // Item-level type check: every function item the
            // import declares must have a registered host function
            // whose signature matches structurally.
            check_items(import, chosen, linker)?;
        }
        bindings.push(binding);
    }
    Ok(Resolution { bindings })
}

#[allow(clippy::result_large_err)]
fn resolve_one(
    import: &ComponentImport,
    registered: &[&InterfaceIdentifier],
) -> core::result::Result<ImportBinding, LinkError> {
    match (&import.name, &import.ty) {
        (ExternalName::Interface(id), ExternType::Instance(instance))
            if instance.items.is_empty() =>
        {
            match find_match(id, registered.iter().copied()) {
                Some(chosen) => Ok(ImportBinding::Resolved {
                    chosen: chosen.clone(),
                }),
                None => Ok(ImportBinding::Vacuous),
            }
        }
        (ExternalName::Interface(id), _) => match find_match(id, registered.iter().copied()) {
            Some(chosen) => Ok(ImportBinding::Resolved {
                chosen: chosen.clone(),
            }),
            None => Err(unresolved_or_incompatible(import, id, registered)),
        },
        (ExternalName::Plain(_), _) => Err(LinkError::UnsupportedRegistration {
            import: import.name.clone(),
            reason: "plain-named imports require host-item registration",
        }),
    }
}

fn check_items<T: 'static>(
    import: &ComponentImport,
    chosen: &InterfaceIdentifier,
    linker: &Linker<T>,
) -> Result<()> {
    let registration = linker.registration_for(chosen).ok_or_else(|| {
        Error::from(LinkError::UnresolvedImport {
            import: import.name.clone(),
        })
    })?;
    let items: &[InstanceItem] = match &import.ty {
        ExternType::Instance(instance) => &instance.items,
        // Non-instance interface-typed imports (e.g. an interface-
        // named function or resource) don't have item lists; the
        // identifier match alone is the contract for now.
        _ => return Ok(()),
    };
    for item in items.iter() {
        match &item.ty {
            ExternType::Function(declared) => {
                check_function_item(chosen, &item.name, declared, registration)?;
            }
            ExternType::Resource(_) | ExternType::ResourceEquals(_) => {
                check_resource_item(&item.name, registration, &import.name)?;
            }
            // Type, Instance, Module, Component, Value: the WIT shapes
            // typical interfaces use are functions plus opaque types;
            // anything else either has no runtime presence or is out
            // of the synchronous baseline.
            _ => {}
        }
    }
    Ok(())
}

fn check_resource_item<T: 'static>(
    item_name: &str,
    registration: &InstanceRegistration<T>,
    import_name: &ExternalName,
) -> Result<()> {
    if registration.resource(item_name).is_some() {
        return Ok(());
    }
    Err(Error::from(LinkError::UnresolvedImport {
        import: import_name.clone(),
    }))
}

fn check_function_item<T: 'static>(
    chosen: &InterfaceIdentifier,
    item_name: &str,
    declared: &FunctionType,
    registration: &InstanceRegistration<T>,
) -> Result<()> {
    let host = registration.func(item_name).ok_or_else(|| {
        Error::from(LinkError::UnresolvedImport {
            import: ExternalName::Interface(chosen.clone()),
        })
    })?;
    if !function_types_compatible(&host.signature, declared) {
        return Err(Error::from(TypeMismatch {
            position: TypeMismatchPosition::HostFunctionRegistration {
                interface: chosen.clone(),
                item: item_name.to_owned(),
            },
            expected: TypeRendering::Function(declared.clone()),
            actual: TypeRendering::Function(host.signature.clone()),
        }));
    }
    Ok(())
}

/// Two function types are compatible when their result types and
/// parameter type lists match structurally. Parameter *names* are
/// not load-bearing at the canonical-ABI level — they live in the
/// component's WIT description for tooling but don't affect the
/// wire format — so the resolver compares by position rather than
/// by `PartialEq` on the full struct.
fn function_types_compatible(a: &FunctionType, b: &FunctionType) -> bool {
    a.result == b.result
        && a.parameters.len() == b.parameters.len()
        && a.parameters
            .iter()
            .zip(b.parameters.iter())
            .all(|(a, b)| a.ty == b.ty)
}

fn unresolved_or_incompatible(
    import: &ComponentImport,
    id: &InterfaceIdentifier,
    registered: &[&InterfaceIdentifier],
) -> LinkError {
    let same_shape: Vec<&InterfaceIdentifier> = registered
        .iter()
        .copied()
        .filter(|c| shape_matches(id, c))
        .collect();
    if same_shape.is_empty() {
        LinkError::UnresolvedImport {
            import: import.name.clone(),
        }
    } else {
        LinkError::IncompatibleVersion {
            import: import.name.clone(),
            requested: id.package().version().cloned(),
            available: same_shape
                .iter()
                .map(|c| c.package().version().cloned())
                .collect(),
        }
    }
}

fn find_match<'a>(
    import: &InterfaceIdentifier,
    candidates: impl Iterator<Item = &'a InterfaceIdentifier>,
) -> Option<&'a InterfaceIdentifier> {
    let mut best: Option<&InterfaceIdentifier> = None;
    for candidate in candidates {
        if !shape_matches(import, candidate) {
            continue;
        }
        if !versions_compatible(import.package().version(), candidate.package().version()) {
            continue;
        }
        best = match best {
            None => Some(candidate),
            Some(current) if prefer(candidate, current) => Some(candidate),
            Some(current) => Some(current),
        };
    }
    best
}

fn shape_matches(import: &InterfaceIdentifier, candidate: &InterfaceIdentifier) -> bool {
    import.package().namespace() == candidate.package().namespace()
        && import.package().name() == candidate.package().name()
        && import.name() == candidate.name()
}

/// True when `candidate` falls in `import`'s WIT-spec compatibility
/// range.
pub fn versions_compatible(import: Option<&Version>, candidate: Option<&Version>) -> bool {
    match (import, candidate) {
        (None, None) => true,
        (None, Some(_)) | (Some(_), None) => false,
        (Some(imp), Some(cand)) => {
            if imp.major == 0 && cand.major == 0 {
                imp.minor == cand.minor
            } else {
                imp.major == cand.major
            }
        }
    }
}

/// Prefer `candidate` over `current` when its version is strictly
/// greater. Both must already have been confirmed compatible with
/// the same import.
fn prefer(candidate: &InterfaceIdentifier, current: &InterfaceIdentifier) -> bool {
    match (candidate.package().version(), current.package().version()) {
        (Some(n), Some(c)) => n > c,
        // Two unversioned candidates tie; keep the first.
        // Mixed shapes shouldn't reach here because shape_matches
        // and versions_compatible would have rejected one.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(v: &str) -> Version {
        v.parse().unwrap()
    }

    fn id(text: &str) -> InterfaceIdentifier {
        text.parse().unwrap()
    }

    #[test]
    fn it_falls_into_the_minor_compatibility_range_for_pre_one_versions() {
        assert!(versions_compatible(
            Some(&version("0.2.0")),
            Some(&version("0.2.7"))
        ));
    }

    #[test]
    fn it_excludes_a_different_minor_below_one() {
        assert!(!versions_compatible(
            Some(&version("0.2.0")),
            Some(&version("0.3.0"))
        ));
    }

    #[test]
    fn it_falls_into_the_major_compatibility_range_for_one_plus_versions() {
        assert!(versions_compatible(
            Some(&version("1.4.0")),
            Some(&version("1.7.2"))
        ));
    }

    #[test]
    fn it_excludes_a_different_major_at_or_above_one() {
        assert!(!versions_compatible(
            Some(&version("1.4.0")),
            Some(&version("2.0.0"))
        ));
    }

    #[test]
    fn it_excludes_pre_one_against_one_plus() {
        assert!(!versions_compatible(
            Some(&version("0.9.0")),
            Some(&version("1.0.0"))
        ));
    }

    #[test]
    fn it_treats_two_unversioned_as_compatible() {
        assert!(versions_compatible(None, None));
    }

    #[test]
    fn it_treats_unversioned_versus_versioned_as_no_match() {
        assert!(!versions_compatible(None, Some(&version("1.0.0"))));
        assert!(!versions_compatible(Some(&version("1.0.0")), None));
    }

    #[test]
    fn it_picks_highest_version_when_multiple_match() {
        let import = id("wasi:cli/run@0.2.0");
        let registered = [id("wasi:cli/run@0.2.5"), id("wasi:cli/run@0.2.7")];
        let chosen = find_match(&import, registered.iter()).cloned();
        assert_eq!(chosen, Some(id("wasi:cli/run@0.2.7")));
    }

    #[test]
    fn it_skips_a_candidate_with_a_different_interface_name() {
        let import = id("wasi:cli/run@0.2.0");
        let registered = [id("wasi:cli/exit@0.2.0")];
        let chosen = find_match(&import, registered.iter()).cloned();
        assert_eq!(chosen, None);
    }

    #[test]
    fn it_skips_a_candidate_with_a_different_package() {
        let import = id("wasi:cli/run@0.2.0");
        let registered = [id("wasi:io/run@0.2.0")];
        let chosen = find_match(&import, registered.iter()).cloned();
        assert_eq!(chosen, None);
    }
}

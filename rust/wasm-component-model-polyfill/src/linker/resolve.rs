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

use std::collections::HashMap;

use semver::Version;

use crate::component::{
    Component, ComponentImport, ExternType, ExternalName, FunctionType, InstanceItem, ModuleType,
};
use crate::error::{Error, LinkError, Result, TypeMismatch, TypeMismatchPosition, TypeRendering};
use crate::identifier::InterfaceIdentifier;
use crate::resource::ResourceTypeId;
use crate::types::{ResourceType, ValueType};

use super::linker::Linker;
use super::module_matching::module_satisfies;
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
    /// The import is plain-named and was satisfied through the root
    /// namespace: the root entry itself for a function or resource
    /// import, or the nested entry under the plain name for an
    /// instance import.
    Root,
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
/// failures (missing match, ambiguous match, unsatisfiable semver)
/// and root-namespace misses for plain-named imports surface as
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
        let binding = match &import.name {
            ExternalName::Plain(name) => resolve_plain(import, name, linker)?,
            ExternalName::Interface(_) => {
                let binding = resolve_one(import, &registered).map_err(Error::from)?;
                if let ImportBinding::Resolved { chosen } = &binding {
                    // Item-level type check: every function item the
                    // import declares must have a registered host function
                    // whose signature matches structurally.
                    let registration = linker.registration_for(chosen).ok_or_else(|| {
                        Error::from(LinkError::UnresolvedImport {
                            import: import.name.clone(),
                        })
                    })?;
                    check_items(import, registration, &ItemPosition::Interface(chosen))?;
                }
                binding
            }
        };
        bindings.push(binding);
    }
    check_shared_identities(component, linker, &bindings)?;
    Ok(Resolution { bindings })
}

/// A component that declares one resource type in several imports
/// (an instance whose item is `(type (eq $r))` of another's) needs
/// one identity behind all of them. Every registration for the same
/// resource must carry the same identity, or a handle minted under
/// one interface could not lower through the other.
fn check_shared_identities<T: 'static>(
    component: &Component,
    linker: &Linker<T>,
    bindings: &[ImportBinding],
) -> Result<()> {
    let mut seen: HashMap<usize, (ResourceTypeId, TypeMismatchPosition)> = HashMap::new();
    for (import, binding) in component.imports.iter().zip(bindings) {
        let ExternType::Instance(instance) = &import.ty else {
            continue;
        };
        let (registration, position) = match (binding, &import.name) {
            (ImportBinding::Resolved { chosen }, _) => {
                let Some(registration) = linker.registration_for(chosen) else {
                    continue;
                };
                (registration, ItemPosition::Interface(chosen))
            }
            (ImportBinding::Root, ExternalName::Plain(name)) => {
                let Some(registration) = linker.root_registration().instance(name) else {
                    continue;
                };
                (registration, ItemPosition::Plain(name))
            }
            _ => continue,
        };
        for item in instance.items.iter() {
            let (ExternType::Resource(resource) | ExternType::ResourceEquals(resource)) = &item.ty
            else {
                continue;
            };
            let (Some(index), Some(host)) = (resource.index(), registration.resource(&item.name))
            else {
                continue;
            };
            match seen.get(&index) {
                None => {
                    seen.insert(index, (host.type_id, position.for_item(&item.name)));
                }
                Some((first, _)) if *first == host.type_id => {}
                Some(_) => {
                    return Err(Error::from(TypeMismatch {
                        position: position.for_item(&item.name),
                        expected: TypeRendering::Value(ValueType::Own(resource.clone())),
                        actual: TypeRendering::Value(ValueType::Own(ResourceType::new(
                            item.name.clone(),
                        ))),
                    }));
                }
            }
        }
    }
    Ok(())
}

/// How a type-mismatch diagnostic names the registration an item
/// belongs to.
enum ItemPosition<'a> {
    /// An item of an interface-named import.
    Interface(&'a InterfaceIdentifier),
    /// An item of a plain-named import: the function itself, or an
    /// item of a plain-named instance.
    Plain(&'a str),
}

impl ItemPosition<'_> {
    fn for_item(&self, item: &str) -> TypeMismatchPosition {
        match self {
            ItemPosition::Interface(interface) => TypeMismatchPosition::HostFunctionRegistration {
                interface: (*interface).clone(),
                item: item.to_owned(),
            },
            ItemPosition::Plain(name) => TypeMismatchPosition::HostFunctionRegistrationPlain {
                name: if *name == item {
                    item.to_owned()
                } else {
                    format!("{name}.{item}")
                },
            },
        }
    }

    fn import_name(&self) -> ExternalName {
        match self {
            ItemPosition::Interface(chosen) => ExternalName::Interface((*chosen).clone()),
            ItemPosition::Plain(name) => ExternalName::Plain((*name).to_owned()),
        }
    }
}

/// Resolve a plain-named import through the root namespace. A
/// function or resource import is an item of the root entry under
/// its own name; an instance import is the nested entry under its
/// name, whose items are checked as an interface's would be.
fn resolve_plain<T: 'static>(
    import: &ComponentImport,
    name: &str,
    linker: &Linker<T>,
) -> Result<ImportBinding> {
    let root = linker.root_registration();
    let unresolved = || {
        Error::from(LinkError::UnresolvedImport {
            import: import.name.clone(),
        })
    };
    match &import.ty {
        ExternType::Function(declared) => {
            check_function_item(&ItemPosition::Plain(name), name, declared, root)?;
            Ok(ImportBinding::Root)
        }
        ExternType::Resource(_) | ExternType::ResourceEquals(_) => root
            .resource(name)
            .map(|_| ImportBinding::Root)
            .ok_or_else(unresolved),
        ExternType::Module(declared) => {
            check_module_item(&import.name, name, declared, root)?;
            Ok(ImportBinding::Root)
        }
        ExternType::Instance(instance) => match root.instance(name) {
            Some(registration) => {
                check_items(import, registration, &ItemPosition::Plain(name))?;
                Ok(ImportBinding::Root)
            }
            None if instance.items.is_empty() => Ok(ImportBinding::Vacuous),
            None => Err(unresolved()),
        },
        _ => Err(Error::from(LinkError::UnsupportedRegistration {
            import: import.name.clone(),
            reason: "plain-named imports of types, components, or values",
        })),
    }
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
            reason: "plain-named imports resolve through the root namespace",
        }),
    }
}

fn check_items<T: 'static>(
    import: &ComponentImport,
    registration: &InstanceRegistration<T>,
    position: &ItemPosition<'_>,
) -> Result<()> {
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
                check_function_item(position, &item.name, declared, registration)?;
            }
            ExternType::Resource(_) | ExternType::ResourceEquals(_) => {
                check_resource_item(&item.name, registration, &import.name)?;
            }
            ExternType::Module(declared) => {
                check_module_item(&import.name, &item.name, declared, registration)?;
            }
            // Type, Instance, Component, Value: the WIT shapes typical
            // interfaces use are functions plus opaque types; anything
            // else either has no runtime presence or is out of the
            // synchronous baseline.
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

/// A module-typed import, or a module item of an instance import,
/// needs a registered module under its name that satisfies the
/// declared module type.
fn check_module_item<T: 'static>(
    import_name: &ExternalName,
    item_name: &str,
    declared: &ModuleType,
    registration: &InstanceRegistration<T>,
) -> Result<()> {
    let module = registration.module(item_name).ok_or_else(|| {
        Error::from(LinkError::UnresolvedImport {
            import: import_name.clone(),
        })
    })?;
    module_satisfies(declared, module).map_err(|reason| {
        Error::from(LinkError::IncompatibleModule {
            import: import_name.clone(),
            item: item_name.to_owned(),
            reason,
        })
    })
}

fn check_function_item<T: 'static>(
    position: &ItemPosition<'_>,
    item_name: &str,
    declared: &FunctionType,
    registration: &InstanceRegistration<T>,
) -> Result<()> {
    let host = registration.func(item_name).ok_or_else(|| {
        Error::from(LinkError::UnresolvedImport {
            import: position.import_name(),
        })
    })?;
    if !function_types_compatible(&host.signature, declared) {
        return Err(Error::from(TypeMismatch {
            position: position.for_item(item_name),
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

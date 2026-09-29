//! Identifier resolution: matching a component's imports against
//! the linker's registered [`LinkerInstance`]s.
//!
//! The matching rules are Wasmtime's. The WIT specification fixes
//! the grammar of a versioned name and leaves to the host the
//! question of which registered version answers an import, so the
//! polyfill follows the host it stands in for: `NameMap::get` and
//! `alternate_lookup_key` in Wasmtime's
//! `crates/environ/src/component/names.rs`. They are narrower than
//! cargo-style caret matching:
//!
//! - An *exact* name answers first. With `a:b/c@0.2.0` and
//!   `a:b/c@0.2.7` both registered, an import of `a:b/c@0.2.0`
//!   resolves to the `0.2.0` registration, not to the newer one.
//! - Failing an exact name, a version sits on a *compatibility
//!   track*, and an import is answered by the highest-versioned
//!   registration sharing its track. A version at or above `1.0.0`
//!   tracks by its major segment: `1.4.0` and `1.7.2` share a
//!   track, `1.4.0` and `2.0.0` do not. A version below `1.0.0`
//!   whose minor segment is non-zero tracks by that minor: `0.2.0`
//!   and `0.2.7` share a track, `0.2.0` and `0.3.0` do not.
//! - A version carrying a prerelease tag sits on no track, so
//!   `0.2.0-rc.1` is answered by a registration of `0.2.0-rc.1` and
//!   by nothing else — not by `0.2.0`, and not by another
//!   prerelease of the same release.
//! - A `0.0.x` version likewise sits on no track: nothing is
//!   compatible with a patch-only version, so `0.0.1` and `0.0.2`
//!   are unrelated names.
//! - Build metadata plays no part in a track, and is compared only
//!   when an exact name is.
//! - An import without a version matches a registration without a
//!   version exactly. A versioned import does not match an
//!   unversioned registration, and vice versa.
//!
//! Identifier matching decides *which* registered linker instance
//! satisfies an import, and it only comes into play for the imports
//! that want one. An import's *type*, not the shape of its name,
//! chooses the namespace it resolves through:
//!
//! - An instance-typed import named by an interface identifier
//!   resolves against the registered linker instances, by the rules
//!   above.
//! - Every other import resolves through the root namespace, under
//!   the import's own name written out in full. That covers the
//!   plain-named function, resource, and module imports, and it
//!   covers the same three sorts written under an interface name:
//!   `(import "pkg:ns/iface@0.1.0" (func async ...))` looks for a
//!   host function registered on the root view under the name
//!   `pkg:ns/iface@0.1.0`.
//!
//! The version rules above govern both namespaces. A root name is a
//! name like any other, so a root registration under
//! `pkg:ns/iface@0.1.0` answers a function import of
//! `pkg:ns/iface@0.1.3`, and a root registration under the import's
//! exact name beats a merely compatible one. That is Wasmtime's
//! arrangement too: its root is the same `NameMap` its interfaces
//! are, and the version fallback lives in `get` rather than in any
//! one caller. A plain root name carries no `@`, sits on no track,
//! and so is only ever matched exactly.
//!
//! An interface name on a function import is a valid shape rather
//! than a malformed one. The Component Model constrains an import's
//! sort from its name only for the annotated plain names —
//! `[constructor]`, `[method]`, `[static]` — and for an
//! `implements`-annotated import, which must be instance-typed;
//! nothing requires an interface name to carry an instance. Such an
//! import asks for a single host item rather than an interface's
//! worth of them, and a single host item is what the root namespace
//! holds. Wasmtime reads every top-level import the same way,
//! looking its name up in the root of the linker whatever the
//! import's sort.
//!
//! Beyond identifier matching, the resolver checks every item an
//! import asks for against the registration that satisfies it. A
//! function item needs a registered host function whose declared
//! signature is structurally equal to the item's, and an import that
//! is itself a function is held to the same comparison against its
//! root entry. A missing item surfaces as
//! [`LinkError::UnresolvedImport`]; an item the host registered
//! under another kind surfaces as [`LinkError::KindMismatch`];
//! signature mismatches surface as [`Error::TypeMismatch`].
//!
//! It also holds each function registration's *form* to the import's
//! `async` effect, wherever that function sits: an async-typed
//! import wants a concurrent registration and a sync-typed import
//! wants a synchronous one, and either pairing the wrong way round
//! fails to link. The rule is Wasmtime's rather than the
//! reference's; the two causes it raises say why the polyfill keeps
//! it.
//!
//! [`LinkerInstance`]: super::LinkerInstance

use std::collections::HashMap;

use semver::Version;

use crate::component::{
    Component, ComponentImport, ExternType, ExternalName, FunctionType, InstanceItem, InstanceType,
    ModuleType,
};
use crate::error::{Error, LinkError, Result, TypeMismatch, TypeMismatchPosition, TypeRendering};
use crate::identifier::InterfaceIdentifier;
use crate::internal::LinkerInternal;
use crate::resource::ResourceTypeId;
use crate::types::{ResourceType, ValueType};

use super::host_func_kind::HostFuncKind;
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
    /// The import was satisfied through the root namespace: the root
    /// entry itself for a function, resource, or module import, or
    /// the nested entry under `name` for a plain-named instance
    /// import. A function, resource, or module import carries this
    /// binding whether its name is plain or an interface identifier.
    Root {
        /// The root key chosen for this import. That is the
        /// import's own name written out in full, except where a
        /// versioned name resolved through the compatibility track
        /// at the top of this module, in which case it is the
        /// registered name that answered.
        name: String,
    },
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
/// and root-namespace misses surface as [`Error::Link`]; signature
/// mismatches between a host registration and the component's
/// declared item surface as [`Error::TypeMismatch`].
pub fn resolve_imports<T: 'static>(
    component: &Component,
    linker: &Linker<T>,
) -> Result<Resolution> {
    let registered: Vec<&InterfaceIdentifier> = linker.registered_keys().collect();
    let mut bindings = Vec::with_capacity(component.imports.len());
    for import in component.imports.iter() {
        let binding = match (&import.name, &import.ty) {
            (ExternalName::Interface(id), ExternType::Instance(instance)) => {
                let binding =
                    resolve_one(import, id, instance, &registered).map_err(Error::from)?;
                if let ImportBinding::Resolved { chosen } = &binding {
                    // Item-level type check: every function item the
                    // import declares must have a registered host function
                    // whose signature matches structurally.
                    let registration = linker.registration_for(chosen).ok_or_else(|| {
                        Error::from(LinkError::UnresolvedImport {
                            import: import.name.clone(),
                            item: None,
                        })
                    })?;
                    check_instance_items(
                        &import.name,
                        &instance.items,
                        registration,
                        &ItemPosition::interface(chosen),
                    )?;
                }
                binding
            }
            // An interface name sits on an import that is not an
            // instance the same way a plain name does: it names one
            // host item, and one host item lives in the root
            // namespace, under the name as written or under a
            // version compatible with it.
            (ExternalName::Interface(id), _) => resolve_root(import, &id.to_string(), linker)?,
            (ExternalName::Plain(name), _) => resolve_root(import, name, linker)?,
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
                (registration, ItemPosition::interface(chosen))
            }
            (ImportBinding::Root { name }, _) => {
                let Some(registration) = linker.root_registration().instance(name) else {
                    continue;
                };
                (registration, ItemPosition::root(&import.name, name))
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
                    seen.insert(index, (host.type_id(), position.for_item(&item.name)));
                }
                Some((first, _)) if *first == host.type_id() => {}
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
/// belongs to: the import, and the nested instances walked from it.
struct ItemPosition<'a> {
    root: PositionRoot<'a>,
    /// The names of the nested instance items walked from the
    /// import, outermost first.
    nested: Vec<String>,
}

enum PositionRoot<'a> {
    /// An item of an interface-named instance import.
    Interface(&'a InterfaceIdentifier),
    /// An item resolved through the root namespace: the import
    /// itself, or an item of a root-namespace instance. `name` is
    /// the key the root registration holds it under, which is the
    /// import's own name as written; `import` is that name in the
    /// form a diagnostic carries it.
    Root {
        import: &'a ExternalName,
        name: &'a str,
    },
}

impl<'a> ItemPosition<'a> {
    fn interface(interface: &'a InterfaceIdentifier) -> Self {
        Self {
            root: PositionRoot::Interface(interface),
            nested: Vec::new(),
        }
    }

    fn root(import: &'a ExternalName, name: &'a str) -> Self {
        Self {
            root: PositionRoot::Root { import, name },
            nested: Vec::new(),
        }
    }

    /// The position of the items inside the nested instance item
    /// `name`.
    fn inside(&self, name: &str) -> Self {
        let mut nested = self.nested.clone();
        nested.push(name.to_owned());
        Self {
            root: match self.root {
                PositionRoot::Interface(interface) => PositionRoot::Interface(interface),
                PositionRoot::Root { import, name } => PositionRoot::Root { import, name },
            },
            nested,
        }
    }

    /// The item's name with the nested instances it sits inside,
    /// joined with dots.
    fn qualified(&self, item: &str) -> String {
        let mut out = String::new();
        for segment in &self.nested {
            out.push_str(segment);
            out.push('.');
        }
        out.push_str(item);
        out
    }

    fn for_item(&self, item: &str) -> TypeMismatchPosition {
        match self.root {
            PositionRoot::Interface(interface) => TypeMismatchPosition::HostFunctionRegistration {
                interface: interface.clone(),
                item: self.qualified(item),
            },
            PositionRoot::Root { name, .. } => {
                TypeMismatchPosition::HostFunctionRegistrationPlain {
                    name: if self.nested.is_empty() && name == item {
                        item.to_owned()
                    } else {
                        format!("{name}.{}", self.qualified(item))
                    },
                }
            }
        }
    }

    fn import_name(&self) -> ExternalName {
        match self.root {
            PositionRoot::Interface(chosen) => ExternalName::Interface(chosen.clone()),
            PositionRoot::Root { import, .. } => import.clone(),
        }
    }

    /// The item name a kind-mismatch diagnostic carries: `None` when
    /// the item is the root-namespace import itself.
    fn item(&self, item: &str) -> Option<String> {
        match self.root {
            PositionRoot::Root { name, .. } if self.nested.is_empty() && name == item => None,
            _ => Some(self.qualified(item)),
        }
    }
}

/// Whether an instance type needs nothing from the host: every item
/// is a type, or an instance that itself needs nothing. Wasmtime
/// links such an import with no definition at all.
fn instance_is_vacuous(instance: &InstanceType) -> bool {
    instance.items.iter().all(|item| match &item.ty {
        ExternType::Instance(inner) => instance_is_vacuous(inner),
        ExternType::Value(_) => true,
        _ => false,
    })
}

/// Resolve an import through the root namespace under `name`, the
/// import's own name written out in full. A function, resource, or
/// module import is an item of the root entry under that name; a
/// plain-named instance import is the nested entry under it, whose
/// items are checked as an interface's would be.
///
/// Every plain-named import comes here, and so does every
/// interface-named import that is not an instance: the name says
/// where a host would look the item up and nothing about the
/// import's sort.
///
/// The root holds versioned names as well as plain ones, so the
/// lookup goes through [`root_key`] rather than straight to `name`:
/// a registration under a version on the import's compatibility
/// track answers when nothing sits under the name as written.
fn resolve_root<T: 'static>(
    import: &ComponentImport,
    name: &str,
    linker: &Linker<T>,
) -> Result<ImportBinding> {
    let root = linker.root_registration();
    let name = &root_key(root, name);
    let unresolved = || {
        Error::from(LinkError::UnresolvedImport {
            import: import.name.clone(),
            item: None,
        })
    };
    let bound = || ImportBinding::Root { name: name.clone() };
    match &import.ty {
        ExternType::Function(declared) => {
            check_function_item(
                &ItemPosition::root(&import.name, name),
                name,
                declared,
                root,
            )?;
            Ok(bound())
        }
        ExternType::Resource(_) | ExternType::ResourceEquals(_) => {
            check_resource_item(name, root, &import.name, None)?;
            Ok(bound())
        }
        ExternType::Module(declared) => {
            check_module_item(&import.name, name, declared, root, None)?;
            Ok(bound())
        }
        ExternType::Instance(instance) => match root.instance(name) {
            Some(registration) => {
                check_instance_items(
                    &import.name,
                    &instance.items,
                    registration,
                    &ItemPosition::root(&import.name, name),
                )?;
                Ok(bound())
            }
            None => {
                check_kind(root, name, "instance", &import.name, None)?;
                if instance_is_vacuous(instance) {
                    Ok(ImportBinding::Vacuous)
                } else {
                    Err(unresolved())
                }
            }
        },
        _ => Err(Error::from(LinkError::UnsupportedRegistration {
            import: import.name.clone(),
            reason: "imports of types, components, or values",
        })),
    }
}

/// Choose the registered linker instance that satisfies an
/// interface-named instance import, by the semver rules at the top
/// of this module. Only that one shape reaches here: every other
/// import, whatever its name looks like, resolves through the root
/// namespace instead, so the caller hands the import's identifier
/// and instance type in already destructured.
///
/// A miss is not always a failure. An instance that needs nothing
/// from the host binds as [`ImportBinding::Vacuous`], which is how
/// Wasmtime links such an import with no definition at all. Any
/// other instance's miss is an unresolved import, or an
/// incompatible version when the linker holds a candidate outside
/// the import's compatibility range.
#[allow(clippy::result_large_err)]
fn resolve_one(
    import: &ComponentImport,
    id: &InterfaceIdentifier,
    instance: &InstanceType,
    registered: &[&InterfaceIdentifier],
) -> core::result::Result<ImportBinding, LinkError> {
    match find_match(id, registered.iter().copied()) {
        Some(chosen) => Ok(ImportBinding::Resolved {
            chosen: chosen.clone(),
        }),
        None if instance_is_vacuous(instance) => Ok(ImportBinding::Vacuous),
        None => Err(unresolved_or_incompatible(import, id, registered)),
    }
}

/// Check the items of one instance type against the registration
/// that satisfies it. A nested instance item is checked against the
/// nested registration under its name, and needs none when it is
/// vacuous.
fn check_instance_items<T: 'static>(
    import_name: &ExternalName,
    items: &[InstanceItem],
    registration: &InstanceRegistration<T>,
    position: &ItemPosition<'_>,
) -> Result<()> {
    for item in items.iter() {
        match &item.ty {
            ExternType::Function(declared) => {
                check_function_item(position, &item.name, declared, registration)?;
            }
            ExternType::Resource(resource) | ExternType::ResourceEquals(resource) => {
                // An item declared equal to a resource the instance
                // names elsewhere carries that resource's first name
                // as its label. It needs no registration of its own,
                // as in Wasmtime: the registration under the first
                // name serves both.
                if registration.resource(&item.name).is_none() && resource.label() != item.name {
                    check_kind(
                        registration,
                        &item.name,
                        "resource",
                        import_name,
                        Some(&item.name),
                    )?;
                    check_resource_item(
                        resource.label(),
                        registration,
                        import_name,
                        Some(&item.name),
                    )?;
                } else {
                    check_resource_item(&item.name, registration, import_name, Some(&item.name))?;
                }
            }
            ExternType::Module(declared) => {
                check_module_item(
                    import_name,
                    &item.name,
                    declared,
                    registration,
                    Some(&item.name),
                )?;
            }
            ExternType::Instance(inner) => match registration.instance(&item.name) {
                Some(nested) => {
                    check_instance_items(
                        import_name,
                        &inner.items,
                        nested,
                        &position.inside(&item.name),
                    )?;
                }
                None => {
                    check_kind(
                        registration,
                        &item.name,
                        "instance",
                        import_name,
                        Some(&item.name),
                    )?;
                    if !instance_is_vacuous(inner) {
                        return Err(Error::from(LinkError::UnresolvedImport {
                            import: import_name.clone(),
                            item: Some(item.name.clone()),
                        }));
                    }
                }
            },
            // Type, Component, Value: the WIT shapes typical interfaces
            // use are functions plus opaque types; anything else either
            // has no runtime presence or is out of the synchronous
            // baseline.
            _ => {}
        }
    }
    Ok(())
}

fn check_resource_item<T: 'static>(
    item_name: &str,
    registration: &InstanceRegistration<T>,
    import_name: &ExternalName,
    item: Option<&str>,
) -> Result<()> {
    if registration.resource(item_name).is_some() {
        return Ok(());
    }
    check_kind(registration, item_name, "resource", import_name, item)?;
    Err(Error::from(LinkError::UnresolvedImport {
        import: import_name.clone(),
        item: item.map(str::to_owned),
    }))
}

/// Reject a registration of another kind under `name`: the host put
/// a function where the component imports an instance, say. A name
/// with nothing registered under it passes, and the caller reports
/// the missing item.
fn check_kind<T: 'static>(
    registration: &InstanceRegistration<T>,
    name: &str,
    expected: &'static str,
    import_name: &ExternalName,
    item: Option<&str>,
) -> Result<()> {
    match registration.kind_of(name) {
        Some(found) if found != expected => Err(Error::from(LinkError::KindMismatch {
            import: import_name.clone(),
            item: item.map(str::to_owned),
            expected,
            found,
        })),
        _ => Ok(()),
    }
}

/// A module-typed import, or a module item of an instance import,
/// needs a registered module under its name that satisfies the
/// declared module type.
fn check_module_item<T: 'static>(
    import_name: &ExternalName,
    item_name: &str,
    declared: &ModuleType,
    registration: &InstanceRegistration<T>,
    item: Option<&str>,
) -> Result<()> {
    let Some(module) = registration.module(item_name) else {
        check_kind(registration, item_name, "module", import_name, item)?;
        return Err(Error::from(LinkError::UnresolvedImport {
            import: import_name.clone(),
            item: item.map(str::to_owned),
        }));
    };
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
    let Some(host) = registration.func(item_name) else {
        let import_name = position.import_name();
        let item = position.item(item_name);
        check_kind(
            registration,
            item_name,
            "func",
            &import_name,
            item.as_deref(),
        )?;
        return Err(Error::from(LinkError::UnresolvedImport {
            import: import_name,
            item,
        }));
    };
    check_registration_kind(position, item_name, declared, &host.kind)?;
    if !function_types_compatible(&host.signature, declared) {
        return Err(Error::from(TypeMismatch {
            position: position.for_item(item_name),
            expected: TypeRendering::Function(declared.clone()),
            actual: TypeRendering::Function(host.signature.clone()),
        }));
    }
    Ok(())
}

/// Hold the registration's form to the import's `async` effect: an
/// async-typed import wants a concurrent registration, and a
/// sync-typed one wants a synchronous registration.
///
/// The check reads the registration's [`HostFuncKind`] and never its
/// declared signature's own `async_`. The kind is the only reliable
/// record of the form: a typed registration derives its signature
/// from the closure's argument tuple and return type, which say
/// nothing about the `async` effect, so `func_wrap_concurrent`
/// derives a signature whose `async_` is false; and an untyped
/// registration takes whatever `FunctionType` the host names, which
/// need not agree with the form it registered under. The kind is
/// fixed by which of the four entries the host called.
///
/// This runs before the signature comparison, as Wasmtime's
/// `typecheck_async` does, so a host that reached for the wrong entry
/// reads that rather than a parameter-by-parameter mismatch.
///
/// [`HostFuncKind`]: super::HostFuncKind
fn check_registration_kind<T: 'static>(
    position: &ItemPosition<'_>,
    item_name: &str,
    declared: &FunctionType,
    kind: &HostFuncKind<T>,
) -> Result<()> {
    let concurrent = matches!(kind, HostFuncKind::Concurrent(_));
    match (declared.async_, concurrent) {
        (true, false) => Err(Error::from(
            LinkError::SynchronousRegistrationForAsyncImport {
                import: position.import_name(),
                item: position.item(item_name),
            },
        )),
        (false, true) => Err(Error::from(
            LinkError::ConcurrentRegistrationForSyncImport {
                import: position.import_name(),
                item: position.item(item_name),
            },
        )),
        _ => Ok(()),
    }
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
            item: None,
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
    let same_shape: Vec<&'a InterfaceIdentifier> = candidates
        .filter(|candidate| shape_matches(import, candidate))
        .collect();
    let wanted = import.package().version();
    // An exact version answers before any compatible one, so that a
    // host registering several versions of one interface hands each
    // import the version it asked for.
    if let Some(exact) = same_shape
        .iter()
        .copied()
        .find(|candidate| candidate.package().version() == wanted)
    {
        return Some(exact);
    }
    let mut best: Option<&InterfaceIdentifier> = None;
    for candidate in same_shape {
        if !versions_compatible(wanted, candidate.package().version()) {
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

/// The root-namespace key that answers a lookup of `name`: `name`
/// itself whenever the root holds anything at all under it, and
/// otherwise the highest-versioned registered key on `name`'s
/// compatibility track. That ordering is Wasmtime's `NameMap::get`,
/// which consults its exact definitions before its table of
/// alternate names.
///
/// When nothing answers, the result is `name` as written, so that
/// the caller's kind check and its unresolved-import diagnostic
/// name what the component asked for.
fn root_key<T: 'static>(root: &InstanceRegistration<T>, name: &str) -> String {
    if root.kind_of(name).is_some() {
        return name.to_owned();
    }
    let Some((stem, wanted)) = split_version(name) else {
        return name.to_owned();
    };
    let Some(track) = compatibility_track(&wanted) else {
        return name.to_owned();
    };
    let mut best: Option<(&str, Version)> = None;
    for candidate in root.names() {
        let Some((candidate_stem, version)) = split_version(candidate) else {
            continue;
        };
        if candidate_stem != stem || compatibility_track(&version) != Some(track) {
            continue;
        }
        let better = match &best {
            None => true,
            Some((_, current)) => version > *current,
        };
        if better {
            best = Some((candidate, version));
        }
    }
    match best {
        Some((chosen, _)) => chosen.to_owned(),
        None => name.to_owned(),
    }
}

/// Split a name into the part ahead of its version and the version
/// itself: `pkg:ns/iface@0.1.0` into `pkg:ns/iface` and `0.1.0`. A
/// name with no `@`, or one whose tail is not a semver version,
/// carries no version and so answers its own name alone.
fn split_version(name: &str) -> Option<(&str, Version)> {
    let at = name.find('@')?;
    let version = name[at + 1..].parse().ok()?;
    Some((&name[..at], version))
}

/// The compatibility track a version sits on, or `None` when it
/// sits on none and so answers its own name alone. Mirrors the
/// `alternate_lookup_key` half of Wasmtime's rule, which chops a
/// registered name down to the segment a compatible import would
/// share with it: `1.7.2` to `1`, `0.2.7` to `0.2`, and `0.2.0-rc.1`
/// and `0.0.1` to nothing.
fn compatibility_track(version: &Version) -> Option<(u64, u64)> {
    if !version.pre.is_empty() {
        // A prerelease is on a track of its own making, which is to
        // say on none: nothing else is a release of it.
        None
    } else if version.major != 0 {
        Some((version.major, 0))
    } else if version.minor != 0 {
        Some((0, version.minor))
    } else {
        // The patch segment is the first non-zero one, and a
        // patch-only version promises compatibility with nothing.
        None
    }
}

/// True when `candidate` answers `import`: the same version, or a
/// version sharing `import`'s compatibility track.
pub fn versions_compatible(import: Option<&Version>, candidate: Option<&Version>) -> bool {
    match (import, candidate) {
        (None, None) => true,
        (None, Some(_)) | (Some(_), None) => false,
        (Some(imp), Some(cand)) => {
            if imp == cand {
                return true;
            }
            match (compatibility_track(imp), compatibility_track(cand)) {
                (Some(imp), Some(cand)) => imp == cand,
                _ => false,
            }
        }
    }
}

/// Prefer `candidate` over `current` when its version is strictly
/// greater. Both must already have been confirmed compatible with
/// the same import, and neither is the import's exact version:
/// [`find_match`] answers an exact version before it gets here.
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

    #[wcmp_macros::test]
    fn it_falls_into_the_minor_compatibility_range_for_pre_one_versions() {
        assert!(versions_compatible(
            Some(&version("0.2.0")),
            Some(&version("0.2.7"))
        ));
    }

    #[wcmp_macros::test]
    fn it_excludes_a_different_minor_below_one() {
        assert!(!versions_compatible(
            Some(&version("0.2.0")),
            Some(&version("0.3.0"))
        ));
    }

    #[wcmp_macros::test]
    fn it_falls_into_the_major_compatibility_range_for_one_plus_versions() {
        assert!(versions_compatible(
            Some(&version("1.4.0")),
            Some(&version("1.7.2"))
        ));
    }

    #[wcmp_macros::test]
    fn it_excludes_a_different_major_at_or_above_one() {
        assert!(!versions_compatible(
            Some(&version("1.4.0")),
            Some(&version("2.0.0"))
        ));
    }

    #[wcmp_macros::test]
    fn it_excludes_pre_one_against_one_plus() {
        assert!(!versions_compatible(
            Some(&version("0.9.0")),
            Some(&version("1.0.0"))
        ));
    }

    #[wcmp_macros::test]
    fn it_treats_two_unversioned_as_compatible() {
        assert!(versions_compatible(None, None));
    }

    #[wcmp_macros::test]
    fn it_treats_unversioned_versus_versioned_as_no_match() {
        assert!(!versions_compatible(None, Some(&version("1.0.0"))));
        assert!(!versions_compatible(Some(&version("1.0.0")), None));
    }

    #[wcmp_macros::test]
    fn it_treats_a_prerelease_as_answering_only_its_own_name() {
        // Wasmtime's `alternate_lookup_key` puts a prerelease on no
        // compatibility track, so `0.2.0-rc.1` is neither answered
        // by the release it precedes nor by a sibling prerelease.
        assert!(versions_compatible(
            Some(&version("0.2.0-rc.1")),
            Some(&version("0.2.0-rc.1"))
        ));
        assert!(!versions_compatible(
            Some(&version("0.2.0-rc.1")),
            Some(&version("0.2.0"))
        ));
        assert!(!versions_compatible(
            Some(&version("0.2.0")),
            Some(&version("0.2.0-rc.1"))
        ));
        assert!(!versions_compatible(
            Some(&version("0.2.0-rc.1")),
            Some(&version("0.2.0-rc.2"))
        ));
    }

    #[wcmp_macros::test]
    fn it_treats_two_patch_only_versions_as_incompatible() {
        // Nothing is compatible with a `0.0.x` but the very same
        // version: the patch segment is the first non-zero one, so
        // there is no track to share.
        assert!(!versions_compatible(
            Some(&version("0.0.1")),
            Some(&version("0.0.2"))
        ));
        assert!(versions_compatible(
            Some(&version("0.0.1")),
            Some(&version("0.0.1"))
        ));
    }

    #[wcmp_macros::test]
    fn it_picks_highest_version_when_multiple_match() {
        let import = id("wasi:cli/run@0.2.0");
        let registered = [id("wasi:cli/run@0.2.5"), id("wasi:cli/run@0.2.7")];
        let chosen = find_match(&import, registered.iter()).cloned();
        assert_eq!(chosen, Some(id("wasi:cli/run@0.2.7")));
    }

    #[wcmp_macros::test]
    fn it_picks_the_exact_version_over_a_higher_compatible_one() {
        // A host that registers two versions of one interface hands
        // each import the version it asked for; only an import with
        // no registration of its own falls through to the track.
        let registered = [id("wasi:cli/run@0.2.0"), id("wasi:cli/run@0.2.7")];
        for (import, expected) in [
            ("wasi:cli/run@0.2.0", "wasi:cli/run@0.2.0"),
            ("wasi:cli/run@0.2.7", "wasi:cli/run@0.2.7"),
            ("wasi:cli/run@0.2.3", "wasi:cli/run@0.2.7"),
        ] {
            let chosen = find_match(&id(import), registered.iter()).cloned();
            assert_eq!(chosen, Some(id(expected)), "import of `{import}`");
        }
    }

    #[wcmp_macros::test]
    fn it_answers_a_root_lookup_from_a_compatible_registration() {
        // The root namespace matches a versioned name the way a
        // registered interface key is matched: `@0.1.0` answers an
        // import of `@0.1.3`, and an exact registration answers
        // ahead of it.
        let engine = crate::Engine::new().expect("engine construction succeeds");
        let mut linker: Linker<()> = Linker::new(&engine);
        let register = |linker: &mut Linker<()>, name: &str| {
            linker
                .root()
                .func_wrap(
                    name,
                    |_call: crate::linker::HostCall<'_, ()>, (): ()| Ok(()),
                )
                .expect("the registration");
        };

        register(&mut linker, "pdd-tests:host/answers@0.1.0");
        assert_eq!(
            root_key(linker.root_registration(), "pdd-tests:host/answers@0.1.3"),
            "pdd-tests:host/answers@0.1.0"
        );

        register(&mut linker, "pdd-tests:host/answers@0.1.3");
        assert_eq!(
            root_key(linker.root_registration(), "pdd-tests:host/answers@0.1.3"),
            "pdd-tests:host/answers@0.1.3"
        );

        // The highest registration on the track answers a version
        // that has none of its own.
        register(&mut linker, "pdd-tests:host/answers@0.1.9");
        assert_eq!(
            root_key(linker.root_registration(), "pdd-tests:host/answers@0.1.5"),
            "pdd-tests:host/answers@0.1.9"
        );

        // A version off the track, and a plain name, are left as
        // written so the caller reports what the component asked
        // for.
        assert_eq!(
            root_key(linker.root_registration(), "pdd-tests:host/answers@0.2.0"),
            "pdd-tests:host/answers@0.2.0"
        );
        assert_eq!(root_key(linker.root_registration(), "answers"), "answers");
    }

    #[wcmp_macros::test]
    fn it_skips_a_candidate_with_a_different_interface_name() {
        let import = id("wasi:cli/run@0.2.0");
        let registered = [id("wasi:cli/exit@0.2.0")];
        let chosen = find_match(&import, registered.iter()).cloned();
        assert_eq!(chosen, None);
    }

    #[wcmp_macros::test]
    fn it_skips_a_candidate_with_a_different_package() {
        let import = id("wasi:cli/run@0.2.0");
        let registered = [id("wasi:io/run@0.2.0")];
        let chosen = find_match(&import, registered.iter()).cloned();
        assert_eq!(chosen, None);
    }

    /// The root namespace answers for a host function, a resource,
    /// a module, and a plain-named instance. A type, a component,
    /// or a value import is none of those, and no registration a
    /// host can make would satisfy one, so the resolver refuses the
    /// import outright rather than reporting it missing.
    ///
    /// No component binary reaches this arm today. The translator
    /// drops a type import that is not a resource — it is type
    /// information for the component's own use and asks the host
    /// for nothing — and refuses a root-level component or value
    /// import before the polyfill sees it. The imports below are
    /// built by hand for that reason.
    #[wcmp_macros::test]
    fn it_refuses_an_import_whose_sort_the_root_namespace_cannot_hold() {
        let engine = crate::Engine::new().expect("engine construction succeeds");
        let linker: Linker<()> = Linker::new(&engine);
        let name = "pdd-tests:host/point@0.1.0";
        for ty in [
            ExternType::Component,
            ExternType::Value(ValueType::Primitive(crate::types::PrimitiveType::U32)),
        ] {
            let import = ComponentImport {
                name: ExternalName::Interface(id(name)),
                ty,
            };
            match resolve_root(&import, name, &linker) {
                Err(Error::Link(inner)) => match *inner {
                    LinkError::UnsupportedRegistration { import, reason } => {
                        assert_eq!(import, ExternalName::Interface(id(name)));
                        assert_eq!(reason, "imports of types, components, or values");
                    }
                    other => panic!("expected an unsupported registration, got {other:?}"),
                },
                other => panic!("expected a link error, got {other:?}"),
            }
        }
    }
}

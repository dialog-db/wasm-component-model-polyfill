// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The identity of a resource type.
//!
//! A resource type is told from every other by its identity alone,
//! as in Wasmtime (`crates/wasmtime/src/runtime/component/resources/ty.rs`
//! at `v49.0.0-rc.1`). Two resource types that a component names the
//! same way are different types when their identities differ, and the
//! identity carries no name: an error names a resource by the name the
//! component declares where the error is.

use core::any::TypeId;
use core::hash::{Hash, Hasher};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use crate::internal::ResourceTypeInternal;

/// The identity of a resource type.
///
/// A host resource type has the identity the host gives it:
/// [`ResourceType::host`] keys it on a Rust type, and
/// [`ResourceType::host_dynamic`] on a `u32`. A resource type that a
/// component defines takes a fresh identity at every instantiation, so
/// two instances of one component define two resource types. Before
/// instantiation, the reflection of a component names each of its
/// resource types by the component and the resource's position in it.
///
/// Equality compares identities and nothing else:
///
/// ```rust
/// use wcmp::ResourceType;
///
/// struct File;
/// struct Socket;
///
/// assert_eq!(ResourceType::host::<File>(), ResourceType::host::<File>());
/// assert_ne!(ResourceType::host::<File>(), ResourceType::host::<Socket>());
/// assert_ne!(ResourceType::host_dynamic(1), ResourceType::host_dynamic(2));
/// assert_ne!(ResourceType::host::<u32>(), ResourceType::host_dynamic(0));
/// ```
#[derive(Clone, Copy)]
pub struct ResourceType {
    /// Which kind of identity this is, in the low three bits, and for
    /// a type of a component, the component's number above them.
    kind: u32,
    /// The first number of the identity: the Rust type's number, the
    /// dynamic payload, the low half of a guest's number, or the
    /// resource's index in its component.
    first: u32,
    /// The second number: the high half of a guest's number, or, for a
    /// type of a component that is not instantiated, the resource
    /// table that the position of the type reads a handle from. The
    /// table is where a handle is, not what the handle is, so it takes
    /// no part in equality.
    second: u32,
}

// The identity is three `u32`s, so that a handle, and with it a
// [`Val`](crate::Val), stays small: a lifted list holds one `Val` per
// element, and the copy budget charges its size.

/// A host resource type that a Rust type stands for, by the number
/// [`host_number`] gives the Rust type.
const HOST: u32 = 0;
/// A host resource type that the host keys on a `u32`.
const HOST_DYNAMIC: u32 = 1;
/// A resource type that one instantiation of a component defines, by a
/// number unique in the process.
const GUEST: u32 = 2;
/// A resource type of a component that is not instantiated: the
/// component, and the resource's index among its resources. An
/// instantiation gives each one the identity the linker or the
/// instance supplies.
const UNINSTANTIATED: u32 = 3;
/// A resource type that a component's type names but that no instance
/// holds, such as a resource of an imported component type. It equals
/// itself and nothing else.
const ABSTRACT: u32 = 4;
/// The bits of `kind` that say which kind of identity it is.
const KIND_BITS: u32 = 3;

impl ResourceType {
    /// An identity of the kind `kind`, of the component `component`
    /// when it is a type of one.
    fn of(kind: u32, component: u32, first: u32, second: u32) -> Self {
        Self {
            kind: kind | (component << KIND_BITS),
            first,
            second,
        }
    }

    /// Which kind of identity this is.
    fn kind(self) -> u32 {
        self.kind & ((1 << KIND_BITS) - 1)
    }

    /// The number of the component, for a type of one.
    fn component(self) -> u32 {
        self.kind >> KIND_BITS
    }

    /// The parts that make the identity, without the position of an
    /// uninstantiated type.
    fn identity(self) -> (u32, u32, u32) {
        let second = if self.kind() == UNINSTANTIATED {
            0
        } else {
            self.second
        };
        (self.kind, self.first, second)
    }
}

/// The Rust types that stand for host resource types, by the number
/// each was given, with their names: the number is what an identity
/// holds, and the name is what its rendering shows.
struct HostTypes {
    numbers: HashMap<TypeId, u32>,
    names: Vec<&'static str>,
}

/// The process's [`HostTypes`].
fn host_types() -> &'static RwLock<HostTypes> {
    static TYPES: OnceLock<RwLock<HostTypes>> = OnceLock::new();
    TYPES.get_or_init(|| {
        RwLock::new(HostTypes {
            numbers: HashMap::new(),
            names: Vec::new(),
        })
    })
}

/// The number the Rust type `T` stands for as a host resource type:
/// the same for every call with the same `T`, and different for every
/// other Rust type.
fn host_number<T: 'static>() -> u32 {
    let id = TypeId::of::<T>();
    // A lock that a panic poisoned still holds a consistent map: an
    // insert is the only write, and it completes or does not happen.
    let read = host_types()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(number) = read.numbers.get(&id) {
        return *number;
    }
    drop(read);
    let mut write = host_types()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let next = write.names.len() as u32;
    let number = *write.numbers.entry(id).or_insert(next);
    if number == next {
        write.names.push(core::any::type_name::<T>());
    }
    number
}

impl ResourceType {
    /// The identity of the host resource type that the Rust type `T`
    /// stands for. `T` is a marker: the resource need not be stored as
    /// a `T`. Each call with the same `T` gives the same identity.
    pub fn host<T: 'static>() -> Self {
        Self::of(HOST, 0, host_number::<T>(), 0)
    }

    /// The identity of a host resource type that the host keys on
    /// `payload`, for a host that makes resource types at run time.
    /// It differs from every identity [`ResourceType::host`] gives.
    pub fn host_dynamic(payload: u32) -> Self {
        Self::of(HOST_DYNAMIC, 0, payload, 0)
    }
}

/// An identity renders as where it comes from: the Rust type of a host
/// resource type, the payload of a dynamic one, and a number for the
/// others, which a component declares.
impl core::fmt::Debug for ResourceType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind() {
            HOST => {
                let types = host_types()
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let name = types.names.get(self.first as usize).copied().unwrap_or("?");
                write!(f, "ResourceType::host::<{name}>()")
            }
            HOST_DYNAMIC => write!(f, "ResourceType::host_dynamic({})", self.first),
            GUEST => write!(
                f,
                "ResourceType::guest({})",
                u64::from(self.first) | (u64::from(self.second) << 32)
            ),
            UNINSTANTIATED => write!(
                f,
                "ResourceType::component({}, resource {})",
                self.component(),
                self.first
            ),
            _ => write!(
                f,
                "ResourceType::component({}, abstract {})",
                self.component(),
                self.first
            ),
        }
    }
}

impl PartialEq for ResourceType {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity()
    }
}

impl Eq for ResourceType {}

impl Hash for ResourceType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.identity().hash(state);
    }
}

impl ResourceTypeInternal for ResourceType {
    fn fresh_guest() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let number = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self::of(GUEST, 0, number as u32, (number >> 32) as u32)
    }

    fn uninstantiated(component: u32, resource: usize, table: usize) -> Self {
        Self::of(UNINSTANTIATED, component, resource as u32, table as u32)
    }

    fn abstract_(component: u32, index: usize) -> Self {
        Self::of(ABSTRACT, component, index as u32, 0)
    }

    fn resource_index(&self) -> Option<usize> {
        (self.kind() == UNINSTANTIATED).then_some(self.first as usize)
    }

    fn table_index(&self) -> Option<usize> {
        (self.kind() == UNINSTANTIATED).then_some(self.second as usize)
    }

    fn is_host<T: 'static>(&self) -> bool {
        self.kind() == HOST && self.first == host_number::<T>()
    }

    fn as_host_dynamic(&self) -> Option<u32> {
        (self.kind() == HOST_DYNAMIC).then_some(self.first)
    }

    fn is_guest(&self) -> bool {
        self.kind() == GUEST
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct First;
    struct Second;

    #[wcmp_macros::test]
    fn it_compares_host_resource_types_by_the_rust_type() {
        assert_eq!(ResourceType::host::<First>(), ResourceType::host::<First>());
        assert_ne!(
            ResourceType::host::<First>(),
            ResourceType::host::<Second>()
        );
    }

    #[wcmp_macros::test]
    fn it_compares_dynamic_resource_types_by_the_payload() {
        assert_eq!(ResourceType::host_dynamic(7), ResourceType::host_dynamic(7));
        assert_ne!(ResourceType::host_dynamic(1), ResourceType::host_dynamic(2));
        assert_ne!(ResourceType::host_dynamic(0), ResourceType::host::<u32>());
    }

    #[wcmp_macros::test]
    fn it_tells_where_an_identity_comes_from() {
        let host = ResourceType::host::<First>();
        assert!(host.is_host::<First>());
        assert!(!host.is_host::<Second>());
        assert!(!host.is_guest());
        assert_eq!(host.as_host_dynamic(), None);

        let dynamic = ResourceType::host_dynamic(3);
        assert_eq!(dynamic.as_host_dynamic(), Some(3));
        assert!(!dynamic.is_host::<First>());
        assert!(!dynamic.is_guest());

        let guest = ResourceType::fresh_guest();
        assert!(guest.is_guest());
        assert!(!guest.is_host::<First>());
        assert_eq!(guest.as_host_dynamic(), None);
    }

    #[wcmp_macros::test]
    fn it_keeps_an_identity_to_three_words_so_a_val_stays_32_bytes() {
        assert_eq!(core::mem::size_of::<ResourceType>(), 12);
        assert_eq!(core::mem::align_of::<ResourceType>(), 4);
    }

    #[wcmp_macros::test]
    fn it_renders_a_host_identity_by_its_rust_type() {
        assert!(format!("{:?}", ResourceType::host::<First>()).contains("First"));
    }

    #[wcmp_macros::test]
    fn it_gives_every_guest_resource_type_its_own_identity() {
        let first = ResourceType::fresh_guest();
        let copy = first;
        assert_eq!(copy, first, "a copy of an identity is the same identity");
        assert_ne!(first, ResourceType::fresh_guest());
    }

    #[wcmp_macros::test]
    fn it_compares_an_uninstantiated_resource_type_without_its_table() {
        let one = ResourceType::uninstantiated(1, 0, 3);
        let other_table = ResourceType::uninstantiated(1, 0, 5);
        assert_eq!(one, other_table);
        assert_ne!(one, ResourceType::uninstantiated(1, 1, 3));
        assert_ne!(one, ResourceType::uninstantiated(2, 0, 3));

        let mut set = std::collections::HashSet::new();
        set.insert(one);
        assert!(set.contains(&other_table));
    }
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the generator needs to know of each WIT type.

use std::collections::HashMap;
use std::ops::BitOrAssign;

use wit_parser::{Function, Resolve, Result_, Type, TypeDefKind, TypeId, WorldId, WorldItem};

use crate::names::resolve_type_definition_id;

/// What the generator needs to know of one WIT type, as Wasmtime's
/// `TypeInfo` records it (`crates/wit-bindgen/src/types.rs` at
/// `v50.0.0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypeInfo {
    /// The type is the error of a function's `result`, so its Rust type
    /// implements `Display` and `Error`.
    pub error: bool,
    /// The type holds a list or a string, so it is not `Copy`.
    pub has_list: bool,
    /// The type holds a resource handle, so it is neither `Copy` nor
    /// `Clone`.
    pub has_handle: bool,
    /// The type holds a stream or a future, so it is neither `Copy` nor
    /// `Clone`.
    pub has_future_or_stream: bool,
}

impl TypeInfo {
    /// Whether the Rust type can derive `Copy`.
    pub fn is_copy(self) -> bool {
        !self.has_list && !self.has_handle && !self.has_future_or_stream
    }

    /// Whether the Rust type can derive `Clone`.
    pub fn is_clone(self) -> bool {
        !self.has_handle && !self.has_future_or_stream
    }
}

impl BitOrAssign for TypeInfo {
    fn bitor_assign(&mut self, rhs: Self) {
        self.error |= rhs.error;
        self.has_list |= rhs.has_list;
        self.has_handle |= rhs.has_handle;
        self.has_future_or_stream |= rhs.has_future_or_stream;
    }
}

/// The [`TypeInfo`] of every type of a `Resolve`.
#[derive(Default)]
pub struct TypeInfos {
    infos: HashMap<TypeId, TypeInfo>,
}

impl TypeInfos {
    /// Analyze every type of `resolve`, and mark the error types of the
    /// functions `world` imports and exports.
    pub fn analyze(resolve: &Resolve, world: WorldId) -> Self {
        let mut infos = TypeInfos::default();
        for (id, _) in resolve.types.iter() {
            infos.type_id_info(resolve, id);
        }
        let world = &resolve.worlds[world];
        for (_, item) in world.imports.iter().chain(world.exports.iter()) {
            match item {
                WorldItem::Function(func) => infos.mark_error(resolve, func),
                WorldItem::Interface { id, .. } => {
                    for (_, func) in resolve.interfaces[*id].functions.iter() {
                        infos.mark_error(resolve, func);
                    }
                }
                WorldItem::Type { .. } => {}
            }
        }
        infos
    }

    /// The information of the type `id`.
    pub fn get(&self, id: TypeId) -> TypeInfo {
        self.infos.get(&id).copied().unwrap_or_default()
    }

    /// Mark the error type of `func`'s `result`, if it returns one.
    fn mark_error(&mut self, resolve: &Resolve, func: &Function) {
        let Some(Type::Id(id)) = func.result else {
            return;
        };
        if let TypeDefKind::Result(Result_ {
            err: Some(Type::Id(err)),
            ..
        }) = &resolve.types[id].kind
        {
            let err = resolve_type_definition_id(resolve, *err);
            self.infos.entry(err).or_default().error = true;
        }
    }

    /// The information of the type `id`, computed once.
    fn type_id_info(&mut self, resolve: &Resolve, id: TypeId) -> TypeInfo {
        if let Some(info) = self.infos.get(&id) {
            return *info;
        }
        let mut info = TypeInfo::default();
        match &resolve.types[id].kind {
            TypeDefKind::Record(record) => {
                for field in record.fields.iter() {
                    info |= self.type_info(resolve, &field.ty);
                }
            }
            TypeDefKind::Tuple(tuple) => {
                for ty in tuple.types.iter() {
                    info |= self.type_info(resolve, ty);
                }
            }
            TypeDefKind::Variant(variant) => {
                for case in variant.cases.iter() {
                    info |= self.optional_type_info(resolve, case.ty.as_ref());
                }
            }
            TypeDefKind::List(ty) => {
                info = self.type_info(resolve, ty);
                info.has_list = true;
            }
            TypeDefKind::Map(key, value) => {
                info = self.type_info(resolve, key);
                info |= self.type_info(resolve, value);
                info.has_list = true;
            }
            TypeDefKind::Type(ty)
            | TypeDefKind::Option(ty)
            | TypeDefKind::FixedLengthList(ty, _) => {
                info = self.type_info(resolve, ty);
            }
            TypeDefKind::Result(result) => {
                info = self.optional_type_info(resolve, result.ok.as_ref());
                info |= self.optional_type_info(resolve, result.err.as_ref());
            }
            TypeDefKind::Future(ty) | TypeDefKind::Stream(ty) => {
                info = self.optional_type_info(resolve, ty.as_ref());
                info.has_future_or_stream = true;
            }
            TypeDefKind::Handle(_) => info.has_handle = true,
            TypeDefKind::Flags(_)
            | TypeDefKind::Enum(_)
            | TypeDefKind::Resource
            | TypeDefKind::Unknown => {}
        }
        // An entry an error mark made first keeps its mark.
        let entry = self.infos.entry(id).or_default();
        *entry |= info;
        *entry
    }

    /// The information of `ty`.
    fn type_info(&mut self, resolve: &Resolve, ty: &Type) -> TypeInfo {
        match ty {
            Type::String => TypeInfo {
                has_list: true,
                ..TypeInfo::default()
            },
            Type::Id(id) => self.type_id_info(resolve, *id),
            _ => TypeInfo::default(),
        }
    }

    /// The information of `ty`, or none for no type.
    fn optional_type_info(&mut self, resolve: &Resolve, ty: Option<&Type>) -> TypeInfo {
        ty.map(|ty| self.type_info(resolve, ty)).unwrap_or_default()
    }
}

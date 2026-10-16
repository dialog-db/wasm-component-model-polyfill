// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A declared type with its resource types read as the identities an
//! instantiation gave them.

use super::fixed_length_list_type::FixedLengthListType;
use super::future_type::FutureType;
use super::list_type::ListType;
use super::map_type::MapType;
use super::option_type::OptionType;
use super::record_type::{RecordField, RecordType};
use super::resource_type::ResourceType;
use super::result_type::ResultType;
use super::stream_type::StreamType;
use super::tuple_type::TupleType;
use super::value_type::ValueType;
use super::variant_type::{VariantCase, VariantType};

/// `ty` with each resource type it names, at any depth, replaced by
/// the identity `resources` gives it.
pub fn resolve_resources(
    ty: &ValueType,
    resources: &dyn Fn(ResourceType) -> ResourceType,
) -> ValueType {
    let each = |ty: &ValueType| resolve_resources(ty, resources);
    let maybe = |ty: Option<&ValueType>| ty.map(each);
    match ty {
        ValueType::Own(resource) => ValueType::Own(resources(*resource)),
        ValueType::Borrow(resource) => ValueType::Borrow(resources(*resource)),
        ValueType::Record(record) => ValueType::Record(RecordType::new(
            record
                .fields()
                .iter()
                .map(|field| RecordField::new(field.name(), each(field.ty()))),
        )),
        ValueType::Variant(variant) => ValueType::Variant(VariantType::new(
            variant
                .cases()
                .iter()
                .map(|case| VariantCase::new(case.name(), maybe(case.payload()))),
        )),
        ValueType::List(list) => ValueType::List(ListType::new(each(list.element()))),
        ValueType::Map(map) => ValueType::Map(MapType::new(each(map.key()), each(map.value()))),
        ValueType::FixedLengthList(list) => ValueType::FixedLengthList(FixedLengthListType::new(
            each(list.element()),
            list.length(),
        )),
        ValueType::Option(option) => ValueType::Option(OptionType::new(each(option.payload()))),
        ValueType::Result(result) => {
            ValueType::Result(ResultType::new(maybe(result.ok()), maybe(result.err())))
        }
        ValueType::Tuple(tuple) => {
            ValueType::Tuple(TupleType::new(tuple.elements().iter().map(each)))
        }
        ValueType::Stream(stream) => ValueType::Stream(StreamType::new(maybe(stream.payload()))),
        ValueType::Future(future) => ValueType::Future(FutureType::new(maybe(future.payload()))),
        other => other.clone(),
    }
}

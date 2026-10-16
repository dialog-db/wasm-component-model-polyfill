// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The comparison of a type the host names with a type a component
//! declares, whose resource types have the identities an instantiation
//! or a link gives them.

use super::resource_type::ResourceType;
use super::value_type::ValueType;

/// Whether the type `host` names is the type `declared`, all the way
/// down, with each resource `declared` names read as the identity
/// `resources` gives it.
pub fn value_types_match(
    host: &ValueType,
    declared: &ValueType,
    resources: &dyn Fn(ResourceType) -> ResourceType,
) -> bool {
    let both = |host: Option<&ValueType>, declared: Option<&ValueType>| match (host, declared) {
        (Some(host), Some(declared)) => value_types_match(host, declared, resources),
        (None, None) => true,
        _ => false,
    };
    match (host, declared) {
        (ValueType::Own(host), ValueType::Own(declared))
        | (ValueType::Borrow(host), ValueType::Borrow(declared)) => *host == resources(*declared),
        (ValueType::Record(host), ValueType::Record(declared)) => {
            host.fields().len() == declared.fields().len()
                && host
                    .fields()
                    .iter()
                    .zip(declared.fields())
                    .all(|(host, declared)| {
                        host.name() == declared.name()
                            && value_types_match(host.ty(), declared.ty(), resources)
                    })
        }
        (ValueType::Variant(host), ValueType::Variant(declared)) => {
            host.cases().len() == declared.cases().len()
                && host
                    .cases()
                    .iter()
                    .zip(declared.cases())
                    .all(|(host, declared)| {
                        host.name() == declared.name() && both(host.payload(), declared.payload())
                    })
        }
        (ValueType::List(host), ValueType::List(declared)) => {
            value_types_match(host.element(), declared.element(), resources)
        }
        (ValueType::Map(host), ValueType::Map(declared)) => {
            value_types_match(host.key(), declared.key(), resources)
                && value_types_match(host.value(), declared.value(), resources)
        }
        (ValueType::FixedLengthList(host), ValueType::FixedLengthList(declared)) => {
            host.length() == declared.length()
                && value_types_match(host.element(), declared.element(), resources)
        }
        (ValueType::Option(host), ValueType::Option(declared)) => {
            value_types_match(host.payload(), declared.payload(), resources)
        }
        (ValueType::Result(host), ValueType::Result(declared)) => {
            both(host.ok(), declared.ok()) && both(host.err(), declared.err())
        }
        (ValueType::Tuple(host), ValueType::Tuple(declared)) => {
            host.elements().len() == declared.elements().len()
                && host
                    .elements()
                    .iter()
                    .zip(declared.elements())
                    .all(|(host, declared)| value_types_match(host, declared, resources))
        }
        (ValueType::Stream(host), ValueType::Stream(declared)) => {
            both(host.payload(), declared.payload())
        }
        (ValueType::Future(host), ValueType::Future(declared)) => {
            both(host.payload(), declared.payload())
        }
        (host, declared) => host == declared,
    }
}

/// Whether `carried`, the payload of a stream or a future as its record
/// keeps it, with its resources' identities, is `declared`, the payload
/// a component declares where the end crosses, whose resources
/// `resources` reads as the identities the instantiation gave them.
/// Two absent payloads match.
pub fn payloads_match(
    carried: Option<&ValueType>,
    declared: Option<&ValueType>,
    resources: &dyn Fn(ResourceType) -> ResourceType,
) -> bool {
    match (carried, declared) {
        (Some(carried), Some(declared)) => value_types_match(carried, declared, resources),
        (None, None) => true,
        _ => false,
    }
}

// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The type checks the derive macros call: a record, a variant, and an
//! enum, compared with the declared type in Wasmtime's order
//! (`crates/wasmtime/src/runtime/component/func/typed.rs`, `typecheck_record`,
//! `typecheck_variant`, and `typecheck_enum`, at `v49.0.0-rc.1`).

use crate::error::TypePathStep;
use crate::types::ValueType;

use super::type_check_error::TypeCheckError;
use super::type_checker::TypeChecker;

/// The type check of one field's or one payload's Rust type.
pub type TypeCheckFn =
    for<'a, 'b, 'c> fn(&'a ValueType, &'b TypeChecker<'c>) -> Result<(), TypeCheckError>;

/// Compare a derived record `R`, whose fields are `fields` (each its
/// WIT name and its type's check) in order, with `ty`: the count of
/// fields, then each field in order, its type and then its name. A
/// mismatch names the field by the name the component declares, and a
/// count that differs names the first field one side lacks.
pub fn typecheck_record<R: ?Sized>(
    ty: &ValueType,
    types: &TypeChecker<'_>,
    fields: &[(&str, TypeCheckFn)],
) -> Result<(), TypeCheckError> {
    let ValueType::Record(record) = ty else {
        return Err(TypeCheckError::mismatch::<R>(ty));
    };
    if record.fields().len() != fields.len() {
        // The first field one side has and the other does not is the
        // first wrong one.
        let first = record.fields().len().min(fields.len());
        let name = match (record.fields().get(first), fields.get(first)) {
            (Some(declared), _) => declared.name(),
            (None, Some((name, _))) => name,
            (None, None) => "",
        };
        return Err(TypeCheckError::count(record.fields().len(), fields.len())
            .at(TypePathStep::Field(name.to_owned())));
    }
    for (declared, (name, check)) in record.fields().iter().zip(fields) {
        let step = || TypePathStep::Field(declared.name().to_owned());
        check(declared.ty(), types).map_err(|error| error.at(step()))?;
        if declared.name() != *name {
            return Err(TypeCheckError::name(declared.name(), name).at(step()));
        }
    }
    Ok(())
}

/// Compare a derived variant `V`, whose cases are `cases` (each its WIT
/// name and, for a case with a payload, the payload type's check) in
/// order, with `ty`: the count of cases, then each case in order, its
/// name, then whether it has a payload, then the payload's type. A
/// mismatch names the case by the name the component declares.
pub fn typecheck_variant<V: ?Sized>(
    ty: &ValueType,
    types: &TypeChecker<'_>,
    cases: &[(&str, Option<TypeCheckFn>)],
) -> Result<(), TypeCheckError> {
    let ValueType::Variant(variant) = ty else {
        return Err(TypeCheckError::mismatch::<V>(ty));
    };
    if variant.cases().len() != cases.len() {
        return Err(TypeCheckError::count(variant.cases().len(), cases.len()));
    }
    for (declared, (name, check)) in variant.cases().iter().zip(cases) {
        let step = || TypePathStep::Case(declared.name().to_owned());
        if declared.name() != *name {
            return Err(TypeCheckError::name(declared.name(), name).at(step()));
        }
        match (declared.payload(), check) {
            (None, None) => {}
            (Some(payload), Some(check)) => {
                check(payload, types).map_err(|error| error.at(step()))?;
            }
            (Some(payload), None) => {
                return Err(TypeCheckError::mismatch::<()>(payload).at(step()));
            }
            (None, Some(_)) => {
                return Err(TypeCheckError::mismatch::<V>(ty).at(step()));
            }
        }
    }
    Ok(())
}

/// Compare a derived enum `E`, whose cases are named `names` in order,
/// with `ty`: the count of cases, then each name in order.
pub fn typecheck_enum<E: ?Sized>(ty: &ValueType, names: &[&str]) -> Result<(), TypeCheckError> {
    let ValueType::Enum(declared) = ty else {
        return Err(TypeCheckError::mismatch::<E>(ty));
    };
    if declared.cases().len() != names.len() {
        return Err(TypeCheckError::count(declared.cases().len(), names.len()));
    }
    for (declared, name) in declared.cases().iter().zip(names) {
        if declared != name {
            return Err(
                TypeCheckError::name(declared, name).at(TypePathStep::Case(declared.clone()))
            );
        }
    }
    Ok(())
}

/// Compare a flags type `F` that `flags!` declares, whose flags are
/// named `names` in order, with `ty`: the count of flags, then each
/// name in order.
pub fn typecheck_flags<F: ?Sized>(ty: &ValueType, names: &[&str]) -> Result<(), TypeCheckError> {
    let ValueType::Flags(declared) = ty else {
        return Err(TypeCheckError::mismatch::<F>(ty));
    };
    if declared.names().len() != names.len() {
        return Err(TypeCheckError::count(declared.names().len(), names.len()));
    }
    for (declared, name) in declared.names().iter().zip(names) {
        if declared != name {
            return Err(TypeCheckError::name(declared, name));
        }
    }
    Ok(())
}

/// Render the flags set in `words`, the words of a flags type whose
/// flags are named `names`, as `(READ | WRITE)`, as Wasmtime's
/// `format_flags` does.
pub fn format_flags(
    words: &[u32],
    names: &[&str],
    f: &mut core::fmt::Formatter<'_>,
) -> core::fmt::Result {
    f.write_str("(")?;
    let mut wrote = false;
    for (index, name) in names.iter().enumerate() {
        if words
            .get(index / 32)
            .is_some_and(|word| word & (1 << (index % 32)) != 0)
        {
            if wrote {
                f.write_str("|")?;
            }
            f.write_str(name)?;
            wrote = true;
        }
    }
    f.write_str(")")
}

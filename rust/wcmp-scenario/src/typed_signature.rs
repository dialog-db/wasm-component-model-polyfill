// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The signatures a typed call can have.

use core::fmt;

use crate::error::{Error, Result};
use crate::value_type::ValueType;

/// The signature of a typed call, from the closed set that a runner
/// makes typed calls for.
///
/// A typed function needs its Rust types when the runner is compiled,
/// so a runner cannot make a typed call of any signature a scenario
/// names. It makes one for each signature in this set: no more than
/// [`TypedSignature::MAX_PARAMETERS`] parameters, all of one scalar or
/// `string` type, and no result or one result of that same type. With
/// no parameter, the one result can have any of those types.
///
/// The parameter types are the types of the call's arguments, which
/// the expectations file spells with each value. The result type is
/// the export's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedSignature {
    /// The type of every parameter and of the result. `None` only when
    /// the call has neither.
    pub ty: Option<ValueType>,
    /// The number of parameters.
    pub parameters: usize,
    /// Whether the call has a result.
    pub result: bool,
}

impl TypedSignature {
    /// The most parameters a typed call can have.
    pub const MAX_PARAMETERS: usize = 2;

    /// The signature of a typed call with `parameters` and `results`.
    ///
    /// # Errors
    ///
    /// [`Error::Untyped`] when the signature is not in the closed set.
    pub fn new(parameters: &[ValueType], results: &[ValueType]) -> Result<Self> {
        let outside = || Error::Untyped {
            signature: Spelling {
                parameters,
                results,
            }
            .to_string(),
        };
        let mut types = parameters.iter().chain(results);
        let ty = types.next().copied();
        if parameters.len() > Self::MAX_PARAMETERS
            || results.len() > 1
            || types.any(|other| Some(*other) != ty)
        {
            return Err(outside());
        }
        Ok(TypedSignature {
            ty,
            parameters: parameters.len(),
            result: !results.is_empty(),
        })
    }
}

/// A signature as an error message spells it: `(s32, s32) -> s32`.
struct Spelling<'a> {
    parameters: &'a [ValueType],
    results: &'a [ValueType],
}

impl fmt::Display for Spelling<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |formatter: &mut fmt::Formatter<'_>, types: &[ValueType]| {
            formatter.write_str("(")?;
            for (index, ty) in types.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "{ty}")?;
            }
            formatter.write_str(")")
        };
        list(formatter, self.parameters)?;
        match self.results {
            [] => Ok(()),
            [result] => write!(formatter, " -> {result}"),
            results => {
                formatter.write_str(" -> ")?;
                list(formatter, results)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ValueType::{S32, U32};

    const STRING: ValueType = ValueType::String;

    #[wcmp_macros::test]
    fn it_takes_up_to_two_parameters_and_a_result_of_one_type() {
        assert_eq!(
            TypedSignature::new(&[S32, S32], &[S32]),
            Ok(TypedSignature {
                ty: Some(S32),
                parameters: 2,
                result: true,
            })
        );
        assert_eq!(
            TypedSignature::new(&[STRING], &[STRING]),
            Ok(TypedSignature {
                ty: Some(STRING),
                parameters: 1,
                result: true,
            })
        );
        assert_eq!(
            TypedSignature::new(&[U32], &[]),
            Ok(TypedSignature {
                ty: Some(U32),
                parameters: 1,
                result: false,
            })
        );
        assert_eq!(
            TypedSignature::new(&[], &[STRING]),
            Ok(TypedSignature {
                ty: Some(STRING),
                parameters: 0,
                result: true,
            })
        );
        assert_eq!(
            TypedSignature::new(&[], &[]),
            Ok(TypedSignature {
                ty: None,
                parameters: 0,
                result: false,
            })
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_signature_outside_the_closed_set_and_spells_it() {
        let cases: [(&[ValueType], &[ValueType], &str); 4] = [
            (&[S32, U32], &[S32], "(s32, u32) -> s32"),
            (&[STRING], &[U32], "(string) -> u32"),
            (&[S32, S32, S32], &[S32], "(s32, s32, s32) -> s32"),
            (&[], &[S32, S32], "() -> (s32, s32)"),
        ];
        for (parameters, results, spelled) in cases {
            assert_eq!(
                TypedSignature::new(parameters, results),
                Err(Error::Untyped {
                    signature: spelled.to_string()
                })
            );
        }
    }
}

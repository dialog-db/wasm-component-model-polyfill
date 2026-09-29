//! The structural shape of a `result<T, E>` value type.

use std::fmt;

use super::value_type::ValueType;
use crate::abi::shape::AbiShape;
use crate::internal::CompoundTypeInternal;

/// A success-or-failure value with optional payload types on each
/// arm.
///
/// Either or both of the `ok` and `err` arms may carry a payload, or
/// neither may. Two result types are structurally equal when both
/// arms agree — same presence pattern, structurally-equal payload
/// types when present.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ResultType {
    ok: Option<Box<ValueType>>,
    err: Option<Box<ValueType>>,
    shape: AbiShape,
}

impl ResultType {
    /// Construct a result type from optional `ok` and `err` payload
    /// types.
    pub fn new(ok: Option<ValueType>, err: Option<ValueType>) -> Self {
        let shape = AbiShape::variant([ok.as_ref(), err.as_ref()].into_iter());
        Self {
            ok: ok.map(Box::new),
            err: err.map(Box::new),
            shape,
        }
    }

    /// The payload type of the `ok` arm, if any.
    pub fn ok(&self) -> Option<&ValueType> {
        self.ok.as_deref()
    }

    /// The payload type of the `err` arm, if any.
    pub fn err(&self) -> Option<&ValueType> {
        self.err.as_deref()
    }
}

impl CompoundTypeInternal for ResultType {
    fn abi_shape(&self) -> &AbiShape {
        &self.shape
    }
}

impl fmt::Debug for ResultType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResultType")
            .field("ok", &self.ok)
            .field("err", &self.err)
            .finish()
    }
}

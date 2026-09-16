//! Data shapes describing every value type the polyfill can carry
//! across a component boundary.
//!
//! These types are *introspection data only*. They capture the
//! structural identity of every valtype the polyfill recognises — the
//! shapes of records, variants, lists, options, results, tuples,
//! flags, enums, and resource handles — so that a parsed [`Component`]
//! can be walked without reaching for an upstream type. Host-side
//! values, lift/lower, and the canonical ABI proper are not part of
//! this module.
//!
//! Two types compare equal if their structural shape matches: two
//! record types with identical field names in identical order, both
//! pointing at structurally-equal field types, are the same value
//! type. Subtyping (variance, depth, width) is a separate matter and
//! is not modelled here.
//!
//! [`Component`]: crate::Component

mod enum_type;
mod fixed_length_list_type;
mod flags_type;
mod list_type;
mod map_type;
mod option_type;
mod primitive_type;
mod record_type;
mod resource_type;
mod result_type;
mod tuple_type;
mod value_type;
mod variant_type;

pub use enum_type::EnumType;
pub use fixed_length_list_type::FixedLengthListType;
pub use flags_type::FlagsType;
pub use list_type::ListType;
pub use map_type::MapType;
pub use option_type::OptionType;
pub use primitive_type::PrimitiveType;
pub use record_type::{RecordField, RecordType};
pub use resource_type::ResourceType;
pub use result_type::ResultType;
pub use tuple_type::TupleType;
pub use value_type::ValueType;
pub use variant_type::{VariantCase, VariantType};

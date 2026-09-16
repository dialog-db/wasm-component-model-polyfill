//! The polyfill's umbrella value-type enum.

use super::enum_type::EnumType;
use super::fixed_length_list_type::FixedLengthListType;
use super::flags_type::FlagsType;
use super::list_type::ListType;
use super::map_type::MapType;
use super::option_type::OptionType;
use super::primitive_type::PrimitiveType;
use super::record_type::RecordType;
use super::resource_type::ResourceType;
use super::result_type::ResultType;
use super::tuple_type::TupleType;
use super::variant_type::VariantType;

/// The structural identity of any value type the polyfill recognises.
///
/// `ValueType` carries the shape of every primitive, compound type,
/// and resource handle the synchronous Component Model surface
/// admits. It is data only: there is no host-side value attached, no
/// canonical-ABI behaviour, and no runtime-state coupling — those
/// concerns are layered on top elsewhere.
///
/// Equality on `ValueType` is structural: two values are equal when
/// their shapes match all the way down. Two record types with the
/// same field names in the same order pointing at structurally-equal
/// field types are the same `ValueType`, even if they were declared
/// in different components.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ValueType {
    /// A primitive scalar type.
    Primitive(PrimitiveType),
    /// A record: an ordered, named collection of typed fields.
    Record(RecordType),
    /// A variant: a tagged union with named, optionally-payloaded
    /// cases.
    Variant(VariantType),
    /// A homogeneous list of element values.
    List(ListType),
    /// An association from keys to values (`map<K, V>`).
    Map(MapType),
    /// A list of a fixed number of elements (`list<T, N>`).
    FixedLengthList(FixedLengthListType),
    /// A value that may be present or absent.
    Option(OptionType),
    /// A success-or-failure value with optional payloads on each arm.
    Result(ResultType),
    /// A heterogeneous, positionally-addressed tuple.
    Tuple(TupleType),
    /// A bit-set whose discriminants are addressed by name.
    Flags(FlagsType),
    /// A tag-only enumeration with named, payload-free cases.
    Enum(EnumType),
    /// An owning handle to a resource (`own<T>`).
    ///
    /// The full handle-table semantics — index allocation, transfer,
    /// destructor invocation — are not modelled here; this variant
    /// names the resource type the handle points at so the
    /// introspection surface is complete.
    Own(ResourceType),
    /// A borrow handle to a resource (`borrow<T>`).
    Borrow(ResourceType),
}

#[cfg(test)]
mod tests {
    use super::super::record_type::RecordField;
    use super::*;

    #[test]
    fn it_treats_two_identical_records_as_structurally_equal() {
        let lhs = ValueType::Record(RecordType::new([
            RecordField::new("x", ValueType::Primitive(PrimitiveType::S32)),
            RecordField::new("y", ValueType::Primitive(PrimitiveType::S32)),
        ]));
        let rhs = ValueType::Record(RecordType::new([
            RecordField::new("x", ValueType::Primitive(PrimitiveType::S32)),
            RecordField::new("y", ValueType::Primitive(PrimitiveType::S32)),
        ]));
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn it_distinguishes_records_with_different_field_orders() {
        let lhs = ValueType::Record(RecordType::new([
            RecordField::new("x", ValueType::Primitive(PrimitiveType::S32)),
            RecordField::new("y", ValueType::Primitive(PrimitiveType::S32)),
        ]));
        let rhs = ValueType::Record(RecordType::new([
            RecordField::new("y", ValueType::Primitive(PrimitiveType::S32)),
            RecordField::new("x", ValueType::Primitive(PrimitiveType::S32)),
        ]));
        assert_ne!(lhs, rhs);
    }

    #[test]
    fn it_distinguishes_records_with_different_field_types() {
        let lhs = ValueType::Record(RecordType::new([RecordField::new(
            "x",
            ValueType::Primitive(PrimitiveType::S32),
        )]));
        let rhs = ValueType::Record(RecordType::new([RecordField::new(
            "x",
            ValueType::Primitive(PrimitiveType::U32),
        )]));
        assert_ne!(lhs, rhs);
    }

    #[test]
    fn it_distinguishes_an_enum_from_a_payloadless_variant() {
        // The two shapes are spelled differently in WIT and the
        // polyfill keeps that distinction at the data level.
        let as_enum = ValueType::Enum(EnumType::new(["a".into(), "b".into()]));
        let as_variant = ValueType::Variant(VariantType::new([
            super::super::variant_type::VariantCase::new("a", None),
            super::super::variant_type::VariantCase::new("b", None),
        ]));
        assert_ne!(as_enum, as_variant);
    }

    #[test]
    fn it_supports_recursive_compound_shapes() {
        let inner = ValueType::List(ListType::new(ValueType::Primitive(PrimitiveType::U8)));
        let outer = ValueType::Option(OptionType::new(inner.clone()));

        if let ValueType::Option(option) = &outer {
            assert_eq!(option.payload(), &inner);
        } else {
            panic!("expected an option");
        }
    }
}

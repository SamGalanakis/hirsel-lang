//! Canonical scalar/type-constructor tags, owned with the semantic enums.
use crate::{BuiltinType, Primitive};
use hirsel_codec::{DecodeError, Reader, Snapshot, Writer};

macro_rules! unit_enum {
    ($ty:ident { $($variant:ident),* $(,)? }) => {
        impl<C> Snapshot<C> for $ty {
            fn write(&self, w: &mut Writer<C>) {
                w.open(match self {
                    $(Self::$variant => stringify!($variant),)*
                });
                w.close();
            }
            fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
                let value = match r.open_any()? {
                    $(stringify!($variant) => Self::$variant,)*
                    other => return Err(r.error(format!(concat!("unknown ", stringify!($ty), " variant {}"), other))),
                };
                r.close()?;
                Ok(value)
            }
        }
    };
}

unit_enum!(Primitive {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    String,
    Bytes,
    Never
});
unit_enum!(BuiltinType {
    Array,
    List,
    Map,
    Set,
    SortedMap,
    Inline,
    Simd,
    Packed,
    Shared,
    Slots,
    SlotId,
    Region,
    Iter,
    Task,
    Sender,
    Receiver,
    Mutex,
    Atomic,
    Ptr,
    FnPtr,
    TypeShape
});

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_codec::{from_text, to_text};

    #[test]
    fn type_tags_preserve_canonical_bytes() {
        for (value, tag) in [
            (Primitive::Bool, "Bool"),
            (Primitive::U8, "U8"),
            (Primitive::U16, "U16"),
            (Primitive::U32, "U32"),
            (Primitive::U64, "U64"),
            (Primitive::U128, "U128"),
            (Primitive::I8, "I8"),
            (Primitive::I16, "I16"),
            (Primitive::I32, "I32"),
            (Primitive::I64, "I64"),
            (Primitive::I128, "I128"),
            (Primitive::F32, "F32"),
            (Primitive::F64, "F64"),
            (Primitive::String, "String"),
            (Primitive::Bytes, "Bytes"),
            (Primitive::Never, "Never"),
        ] {
            let expected = format!("({tag})");
            assert_eq!(to_text(&value), expected);
            assert_eq!(from_text::<Primitive>(&expected).unwrap(), value);
        }
        for (value, tag) in [
            (BuiltinType::Array, "Array"),
            (BuiltinType::List, "List"),
            (BuiltinType::Map, "Map"),
            (BuiltinType::Set, "Set"),
            (BuiltinType::SortedMap, "SortedMap"),
            (BuiltinType::Inline, "Inline"),
            (BuiltinType::Simd, "Simd"),
            (BuiltinType::Packed, "Packed"),
            (BuiltinType::Shared, "Shared"),
            (BuiltinType::Slots, "Slots"),
            (BuiltinType::SlotId, "SlotId"),
            (BuiltinType::Region, "Region"),
            (BuiltinType::Iter, "Iter"),
            (BuiltinType::Task, "Task"),
            (BuiltinType::Sender, "Sender"),
            (BuiltinType::Receiver, "Receiver"),
            (BuiltinType::Mutex, "Mutex"),
            (BuiltinType::Atomic, "Atomic"),
            (BuiltinType::Ptr, "Ptr"),
            (BuiltinType::FnPtr, "FnPtr"),
            (BuiltinType::TypeShape, "TypeShape"),
        ] {
            let expected = format!("({tag})");
            assert_eq!(to_text(&value), expected);
            assert_eq!(from_text::<BuiltinType>(&expected).unwrap(), value);
        }
    }
}

use super::*;

/// A field of the register pair a plain scalar result returns as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairField {
    Status,
    Value,
}

/// The register pair's fields in order: `{ i8 status, word }` (`register_result`).
/// Emission's return type and the fields it reads and writes, and the C typedefs every
/// harness uses (`c_entry`), all come from here and [`PairField::word`].
pub(crate) const REGISTER_PAIR: [PairField; 2] = [PairField::Status, PairField::Value];

/// What a register pair field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairWord {
    /// An unsigned integer of this many bits.
    Int(u32),
    Pointer,
}

impl PairField {
    /// The field's index in the returned struct.
    pub(crate) fn index(self) -> u32 {
        REGISTER_PAIR
            .iter()
            .position(|&field| field == self)
            .expect("every pair field is in the pair") as u32
    }

    /// The field's member name in the C typedef.
    pub(crate) fn name(self) -> &'static str {
        match self {
            PairField::Status => "status",
            PairField::Value => "value",
        }
    }

    /// The word this field holds for a register result of `kind`: the u8 status, and a
    /// pointer for a reference and 64 bits otherwise.
    pub(crate) fn word(self, kind: ScalarKind) -> PairWord {
        match self {
            PairField::Status => PairWord::Int(8),
            PairField::Value if kind == ScalarKind::Pointer => PairWord::Pointer,
            PairField::Value => PairWord::Int(64),
        }
    }
}

pub(super) fn pair_word<'m>(module: &'m Module<'m>, word: PairWord) -> Type<'m> {
    match word {
        PairWord::Int(bits) => module.int_type(bits),
        PairWord::Pointer => module.ptr_type(),
    }
}

/// The struct a Hirsel entry with a register result of `kind` returns.
pub(crate) fn pair_type<'m>(module: &'m Module<'m>, kind: ScalarKind) -> Type<'m> {
    module.struct_type(&REGISTER_PAIR.map(|field| pair_word(module, field.word(kind))))
}

/// The register word carrying a returned scalar of `kind`.
pub(super) fn value_word<'m>(module: &'m Module<'m>, kind: ScalarKind) -> Type<'m> {
    pair_word(module, PairField::Value.word(kind))
}

/// `value` (an SSA scalar of `kind`) as a word of type `word`, zero-extended.
pub(super) fn encode_word<'m>(
    builder: &Builder<'m>,
    kind: ScalarKind,
    value: Value<'m>,
    word: Type<'m>,
) -> Value<'m> {
    let module = builder.module();
    let i64_ty = module.int_type(64);
    let bits = match kind {
        ScalarKind::Pointer if word == module.ptr_type() => return value,
        ScalarKind::Pointer => return builder.cast(Cast::PtrToInt, value, word),
        ScalarKind::F32 => builder.cast(Cast::Bits, value, module.int_type(32)),
        ScalarKind::F64 => return builder.cast(Cast::Bits, value, i64_ty),
        ScalarKind::U64 | ScalarKind::I64 => return value,
        _ => value,
    };
    builder.cast(Cast::ZExt, bits, i64_ty)
}

/// The scalar of `kind` a returned `word` of type `ty` carries in its low bits.
pub(super) fn decode_word<'m>(
    builder: &Builder<'m>,
    kind: ScalarKind,
    word: Value<'m>,
    ty: Type<'m>,
) -> Value<'m> {
    let module = builder.module();
    match kind {
        ScalarKind::Pointer if ty == module.ptr_type() => word,
        ScalarKind::Pointer => builder.cast(Cast::IntToPtr, word, module.ptr_type()),
        ScalarKind::Bool => {
            // A Bool is one byte, as in memory: any nonzero byte is true.
            let byte = builder.cast(Cast::Trunc, word, module.int_type(8));
            builder.icmp(
                IntPredicate::Ne,
                byte,
                module.const_int(module.int_type(8), 0),
            )
        }
        ScalarKind::F32 => {
            let bits = builder.cast(Cast::Trunc, word, module.int_type(32));
            builder.cast(Cast::Bits, bits, module.f32_type())
        }
        ScalarKind::F64 => builder.cast(Cast::Bits, word, module.f64_type()),
        ScalarKind::U64 | ScalarKind::I64 => word,
        _ => {
            let width = scalar::integer(kind).expect("an integer kind").0;
            builder.cast(Cast::Trunc, word, module.int_type(width))
        }
    }
}

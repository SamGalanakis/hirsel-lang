use super::*;

/// The scalar a value of this layout kind is held as in SSA form, if it is one.
pub(super) fn scalar_shape(kind: &LayoutKind) -> Option<ScalarKind> {
    match kind {
        LayoutKind::Scalar(kind) => Some(*kind),
        LayoutKind::Path => Some(ScalarKind::U64),
        LayoutKind::Block { .. }
        | LayoutKind::Box { .. }
        | LayoutKind::Header
        | LayoutKind::Arena => Some(ScalarKind::Pointer),
        _ => None,
    }
}

/// The scalar a Hirsel entry of `abi` returns in a register word, when its result is a
/// plain scalar of at most 64 bits. Every other result keeps its result pointer.
///
/// Register result invariant (ABI v1 lowering): such an entry returns the two-field
/// struct `{ i8 status, word }`, where the word is `ptr` for a pointer result and `i64`
/// otherwise. On success the word holds the value's bits in its low bits (Bool as one
/// byte, floats by bit pattern); emitted code zero-extends them and never assumes a
/// caller or callee did. On failure the word is zero and meaningless. Both targets
/// return the two fields in registers (RAX/AL and RDX, W0 and X1), which is also the C
/// struct `{ uint8_t status; uint64_t value; }` (or `void *value`). Definitions,
/// function-reference thunks, closure and iterator bodies follow it alike, so a direct,
/// a private and an indirect call agree; runtime primitives keep their result pointer.
pub fn register_result(layouts: &LayoutTable, abi: &FnAbi) -> Option<ScalarKind> {
    let result = abi.result.as_ref()?;
    let kind = scalar_shape(&layouts.layout(result.layout)?.kind)?;
    (!matches!(kind, ScalarKind::U128 | ScalarKind::I128)).then_some(kind)
}

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

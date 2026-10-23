//! Exact-width, alignment-one byte access. The range decision precedes every data load.
use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    fn byte_range(
        &self,
        bytes: ValueId,
        offset: ValueId,
        width: u64,
    ) -> Emitted<(Value<'m>, Value<'m>)> {
        let (data, len) = self.slice_bytes(bytes)?;
        let offset = self.get(offset)?;
        let within = self.builder.icmp(IntPredicate::Ule, offset, len);
        // Native subtraction deliberately wraps; the conjunction rejects underflow.
        let remaining = self.builder.binary(BinOp::Sub, len, offset);
        let fits = self
            .builder
            .icmp(IntPredicate::Ule, self.const_u64(width), remaining);
        let valid = self.builder.binary(BinOp::And, within, fits);
        Ok((self.builder.offset(data, offset), valid))
    }

    pub(super) fn read_byte_word(
        &mut self,
        result: ValueId,
        valid: ValueId,
        bytes: ValueId,
        offset: ValueId,
        width: u32,
        big: bool,
    ) -> Emitted {
        let (ptr, condition) = self.byte_range(bytes, offset, u64::from(width))?;
        let ty = self.module.int_type(width * 8);
        let origin = self.builder.current();
        let read = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.cond_br(condition, read, done);
        self.builder.position(read);
        let mut word = self.builder.load(ty, ptr, 1);
        if big != (self.inputs.target.endian == Endian::Big) {
            let swap = self.module.intrinsic("llvm.bswap", &[ty]).map_err(|what| {
                CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what,
                }
            })?;
            word = self.builder.call(swap, &[word]);
        }
        self.builder.br(done);
        self.builder.position(done);
        let value = self.builder.phi(ty);
        self.builder
            .add_incoming(value, self.module.const_zero(ty), origin);
        self.builder.add_incoming(value, word, read);
        self.define(result, value)?;
        self.define(valid, condition)
    }

    pub(super) fn read_byte_vector(
        &mut self,
        result: ValueId,
        valid: ValueId,
        bytes: ValueId,
        offset: ValueId,
        lanes: u8,
    ) -> Emitted {
        let (ptr, condition) = self.byte_range(bytes, offset, u64::from(lanes))?;
        let layout = self.value_type(result)?.layout;
        let dest = self.stack(layout)?;
        self.builder
            .memset(dest.ptr, 0, self.fixed_size(layout)?, dest.align);
        let read = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.cond_br(condition, read, done);
        self.builder.position(read);
        let mut at = 0u64;
        while at < u64::from(lanes) {
            let n = if u64::from(lanes) - at >= 16 { 16 } else { 1 };
            let ty = if n == 16 {
                self.module.vector_type(self.module.int_type(8), 16)
            } else {
                self.module.int_type(8)
            };
            let src = self.builder.offset(ptr, self.const_u64(at));
            let value = self.builder.load(ty, src, 1);
            let out = self.builder.offset(dest.ptr, self.const_u64(at));
            self.builder.store(value, out, 1);
            at += n;
        }
        self.builder.br(done);
        self.builder.position(done);
        let value = self.load(self.value_shape(result)?, dest);
        self.define(result, value)?;
        self.define(valid, condition)
    }

    // The existing Simd representation stays inline aggregate storage. Only legal
    // 16-byte chunks become machine vectors; odd tails access one byte each.
    pub(super) fn byte_vector_binary(
        &mut self,
        result: ValueId,
        left: ValueId,
        right: ValueId,
        lanes: u8,
        equal: bool,
    ) -> Emitted {
        let layout = self.value_type(result)?.layout;
        let a = self.stack(layout)?;
        let b = self.stack(layout)?;
        let out = self.stack(layout)?;
        let shape = self.value_shape(result)?;
        self.store(shape, self.get(left)?, a);
        self.store(shape, self.get(right)?, b);
        self.builder
            .memset(out.ptr, 0, self.fixed_size(layout)?, out.align);
        let mut at = 0u64;
        while at < u64::from(lanes) {
            let n = if u64::from(lanes) - at >= 16 { 16 } else { 1 };
            let ty = if n == 16 {
                self.module.vector_type(self.module.int_type(8), 16)
            } else {
                self.module.int_type(8)
            };
            let a = self
                .builder
                .load(ty, self.builder.offset(a.ptr, self.const_u64(at)), 1);
            let b = self
                .builder
                .load(ty, self.builder.offset(b.ptr, self.const_u64(at)), 1);
            let value = if equal {
                let mask = self.builder.icmp(IntPredicate::Eq, a, b);
                self.builder.cast(Cast::SExt, mask, ty)
            } else {
                self.builder.binary(BinOp::Or, a, b)
            };
            self.builder
                .store(value, self.builder.offset(out.ptr, self.const_u64(at)), 1);
            at += n;
        }
        self.define(result, self.load(shape, out))
    }

    pub(super) fn byte_vector_mask(
        &mut self,
        result: ValueId,
        value: ValueId,
        lanes: u8,
    ) -> Emitted {
        let layout = self.value_type(value)?.layout;
        let source = self.stack(layout)?;
        self.store(self.value_shape(value)?, self.get(value)?, source);
        let mut mask = self.const_u64(0);
        let mut at = 0u64;
        while at < u64::from(lanes) {
            let n = if self.inputs.target.endian == Endian::Little && u64::from(lanes) - at >= 16 {
                16
            } else {
                1
            };
            let ty = if n == 16 {
                self.module.vector_type(self.module.int_type(8), 16)
            } else {
                self.module.int_type(8)
            };
            let raw = self
                .builder
                .load(ty, self.builder.offset(source.ptr, self.const_u64(at)), 1);
            let high = self
                .builder
                .icmp(IntPredicate::Slt, raw, self.module.const_zero(ty));
            let mut chunk = if n == 16 {
                self.builder
                    .cast(Cast::Bits, high, self.module.int_type(16))
            } else {
                high
            };
            // Lane zero is the low bit on supported little-endian native targets.
            // For a big-endian target use exact scalar lanes instead of vector bitcasts.
            chunk = self
                .builder
                .cast(Cast::ZExt, chunk, self.module.int_type(64));
            chunk = self.builder.binary(BinOp::Shl, chunk, self.const_u64(at));
            mask = self.builder.binary(BinOp::Or, mask, chunk);
            at += n;
        }
        self.define(result, mask)
    }
}

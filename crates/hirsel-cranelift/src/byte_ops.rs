//! Total byte reads and portable 16-byte vector chunks, with exact scalar tails.
use crate::function::{FnEmitter, V};
use crate::module::Emitted;
use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use hirsel_lir::{ir::ValueId, layout_types::Endian};

impl FnEmitter<'_, '_, '_> {
    fn byte_range(
        &mut self,
        bytes: ValueId,
        offset: ValueId,
        width: u64,
    ) -> Emitted<(Value, Value)> {
        let (data, len) = self.slice_bytes(bytes)?;
        let offset = self.get1(offset)?;
        let within = self
            .b
            .ins()
            .icmp(IntCC::UnsignedLessThanOrEqual, offset, len);
        let remaining = self.b.ins().isub(len, offset);
        let fits =
            self.b
                .ins()
                .icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, remaining, width as i64);
        let valid = self.b.ins().band(within, fits);
        let ptr = self.b.ins().iadd(data, offset);
        Ok((ptr, valid))
    }

    pub(crate) fn read_byte_word(
        &mut self,
        result: ValueId,
        valid: ValueId,
        bytes: ValueId,
        offset: ValueId,
        width: u32,
        big: bool,
    ) -> Emitted {
        let (ptr, condition) = self.byte_range(bytes, offset, u64::from(width))?;
        let ty = match width {
            2 => types::I16,
            4 => types::I32,
            8 => types::I64,
            _ => return self.invalid("scalar byte width"),
        };
        let zero = self.b.ins().iconst(ty, 0);
        let read = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(done, ty);
        self.b
            .ins()
            .brif(condition, read, &[], done, &[zero.into()]);
        self.b.switch_to_block(read);
        self.b.seal_block(read);
        let mut word = self.load_ty(ty, ptr, 0);
        if big != (self.em.inputs.target.endian == Endian::Big) {
            word = self.b.ins().bswap(word);
        }
        self.b.ins().jump(done, &[word.into()]);
        self.b.switch_to_block(done);
        self.b.seal_block(done);
        let word = self.b.block_params(done)[0];
        self.define1(result, word)?;
        self.define1(valid, condition)
    }

    pub(crate) fn read_byte_vector(
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
        self.zero(dest.ptr, self.fixed_size(layout)?, dest.align);
        let read = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(condition, read, &[], done, &[]);
        self.b.switch_to_block(read);
        self.b.seal_block(read);
        let mut at = 0u64;
        while at < u64::from(lanes) {
            let (n, ty) = if u64::from(lanes) - at >= 16 {
                (16, types::I8X16)
            } else {
                (1, types::I8)
            };
            let value = self.load_ty(ty, ptr, at);
            self.store_at(value, dest.ptr, at);
            at += n;
        }
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        self.b.seal_block(done);
        self.define(
            result,
            V::Mem {
                addr: dest.ptr,
                slot: None,
            },
        )?;
        self.define1(valid, condition)
    }

    pub(crate) fn byte_vector_binary(
        &mut self,
        result: ValueId,
        left: ValueId,
        right: ValueId,
        lanes: u8,
        equal: bool,
    ) -> Emitted {
        let (V::Mem { addr: a, .. }, V::Mem { addr: b, .. }) = (self.get(left)?, self.get(right)?)
        else {
            return self.invalid("byte SIMD operand storage");
        };
        let layout = self.value_type(result)?.layout;
        let dest = self.stack(layout)?;
        self.zero(dest.ptr, self.fixed_size(layout)?, dest.align);
        let mut at = 0u64;
        while at < u64::from(lanes) {
            let (n, ty) = if u64::from(lanes) - at >= 16 {
                (16, types::I8X16)
            } else {
                (1, types::I8)
            };
            let x = self.load_ty(ty, a, at);
            let y = self.load_ty(ty, b, at);
            let mut out = if equal {
                self.b.ins().icmp(IntCC::Equal, x, y)
            } else {
                self.b.ins().bor(x, y)
            };
            if equal && n == 1 {
                out = self.b.ins().ineg(out);
            } // scalar icmp is 0/1
            self.store_at(out, dest.ptr, at);
            at += n;
        }
        self.define(
            result,
            V::Mem {
                addr: dest.ptr,
                slot: None,
            },
        )
    }

    pub(crate) fn byte_vector_mask(
        &mut self,
        result: ValueId,
        value: ValueId,
        lanes: u8,
    ) -> Emitted {
        let V::Mem { addr, .. } = self.get(value)? else {
            return self.invalid("byte SIMD operand storage");
        };
        let mut mask = self.b.ins().iconst(types::I64, 0);
        let mut at = 0u64;
        while at < u64::from(lanes) {
            let (n, ty) = if u64::from(lanes) - at >= 16 {
                (16, types::I8X16)
            } else {
                (1, types::I8)
            };
            let raw = self.load_ty(ty, addr, at);
            let chunk = if n == 16 {
                self.b.ins().vhigh_bits(types::I16, raw)
            } else {
                self.b.ins().ushr_imm_u(raw, 7)
            };
            let chunk = self.b.ins().uextend(types::I64, chunk);
            let chunk = self.b.ins().ishl_imm_u(chunk, at as i64);
            mask = self.b.ins().bor(mask, chunk);
            at += n;
        }
        self.define1(result, mask)
    }
}

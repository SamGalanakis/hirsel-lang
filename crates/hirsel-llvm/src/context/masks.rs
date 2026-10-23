//! Keep SEM-1's unsigned wrapping provenance through inlining, then expose
//! Boolean all-bits masks as conditional XORs before LLVM's loop idiom pass.

use super::*;

impl Module<'_> {
    fn wrapping_sub_kind(&self) -> u32 {
        const NAME: &[u8] = b"hirsel.unsigned.wrapping.sub";
        // SAFETY: the name is readable for its length and the context is live.
        unsafe {
            LLVMGetMDKindIDInContext(self.context.raw, NAME.as_ptr().cast(), NAME.len() as u32)
        }
    }

    pub(crate) fn mark_unsigned_wrapping_sub(&self, result: Value<'_>) {
        // The builder may constant-fold the subtraction. Only instructions need
        // provenance, and no checked or signed operation calls this method.
        // SAFETY: the value belongs to this module; only instructions get metadata.
        unsafe {
            if !LLVMIsAInstruction(result.raw).is_null() {
                let node = LLVMMDNodeInContext2(self.context.raw, ptr::null_mut(), 0);
                LLVMSetMetadata(
                    result.raw,
                    self.wrapping_sub_kind(),
                    LLVMMetadataAsValue(self.context.raw, node),
                );
            }
        }
    }

    fn mask_instructions(&self) -> Vec<Value<'_>> {
        let mut found = Vec::new();
        // SAFETY: all walked instructions belong to this live module. The walk
        // finishes before canonicalization inserts or replaces instructions.
        unsafe {
            let mut function = LLVMGetFirstFunction(self.raw);
            while !function.is_null() {
                let mut block = LLVMGetFirstBasicBlock(function);
                while !block.is_null() {
                    let mut instruction = LLVMGetFirstInstruction(block);
                    while !instruction.is_null() {
                        found.push(value(instruction));
                        instruction = LLVMGetNextInstruction(instruction);
                    }
                    block = LLVMGetNextBasicBlock(block);
                }
                function = LLVMGetNextFunction(function);
            }
        }
        found
    }

    pub(crate) fn has_unsigned_wrapping_sub(&self) -> bool {
        let kind = self.wrapping_sub_kind();
        self.mask_instructions().iter().any(|v| {
            // SAFETY: the walk returns instructions in this module.
            unsafe { !LLVMGetMetadata(v.raw, kind).is_null() }
        })
    }

    pub(crate) fn canonicalize_wrapping_masks(&self) {
        let kind = self.wrapping_sub_kind();
        let builder = Builder::new(self);
        for xor in self.mask_instructions() {
            let Some((left, right)) = operands(xor, LLVMOpcode::LLVMXor) else {
                continue;
            };
            for (base, masked) in [(left, right), (right, left)] {
                let Some((a, b)) = operands(masked, LLVMOpcode::LLVMAnd) else {
                    continue;
                };
                let mut replacement = None;
                for (mask, polynomial) in [(a, b), (b, a)] {
                    let Some((zero, bit)) = operands(mask, LLVMOpcode::LLVMSub) else {
                        continue;
                    };
                    // SAFETY: `mask` is a subtraction instruction. Width checks
                    // exclude vectors, and original unsigned wrapping provenance
                    // excludes checked overflow results and signed arithmetic.
                    let proven = unsafe {
                        !LLVMGetMetadata(mask.raw, kind).is_null()
                            && LLVMGetNUW(mask.raw) == 0
                            && LLVMGetNSW(mask.raw) == 0
                            && LLVMGetTypeKind(LLVMTypeOf(mask.raw))
                                == llvm_sys::LLVMTypeKind::LLVMIntegerTypeKind
                            && matches!(
                                LLVMGetIntTypeWidth(LLVMTypeOf(mask.raw)),
                                8 | 16 | 32 | 64 | 128
                            )
                    };
                    if !proven || zero.known_u64() != Some(0) || !boolean_integer(bit) {
                        continue;
                    }
                    // Preserve the original expressions for any other live uses.
                    // No shift, overflow guard, load or failure edge is removed.
                    // SAFETY: the new instructions precede the replaced XOR and
                    // consume only its dominating operands (or their operands).
                    unsafe { LLVMPositionBuilderBefore(builder.raw, xor.raw) };
                    let set = builder.icmp(IntPredicate::Ne, bit, zero);
                    let with_polynomial = builder.binary(BinOp::Xor, base, polynomial);
                    replacement = Some(builder.select(set, with_polynomial, base));
                    break;
                }
                if let Some(replacement) = replacement {
                    // SAFETY: both values have the same scalar integer type;
                    // retain the old XOR until all walk handles are dead.
                    unsafe { LLVMReplaceAllUsesWith(xor.raw, replacement.raw) };
                    break;
                }
            }
        }
    }
}

fn operands(operand: Value<'_>, opcode: LLVMOpcode) -> Option<(Value<'_>, Value<'_>)> {
    // SAFETY: inspect the opcode only for an instruction, and operands only for
    // the requested binary opcode. The returned handles share its module.
    unsafe {
        if LLVMIsAInstruction(operand.raw).is_null()
            || LLVMGetInstructionOpcode(operand.raw) != opcode
        {
            return None;
        }
        Some((
            value(LLVMGetOperand(operand.raw, 0)),
            value(LLVMGetOperand(operand.raw, 1)),
        ))
    }
}

fn boolean_integer(bit: Value<'_>) -> bool {
    if let Some((a, b)) = operands(bit, LLVMOpcode::LLVMAnd) {
        return a.known_u64() == Some(1) || b.known_u64() == Some(1);
    }
    // SAFETY: test the instruction before reading its opcode and operand.
    unsafe {
        !LLVMIsAInstruction(bit.raw).is_null()
            && LLVMGetInstructionOpcode(bit.raw) == LLVMOpcode::LLVMZExt
            && LLVMGetIntTypeWidth(LLVMTypeOf(LLVMGetOperand(bit.raw, 0))) == 1
    }
}

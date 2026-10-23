//! Checked-H byte reads independently inspect the actual slice and ledger bytewise.
use super::*;

impl<S: Stage> Machine<'_, S> {
    /// Read only metadata before the range decision. Invalid reads never inspect
    /// the backing allocation. A valid read checks each accessed byte in H's ledger.
    fn checked_bytes(
        &mut self,
        f: &Frame,
        bytes: ValueId,
        offset: ValueId,
        width: u64,
    ) -> Checked<Option<Vec<u8>>> {
        let layout = self.value_type(f.unit, bytes)?.layout;
        let Val::Image(image) = self.read_use(f, bytes)? else {
            return fail(V::TypeConfusion, "byte read source");
        };
        let record = self.record(layout)?;
        let word = |n: u32| {
            record
                .fields
                .iter()
                .find(|x| x.ordinal == n)
                .map(|x| x.offset)
                .ok_or_else(|| err(V::Malformed, "slice metadata"))
        };
        let owner_at = word(0)?;
        let metadata = |at: u64| -> Checked<u64> {
            image.check_range(at, 8)?;
            if !image.all_init(at, 8) {
                return fail(V::UninitializedRead, "slice metadata");
            }
            Ok(u64::from_le_bytes(
                image.read_bytes(at, 8).try_into().unwrap(),
            ))
        };
        let start = metadata(word(1)?)?;
        let len = metadata(word(2)?)?;
        let Val::Bits {
            scalar: ScalarKind::U64,
            bits: at,
        } = self.read_use(f, offset)?
        else {
            return fail(V::TypeConfusion, "byte read offset");
        };
        let at = u64::try_from(at).map_err(|_| err(V::TypeConfusion, "byte read offset width"))?;
        if !(at <= len && width <= len - at) {
            return Ok(None);
        }
        let Some(Cell::Data { ptr: owner, .. }) = image.cells.get(&owner_at) else {
            return fail(V::ForgedPointer, "byte read owner");
        };
        let mut out = Vec::with_capacity(width as usize);
        for i in 0..width {
            let index = 8u64
                .checked_add(start)
                .and_then(|v| v.checked_add(at))
                .and_then(|v| v.checked_add(i))
                .ok_or_else(|| err(V::OutOfExtent, "byte read extent"))?;
            out.push(self.read_scalar(owner.at(index), 1)? as u8);
        }
        Ok(Some(out))
    }
    pub(super) fn byte_word(
        &mut self,
        f: &mut Frame,
        result: ValueId,
        valid: ValueId,
        bytes: ValueId,
        offset: ValueId,
        width: BytesWidth,
        endian: ByteOrder,
    ) -> Checked<()> {
        let data = self.checked_bytes(f, bytes, offset, u64::from(width.bytes()))?;
        let mut bits = 0u128;
        if let Some(ref data) = data {
            for (i, &byte) in data.iter().enumerate() {
                let lane = if endian == ByteOrder::Little {
                    i
                } else {
                    data.len() - 1 - i
                };
                bits |= u128::from(byte) << (8 * lane);
            }
        }
        self.define_fresh(
            f,
            result,
            Val::Bits {
                scalar: width.scalar(),
                bits,
            },
        )?;
        self.define_fresh(
            f,
            valid,
            Val::Bits {
                scalar: ScalarKind::Bool,
                bits: u128::from(data.is_some()),
            },
        )
    }
    pub(super) fn byte_vector_read(
        &mut self,
        f: &mut Frame,
        result: ValueId,
        valid: ValueId,
        bytes: ValueId,
        offset: ValueId,
        lanes: u8,
    ) -> Checked<()> {
        let data = self.checked_bytes(f, bytes, offset, u64::from(lanes))?;
        let unit = f.unit;
        let layout = self.value_type(unit, result)?.layout;
        let mut image = Extent::new(self.size(layout)?);
        image.write_bytes(0, &vec![0; image.len() as usize]);
        if let Some(ref data) = data {
            image.write_bytes(0, data);
        }
        self.define_fresh(f, result, Val::Image(image))?;
        self.define_fresh(
            f,
            valid,
            Val::Bits {
                scalar: ScalarKind::Bool,
                bits: u128::from(data.is_some()),
            },
        )
    }
    pub(super) fn byte_vector_op(
        &mut self,
        f: &mut Frame,
        result: ValueId,
        op: ByteVectorOp,
        lanes: u8,
    ) -> Checked<()> {
        let read = |machine: &mut Self, value| -> Checked<Vec<u8>> {
            let Val::Image(image) = machine.read_use(f, value)? else {
                return fail(V::TypeConfusion, "byte vector storage");
            };
            image.check_range(0, u64::from(lanes))?;
            if !image.all_init(0, u64::from(lanes)) {
                return fail(V::UninitializedRead, "byte vector lanes");
            }
            Ok(image.read_bytes(0, u64::from(lanes)).to_vec())
        };
        let value = match op {
            ByteVectorOp::Bitmask { value } => {
                let data = read(self, value)?;
                let mut bits = 0u128;
                for (i, byte) in data.into_iter().enumerate() {
                    bits |= u128::from(byte >> 7) << i;
                }
                Val::Bits {
                    scalar: ScalarKind::U64,
                    bits,
                }
            }
            ByteVectorOp::Eq { left, right } | ByteVectorOp::Or { left, right } => {
                let (a, b) = (read(self, left)?, read(self, right)?);
                let layout = self.value_type(f.unit, result)?.layout;
                let mut image = Extent::new(self.size(layout)?);
                image.write_bytes(0, &vec![0; image.len() as usize]);
                for (i, (&a, &b)) in a.iter().zip(&b).enumerate() {
                    let byte = if matches!(op, ByteVectorOp::Eq { .. }) {
                        if a == b { 255 } else { 0 }
                    } else {
                        a | b
                    };
                    image.write_bytes(i as u64, &[byte]);
                }
                Val::Image(image)
            }
        };
        self.define_fresh(f, result, value)
    }
}

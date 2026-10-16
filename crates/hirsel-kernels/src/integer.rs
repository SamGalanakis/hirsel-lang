//! Mathematical bit-vector conversion and shifts, independent of either value model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Width {
    W8,
    W16,
    W32,
    W64,
    W128,
}
impl Width {
    pub const fn bits(self) -> u32 {
        match self {
            Self::W8 => 8,
            Self::W16 => 16,
            Self::W32 => 32,
            Self::W64 => 64,
            Self::W128 => 128,
        }
    }
    pub const fn mask(self) -> u128 {
        u128::MAX >> (128 - self.bits())
    }
    pub fn from_bits(bits: u32) -> Option<Self> {
        match bits {
            8 => Some(Self::W8),
            16 => Some(Self::W16),
            32 => Some(Self::W32),
            64 => Some(Self::W64),
            128 => Some(Self::W128),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Format {
    pub width: Width,
    pub signed: bool,
}
impl Format {
    pub fn signed_value(self, bits: u128) -> i128 {
        ((bits << (128 - self.width.bits())) as i128) >> (128 - self.width.bits())
    }
}
pub fn exact(bits: u128, from: Format, to: Format) -> Option<u128> {
    let bits = bits & from.width.mask();
    let negative = from.signed && from.signed_value(bits) < 0;
    let fits = if to.signed {
        let min = i128::MIN >> (128 - to.width.bits());
        let max = i128::MAX >> (128 - to.width.bits());
        if negative {
            from.signed_value(bits) >= min
        } else {
            bits <= max as u128
        }
    } else {
        !negative && bits <= to.width.mask()
    };
    fits.then(|| truncate(bits, from, to))
}
pub fn truncate(bits: u128, from: Format, to: Format) -> u128 {
    (if from.signed {
        from.signed_value(bits) as u128
    } else {
        bits
    }) & to.width.mask()
}
/// SEM-1 shifts mask the count by width, including negative two's-complement counts.
pub fn shift(bits: u128, format: Format, count: u128, left: bool) -> u128 {
    let n = (count & u128::from(format.width.bits() - 1)) as u32;
    let bits = bits & format.width.mask();
    (if left {
        bits << n
    } else if format.signed {
        (format.signed_value(bits) >> n) as u128
    } else {
        bits >> n
    }) & format.width.mask()
}
